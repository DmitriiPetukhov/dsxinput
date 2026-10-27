//! Linux input ABI definitions and event I/O.
use std::{
    io,
    mem::{size_of, zeroed},
    os::fd::RawFd,
};

pub(crate) const EV_SYN: u16 = 0;
pub(crate) const EV_KEY: u16 = 1;
pub(crate) const EV_ABS: u16 = 3;
pub(crate) const EV_FF: u16 = 21;
pub(crate) const EV_UINPUT: u16 = 0x0101;
pub(crate) const SYN_REPORT: u16 = 0;
pub(crate) const SYN_DROPPED: u16 = 3;
pub(crate) const EVIOCGKEY: libc::c_ulong = ior(E, 0x18, 96);
pub(crate) const ABS_X: u16 = 0;
pub(crate) const ABS_Y: u16 = 1;
pub(crate) const ABS_Z: u16 = 2;
pub(crate) const ABS_RX: u16 = 3;
pub(crate) const ABS_RY: u16 = 4;
pub(crate) const ABS_RZ: u16 = 5;
pub(crate) const ABS_HAT0X: u16 = 16;
pub(crate) const ABS_HAT0Y: u16 = 17;
pub(crate) const BTN_SOUTH: u16 = 0x130;
pub(crate) const BTN_EAST: u16 = 0x131;
pub(crate) const BTN_NORTH: u16 = 0x133;
pub(crate) const BTN_WEST: u16 = 0x134;
pub(crate) const BTN_TL: u16 = 0x136;
pub(crate) const BTN_TR: u16 = 0x137;
#[cfg(test)]
pub(crate) const BTN_TL2: u16 = 0x138;
#[cfg(test)]
pub(crate) const BTN_TR2: u16 = 0x139;
pub(crate) const BTN_SELECT: u16 = 0x13a;
pub(crate) const BTN_START: u16 = 0x13b;
pub(crate) const BTN_MODE: u16 = 0x13c;
pub(crate) const BTN_THUMBL: u16 = 0x13d;
pub(crate) const BTN_THUMBR: u16 = 0x13e;
pub(crate) const FF_RUMBLE: u16 = 0x50;
pub(crate) const UI_FF_UPLOAD: u16 = 1;
pub(crate) const UI_FF_ERASE: u16 = 2;
// Generic Linux ioctl encoding, arch-independent for Fedora x86_64/aarch64.
/// Encode a Linux ioctl request for x86_64 and aarch64.
const fn ioc(dir: u64, ty: u64, nr: u64, size: u64) -> libc::c_ulong {
    ((dir << 30) | (size << 16) | (ty << 8) | nr) as libc::c_ulong
}

/// Encode an ioctl without a data payload.
const fn io(ty: u64, nr: u64) -> libc::c_ulong {
    ioc(0, ty, nr, 0)
}

/// Encode an ioctl that sends a payload to the kernel.
const fn iow(ty: u64, nr: u64, size: u64) -> libc::c_ulong {
    ioc(1, ty, nr, size)
}

/// Encode an ioctl that receives a payload from the kernel.
pub(crate) const fn ior(ty: u64, nr: u64, size: u64) -> libc::c_ulong {
    ioc(2, ty, nr, size)
}

/// Encode an ioctl with a bidirectional payload.
const fn iowr(ty: u64, nr: u64, size: u64) -> libc::c_ulong {
    ioc(3, ty, nr, size)
}
const U: u64 = b'U' as u64;
pub(crate) const E: u64 = b'E' as u64;
pub(crate) const UI_DEV_CREATE: libc::c_ulong = io(U, 1);
pub(crate) const UI_DEV_DESTROY: libc::c_ulong = io(U, 2);
pub(crate) const UI_SET_EVBIT: libc::c_ulong = iow(U, 100, 4);
pub(crate) const UI_SET_KEYBIT: libc::c_ulong = iow(U, 101, 4);
pub(crate) const UI_SET_ABSBIT: libc::c_ulong = iow(U, 103, 4);
pub(crate) const UI_SET_FFBIT: libc::c_ulong = iow(U, 107, 4);
pub(crate) const UI_DEV_SETUP: libc::c_ulong = iow(U, 3, size_of::<UinputSetup>() as u64);
pub(crate) const UI_ABS_SETUP: libc::c_ulong = iow(U, 4, size_of::<UinputAbsSetup>() as u64);
pub(crate) const UI_BEGIN_FF_UPLOAD: libc::c_ulong =
    iowr(U, 200, size_of::<UinputFfUpload>() as u64);
pub(crate) const UI_END_FF_UPLOAD: libc::c_ulong = iow(U, 201, size_of::<UinputFfUpload>() as u64);
pub(crate) const UI_BEGIN_FF_ERASE: libc::c_ulong = iowr(U, 202, size_of::<UinputFfErase>() as u64);
pub(crate) const UI_END_FF_ERASE: libc::c_ulong = iow(U, 203, size_of::<UinputFfErase>() as u64);
pub(crate) const EVIOCGRAB: libc::c_ulong = iow(E, 0x90, 4);
pub(crate) const EVIOCSFF: libc::c_ulong = iow(E, 0x80, size_of::<FfEffect>() as u64);
pub(crate) const EVIOCRMFF: libc::c_ulong = iow(E, 0x81, 4);
pub(crate) const EVIOCGID: libc::c_ulong = ior(E, 0x02, size_of::<InputId>() as u64);
const EVIOCGABS_X: libc::c_ulong = ior(E, 0x40, size_of::<InputAbsInfo>() as u64);
/// Build the request for an axis calibration record.
pub(crate) const fn eviocgabs(axis: u16) -> libc::c_ulong {
    EVIOCGABS_X + axis as libc::c_ulong
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct InputId {
    pub(crate) bustype: u16,
    pub(crate) vendor: u16,
    pub(crate) product: u16,
    pub(crate) version: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct InputAbsInfo {
    pub(crate) value: i32,
    pub(crate) minimum: i32,
    pub(crate) maximum: i32,
    pub(crate) fuzz: i32,
    pub(crate) flat: i32,
    pub(crate) resolution: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct UinputAbsSetup {
    pub(crate) code: u16,
    pub(crate) _pad: u16,
    pub(crate) absinfo: InputAbsInfo,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct UinputSetup {
    pub(crate) id: InputId,
    pub(crate) name: [u8; 80],
    pub(crate) ff_effects_max: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct InputEvent {
    pub(crate) time: libc::timeval,
    pub(crate) kind: u16,
    pub(crate) code: u16,
    pub(crate) value: i32,
}
// The 64-bit Linux union is 32 bytes, aligned for its periodic-effect pointer.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub(crate) struct FfEffect {
    pub(crate) type_: u16,
    pub(crate) id: i16,
    pub(crate) direction: u16,
    pub(crate) trigger: [u16; 2],
    pub(crate) replay: [u16; 2],
    // The C union begins at an eight-byte boundary, after two padding bytes.
    pub(crate) _padding: u16,
    pub(crate) data: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct UinputFfUpload {
    pub(crate) request_id: u32,
    pub(crate) retval: i32,
    pub(crate) effect: FfEffect,
    pub(crate) old: FfEffect,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct UinputFfErase {
    pub(crate) request_id: u32,
    pub(crate) retval: i32,
    pub(crate) effect_id: u32,
}

/// Issue an integer-valued ioctl, preserving the kernel error.
pub(crate) fn ioctl_val(fd: RawFd, req: libc::c_ulong, val: i32) -> io::Result<()> {
    let r = retry_interrupted(|| unsafe { libc::ioctl(fd, req, val) } as isize) as libc::c_int;
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Issue an ioctl with its matching C-layout payload.
pub(crate) fn ioctl_ref<T>(fd: RawFd, req: libc::c_ulong, v: &mut T) -> io::Result<()> {
    // Call sites pair each request with its matching repr(C) payload.
    let r =
        retry_interrupted(|| unsafe { libc::ioctl(fd, req, v as *mut T) } as isize) as libc::c_int;
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Read one complete input event or report a disconnect/error.
pub(crate) fn read_event(fd: RawFd) -> io::Result<InputEvent> {
    let mut ev: InputEvent = unsafe { zeroed() };
    // InputEvent contains only integer fields; the buffer is valid for its full size.
    let n = retry_interrupted(|| unsafe {
        libc::read(
            fd,
            &mut ev as *mut _ as *mut libc::c_void,
            size_of::<InputEvent>(),
        )
    });
    if n == size_of::<InputEvent>() as isize {
        Ok(ev)
    } else if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "incomplete event / device disconnected",
        ))
    }
}

/// Write one input event with a zero timestamp.
pub(crate) fn write_event(fd: RawFd, kind: u16, code: u16, value: i32) -> io::Result<()> {
    let ev = InputEvent {
        time: unsafe { zeroed() },
        kind,
        code,
        value,
    };
    let n = retry_interrupted(|| unsafe {
        libc::write(
            fd,
            &ev as *const _ as *const libc::c_void,
            size_of::<InputEvent>(),
        )
    });
    check_event_write(n)
}

fn check_event_write(n: isize) -> io::Result<()> {
    if n == size_of::<InputEvent>() as isize {
        Ok(())
    } else if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "incomplete input event write",
        ))
    }
}

#[cfg(test)]
mod linux_tests {
    use super::*;
    use std::{
        io::Write,
        os::{fd::AsRawFd, unix::net::UnixStream},
    };

    #[test]
    fn event_io_preserves_type_code_value_and_zero_timestamp() {
        let (writer, reader) = UnixStream::pair().unwrap();
        write_event(writer.as_raw_fd(), EV_KEY, BTN_SOUTH, 1).unwrap();
        let event = read_event(reader.as_raw_fd()).unwrap();
        assert_eq!(
            (event.kind, event.code, event.value),
            (EV_KEY, BTN_SOUTH, 1)
        );
        assert_eq!((event.time.tv_sec, event.time.tv_usec), (0, 0));
    }

    #[test]
    fn truncated_event_is_unexpected_eof() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(&[0; 3]).unwrap();
        drop(writer);
        assert_eq!(
            read_event(reader.as_raw_fd()).err().unwrap().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn disconnected_stream_is_unexpected_eof() {
        let (writer, reader) = UnixStream::pair().unwrap();
        drop(writer);
        assert_eq!(
            read_event(reader.as_raw_fd()).err().unwrap().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn invalid_descriptor_errors_are_preserved() {
        assert_eq!(
            read_event(-1).err().unwrap().raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(
            write_event(-1, EV_SYN, SYN_REPORT, 0)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(
            ioctl_val(-1, EVIOCGRAB, 1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
        assert_eq!(
            ioctl_ref(-1, EVIOCGID, &mut InputId::default())
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EBADF)
        );
    }

    #[test]
    fn force_feedback_union_has_kernel_alignment() {
        assert_eq!(std::mem::offset_of!(FfEffect, data), 16);
        assert_eq!(std::mem::align_of::<FfEffect>(), 8);
        assert_eq!(size_of::<FfEffect>(), 48);
        assert_eq!(size_of::<UinputFfUpload>(), 104);
        assert_eq!(size_of::<UinputFfErase>(), 12);
    }

    #[test]
    fn ioctl_requests_match_linux_uapi() {
        assert_eq!(UI_DEV_CREATE, 0x5501);
        assert_eq!(EVIOCGID, 0x8008_4502);
        assert_eq!(EVIOCSFF, 0x4030_4580);
        assert_eq!(eviocgabs(ABS_RX), 0x8018_4543);
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn event_and_effect_sizes_match_linux() {
        assert_eq!(size_of::<InputEvent>(), 24);
        assert_eq!(size_of::<FfEffect>(), 48);
    }
}

/// Retry only operations that transferred no bytes when interrupted.
fn retry_interrupted(mut operation: impl FnMut() -> isize) -> isize {
    loop {
        let result = operation();
        if result >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return result;
        }
    }
}

#[cfg(test)]
mod interruption_tests {
    use super::*;

    #[test]
    fn retries_interruptions_but_preserves_other_failures_and_short_results() {
        for final_result in [-1, 0, 3, size_of::<InputEvent>() as isize] {
            let mut calls = 0;
            let result = retry_interrupted(|| {
                calls += 1;
                unsafe {
                    *libc::__errno_location() = if calls < 3 { libc::EINTR } else { libc::EBADF };
                }
                if calls < 3 {
                    -1
                } else {
                    final_result
                }
            });
            assert_eq!(result, final_result);
            assert_eq!(calls, 3);
        }
    }
}

#[cfg(test)]
mod transfer_failure_tests {
    use super::*;
    use std::os::{fd::AsRawFd, unix::net::UnixStream};

    #[test]
    fn short_writes_report_write_zero_even_with_stale_errno() {
        for size in 0..size_of::<InputEvent>() {
            unsafe {
                *libc::__errno_location() = libc::EACCES;
            }
            let error = check_event_write(size as isize).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::WriteZero);
            assert_eq!(error.raw_os_error(), None);
        }
        check_event_write(size_of::<InputEvent>() as isize).unwrap();
    }

    #[test]
    fn nonblocking_empty_input_reports_would_block() {
        let (_writer, reader) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        assert_eq!(
            read_event(reader.as_raw_fd()).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn queued_events_are_read_one_at_a_time_in_order() {
        let (writer, reader) = UnixStream::pair().unwrap();
        for value in [1, 0, 2] {
            write_event(writer.as_raw_fd(), EV_KEY, BTN_SOUTH, value).unwrap();
        }
        for value in [1, 0, 2] {
            assert_eq!(read_event(reader.as_raw_fd()).unwrap().value, value);
        }
    }
}
