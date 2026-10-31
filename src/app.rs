use std::error::Error;

use crate::controllers::ControllerTracker;

pub fn run() {
    if let Err(error) = track_controllers() {
        eprintln!("Controller tracking failed: {error}");
    }
}

fn track_controllers() -> Result<(), Box<dyn Error>> {
    let mut tracker = ControllerTracker::new()?;

    tracker.update()?;

    loop {
        tracker.wait_for_change()?;
    }
}
