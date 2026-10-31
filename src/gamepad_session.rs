use std::{io, path::Path};

use crate::{physical_gamepad::PhysicalGamepad, virtual_gamepad::VirtualGamepad};

pub struct GamepadSession {
    physical_gamepad: PhysicalGamepad,
    virtual_gamepad: VirtualGamepad,
}

impl GamepadSession {
    pub fn new(path: &Path) -> io::Result<Self> {
        let physical_gamepad = PhysicalGamepad::open(path)?;
        let virtual_gamepad = VirtualGamepad::new(&path.to_string_lossy())?;

        Ok(Self {
            physical_gamepad,
            virtual_gamepad,
        })
    }
}
