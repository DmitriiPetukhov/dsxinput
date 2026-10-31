use std::{
    io,
    os::{fd::AsFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    thread::JoinHandle,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{physical_gamepad::PhysicalGamepad, virtual_gamepad::VirtualGamepad};

pub struct GamepadSession {
    worker: Option<JoinHandle<()>>,
    shutdown: Option<UnixStream>,
}

impl GamepadSession {
    pub fn new(path: &Path, exit_sender: Sender<SessionExit>) -> io::Result<Self> {
        let physical_gamepad = PhysicalGamepad::open(path)?;
        let virtual_gamepad = VirtualGamepad::new(&path.to_string_lossy())?;

        // Create a shutdown signal socket.
        let (shutdown, shutdown_receiver) = UnixStream::pair()?;

        // Create a background worker thread that will be handling events from the gamepad.
        let controller_path = path.to_owned();
        let worker = std::thread::Builder::new()
            .name(format!("gamepad-session {path:?}"))
            .spawn(move || {
                let result = Self::worker(physical_gamepad, virtual_gamepad, shutdown_receiver);

                let _ = exit_sender.send(SessionExit {
                    path: controller_path,
                    result,
                });
            })?;

        Ok(Self {
            worker: Some(worker),
            shutdown: Some(shutdown),
        })
    }

    fn worker(
        physical_gamepad: PhysicalGamepad,
        virtual_gamepad: VirtualGamepad,
        shutdown_receiver: UnixStream,
    ) -> io::Result<()> {
        let mut fds = [PollFd::new(shutdown_receiver.as_fd(), PollFlags::POLLIN)];

        loop {
            match poll(&mut fds, PollTimeout::NONE) {
                Ok(_) => {
                    // TODO
                }
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => {
                    eprintln!("Session poll failed: {error}");
                    return Err(io::Error::from_raw_os_error(error as i32));
                }
            }

            let events = fds[0].revents().unwrap_or(PollFlags::empty());

            if !events.is_empty() {
                return Ok(());
            }
        }
    }
}

impl Drop for GamepadSession {
    fn drop(&mut self) {
        drop(self.shutdown.take());

        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("Gamepad worker panicked");
            }
        }
    }
}

pub(crate) struct SessionExit {
    pub(crate) path: PathBuf,
    pub(crate) result: io::Result<()>,
}
