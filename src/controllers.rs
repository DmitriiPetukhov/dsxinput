use std::{collections::HashSet, error::Error, ffi::CString, os::fd::AsFd};

use hidapi::{HidApi, HidError};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

const SONY_VENDOR_ID: u16 = 0x054c;
const DUALSENSE_PRODUCT_ID: u16 = 0x0ce6;
const DUALSENSE_EDGE_PRODUCT_ID: u16 = 0x0df2;

pub struct ControllerTracker {
    api: HidApi,
    connected: HashSet<CString>,
    monitor: udev::MonitorSocket,
}

impl ControllerTracker {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let monitor = udev::MonitorBuilder::new()?
            .match_subsystem("hidraw")?
            .listen()?;

        Ok(Self {
            api: HidApi::new()?,
            connected: HashSet::new(),
            monitor,
        })
    }

    pub fn wait_for_change(&mut self) -> Result<(), Box<dyn Error>> {
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

        self.update()?;
        Ok(())
    }

    pub fn update(&mut self) -> Result<(), HidError> {
        self.api.refresh_devices()?;

        let current: HashSet<CString> = self
            .api
            .device_list()
            .filter(|device| is_dualsense(device))
            .map(|device| device.path().to_owned())
            .collect();

        for path in current.difference(&self.connected) {
            println!("Controller connected: {path:?}");
        }

        for path in self.connected.difference(&current) {
            println!("Controller disconnected: {path:?}");
        }

        self.connected = current;
        Ok(())
    }
}

fn is_dualsense(device: &hidapi::DeviceInfo) -> bool {
    device.vendor_id() == SONY_VENDOR_ID
        && matches!(
            device.product_id(),
            DUALSENSE_PRODUCT_ID | DUALSENSE_EDGE_PRODUCT_ID
        )
}
