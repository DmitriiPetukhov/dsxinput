//! Forward virtual rumble effects to physical controller effect IDs.
use crate::linux::{
    ioctl_ref, ioctl_val, write_event, InputEvent, UinputFfErase, UinputFfUpload, EVIOCRMFF,
    EVIOCSFF, EV_FF, EV_UINPUT, FF_RUMBLE, UI_BEGIN_FF_ERASE, UI_BEGIN_FF_UPLOAD, UI_END_FF_ERASE,
    UI_END_FF_UPLOAD, UI_FF_ERASE, UI_FF_UPLOAD,
};
use std::{io, mem::zeroed, os::fd::RawFd};

/// Handle an upload, erase, or playback event, updating IDs only on success.
pub(crate) fn forward_feedback(
    virtual_fd: RawFd,
    physical_fd: RawFd,
    ev: InputEvent,
    ids: &mut [i16; 16],
) -> io::Result<()> {
    if ev.kind == EV_UINPUT && ev.code == UI_FF_UPLOAD {
        let mut u: UinputFfUpload = unsafe { zeroed() };
        u.request_id = ev.value as u32;
        ioctl_ref(virtual_fd, UI_BEGIN_FF_UPLOAD, &mut u)?;
        let slot = u.effect.id;
        if slot >= 0 && (slot as usize) < ids.len() && u.effect.type_ == FF_RUMBLE {
            let mut effect = u.effect;
            effect.id = ids[slot as usize];
            match ioctl_ref(physical_fd, EVIOCSFF, &mut effect) {
                Ok(()) => {
                    ids[slot as usize] = effect.id;
                    u.retval = 0
                }
                Err(e) => u.retval = -e.raw_os_error().unwrap_or(libc::EIO),
            }
        } else {
            u.retval = -libc::EINVAL;
        }
        ioctl_ref(virtual_fd, UI_END_FF_UPLOAD, &mut u)?;
    } else if ev.kind == EV_UINPUT && ev.code == UI_FF_ERASE {
        let mut e = UinputFfErase {
            request_id: ev.value as u32,
            ..Default::default()
        };
        ioctl_ref(virtual_fd, UI_BEGIN_FF_ERASE, &mut e)?;
        let slot = e.effect_id as usize;
        if slot < ids.len() && ids[slot] >= 0 {
            let id = ids[slot];
            match ioctl_val(physical_fd, EVIOCRMFF, id as i32) {
                Ok(()) => {
                    ids[slot] = -1;
                    e.retval = 0
                }
                Err(err) => e.retval = -err.raw_os_error().unwrap_or(libc::EIO),
            }
        } else {
            e.retval = -libc::EINVAL;
        }
        ioctl_ref(virtual_fd, UI_END_FF_ERASE, &mut e)?;
    } else if ev.kind == EV_FF {
        let slot = ev.code as usize;
        if slot < ids.len() && ids[slot] >= 0 {
            write_event(physical_fd, EV_FF, ids[slot] as u16, ev.value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod feedback_tests {
    use super::*;
    use crate::linux::{read_event, BTN_SOUTH, EV_KEY};
    use std::os::{fd::AsRawFd, unix::net::UnixStream};

    fn event(kind: u16, code: u16, value: i32) -> InputEvent {
        InputEvent {
            time: unsafe { zeroed() },
            kind,
            code,
            value,
        }
    }

    #[test]
    fn playback_uses_physical_effect_id_and_keeps_value() {
        let (writer, reader) = UnixStream::pair().unwrap();
        let mut ids = [-1; 16];
        ids[2] = 9;
        for value in [0, 3] {
            forward_feedback(-1, writer.as_raw_fd(), event(EV_FF, 2, value), &mut ids).unwrap();
            let forwarded = read_event(reader.as_raw_fd()).unwrap();
            assert_eq!(
                (forwarded.kind, forwarded.code, forwarded.value),
                (EV_FF, 9, value)
            );
        }
        assert_eq!(ids[2], 9);
    }

    #[test]
    fn ignores_unallocated_out_of_range_and_unrelated_events() {
        let mut ids = [-1; 16];
        for event in [
            event(EV_FF, 0, 1),
            event(EV_FF, 16, 1),
            event(EV_FF, u16::MAX, 1),
            event(EV_KEY, BTN_SOUTH, 1),
            event(EV_UINPUT, 99, 1),
        ] {
            forward_feedback(-1, -1, event, &mut ids).unwrap();
        }
        assert_eq!(ids, [-1; 16]);
    }

    #[test]
    fn failed_begin_requests_preserve_effect_ids() {
        let mut ids = [4; 16];
        for code in [UI_FF_UPLOAD, UI_FF_ERASE] {
            let error = forward_feedback(-1, -1, event(EV_UINPUT, code, 12), &mut ids).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        }
        assert_eq!(ids, [4; 16]);
    }

    #[test]
    fn propagates_physical_playback_errors() {
        let mut ids = [4; 16];
        let error = forward_feedback(-1, -1, event(EV_FF, 0, 1), &mut ids).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    }
}
