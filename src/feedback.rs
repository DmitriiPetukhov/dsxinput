//! Forward virtual rumble effects to physical controller effect IDs.
use crate::linux::{
    ioctl_ref, ioctl_val, write_event, InputEvent, UinputFfErase, UinputFfUpload, EVIOCRMFF,
    EVIOCSFF, EV_FF, EV_UINPUT, FF_RUMBLE, UI_BEGIN_FF_ERASE, UI_BEGIN_FF_UPLOAD, UI_END_FF_ERASE,
    UI_END_FF_UPLOAD, UI_FF_ERASE, UI_FF_UPLOAD,
};
use std::{io, mem::zeroed, os::fd::RawFd};

/// Effects belong to the persistent virtual device; physical IDs do not.
pub(crate) struct FeedbackState {
    effects: [Option<crate::linux::FfEffect>; 16],
    ids: [i16; 16],
}
impl Default for FeedbackState {
    fn default() -> Self {
        Self {
            effects: [None; 16],
            ids: [-1; 16],
        }
    }
}
impl FeedbackState {
    pub(crate) fn process(
        &mut self,
        virtual_fd: RawFd,
        physical_fd: RawFd,
        event: InputEvent,
    ) -> io::Result<()> {
        forward_cached(
            &mut Kernel {
                virtual_fd,
                physical_fd,
            },
            event,
            &mut self.ids,
            &mut self.effects,
        )
    }

    pub(crate) fn attach(&mut self, fd: RawFd) {
        self.attach_with(|effect| ioctl_ref(fd, EVIOCSFF, effect));
    }

    fn attach_with(
        &mut self,
        mut upload: impl FnMut(&mut crate::linux::FfEffect) -> io::Result<()>,
    ) {
        self.ids = [-1; 16];
        for (slot, effect) in self.effects.iter().enumerate() {
            if let Some(mut effect) = *effect {
                effect.id = -1;
                match upload(&mut effect) {
                    Ok(()) => self.ids[slot] = effect.id,
                    Err(error) => {
                        eprintln!("dsxinput: failed to restore rumble slot {slot}: {error}")
                    }
                }
            }
        }
    }

    pub(crate) fn detach(&mut self, fd: RawFd) {
        for (slot, id) in self.ids.iter_mut().enumerate() {
            if *id >= 0 {
                let _ = ioctl_val(fd, EVIOCRMFF, *id as i32);
            }
            // Placeholder IDs let uploads and erases complete while disconnected.
            *id = if self.effects[slot].is_some() { 0 } else { -1 };
        }
    }
}

#[cfg(test)]
/// Handle an upload, erase, or playback event, updating IDs only on success.
pub(crate) fn forward_feedback(
    virtual_fd: RawFd,
    physical_fd: RawFd,
    ev: InputEvent,
    ids: &mut [i16; 16],
) -> io::Result<()> {
    forward_with(
        &mut Kernel {
            virtual_fd,
            physical_fd,
        },
        ev,
        ids,
    )
}

trait FeedbackIo {
    fn begin_upload(&mut self, upload: &mut UinputFfUpload) -> io::Result<()>;
    fn end_upload(&mut self, upload: &mut UinputFfUpload) -> io::Result<()>;
    fn upload(&mut self, effect: &mut crate::linux::FfEffect) -> io::Result<()>;
    fn begin_erase(&mut self, erase: &mut UinputFfErase) -> io::Result<()>;
    fn end_erase(&mut self, erase: &mut UinputFfErase) -> io::Result<()>;
    fn erase(&mut self, id: i32) -> io::Result<()>;
    fn play(&mut self, id: u16, value: i32) -> io::Result<()>;
}

struct Kernel {
    virtual_fd: RawFd,
    physical_fd: RawFd,
}
impl FeedbackIo for Kernel {
    fn begin_upload(&mut self, u: &mut UinputFfUpload) -> io::Result<()> {
        ioctl_ref(self.virtual_fd, UI_BEGIN_FF_UPLOAD, u)
    }
    fn end_upload(&mut self, u: &mut UinputFfUpload) -> io::Result<()> {
        ioctl_ref(self.virtual_fd, UI_END_FF_UPLOAD, u)
    }
    fn upload(&mut self, e: &mut crate::linux::FfEffect) -> io::Result<()> {
        if self.physical_fd < 0 {
            e.id = 0;
            Ok(())
        } else {
            ioctl_ref(self.physical_fd, EVIOCSFF, e)
        }
    }
    fn begin_erase(&mut self, e: &mut UinputFfErase) -> io::Result<()> {
        ioctl_ref(self.virtual_fd, UI_BEGIN_FF_ERASE, e)
    }
    fn end_erase(&mut self, e: &mut UinputFfErase) -> io::Result<()> {
        ioctl_ref(self.virtual_fd, UI_END_FF_ERASE, e)
    }
    fn erase(&mut self, id: i32) -> io::Result<()> {
        if self.physical_fd < 0 {
            Ok(())
        } else {
            ioctl_val(self.physical_fd, EVIOCRMFF, id)
        }
    }
    fn play(&mut self, id: u16, value: i32) -> io::Result<()> {
        if self.physical_fd < 0 {
            Ok(())
        } else {
            write_event(self.physical_fd, EV_FF, id, value)
        }
    }
}

#[cfg(test)]
fn forward_with(
    backend: &mut impl FeedbackIo,
    ev: InputEvent,
    ids: &mut [i16; 16],
) -> io::Result<()> {
    forward_cached(backend, ev, ids, &mut [None; 16])
}

fn forward_cached(
    backend: &mut impl FeedbackIo,
    ev: InputEvent,
    ids: &mut [i16; 16],
    effects: &mut [Option<crate::linux::FfEffect>; 16],
) -> io::Result<()> {
    if ev.kind == EV_UINPUT && ev.code == UI_FF_UPLOAD {
        let mut u: UinputFfUpload = unsafe { zeroed() };
        u.request_id = ev.value as u32;
        backend.begin_upload(&mut u)?;
        let slot = u.effect.id;
        if slot >= 0 && (slot as usize) < ids.len() && u.effect.type_ == FF_RUMBLE {
            let mut effect = u.effect;
            effect.id = ids[slot as usize];
            match backend.upload(&mut effect) {
                Ok(()) => {
                    ids[slot as usize] = effect.id;
                    effects[slot as usize] = Some(u.effect);
                    u.retval = 0
                }
                Err(e) => u.retval = -e.raw_os_error().unwrap_or(libc::EIO),
            }
        } else {
            u.retval = -libc::EINVAL;
        }
        backend.end_upload(&mut u)?;
    } else if ev.kind == EV_UINPUT && ev.code == UI_FF_ERASE {
        let mut e = UinputFfErase {
            request_id: ev.value as u32,
            ..Default::default()
        };
        backend.begin_erase(&mut e)?;
        let slot = e.effect_id as usize;
        if slot < ids.len() && ids[slot] >= 0 {
            let id = ids[slot];
            match backend.erase(id as i32) {
                Ok(()) => {
                    ids[slot] = -1;
                    effects[slot] = None;
                    e.retval = 0
                }
                Err(err) => e.retval = -err.raw_os_error().unwrap_or(libc::EIO),
            }
        } else if slot < effects.len() && effects[slot].is_some() {
            effects[slot] = None;
            e.retval = 0;
        } else {
            e.retval = -libc::EINVAL;
        }
        backend.end_erase(&mut e)?;
    } else if ev.kind == EV_FF {
        let slot = ev.code as usize;
        if slot < ids.len() && ids[slot] >= 0 {
            backend.play(ids[slot] as u16, ev.value)?;
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
        let error = forward_feedback(-1, i32::MAX, event(EV_FF, 0, 1), &mut ids).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use crate::linux::FfEffect;

    struct Fake {
        slot: i16,
        kind: u16,
        physical_error: Option<i32>,
        end_error: bool,
        calls: Vec<&'static str>,
        retval: Option<i32>,
        previous_id: Option<i16>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                slot: 0,
                kind: FF_RUMBLE,
                physical_error: None,
                end_error: false,
                calls: Vec::new(),
                retval: None,
                previous_id: None,
            }
        }
    }
    impl FeedbackIo for Fake {
        fn begin_upload(&mut self, u: &mut UinputFfUpload) -> io::Result<()> {
            self.calls.push("begin");
            assert_eq!(u.request_id, 42);
            u.effect.id = self.slot;
            u.effect.type_ = self.kind;
            Ok(())
        }
        fn end_upload(&mut self, u: &mut UinputFfUpload) -> io::Result<()> {
            self.calls.push("end");
            self.retval = Some(u.retval);
            if self.end_error {
                Err(io::Error::from_raw_os_error(libc::ENODEV))
            } else {
                Ok(())
            }
        }
        fn upload(&mut self, effect: &mut FfEffect) -> io::Result<()> {
            self.calls.push("upload");
            self.previous_id = Some(effect.id);
            if let Some(errno) = self.physical_error {
                return Err(io::Error::from_raw_os_error(errno));
            }
            effect.id = 9;
            Ok(())
        }
        fn begin_erase(&mut self, e: &mut UinputFfErase) -> io::Result<()> {
            self.calls.push("begin");
            assert_eq!(e.request_id, 42);
            e.effect_id = self.slot as u32;
            Ok(())
        }
        fn end_erase(&mut self, e: &mut UinputFfErase) -> io::Result<()> {
            self.calls.push("end");
            self.retval = Some(e.retval);
            if self.end_error {
                Err(io::Error::from_raw_os_error(libc::ENODEV))
            } else {
                Ok(())
            }
        }
        fn erase(&mut self, id: i32) -> io::Result<()> {
            self.calls.push("erase");
            assert_eq!(id, 7);
            if let Some(errno) = self.physical_error {
                Err(io::Error::from_raw_os_error(errno))
            } else {
                Ok(())
            }
        }
        fn play(&mut self, _: u16, _: i32) -> io::Result<()> {
            panic!("unexpected playback")
        }
    }
    fn request(code: u16) -> InputEvent {
        InputEvent {
            time: unsafe { zeroed() },
            kind: EV_UINPUT,
            code,
            value: 42,
        }
    }

    #[test]
    fn uploads_allocate_update_and_address_every_slot() {
        for slot in 0..16 {
            for previous in [-1, 7] {
                let mut io = Fake {
                    slot,
                    ..Default::default()
                };
                let mut ids = [-1; 16];
                ids[slot as usize] = previous;
                forward_with(&mut io, request(UI_FF_UPLOAD), &mut ids).unwrap();
                assert_eq!(io.previous_id, Some(previous));
                assert_eq!(io.calls, ["begin", "upload", "end"]);
                assert_eq!(io.retval, Some(0));
                for (index, id) in ids.iter().enumerate() {
                    assert_eq!(*id, if index == slot as usize { 9 } else { -1 });
                }
            }
        }
    }

    #[test]
    fn invalid_uploads_complete_handshake_without_touching_physical_device() {
        for (slot, kind) in [
            (-1, FF_RUMBLE),
            (16, FF_RUMBLE),
            (i16::MAX, FF_RUMBLE),
            (0, 0),
        ] {
            let mut io = Fake {
                slot,
                kind,
                ..Default::default()
            };
            let mut ids = [7; 16];
            forward_with(&mut io, request(UI_FF_UPLOAD), &mut ids).unwrap();
            assert_eq!(io.calls, ["begin", "end"]);
            assert_eq!(io.retval, Some(-libc::EINVAL));
            assert_eq!(ids, [7; 16]);
        }
    }

    #[test]
    fn physical_errors_are_acknowledged_and_mapping_survives() {
        for code in [UI_FF_UPLOAD, UI_FF_ERASE] {
            for errno in [
                libc::ENOSPC,
                libc::ENODEV,
                libc::EIO,
                libc::EINVAL,
                libc::EACCES,
            ] {
                let mut io = Fake {
                    physical_error: Some(errno),
                    ..Default::default()
                };
                let mut ids = [7; 16];
                forward_with(&mut io, request(code), &mut ids).unwrap();
                assert_eq!(io.retval, Some(-errno));
                assert_eq!(io.calls.last(), Some(&"end"));
                assert_eq!(ids, [7; 16]);
            }
        }
    }

    #[test]
    fn erase_releases_only_allocated_slot_and_rejects_invalid_ids() {
        for slot in [-1, 0, 15, 16, i16::MAX] {
            let mut io = Fake {
                slot,
                ..Default::default()
            };
            let mut ids = [7; 16];
            forward_with(&mut io, request(UI_FF_ERASE), &mut ids).unwrap();
            let valid = (0..16).contains(&slot);
            assert_eq!(io.retval, Some(if valid { 0 } else { -libc::EINVAL }));
            for (index, id) in ids.iter().enumerate() {
                assert_eq!(
                    *id,
                    if valid && index == slot as usize {
                        -1
                    } else {
                        7
                    }
                );
            }
        }
        let mut io = Fake::default();
        let mut ids = [-1; 16];
        forward_with(&mut io, request(UI_FF_ERASE), &mut ids).unwrap();
        assert_eq!(io.calls, ["begin", "end"]);
        assert_eq!(io.retval, Some(-libc::EINVAL));
    }

    #[test]
    fn cached_effect_can_be_erased_after_physical_restore_failed() {
        let mut backend = Fake::default();
        let mut ids = [-1; 16];
        let mut effects = [None; 16];
        forward_cached(&mut backend, request(UI_FF_UPLOAD), &mut ids, &mut effects).unwrap();
        assert!(effects[0].is_some());
        ids[0] = -1; // Reconnection could not allocate a physical effect.
        backend.calls.clear();
        forward_cached(&mut backend, request(UI_FF_ERASE), &mut ids, &mut effects).unwrap();
        assert_eq!(backend.retval, Some(0));
        assert!(effects[0].is_none());
        assert_eq!(backend.calls, ["begin", "end"]);
    }

    #[test]
    fn failed_acknowledgement_preserves_actual_physical_state() {
        for code in [UI_FF_UPLOAD, UI_FF_ERASE] {
            let mut io = Fake {
                end_error: true,
                ..Default::default()
            };
            let mut ids = [7; 16];
            let error = forward_with(&mut io, request(code), &mut ids).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::ENODEV));
            assert_eq!(ids[0], if code == UI_FF_UPLOAD { 9 } else { -1 });
        }
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    #[test]
    fn reconnect_restores_effects_under_the_same_virtual_slots() {
        let mut state = FeedbackState::default();
        let mut effect: crate::linux::FfEffect = unsafe { zeroed() };
        effect.type_ = FF_RUMBLE;
        effect.id = 3;
        state.effects[3] = Some(effect);
        state.ids[3] = 9;
        state.detach(-1);
        let mut uploads = 0;
        state.attach_with(|effect| {
            uploads += 1;
            assert_eq!(effect.id, -1, "old physical ID must not be reused");
            effect.id = 12;
            Ok(())
        });
        assert_eq!(uploads, 1);
        assert_eq!(state.ids[3], 12);
        assert!(state.effects[3].is_some());
    }
}
