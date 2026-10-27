//! Synchronize stateful evdev input, including recovery after queue overflow.

use crate::{linux::*, mapping::translate_event};
use std::{io, os::fd::RawFd};

const AXES: [u16; 8] = [
    ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y,
];
const KEYS: [u16; 11] = [
    BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START, BTN_MODE,
    BTN_THUMBL, BTN_THUMBR,
];

pub(crate) struct Snapshot {
    axes: [InputAbsInfo; 18],
    keys: [u8; 96],
}

impl Snapshot {
    pub(crate) fn read(fd: RawFd) -> io::Result<Self> {
        Self::read_with(
            |code, info| ioctl_ref(fd, eviocgabs(code), info),
            |keys| ioctl_ref(fd, EVIOCGKEY, keys),
        )
    }

    fn read_with(
        mut query_axis: impl FnMut(u16, &mut InputAbsInfo) -> io::Result<()>,
        mut query_keys: impl FnMut(&mut [u8; 96]) -> io::Result<()>,
    ) -> io::Result<Self> {
        let mut axes = [InputAbsInfo::default(); 18];
        for code in AXES {
            query_axis(code, &mut axes[code as usize])?;
            if axes[code as usize].minimum >= axes[code as usize].maximum {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid controller axis range for code {code}"),
                ));
            }
        }
        let mut keys = [0; 96];
        query_keys(&mut keys)?;
        Ok(Self { axes, keys })
    }

    fn publish(&self, emit: &mut impl FnMut(u16, u16, i32) -> io::Result<()>) -> io::Result<()> {
        for code in AXES {
            self.publish_event(EV_ABS, code, self.axes[code as usize].value, emit)?;
        }
        for code in KEYS {
            let pressed = i32::from((self.keys[code as usize / 8] >> (code % 8)) & 1);
            self.publish_event(EV_KEY, code, pressed, emit)?;
        }
        emit(EV_SYN, SYN_REPORT, 0)
    }

    fn publish_event(
        &self,
        kind: u16,
        code: u16,
        value: i32,
        emit: &mut impl FnMut(u16, u16, i32) -> io::Result<()>,
    ) -> io::Result<()> {
        let event = InputEvent {
            time: unsafe { std::mem::zeroed() },
            kind,
            code,
            value,
        };
        if let Some((kind, code, value)) = translate_event(event, &self.axes) {
            emit(kind, code, value)?;
        }
        Ok(())
    }
}

pub(crate) struct Relay {
    axes: [InputAbsInfo; 18],
    dropped: bool,
}

impl Relay {
    pub(crate) fn new(
        snapshot: Snapshot,
        emit: &mut impl FnMut(u16, u16, i32) -> io::Result<()>,
    ) -> io::Result<Self> {
        snapshot.publish(emit)?;
        Ok(Self {
            axes: snapshot.axes,
            dropped: false,
        })
    }

    pub(crate) fn process(
        &mut self,
        event: InputEvent,
        snapshot: &mut impl FnMut() -> io::Result<Snapshot>,
        emit: &mut impl FnMut(u16, u16, i32) -> io::Result<()>,
    ) -> io::Result<()> {
        if event.kind == EV_SYN && event.code == SYN_DROPPED {
            self.dropped = true;
            eprintln!("dsxinput: input queue overflow; resynchronizing controller state");
        } else if self.dropped {
            if event.kind == EV_SYN && event.code == SYN_REPORT {
                let current = snapshot()?;
                current.publish(emit)?;
                self.axes = current.axes;
                self.dropped = false;
            }
        } else if let Some((kind, code, value)) = translate_event(event, &self.axes) {
            emit(kind, code, value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        let mut axes = [InputAbsInfo::default(); 18];
        for code in AXES {
            axes[code as usize] = InputAbsInfo {
                minimum: 0,
                maximum: 255,
                value: 128,
                ..Default::default()
            };
        }
        let mut keys = [0u8; 96];
        keys[BTN_SOUTH as usize / 8] |= 1 << (BTN_SOUTH % 8);
        Snapshot { axes, keys }
    }

    fn event(kind: u16, code: u16, value: i32) -> InputEvent {
        InputEvent {
            time: unsafe { std::mem::zeroed() },
            kind,
            code,
            value,
        }
    }

    #[test]
    fn startup_publishes_held_buttons_axes_and_report() {
        let mut output = Vec::new();
        Relay::new(snapshot(), &mut |kind, code, value| {
            output.push((kind, code, value));
            Ok(())
        })
        .unwrap();
        assert!(output.contains(&(EV_KEY, BTN_SOUTH, 1)));
        assert!(output.contains(&(EV_KEY, BTN_EAST, 0)));
        assert!(output.contains(&(EV_ABS, ABS_X, 128)));
        assert!(output.contains(&(EV_ABS, ABS_Z, 128)));
        assert_eq!(output.last(), Some(&(EV_SYN, SYN_REPORT, 0)));
        assert_eq!(output.len(), AXES.len() + KEYS.len() + 1);
    }

    #[test]
    fn dropped_events_are_discarded_until_report_then_state_is_restored() {
        let mut relay = Relay::new(snapshot(), &mut |_, _, _| Ok(())).unwrap();
        let mut output = Vec::new();
        let mut queries = 0;
        let mut query = || {
            queries += 1;
            let state = snapshot();
            Ok(Snapshot {
                keys: [0; 96],
                ..state
            })
        };
        let mut emit = |kind, code, value| {
            output.push((kind, code, value));
            Ok(())
        };
        for ev in [
            event(EV_SYN, SYN_DROPPED, 0),
            event(EV_KEY, BTN_EAST, 1),
            event(EV_SYN, SYN_DROPPED, 0),
            event(EV_ABS, ABS_X, 255),
        ] {
            relay.process(ev, &mut query, &mut emit).unwrap();
        }
        relay
            .process(event(EV_SYN, SYN_REPORT, 0), &mut query, &mut emit)
            .unwrap();
        assert_eq!(queries, 1);
        assert_eq!(output.len(), AXES.len() + KEYS.len() + 1);
        assert!(output.contains(&(EV_KEY, BTN_SOUTH, 0)));
        assert!(!output.contains(&(EV_KEY, BTN_EAST, 1)));
        assert!(!output.contains(&(EV_ABS, ABS_X, 32767)));
        output.clear();
        relay
            .process(
                event(EV_KEY, BTN_EAST, 1),
                &mut || panic!("unexpected resync"),
                &mut |kind, code, value| {
                    output.push((kind, code, value));
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(output, [(EV_KEY, BTN_EAST, 1)]);
    }

    #[test]
    fn failed_recovery_is_reported_without_forwarding_partial_state() {
        let mut relay = Relay::new(snapshot(), &mut |_, _, _| Ok(())).unwrap();
        relay
            .process(
                event(EV_SYN, SYN_DROPPED, 0),
                &mut || panic!(),
                &mut |_, _, _| Ok(()),
            )
            .unwrap();
        let error = relay
            .process(
                event(EV_SYN, SYN_REPORT, 0),
                &mut || Err(std::io::Error::other("query failed")),
                &mut |_, _, _| panic!("unexpected output"),
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "query failed");
    }

    #[test]
    fn state_queries_preserve_kernel_errors() {
        assert_eq!(
            Snapshot::read(-1).err().unwrap().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    fn state(value: i32) -> Snapshot {
        let mut axes = [InputAbsInfo::default(); 18];
        for code in AXES {
            axes[code as usize] = InputAbsInfo {
                minimum: 0,
                maximum: 255,
                value,
                ..Default::default()
            };
        }
        Snapshot {
            axes,
            keys: [0; 96],
        }
    }
    fn event(code: u16) -> InputEvent {
        InputEvent {
            time: unsafe { std::mem::zeroed() },
            kind: EV_SYN,
            code,
            value: 0,
        }
    }

    #[test]
    fn startup_stops_at_each_possible_output_failure() {
        for fail_at in 0..20 {
            let mut calls = 0;
            let error = Relay::new(state(0), &mut |_, _, _| {
                calls += 1;
                if calls == fail_at + 1 {
                    Err(io::Error::from_raw_os_error(libc::ENODEV))
                } else {
                    Ok(())
                }
            })
            .err()
            .expect("failure must propagate");
            assert_eq!(calls, fail_at + 1);
            assert_eq!(error.raw_os_error(), Some(libc::ENODEV));
        }
    }

    #[test]
    fn failed_publication_keeps_recovery_mode_until_a_complete_snapshot() {
        for fail_at in 0..20 {
            let mut relay = Relay::new(state(0), &mut |_, _, _| Ok(())).unwrap();
            relay
                .process(
                    event(SYN_DROPPED),
                    &mut || panic!(),
                    &mut |_, _, _| panic!(),
                )
                .unwrap();
            let mut calls = 0;
            assert!(relay
                .process(event(SYN_REPORT), &mut || Ok(state(255)), &mut |_, _, _| {
                    calls += 1;
                    if calls == fail_at + 1 {
                        Err(io::Error::other("write failed"))
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            assert_eq!(calls, fail_at + 1);
            let input = InputEvent {
                kind: EV_KEY,
                code: BTN_SOUTH,
                value: 1,
                ..event(0)
            };
            relay
                .process(input, &mut || panic!(), &mut |_, _, _| {
                    panic!("must discard stale events")
                })
                .unwrap();
            let mut published = Vec::new();
            relay
                .process(event(SYN_REPORT), &mut || Ok(state(255)), &mut |k, c, v| {
                    published.push((k, c, v));
                    Ok(())
                })
                .unwrap();
            assert_eq!(published.len(), 20);
            assert!(published.contains(&(EV_ABS, ABS_X, 32767)));
            assert!(!relay.dropped);
        }
    }

    #[test]
    fn normal_output_errors_propagate_without_querying_state() {
        let mut relay = Relay::new(state(0), &mut |_, _, _| Ok(())).unwrap();
        let error = relay
            .process(event(SYN_REPORT), &mut || panic!(), &mut |_, _, _| {
                Err(io::Error::from_raw_os_error(libc::EIO))
            })
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
    }
}

#[cfg(test)]
mod snapshot_query_tests {
    use super::*;

    #[test]
    fn rejects_each_invalid_axis_before_querying_buttons() {
        for bad in AXES {
            for (minimum, maximum) in [(0, 0), (1, 0), (i32::MAX, i32::MIN)] {
                let result = Snapshot::read_with(
                    |code, info| {
                        *info = InputAbsInfo {
                            minimum: if code == bad { minimum } else { 0 },
                            maximum: if code == bad { maximum } else { 255 },
                            ..Default::default()
                        };
                        Ok(())
                    },
                    |_| panic!("must reject invalid calibration first"),
                );
                let error = result.err().unwrap();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert!(error.to_string().contains(&bad.to_string()));
            }
        }
    }

    #[test]
    fn query_errors_abort_at_every_axis_and_key_stage() {
        for failure in 0..=AXES.len() {
            let mut calls = 0;
            let result = Snapshot::read_with(
                |_, info| {
                    calls += 1;
                    if calls == failure + 1 {
                        return Err(io::Error::from_raw_os_error(libc::ENODEV));
                    }
                    info.maximum = 255;
                    Ok(())
                },
                |_| Err(io::Error::from_raw_os_error(libc::ENODEV)),
            );
            assert_eq!(result.err().unwrap().raw_os_error(), Some(libc::ENODEV));
            assert_eq!(calls, (failure + 1).min(AXES.len()));
        }
    }
}
