use std::{
    io,
    os::{fd::AsFd, unix::net::UnixStream},
    path::Path,
    thread::JoinHandle,
};

use evdev::{AbsoluteAxisCode, EventType, InputEvent, KeyCode, SynchronizationCode};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{
    physical_gamepad::PhysicalGamepad, stick_axis_filter::StickAxisFilter,
    virtual_gamepad::VirtualGamepad,
};

pub struct GamepadSession {
    worker: Option<JoinHandle<io::Result<()>>>,
    shutdown: Option<UnixStream>,
    exit_receiver: UnixStream,
}

impl GamepadSession {
    pub fn new(path: &Path) -> io::Result<Self> {
        let physical_gamepad = PhysicalGamepad::open(path)?;
        let virtual_gamepad = VirtualGamepad::new(&path.to_string_lossy())?;

        Self::spawn_worker(format!("gamepad-session {path:?}"), move |shutdown| {
            Self::worker(physical_gamepad, virtual_gamepad, shutdown)
        })
    }

    fn spawn_worker(
        name: String,
        run: impl FnOnce(UnixStream) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let (shutdown, shutdown_receiver) = UnixStream::pair()?;
        let (exit_receiver, completion) = UnixStream::pair()?;
        let worker = std::thread::Builder::new().name(name).spawn(move || {
            // This guard closes on both return and panic, after device cleanup.
            let _completion = completion;
            run(shutdown_receiver)
        })?;

        Ok(Self {
            worker: Some(worker),
            shutdown: Some(shutdown),
            exit_receiver,
        })
    }

    pub fn exit_socket(&self) -> &UnixStream {
        &self.exit_receiver
    }

    /// Stop the worker if necessary and collect its result.
    pub fn finish(mut self) -> io::Result<()> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> io::Result<()> {
        drop(self.shutdown.take());
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("Gamepad worker panicked"))),
            None => Ok(()),
        }
    }

    /// Translate DualSense evdev events to the declared virtual Xbox layout.
    /// Physical stick and trigger ranges are assumed to be 0..=255.
    fn map_event(event: &InputEvent) -> Option<InputEvent> {
        let value = match event.event_type() {
            EventType::KEY => match KeyCode(event.code()) {
                // DualSense evdev labels Square as NORTH and Triangle as WEST.
                KeyCode::BTN_NORTH => {
                    return Some(InputEvent::new(
                        EventType::KEY.0,
                        KeyCode::BTN_WEST.0,
                        event.value(),
                    ));
                }
                KeyCode::BTN_WEST => {
                    return Some(InputEvent::new(
                        EventType::KEY.0,
                        KeyCode::BTN_NORTH.0,
                        event.value(),
                    ));
                }
                KeyCode::BTN_SOUTH
                | KeyCode::BTN_EAST
                | KeyCode::BTN_TL
                | KeyCode::BTN_TR
                | KeyCode::BTN_SELECT
                | KeyCode::BTN_START
                | KeyCode::BTN_MODE
                | KeyCode::BTN_THUMBL
                | KeyCode::BTN_THUMBR => event.value(),
                _ => return None,
            },
            EventType::ABSOLUTE => match AbsoluteAxisCode(event.code()) {
                AbsoluteAxisCode::ABS_X
                | AbsoluteAxisCode::ABS_Y
                | AbsoluteAxisCode::ABS_RX
                | AbsoluteAxisCode::ABS_RY => {
                    let raw = event.value().clamp(0, 255);
                    if raw <= 128 {
                        (raw - 128) * 32768 / 128
                    } else {
                        (raw - 128) * 32767 / 127
                    }
                }
                AbsoluteAxisCode::ABS_Z | AbsoluteAxisCode::ABS_RZ => event.value().clamp(0, 255),
                AbsoluteAxisCode::ABS_HAT0X | AbsoluteAxisCode::ABS_HAT0Y => {
                    event.value().clamp(-1, 1)
                }
                _ => return None,
            },
            _ => return None,
        };

        Some(InputEvent::new(event.event_type().0, event.code(), value))
    }

    fn worker(
        mut physical_gamepad: PhysicalGamepad,
        mut virtual_gamepad: VirtualGamepad,
        shutdown_receiver: UnixStream,
    ) -> io::Result<()> {
        let mut events_frame = Vec::new();
        let mut stick_filters = [
            (AbsoluteAxisCode::ABS_X, StickAxisFilter::new(0.1)),
            (AbsoluteAxisCode::ABS_Y, StickAxisFilter::new(0.1)),
            (AbsoluteAxisCode::ABS_RX, StickAxisFilter::new(0.1)),
            (AbsoluteAxisCode::ABS_RY, StickAxisFilter::new(0.1)),
        ];

        loop {
            let (shutdown_events, input_events) = {
                let mut fds = [
                    PollFd::new(shutdown_receiver.as_fd(), PollFlags::POLLIN),
                    PollFd::new(physical_gamepad.as_fd(), PollFlags::POLLIN),
                ];

                match poll(&mut fds, PollTimeout::NONE) {
                    Ok(_) => {
                        // TODO
                    }
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => {
                        return Err(io::Error::from_raw_os_error(error as i32));
                    }
                }

                (
                    fds[0].revents().unwrap_or(PollFlags::empty()),
                    fds[1].revents().unwrap_or(PollFlags::empty()),
                )
            };

            if !shutdown_events.is_empty() {
                return Ok(());
            }

            let failure = PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL;

            if input_events.intersects(failure) {
                return Err(io::Error::other("Physical gamepad disconnected"));
            }

            if input_events.contains(PollFlags::POLLIN) {
                match physical_gamepad.read_events() {
                    Ok(events) => {
                        for mut event in events {
                            let is_report = event.event_type() == EventType::SYNCHRONIZATION
                                && event.code() == SynchronizationCode::SYN_REPORT.0;

                            if is_report {
                                if !events_frame.is_empty() {
                                    println!("[{}] {events_frame:?}", physical_gamepad.name());
                                    let mapped: Vec<_> =
                                        events_frame.iter().filter_map(Self::map_event).collect();
                                    if !mapped.is_empty() {
                                        virtual_gamepad.emit(&mapped)?;
                                    }
                                    events_frame.clear();
                                }
                            } else {
                                if event.event_type() == EventType::ABSOLUTE {
                                    if let Some((_, filter)) = stick_filters
                                        .iter_mut()
                                        .find(|(axis, _)| axis.0 == event.code())
                                    {
                                        let Some(value) = filter.apply(event.value()) else {
                                            continue;
                                        };
                                        if value != event.value() {
                                            event = InputEvent::new(
                                                event.event_type().0,
                                                event.code(),
                                                value,
                                            );
                                        }
                                    }
                                }
                                events_frame.push(event);
                            }
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

impl Drop for GamepadSession {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_join() {
            eprintln!("Gamepad session failed during cleanup: {error}");
        }
    }
}
