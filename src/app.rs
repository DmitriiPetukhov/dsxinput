use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    io,
    os::{fd::AsFd, unix::process::CommandExt},
    path::PathBuf,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{
    cli::Cli,
    controller_tracker::{ControllerEvent, ControllerTracker},
    gamepad_session::GamepadSession,
};

pub(crate) struct App {
    cli: Cli,
}

impl App {
    pub fn new(cli: Cli) -> Self {
        Self { cli }
    }

    pub fn run(&self) {
        if !self.cli.standalone {
            if let Err(error) = self.launch_child_process() {
                eprintln!("Could not start a child process: {error}");
                return;
            }
        }

        if let Err(error) = self.track_controllers() {
            eprintln!("Controller tracking failed: {error}");
        }
    }

    fn launch_child_process(&self) -> Result<(), Box<dyn Error>> {
        if !self.cli.standalone {
            if let Some((executable, args)) = self.cli.command.split_first() {
                std::process::Command::new(executable)
                    .args(args)
                    .process_group(0)
                    .spawn()?;
            }
        }

        Ok(())
    }

    fn track_controllers(&self) -> Result<(), Box<dyn Error>> {
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

            let finished_paths = {
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

                let mut finished_paths = Vec::new();
                for (path, fd) in paths.iter().zip(fds.iter().skip(1)) {
                    let events = fd.revents().unwrap_or(PollFlags::empty());

                    if events.intersects(PollFlags::POLLIN | failure) {
                        finished_paths.push(path.clone());
                    }
                }

                finished_paths
            };

            for path in finished_paths {
                if let Some(session) = sessions.remove(&path) {
                    if let Err(error) = session.finish() {
                        eprintln!("Controller session failed for {path:?}: {error}");
                    }
                }
            }

            tracker.drain_notifications();
        }
    }
}
