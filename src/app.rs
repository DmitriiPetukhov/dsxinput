use std::{thread, time::Duration};

use crate::controllers::{self, ControllerTracker};

pub fn run() {
    let mut tracker = match ControllerTracker::new() {
        Ok(tracker) => tracker,
        Err(error) => {
            eprintln!("Failed to initialize controller tracking: {error}");
            return;
        }
    };

    loop {
        if let Err(error) = tracker.update() {
            eprintln!("Failed to refresh controllers: {error}");
        }

        thread::sleep(Duration::from_millis(500));
    }
}
