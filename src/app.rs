use std::{error::Error, io, os::fd::AsFd};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{
    cli::Cli, controller_tracker::ControllerTracker, game_process::GameProcess,
    gamepad_session_manager::GamepadSessionManager,
};

pub(crate) struct App {
    cli: Cli,
}

impl App {
    pub fn new(cli: Cli) -> Self {
        Self { cli }
    }

    pub fn run(&self) {
        if let Err(error) = self.run_event_loop() {
            eprintln!("Application failed: {error}");
        }
    }

    fn run_event_loop(&self) -> Result<(), Box<dyn Error>> {
        let mut tracker = ControllerTracker::new()?;
        let mut sessions = GamepadSessionManager::new();
        sessions.handle_controller_events(tracker.update()?);
        let mut game = if self.cli.standalone {
            None
        } else {
            Some(GameProcess::launch(&self.cli.command)?)
        };

        loop {
            let (finished_paths, game_finished) = {
                let watched = sessions.completion_sockets();

                let mut fds = Vec::with_capacity(watched.len() + 2);
                fds.push(PollFd::new(tracker.as_fd(), PollFlags::POLLIN));

                for (_, socket) in &watched {
                    fds.push(PollFd::new(socket.as_fd(), PollFlags::POLLIN));
                }

                let game_index = game.as_ref().map(|game| {
                    let index = fds.len();
                    fds.push(PollFd::new(
                        game.completion_socket().as_fd(),
                        PollFlags::POLLIN,
                    ));
                    index
                });

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
                for ((path, _), fd) in watched.iter().zip(fds.iter().skip(1)) {
                    let events = fd.revents().unwrap_or(PollFlags::empty());

                    if events.intersects(PollFlags::POLLIN | failure) {
                        finished_paths.push((*path).to_owned());
                    }
                }

                let game_finished = game_index.is_some_and(|index| {
                    !fds[index]
                        .revents()
                        .unwrap_or(PollFlags::empty())
                        .is_empty()
                });

                (finished_paths, game_finished)
            };

            if game_finished {
                if let Some(game) = game.take() {
                    let status = game.finish()?;
                    println!("Game exited: {status}");
                }
                return Ok(());
            }

            sessions.collect_finished(finished_paths);

            tracker.drain_notifications();
            sessions.handle_controller_events(tracker.update()?);
        }
    }
}
