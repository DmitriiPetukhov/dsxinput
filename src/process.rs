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
