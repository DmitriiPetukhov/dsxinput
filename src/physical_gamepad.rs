use std::{io, path::Path};

use evdev::{Device, InputEvent};

pub struct PhysicalGamepad {
    device: Device,
}

impl PhysicalGamepad {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            device: Device::open(path)?,
        })
    }

    pub fn read_events(&mut self) -> io::Result<Vec<InputEvent>> {
        let events = self.device.fetch_events()?.collect();
        Ok(events)
    }
}
