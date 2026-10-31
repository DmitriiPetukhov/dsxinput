use std::error::Error;

use crate::{
    controller_tracker::{ControllerEvent, ControllerTracker, LinuxControllerSource},
    virtual_gamepad::VirtualGamepad,
    virtual_gamepad_manager::VirtualGamepadManager,
};

pub fn run() {
    if let Err(error) = track_controllers() {
        eprintln!("Controller tracking failed: {error}");
    }
}

fn track_controllers() -> Result<(), Box<dyn Error>> {
    let mut tracker = ControllerTracker::new(LinuxControllerSource::new()?)?;
    let mut manager = VirtualGamepadManager::new(VirtualGamepad::new);

    tracker.run(|event| match event {
        ControllerEvent::Connected(path) => match manager.create(&path) {
            Ok(()) => println!("Controller connected: {path:?}"),
            Err(error) => {
                eprintln!("Failed to create virtual gamepad for {path:?}: {error}");
            }
        },
        ControllerEvent::Disconnected(path) => {
            manager.remove(&path);
            println!("Controller disconnected: {path:?}");
        }
    })
}
