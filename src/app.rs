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
    let (exit_sender, exit_receiver) = mpsc::channel::<SessionExit>();

    loop {
        while let Ok(exit) = exit_receiver.try_recv() {
            if let Err(error) = exit.result {
                sessions.remove(&exit.path);
                eprintln!("Controller session failed for {:?}: {error}", exit.path);
            }
        }

        for event in tracker.update()? {
            match event {
                ControllerEvent::Connected(path) => {
                    if let Entry::Vacant(entry) = sessions.entry(path.clone()) {
                        match GamepadSession::new(&path, exit_sender.clone()) {
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

        {
            let mut fds = [PollFd::new(tracker.as_fd(), PollFlags::POLLIN)];

            loop {
                match poll(&mut fds, PollTimeout::NONE) {
                    Ok(_) => break,
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => return Err(error.into()),
                }
            }

            let events = fds[0].revents().unwrap_or(PollFlags::empty());

            if events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
                return Err(io::Error::other("udev monitor failed").into());
            }
        }

        tracker.drain_notifications();
    }
}
