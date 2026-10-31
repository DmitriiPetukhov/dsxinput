use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    io,
    os::fd::AsFd,
    path::PathBuf,
    sync::mpsc,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{
    controller_tracker::{ControllerEvent, ControllerTracker},
    gamepad_session::{GamepadSession, SessionExit},
};

pub fn run() {
    if let Err(error) = track_controllers() {
        eprintln!("Controller tracking failed: {error}");
    }
}

fn track_controllers() -> Result<(), Box<dyn Error>> {
    let mut tracker = ControllerTracker::new()?;
    let mut sessions: HashMap<PathBuf, GamepadSession> = HashMap::new();

    loop {
        for event in tracker.update()? {
            match event {
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
            }
        }

        let failed_paths = {
            let paths: Vec<PathBuf> = sessions.keys().cloned().collect();

            let mut fds = Vec::with_capacity(paths.len() + 1);
            fds.push(PollFd::new(tracker.as_fd(), PollFlags::POLLIN));

            for path in &paths {
                fds.push(PollFd::new(
                    sessions[path].exit_socket().as_fd(),
                    PollFlags::POLLIN,
                ));
            }

            loop {
                match poll(&mut fds, PollTimeout::NONE) {
                    Ok(_) => break,
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => return Err(error.into()),
                }
            }

            let failure = PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL;
            let hotplug_events = fds[0].revents().unwrap_or(PollFlags::empty());
            if hotplug_events.intersects(failure) {
                return Err(io::Error::other("udev monitor failed").into());
            }

            let mut failed_paths = Vec::new();
            for (path, fd) in paths.iter().zip(fds.iter().skip(1)) {
                let events = fd.revents().unwrap_or(PollFlags::empty());

                if (events.intersects(PollFlags::POLLIN | failure)) {
                    failed_paths.push(path.clone());
                }
            }

            failed_paths
        };

        for path in failed_paths {
            if let Some(session) = sessions.remove(&path) {
                match session.read_failure() {
                    Ok(error) => {
                        eprintln!("Controller session failed for {path:?}: {error}");
                    }
                    Err(error) => {
                        eprintln!("Failed to read session failure for {path:?}: {error}");
                    }
                }

                drop(session);
            }
        }

        tracker.drain_notifications();
    }
}
