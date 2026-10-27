//! Physical device discovery and virtual controller lifetime.
use crate::linux::{
    eviocgabs, ioctl_ref, ioctl_val, ior, InputAbsInfo, InputId, UinputAbsSetup, UinputSetup,
    ABS_HAT0X, ABS_HAT0Y, ABS_RX, ABS_RY, ABS_RZ, ABS_X, ABS_Y, ABS_Z, BTN_EAST, BTN_MODE,
    BTN_NORTH, BTN_SELECT, BTN_SOUTH, BTN_START, BTN_THUMBL, BTN_THUMBR, BTN_TL, BTN_TR, BTN_WEST,
    E, EVIOCGID, EV_ABS, EV_FF, EV_KEY, EV_SYN, FF_RUMBLE, UI_ABS_SETUP, UI_DEV_CREATE,
    UI_DEV_DESTROY, UI_DEV_SETUP, UI_SET_ABSBIT, UI_SET_EVBIT, UI_SET_FFBIT, UI_SET_KEYBIT,
};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
};

/// Find the first accessible DualSense node with both stick axes.
pub(crate) fn gamepad_path() -> io::Result<PathBuf> {
    let mut list: Vec<_> = fs::read_dir("/dev/input")?
        .flatten()
        .map(|x| x.path())
        .filter(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with("event")
        })
        .collect();
    list.sort();
    for p in list {
        if let Ok(f) = OpenOptions::new().read(true).write(true).open(&p) {
            let mut id = InputId::default();
            let mut axes = InputAbsInfo::default();
            if ioctl_ref(f.as_raw_fd(), EVIOCGID, &mut id).is_ok()
                && id.vendor == 0x054c
                && matches!(id.product, 0x0ce6 | 0x0df2)
                && ioctl_ref(f.as_raw_fd(), eviocgabs(ABS_X), &mut axes).is_ok()
                && ioctl_ref(f.as_raw_fd(), eviocgabs(ABS_RX), &mut axes).is_ok()
            {
                return Ok(p);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "DualSense gamepad evdev node not found or not accessible (check Bluetooth connection / permissions)",
    ))
}

/// Open the physical controller for input and rumble output.
pub(crate) fn open_physical(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// Require rumble support rather than silently creating a controller without it.
pub(crate) fn require_rumble(fd: RawFd) -> io::Result<()> {
    let mut bits = [0u8; 16];
    let req = ior(E, 0x20 + EV_FF as u64, bits.len() as u64);
    ioctl_ref(fd, req, &mut bits)?;
    if bits[(FF_RUMBLE / 8) as usize] & (1 << (FF_RUMBLE % 8)) == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "physical controller does not support FF_RUMBLE",
        ));
    }
    Ok(())
}
pub(crate) struct VirtualController {
    file: File,
}

impl Drop for VirtualController {
    fn drop(&mut self) {
        let _ = ioctl_val(self.file.as_raw_fd(), UI_DEV_DESTROY, 0);
    }
}

/// Create a virtual Xbox controller with rumble support.
pub(crate) fn virtual_pad() -> io::Result<VirtualController> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/uinput")?;
    let fd = f.as_raw_fd();
    configure_virtual(|operation| match operation {
        SetupOperation::Value(request, value) => ioctl_val(fd, request, value),
        SetupOperation::Axis(mut axis) => ioctl_ref(fd, UI_ABS_SETUP, &mut axis),
        SetupOperation::Identity(mut setup) => ioctl_ref(fd, UI_DEV_SETUP, &mut setup),
    })?;
    Ok(VirtualController { file: f })
}

#[derive(Clone, Copy)]
enum SetupOperation {
    Value(libc::c_ulong, i32),
    Axis(UinputAbsSetup),
    Identity(UinputSetup),
}

/// Create only after every capability and calibration operation has succeeded.
fn configure_virtual(mut apply: impl FnMut(SetupOperation) -> io::Result<()>) -> io::Result<()> {
    for ty in [EV_SYN, EV_KEY, EV_ABS] {
        apply(SetupOperation::Value(UI_SET_EVBIT, ty as i32))?;
    }
    for key in [
        BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START, BTN_MODE,
        BTN_THUMBL, BTN_THUMBR,
    ] {
        apply(SetupOperation::Value(UI_SET_KEYBIT, key as i32))?;
    }
    for (code, min, max) in [
        (ABS_X, -32768, 32767),
        (ABS_Y, -32768, 32767),
        (ABS_RX, -32768, 32767),
        (ABS_RY, -32768, 32767),
        (ABS_Z, 0, 255),
        (ABS_RZ, 0, 255),
        (ABS_HAT0X, -1, 1),
        (ABS_HAT0Y, -1, 1),
    ] {
        apply(SetupOperation::Value(UI_SET_ABSBIT, code as i32))?;
        let a = UinputAbsSetup {
            code,
            _pad: 0,
            absinfo: InputAbsInfo {
                value: 0,
                minimum: min,
                maximum: max,
                fuzz: 0,
                flat: 0,
                resolution: 0,
            },
        };
        apply(SetupOperation::Axis(a))?;
    }
    apply(SetupOperation::Value(UI_SET_EVBIT, EV_FF as i32))?;
    apply(SetupOperation::Value(UI_SET_FFBIT, FF_RUMBLE as i32))?;
    apply(SetupOperation::Identity(controller_setup()))?;
    apply(SetupOperation::Value(UI_DEV_CREATE, 0))?;
    Ok(())
}

impl AsRawFd for VirtualController {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

/// Build the virtual Xbox identity and rumble capacity.
fn controller_setup() -> UinputSetup {
    let mut setup = UinputSetup {
        id: InputId {
            bustype: 0x03,
            vendor: 0x045e,
            product: 0x028e,
            version: 0x0114,
        },
        name: [0; 80],
        ff_effects_max: 16,
    };
    let name = b"Xbox 360 Controller (dsxinput)";
    setup.name[..name.len()].copy_from_slice(name);
    setup
}

#[cfg(test)]
mod device_tests {
    use super::*;

    #[test]
    fn virtual_controller_advertises_xbox_identity() {
        let setup = controller_setup();
        assert_eq!(
            (
                setup.id.bustype,
                setup.id.vendor,
                setup.id.product,
                setup.id.version
            ),
            (0x03, 0x045e, 0x028e, 0x0114)
        );
        let name = std::ffi::CStr::from_bytes_until_nul(&setup.name).unwrap();
        assert_eq!(name.to_bytes(), b"Xbox 360 Controller (dsxinput)");
    }

    #[test]
    fn always_advertises_rumble_effect_slots() {
        assert_eq!(controller_setup().ff_effects_max, 16);
    }

    #[test]
    fn rumble_capability_query_preserves_kernel_errors() {
        assert_eq!(
            require_rumble(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}

#[cfg(test)]
mod setup_failure_tests {
    use super::*;

    #[test]
    fn every_setup_failure_stops_before_device_creation() {
        let mut operations = Vec::new();
        configure_virtual(|op| {
            operations.push(op);
            Ok(())
        })
        .unwrap();
        assert!(matches!(
            operations.last(),
            Some(SetupOperation::Value(UI_DEV_CREATE, 0))
        ));
        assert_eq!(
            operations
                .iter()
                .filter(|op| matches!(op, SetupOperation::Value(UI_DEV_CREATE, _)))
                .count(),
            1
        );
        for fail_at in 0..operations.len() {
            let mut calls = 0;
            let error = configure_virtual(|_| {
                calls += 1;
                if calls == fail_at + 1 {
                    Err(io::Error::from_raw_os_error(libc::EACCES))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(calls, fail_at + 1);
            assert_eq!(error.raw_os_error(), Some(libc::EACCES));
        }
    }

    #[test]
    fn setup_configures_all_axes_before_creation_with_bounded_ranges() {
        let mut axes = Vec::new();
        let mut keys = Vec::new();
        configure_virtual(|op| {
            match op {
                SetupOperation::Axis(a) => {
                    assert!(a.absinfo.minimum < a.absinfo.maximum);
                    assert!((a.absinfo.minimum..=a.absinfo.maximum).contains(&a.absinfo.value));
                    axes.push((a.code, a.absinfo.minimum, a.absinfo.maximum));
                }
                SetupOperation::Value(UI_SET_KEYBIT, key) => keys.push(key as u16),
                _ => (),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            axes,
            [
                (ABS_X, -32768, 32767),
                (ABS_Y, -32768, 32767),
                (ABS_RX, -32768, 32767),
                (ABS_RY, -32768, 32767),
                (ABS_Z, 0, 255),
                (ABS_RZ, 0, 255),
                (ABS_HAT0X, -1, 1),
                (ABS_HAT0Y, -1, 1)
            ]
        );
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 11);
    }
}

#[cfg(test)]
mod nonblocking_tests {
    use super::*;

    #[test]
    fn physical_descriptor_is_nonblocking_to_handle_removal_after_poll() {
        let file = open_physical(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Cargo.toml"
        )))
        .unwrap();
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0);
    }
}
