use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    path::PathBuf,
};

use crate::{
    controller_tracker::{ControllerEvent, ControllerTracker, LinuxControllerSource},
    gamepad_session::GamepadSession,
};

pub fn run() {
    if let Err(error) = track_controllers() {
        eprintln!("Controller tracking failed: {error}");
    }
}

fn track_controllers() -> Result<(), Box<dyn Error>> {
    let mut tracker = ControllerTracker::new(LinuxControllerSource::new()?)?;
    let mut sessions: HashMap<PathBuf, GamepadSession> = HashMap::new();

    tracker.run(|event| match event {
        ControllerEvent::Connected(path) => {
            if let Entry::Vacant(entry) = sessions.entry(path.clone()) {
                match GamepadSession::new(&path) {
                    Ok(session) => {
                        entry.insert(session);
                        println!("Controller connected: {path:?}");
                    }
                    Err(error) => {
                        eprintln!("Failed to create session for {path:?}: {error}");
                    }
                }
            }
        }
        ControllerEvent::Disconnected(path) => {
            sessions.remove(&path);
            println!("Controller disconnected: {path:?}");
        }
    })
}
