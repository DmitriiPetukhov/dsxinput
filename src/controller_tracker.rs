use std::{
    collections::HashSet,
    error::Error,
    os::fd::{AsFd, BorrowedFd},
    path::PathBuf,
};

use evdev::{AbsoluteAxisCode, KeyCode};

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

pub struct ControllerTracker {
    monitor: udev::MonitorSocket,
    state: ControllerTrackerState,
}

impl ControllerTracker {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let monitor = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .listen()?;

        Ok(Self {
            monitor,
            state: ControllerTrackerState::new(),
        })
    }

    pub fn update(&mut self) -> Result<Vec<ControllerEvent>, Box<dyn Error>> {
        let current = self.scan()?;
        Ok(self.state.update(current))
    }

    pub fn drain_notifications(&self) {
        for _event in self.monitor.iter() {}
    }

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
}

impl AsFd for ControllerTracker {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.monitor.as_fd()
    }
}

fn is_dualsense(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == SONY_VENDOR_ID
        && matches!(product_id, DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID)
}
