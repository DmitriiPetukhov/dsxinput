use std::{
    io,
    os::fd::{AsFd, BorrowedFd},
    path::Path,
};

use evdev::{Device, InputEvent};

pub struct PhysicalGamepad {
    device: Device,
}

impl PhysicalGamepad {
    pub fn open(path: &Path) -> io::Result<Self> {
        let device = Device::open(path)?;
        device.set_nonblocking(true)?;

        Ok(Self { device })
    }

    pub fn name(&self) -> &str {
        self.device.name().unwrap_or("unknown_gamepad")
    }

    pub fn read_events(&mut self) -> io::Result<Vec<InputEvent>> {
        let events = self.device.fetch_events()?.collect();
        Ok(events)
    }
}

impl AsFd for PhysicalGamepad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}
