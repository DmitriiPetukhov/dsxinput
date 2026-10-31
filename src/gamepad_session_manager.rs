use std::{
    collections::{HashMap, hash_map::Entry},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use crate::{controller_tracker::ControllerEvent, gamepad_session::GamepadSession};

#[derive(Default)]
pub struct GamepadSessionManager {
    sessions: HashMap<PathBuf, GamepadSession>,
}

impl GamepadSessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_controller_events(&mut self, events: Vec<ControllerEvent>) {
        for event in events {
            match event {
                ControllerEvent::Connected(path) => {
                    if let Entry::Vacant(entry) = self.sessions.entry(path.clone()) {
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
                    self.sessions.remove(&path);
                    println!("Controller disconnected: {path:?}");
                }
            }
        }
    }

    pub fn completion_sockets(&self) -> Vec<(&Path, &UnixStream)> {
        self.sessions
            .iter()
            .map(|(path, session)| (path.as_path(), session.exit_socket()))
            .collect()
    }

    pub fn collect_finished(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if let Some(session) = self.sessions.remove(&path) {
                if let Err(error) = session.finish() {
                    eprintln!("Controller session failed for {path:?}: {error}");
                }
            }
        }
    }
}
