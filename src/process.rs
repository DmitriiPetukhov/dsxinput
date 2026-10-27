//! Launch the game in an isolated process group.
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
};

/// Inherit the terminal and create a process group for shutdown signaling.
pub(crate) fn spawn_game(args: &[String]) -> io::Result<Child> {
    let (program, arguments) = args
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "game command is empty"))?;
    let mut cmd = Command::new(program);
    cmd.args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    cmd.process_group(0);
    cmd.spawn()
}

/// Own the child so runtime errors and signals share bounded shutdown.
pub(crate) struct Game {
    child: Option<Child>,
}

impl Game {
    pub(crate) fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        let result = self
            .child
            .as_mut()
            .expect("game already reaped")
            .try_wait()?;
        if result.is_some() {
            self.child = None;
        }
        Ok(result)
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let Some(mut child) = self.child.take() else {
            return;
        };
        let group = -(child.id() as i32);
        unsafe {
            libc::kill(group, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let reaped = matches!(child.try_wait(), Ok(Some(_)));
            let group_gone = unsafe { libc::kill(group, 0) } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if reaped && group_gone {
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        eprintln!("dsxinput: game process group did not stop after SIGTERM; sending SIGKILL");
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        // Also target the direct child if it moved out of its original group.
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let reaped = matches!(child.try_wait(), Ok(Some(_)));
            let group_gone = unsafe { libc::kill(group, 0) } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if reaped && group_gone {
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Uninterruptible kernel waits must not hold controller resources open.
        eprintln!("dsxinput: child still exiting; reaping in background");
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[test]
    fn forwards_child_arguments_and_exit_status() {
        let args = ["/bin/sh", "-c", "exit \"$1\"", "game", "7"].map(String::from);
        assert_eq!(spawn_game(&args).unwrap().wait().unwrap().code(), Some(7));
    }

    #[test]
    fn missing_executable_returns_not_found() {
        let args = ["/nonexistent/dsxinput-test-game".to_owned()];
        assert_eq!(
            spawn_game(&args).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn child_has_its_own_process_group() {
        let args = ["/bin/sleep", "30"].map(String::from);
        let mut child = spawn_game(&args).unwrap();
        let pid = child.id() as i32;
        let group = unsafe { libc::getpgid(pid) };
        // Reap the test process before asserting, including on a group mismatch.
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(group, pid);
    }
}

#[cfg(test)]
mod empty_command_tests {
    use super::*;

    #[test]
    fn empty_command_is_an_input_error() {
        assert_eq!(
            spawn_game(&[]).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        time::{Duration, Instant},
    };

    fn running(script: &str) -> Child {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .process_group(0)
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        child
    }

    #[test]
    fn shutdown_escalates_and_reaps_a_child_ignoring_term() {
        let child = running("trap '' TERM; echo ready; while :; do sleep 1; done");
        let pid = child.id() as i32;
        let started = Instant::now();
        drop(Game::new(child));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn shutdown_escalates_after_launcher_exits_with_stubborn_descendant() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "(trap '' TERM; echo ready; exec sleep 30) & wait"])
            .process_group(0)
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let group = -(child.id() as i32);
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        let fd = output.get_ref().as_raw_fd();
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
        }
        drop(Game::new(child));
        let result = output.read(&mut [0u8; 1]);
        // Ensure even a failing test cannot leak the descendant.
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        assert!(
            matches!(result, Ok(0)),
            "descendant still holds pipe: {result:?}"
        );
    }

    #[test]
    fn shutdown_handles_launcher_already_exited_before_cleanup() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "(trap '' TERM; echo ready; exec sleep 30) & exit 0"])
            .process_group(0)
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let group = -(child.id() as i32);
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        let fd = output.get_ref().as_raw_fd();
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
        }
        while child.try_wait().unwrap().is_none() {
            std::thread::yield_now();
        }
        drop(Game::new(child));
        let result = output.read(&mut [0u8; 1]);
        // Ensure even a failing test cannot leak the descendant.
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        assert!(
            matches!(result, Ok(0)),
            "descendant still holds pipe: {result:?}"
        );
    }

    #[test]
    fn runtime_error_drops_and_reaps_the_game() {
        let child = running("echo ready; exec sleep 30");
        let pid = child.id() as i32;
        fn fail(child: Child) -> io::Result<()> {
            let _game = Game::new(child);
            Err(io::Error::other("input failed"))
        }
        let result = fail(child);
        assert!(result.is_err());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
