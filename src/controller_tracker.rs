use std::{collections::HashSet, error::Error, os::fd::AsFd, path::PathBuf};

use evdev::{AbsoluteAxisCode, KeyCode};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

const SONY_VENDOR_ID: u16 = 0x054c;
const DUALSENSE_PRODUCT_ID: u16 = 0x0ce6;
const DUALSENSE_EDGE_PRODUCT_ID: u16 = 0x0df2;

#[derive(Debug, PartialEq, Eq)]
pub enum ControllerEvent {
    Connected(PathBuf),
    Disconnected(PathBuf),
}

#[derive(Default)]
struct ControllerTrackerState {
    connected: HashSet<PathBuf>,
}

impl ControllerTrackerState {
    fn new() -> Self {
        Self {
            connected: HashSet::new(),
        }
    }

    fn update(&mut self, current: HashSet<PathBuf>) -> Vec<ControllerEvent> {
        let mut events = Vec::new();
        for path in current.difference(&self.connected) {
            events.push(ControllerEvent::Connected(path.clone()));
        }

        for path in self.connected.difference(&current) {
            events.push(ControllerEvent::Disconnected(path.clone()));
        }

        self.connected = current;
        return events;
    }
}

#[cfg_attr(test, mockall::automock)]
pub(crate) trait ControllerSource {
    fn scan(&mut self) -> Result<HashSet<PathBuf>, Box<dyn Error>>;

    fn wait_for_change(&mut self) -> Result<(), Box<dyn Error>>;
}

pub(crate) struct LinuxControllerSource {
    monitor: udev::MonitorSocket,
}

impl LinuxControllerSource {
    pub(crate) fn new() -> Result<Self, Box<dyn Error>> {
        let monitor = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .listen()?;

        return Ok(Self { monitor });
    }
}

impl ControllerSource for LinuxControllerSource {
    fn scan(&mut self) -> Result<HashSet<PathBuf>, Box<dyn Error>> {
        let mut paths = HashSet::new();

        for (path, device) in evdev::enumerate() {
            let id = device.input_id();
            if !is_dualsense(id.vendor(), id.product()) {
                continue;
            }

            let has_buttons = device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::BTN_SOUTH));

            let has_sticks = device.supported_absolute_axes().is_some_and(|axes| {
                axes.contains(AbsoluteAxisCode::ABS_X) && axes.contains(AbsoluteAxisCode::ABS_Y)
            });

            if has_buttons && has_sticks {
                paths.insert(path);
            }
        }

        Ok(paths)
    }

    fn wait_for_change(&mut self) -> Result<(), Box<dyn Error>> {
        let mut fds = [PollFd::new(self.monitor.as_fd(), PollFlags::POLLIN)];

        loop {
            match poll(&mut fds, PollTimeout::NONE) {
                Ok(_) => break,
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }

        let events = fds[0].revents().unwrap_or(PollFlags::empty());
        if events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
            return Err(std::io::Error::other("udev monitor socket failed").into());
        }

        // Drain the queued events before rescanning.
        for _event in self.monitor.iter() {}

        Ok(())
    }
}

pub struct ControllerTracker<S> {
    source: S,
    state: ControllerTrackerState,
}

impl<S: ControllerSource> ControllerTracker<S> {
    pub fn new(source: S) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            source,
            state: ControllerTrackerState::new(),
        })
    }

    pub fn run(&mut self, mut on_event: impl FnMut(ControllerEvent)) -> Result<(), Box<dyn Error>> {
        loop {
            let current = self.source.scan()?;

            for event in self.state.update(current) {
                on_event(event);
            }

            self.source.wait_for_change()?;
        }
    }
}

fn is_dualsense(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == SONY_VENDOR_ID
        && matches!(product_id, DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod state {
        use super::*;

        fn paths(values: &[&str]) -> HashSet<PathBuf> {
            values.iter().map(|value| PathBuf::from(*value)).collect()
        }

        fn connected(path: &str) -> ControllerEvent {
            ControllerEvent::Connected(PathBuf::from(path))
        }

        fn disconnected(path: &str) -> ControllerEvent {
            ControllerEvent::Disconnected(PathBuf::from(path))
        }

        fn assert_events(actual: Vec<ControllerEvent>, expected: Vec<ControllerEvent>) {
            assert_eq!(
                actual.len(),
                expected.len(),
                "Unexpected events: {actual:?}"
            );
            for event in expected {
                assert!(actual.contains(&event), "Missing {event:?} in {actual:?}");
            }
        }

        #[test]
        fn empty_scans_produce_no_events() {
            let mut state = ControllerTrackerState::new();
            assert!(state.update(paths(&[])).is_empty());
            assert!(state.update(paths(&[])).is_empty());
        }

        #[test]
        fn reports_initial_controller() {
            let mut state = ControllerTrackerState::new();
            assert_eq!(state.update(paths(&["a"])), vec![connected("a")]);
        }

        #[test]
        fn reports_all_initial_controllers() {
            let mut state = ControllerTrackerState::new();
            assert_events(
                state.update(paths(&["a", "b"])),
                vec![connected("a"), connected("b")],
            );
        }

        #[test]
        fn unchanged_controllers_produce_no_events() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a", "b"]));
            assert!(state.update(paths(&["b", "a"])).is_empty());
        }

        #[test]
        fn reports_added_controller_only() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a"]));
            assert_eq!(state.update(paths(&["a", "b"])), vec![connected("b")]);
            assert!(state.update(paths(&["a", "b"])).is_empty());
        }

        #[test]
        fn reports_removed_controller_only() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a", "b"]));
            assert_eq!(state.update(paths(&["b"])), vec![disconnected("a")]);
            assert!(state.update(paths(&["b"])).is_empty());
        }

        #[test]
        fn reports_all_disconnections_only_once() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a", "b"]));
            assert_events(
                state.update(paths(&[])),
                vec![disconnected("a"), disconnected("b")],
            );
            assert!(state.update(paths(&[])).is_empty());
        }

        #[test]
        fn reports_replacement_while_preserving_unchanged_controller() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a", "b"]));
            assert_events(
                state.update(paths(&["b", "c"])),
                vec![disconnected("a"), connected("c")],
            );
            assert!(state.update(paths(&["b", "c"])).is_empty());
        }

        #[test]
        fn reports_reconnection() {
            let mut state = ControllerTrackerState::new();
            state.update(paths(&["a"]));
            state.update(paths(&[]));
            assert_eq!(state.update(paths(&["a"])), vec![connected("a")]);
            assert!(state.update(paths(&["a"])).is_empty());
        }
    }

    mod tracker {
        use super::*;
        use mockall::Sequence;
        use std::{cell::RefCell, rc::Rc};

        fn failure(message: &str) -> Box<dyn Error> {
            std::io::Error::other(message).into()
        }

        #[test]
        fn scan_failure_returns_without_waiting_or_emitting_events() {
            let mut source = MockControllerSource::new();
            source
                .expect_scan()
                .times(1)
                .returning(|| Err(failure("scan failed")));
            source.expect_wait_for_change().never();

            let mut tracker = ControllerTracker::new(source).unwrap();
            let error = tracker.run(|_| panic!("unexpected event")).unwrap_err();

            assert_eq!(error.to_string(), "scan failed");
        }

        #[test]
        fn emits_initial_events_before_waiting_and_returns_wait_failure() {
            let mut source = MockControllerSource::new();
            let mut sequence = Sequence::new();
            let events = Rc::new(RefCell::new(Vec::new()));
            let observed = Rc::clone(&events);
            source
                .expect_scan()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Ok(HashSet::from([PathBuf::from("a")])));
            source
                .expect_wait_for_change()
                .times(1)
                .in_sequence(&mut sequence)
                .returning_st(move || {
                    assert_eq!(
                        *observed.borrow(),
                        vec![ControllerEvent::Connected(PathBuf::from("a"))]
                    );
                    Err(failure("wait failed"))
                });

            let mut tracker = ControllerTracker::new(source).unwrap();
            let error = tracker
                .run(|event| events.borrow_mut().push(event))
                .unwrap_err();

            assert_eq!(error.to_string(), "wait failed");
        }

        #[test]
        fn rescans_after_notifications_without_duplicate_events() {
            let mut source = MockControllerSource::new();
            let mut sequence = Sequence::new();
            for paths in [vec!["a"], vec!["a"], vec![], vec!["a"]] {
                source
                    .expect_scan()
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(move || Ok(paths.iter().map(|path| PathBuf::from(*path)).collect()));
                source
                    .expect_wait_for_change()
                    .times(1)
                    .in_sequence(&mut sequence)
                    .returning(|| Ok(()));
            }
            source
                .expect_scan()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Err(failure("later scan failed")));

            let mut tracker = ControllerTracker::new(source).unwrap();
            let mut events = Vec::new();
            let error = tracker.run(|event| events.push(event)).unwrap_err();

            assert_eq!(error.to_string(), "later scan failed");
            assert_eq!(
                events,
                vec![
                    ControllerEvent::Connected(PathBuf::from("a")),
                    ControllerEvent::Disconnected(PathBuf::from("a")),
                    ControllerEvent::Connected(PathBuf::from("a")),
                ]
            );
        }

        #[test]
        fn empty_scan_emits_no_events_and_still_waits() {
            let mut source = MockControllerSource::new();
            let mut sequence = Sequence::new();
            source
                .expect_scan()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Ok(HashSet::new()));
            source
                .expect_wait_for_change()
                .times(1)
                .in_sequence(&mut sequence)
                .returning(|| Err(failure("stop")));

            let mut tracker = ControllerTracker::new(source).unwrap();
            let error = tracker.run(|_| panic!("unexpected event")).unwrap_err();

            assert_eq!(error.to_string(), "stop");
        }
    }

    mod identification {
        use super::*;

        #[test]
        fn recognizes_dualsense() {
            assert!(is_dualsense(0x054c, 0x0ce6));
        }

        #[test]
        fn recognizes_dualsense_edge() {
            assert!(is_dualsense(0x054c, 0x0df2));
        }

        #[test]
        fn rejects_other_sony_products() {
            assert!(!is_dualsense(0x054c, 0xffff));
        }

        #[test]
        fn rejects_other_vendors_with_matching_product_ids() {
            assert!(!is_dualsense(0xffff, 0x0ce6));
            assert!(!is_dualsense(0xffff, 0x0df2));
        }
    }
}
