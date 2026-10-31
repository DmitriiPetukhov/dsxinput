use std::{
    io,
    os::{fd::AsFd, unix::net::UnixStream},
    path::Path,
    thread::JoinHandle,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::{physical_gamepad::PhysicalGamepad, virtual_gamepad::VirtualGamepad};

pub struct GamepadSession {
    worker: Option<JoinHandle<io::Result<()>>>,
    shutdown: Option<UnixStream>,
    exit_receiver: UnixStream,
}

impl GamepadSession {
    pub fn new(path: &Path) -> io::Result<Self> {
        let physical_gamepad = PhysicalGamepad::open(path)?;
        let virtual_gamepad = VirtualGamepad::new(&path.to_string_lossy())?;

        Self::spawn_worker(format!("gamepad-session {path:?}"), move |shutdown| {
            Self::worker(physical_gamepad, virtual_gamepad, shutdown)
        })
    }

    fn spawn_worker(
        name: String,
        run: impl FnOnce(UnixStream) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let (shutdown, shutdown_receiver) = UnixStream::pair()?;
        let (exit_receiver, completion) = UnixStream::pair()?;
        let worker = std::thread::Builder::new().name(name).spawn(move || {
            // This guard closes on both return and panic, after device cleanup.
            let _completion = completion;
            run(shutdown_receiver)
        })?;

        Ok(Self {
            worker: Some(worker),
            shutdown: Some(shutdown),
            exit_receiver,
        })
    }

    pub fn exit_socket(&self) -> &UnixStream {
        &self.exit_receiver
    }

    /// Stop the worker if necessary and collect its result.
    pub fn finish(mut self) -> io::Result<()> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> io::Result<()> {
        drop(self.shutdown.take());
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("Gamepad worker panicked"))),
            None => Ok(()),
        }
    }

    fn worker(
        _physical_gamepad: PhysicalGamepad,
        _virtual_gamepad: VirtualGamepad,
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
        if let Err(error) = self.stop_and_join() {
            eprintln!("Gamepad session failed during cleanup: {error}");
        }
    }
}
