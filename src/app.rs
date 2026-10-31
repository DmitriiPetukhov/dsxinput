use std::error::Error;

use crate::controllers::{ControllerEvent, ControllerTracker};

pub fn run() {
    if let Err(error) = track_controllers() {
        eprintln!("Controller tracking failed: {error}");
    }
}

fn track_controllers() -> Result<(), Box<dyn Error>> {
    let mut tracker = ControllerTracker::new()?;

    tracker.run(|event| match event {
        ControllerEvent::Connected(path) => {
            println!("Controller connected: {path:?}");
        }
        ControllerEvent::Disconnected(path) => {
            println!("Controller disconnected: {path:?}");
        }
    })
}
