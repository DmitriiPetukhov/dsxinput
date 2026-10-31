use std::{
    io,
    os::{
        fd::AsFd,
        unix::net::{UnixDatagram, UnixStream},
    },
    path::{Path, PathBuf},
    thread::JoinHandle,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{physical_gamepad::PhysicalGamepad, virtual_gamepad::VirtualGamepad};

pub struct GamepadSession {
    worker: Option<JoinHandle<()>>,
    shutdown: Option<UnixStream>,
    exit_receiver: UnixDatagram,
}

impl GamepadSession {
    pub fn new(path: &Path) -> io::Result<Self> {
        let physical_gamepad = PhysicalGamepad::open(path)?;
        let virtual_gamepad = VirtualGamepad::new(&path.to_string_lossy())?;

        // Create a shutdown signal socket.
        let (shutdown, shutdown_receiver) = UnixStream::pair()?;

        //
        let (exit_receiver, exit_sender) = UnixDatagram::pair()?;
        exit_receiver.set_nonblocking(true)?;
        exit_sender.set_nonblocking(true)?;

        // Create a background worker thread that will be handling events from the gamepad.
        let worker = std::thread::Builder::new()
            .name(format!("gamepad-session {path:?}"))
            .spawn(move || {
                let result = Self::worker(physical_gamepad, virtual_gamepad, shutdown_receiver);

                if let Err(error) = result {
                    let message = error.to_string();

                    // Keep the single notification small.
                    let bytes = message.as_bytes();
                    let bytes = &bytes[..bytes.len().min(4096)];

                    loop {
                        match exit_sender.send(bytes) {
                            Ok(_) => break,
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(error) => {
                                eprintln!("Failed to send session failure: {error}");
                                break;
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            worker: Some(worker),
            shutdown: Some(shutdown),
            exit_receiver,
        })
    }

    pub fn exit_socket(&self) -> &UnixDatagram {
        &self.exit_receiver
    }

    pub fn read_failure(&self) -> io::Result<String> {
        let mut buffer = [0_u8; 4096];

        loop {
            match self.exit_receiver.recv(&mut buffer) {
                Ok(length) => {
                    return Ok(String::from_utf8_lossy(&buffer[..length]).into_owned());
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
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
