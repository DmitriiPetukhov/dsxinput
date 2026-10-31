use std::{
    ffi::OsString,
    io,
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Command, ExitStatus},
    thread::{self, JoinHandle},
};

pub struct GameProcess {
    completion: UnixStream,
    worker: JoinHandle<io::Result<ExitStatus>>,
}

impl GameProcess {
    pub fn launch(command: &[OsString]) -> io::Result<Self> {
        let (executable, args) = command
            .split_first()
            .ok_or_else(|| io::Error::other("Missing game executable"))?;
        let (completion, worker_completion) = UnixStream::pair()?;

        // Spawn inside the waiter so thread creation cannot orphan an unwaited child.
        let mut command = {
            let mut command = Command::new(executable);
            command.args(args).process_group(0);
            command
        };
        let worker = thread::Builder::new()
            .name("game-process".into())
            .spawn(move || {
                let _completion = worker_completion;
                let mut child = command.spawn()?;
                child.wait()
            })?;

        Ok(Self { completion, worker })
    }

    pub fn completion_socket(&self) -> &UnixStream {
        &self.completion
    }

    pub fn finish(self) -> io::Result<ExitStatus> {
        self.worker
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("Game waiter panicked")))
    }
}
