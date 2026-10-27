//! Exercise the actual binary and process ownership without input hardware.
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "wrapper did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn game_runs_and_returns_its_status_without_an_accessible_controller() {
    for path in ["/nonexistent/dsxinput-controller", "/dev/null"] {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_dsxinput"))
                .args(["--device", path, "/bin/sh", "-c", "sleep 0.7; exit 23"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        assert_eq!(wait(&mut child.0).code(), Some(23));
    }
}
#[test]
fn missing_game_command_still_fails_and_empty_command_keeps_usage_status() {
    for (args, expected) in [
        (
            vec![
                "--device",
                "/nonexistent/dsxinput-controller",
                "/nonexistent/dsxinput-game",
            ],
            1,
        ),
        (vec![], 2),
    ] {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_dsxinput"))
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        assert_eq!(wait(&mut child.0).code(), Some(expected));
    }
}
#[test]
fn signal_while_detached_stops_wrapper_and_its_game() {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_dsxinput"))
                .args([
                    "--device",
                    "/nonexistent/dsxinput-controller",
                    "/bin/sh",
                    "-c",
                    "echo $$; exec sleep 30",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        // Poll before reading so even a broken startup cannot hang this test.
        use std::os::fd::AsRawFd;
        let mut fd = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(
            unsafe { libc::poll(&mut fd, 1, 3000) } > 0,
            "game never started"
        );
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        let pid: i32 = line.trim().parse().unwrap();
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, signal) }, 0);
        assert_eq!(wait(&mut child.0).code(), Some(0));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "game survived wrapper shutdown"
        );
    }
}
#[test]
fn detached_diagnostic_can_be_interrupted_without_a_command() {
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_dsxinput"))
            .args(["--device", "/nonexistent/dsxinput-controller", "--diagnose"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = child.0.stderr.take().unwrap();
    use std::os::fd::AsRawFd;
    let mut fd = libc::pollfd {
        fd: stderr.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert!(unsafe { libc::poll(&mut fd, 1, 3000) } > 0);
    let mut line = String::new();
    BufReader::new(stderr).read_line(&mut line).unwrap();
    assert!(line.contains("waiting for controller"));
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) }, 0);
    assert_eq!(wait(&mut child.0).code(), Some(0));
}
