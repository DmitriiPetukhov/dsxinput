use super::*;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

struct Pad(Rc<RefCell<Vec<String>>>, usize);
impl Drop for Pad {
    fn drop(&mut self) {
        self.0.borrow_mut().push(format!("drop:{}", self.1));
    }
}
struct Fake {
    now: Duration,
    stop_at: Duration,
    exit_at: Option<(Duration, i32)>,
    connections: VecDeque<io::Result<()>>,
    failures: VecDeque<io::Result<()>>,
    log: Rc<RefCell<Vec<String>>>,
    attempts: Vec<Duration>,
    next_id: usize,
    launch_error: bool,
    status_error: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            now: Duration::ZERO,
            stop_at: Duration::from_secs(2),
            exit_at: None,
            connections: VecDeque::new(),
            failures: VecDeque::new(),
            log: Rc::new(RefCell::new(Vec::new())),
            attempts: Vec::new(),
            next_id: 0,
            launch_error: false,
            status_error: false,
        }
    }
}
fn missing() -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::NotFound))
}
impl Runtime for Fake {
    type Controller = Pad;
    fn elapsed(&self) -> Duration {
        self.now
    }
    fn stopped(&self) -> bool {
        self.now >= self.stop_at
    }
    fn connect(&mut self) -> io::Result<Pad> {
        self.attempts.push(self.now);
        self.connections.pop_front().unwrap_or_else(missing)?;
        self.next_id += 1;
        self.log
            .borrow_mut()
            .push(format!("connect:{}", self.next_id));
        Ok(Pad(self.log.clone(), self.next_id))
    }
    fn settle(&mut self) {
        self.log.borrow_mut().push("settle".into());
        self.now += Duration::from_millis(800);
    }
    fn launch(&mut self) -> io::Result<()> {
        self.log.borrow_mut().push("launch".into());
        if self.launch_error {
            Err(io::Error::other("spawn failed"))
        } else {
            Ok(())
        }
    }
    fn exit_status(&mut self) -> io::Result<Option<i32>> {
        if self.status_error {
            return Err(io::Error::other("wait failed"));
        }
        Ok(self
            .exit_at
            .filter(|(at, _)| self.now >= *at)
            .map(|(_, code)| code))
    }
    fn wait(&mut self, controller: Option<&mut Pad>) -> io::Result<()> {
        self.now += Duration::from_millis(100);
        match controller {
            Some(pad) => {
                self.log.borrow_mut().push(format!("service:{}", pad.1));
                self.failures.pop_front().unwrap_or(Ok(()))
            }
            None => {
                self.log.borrow_mut().push("idle".into());
                Ok(())
            }
        }
    }
    fn report(&mut self, message: &str) {
        self.log.borrow_mut().push(message.into());
    }
}
#[test]
fn missing_controller_never_prevents_launch_or_changes_game_status() {
    let mut fake = Fake {
        exit_at: Some((Duration::from_millis(700), 7)),
        ..Default::default()
    };
    assert_eq!(run_runtime(&mut fake, false).unwrap(), 7);
    let log = fake.log.borrow();
    assert_eq!(log.iter().filter(|s| *s == "launch").count(), 1);
    assert!(!log.iter().any(|s| s == "settle"));
    assert_eq!(fake.attempts, [Duration::ZERO, Duration::from_millis(500)]);
    assert_eq!(
        log.iter()
            .filter(|s| s.contains("waiting for controller"))
            .count(),
        1
    );
}
#[test]
fn repeated_disconnect_and_reconnect_drops_each_session_without_relaunching() {
    let mut fake = Fake {
        connections: [missing(), Ok(()), Ok(()), Ok(())].into(),
        failures: [
            Err(io::Error::from_raw_os_error(libc::ENODEV)),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(()),
        ]
        .into(),
        ..Default::default()
    };
    run_runtime(&mut fake, false).unwrap();
    let log = fake.log.borrow();
    for id in 1..=3 {
        assert!(log.contains(&format!("connect:{id}")));
        assert_eq!(
            log.iter().filter(|s| **s == format!("drop:{id}")).count(),
            1
        );
    }
    assert_eq!(log.iter().filter(|s| *s == "launch").count(), 1);
    assert!(
        log.iter().position(|s| s == "drop:1").unwrap()
            < log.iter().position(|s| s == "connect:2").unwrap()
    );
    assert!(fake
        .attempts
        .windows(2)
        .all(|w| w[1] - w[0] >= Duration::from_millis(500)));
}
#[test]
fn initially_connected_pad_settles_before_launch_and_is_not_recreated() {
    let mut fake = Fake {
        connections: [Ok(())].into(),
        ..Default::default()
    };
    run_runtime(&mut fake, false).unwrap();
    let log = fake.log.borrow();
    assert_eq!(&log[..3], ["connect:1", "settle", "launch"]);
    assert_eq!(fake.attempts, [Duration::ZERO]);
    assert_eq!(log.last().unwrap(), "drop:1");
}
#[test]
fn setup_failures_are_retried_and_new_failures_are_reported_once() {
    let mut fake = Fake {
        connections: [
            Err(io::Error::from_raw_os_error(libc::EACCES)),
            Err(io::Error::from_raw_os_error(libc::EACCES)),
            Err(io::Error::from_raw_os_error(libc::ENODEV)),
            Ok(()),
        ]
        .into(),
        ..Default::default()
    };
    run_runtime(&mut fake, false).unwrap();
    assert_eq!(fake.attempts.len(), 4);
    assert_eq!(
        fake.log
            .borrow()
            .iter()
            .filter(|s| s.contains("waiting for controller"))
            .count(),
        2
    );
    assert!(fake.log.borrow().contains(&"connect:1".into()));
}
#[test]
fn signal_before_start_or_during_settle_does_not_launch_game() {
    for stop_at in [Duration::ZERO, Duration::from_millis(400)] {
        let mut fake = Fake {
            stop_at,
            connections: [Ok(())].into(),
            ..Default::default()
        };
        assert_eq!(run_runtime(&mut fake, false).unwrap(), 0);
        assert!(!fake.log.borrow().contains(&"launch".into()));
    }
}
#[test]
fn child_exit_and_signal_take_priority_over_reconnect() {
    for signal in [false, true] {
        let mut fake = Fake {
            stop_at: if signal {
                Duration::from_millis(500)
            } else {
                Duration::from_secs(2)
            },
            exit_at: if signal {
                None
            } else {
                Some((Duration::from_millis(500), 9))
            },
            connections: [missing(), Ok(())].into(),
            ..Default::default()
        };
        assert_eq!(
            run_runtime(&mut fake, false).unwrap(),
            if signal { 0 } else { 9 }
        );
        assert_eq!(fake.attempts.len(), 1);
    }
}
#[test]
fn command_errors_remain_fatal_and_drop_connected_pad() {
    for launch_error in [false, true] {
        let mut fake = Fake {
            launch_error,
            status_error: !launch_error,
            connections: [Ok(())].into(),
            ..Default::default()
        };
        let error = run_runtime(&mut fake, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            if launch_error {
                "spawn failed"
            } else {
                "wait failed"
            }
        );
        assert_eq!(fake.log.borrow().last().unwrap(), "drop:1");
    }
}
#[test]
fn diagnose_times_out_with_or_without_controller_without_launching() {
    for connected in [false, true] {
        let mut fake = Fake {
            stop_at: Duration::from_secs(40),
            connections: if connected {
                [Ok(())].into()
            } else {
                VecDeque::new()
            },
            ..Default::default()
        };
        assert_eq!(run_runtime(&mut fake, true).unwrap(), 0);
        assert_eq!(fake.now, Duration::from_secs(30));
        assert!(!fake
            .log
            .borrow()
            .iter()
            .any(|s| s == "launch" || s == "settle"));
        assert_eq!(fake.attempts.len(), if connected { 1 } else { 60 });
    }
}

#[test]
fn polls_and_dispatches_both_streams_and_propagates_handler_failures() {
    use crate::linux::{EV_FF, EV_KEY};
    use std::{cell::Cell, os::unix::net::UnixStream};
    for failing in [None, Some(0), Some(1)] {
        let (physical_write, physical_read) = UnixStream::pair().unwrap();
        let (virtual_write, virtual_read) = UnixStream::pair().unwrap();
        physical_read.set_nonblocking(true).unwrap();
        virtual_read.set_nonblocking(true).unwrap();
        write_event(physical_write.as_raw_fd(), EV_KEY, 10, 1).unwrap();
        write_event(virtual_write.as_raw_fd(), EV_FF, 2, 3).unwrap();
        let calls = Cell::new(0);
        let result = poll_events(
            physical_read.as_raw_fd(),
            virtual_read.as_raw_fd(),
            |event| {
                calls.set(calls.get() + 1);
                assert_eq!((event.kind, event.code, event.value), (EV_KEY, 10, 1));
                if failing == Some(0) {
                    Err(io::Error::other("relay failed"))
                } else {
                    Ok(())
                }
            },
            |event| {
                calls.set(calls.get() + 1);
                assert_eq!((event.kind, event.code, event.value), (EV_FF, 2, 3));
                if failing == Some(1) {
                    Err(io::Error::other("feedback failed"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.is_err(), failing.is_some());
        assert_eq!(calls.get(), if failing == Some(0) { 1 } else { 2 });
    }
}
#[test]
fn poll_timeout_and_terminal_conditions_do_not_dispatch_events() {
    use std::os::unix::net::UnixStream;
    let (pwrite, pread) = UnixStream::pair().unwrap();
    let (vwrite, vread) = UnixStream::pair().unwrap();
    poll_events(
        pread.as_raw_fd(),
        vread.as_raw_fd(),
        |_| panic!("physical timeout"),
        |_| panic!("virtual timeout"),
    )
    .unwrap();
    // Even a ready physical event must not be dispatched after virtual HUP.
    write_event(pwrite.as_raw_fd(), crate::linux::EV_KEY, 1, 1).unwrap();
    drop(vwrite);
    let error = poll_events(
        pread.as_raw_fd(),
        vread.as_raw_fd(),
        |_| panic!("physical before disconnect"),
        |_| panic!("virtual disconnect"),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    assert!(error.to_string().contains("virtual controller"));
    drop(pwrite);
    let error = poll_events(
        pread.as_raw_fd(),
        vread.as_raw_fd(),
        |_| panic!("disconnected"),
        |_| panic!("disconnected"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("physical controller"));
}
#[test]
fn controller_connect_reports_missing_source_and_rejects_non_controller_files() {
    let error = Controller::connect(PathBuf::from("/nonexistent/dsxinput-device"))
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains("/nonexistent/dsxinput-device"));
    let error = Controller::connect(PathBuf::from("/dev/null"))
        .err()
        .unwrap();
    assert_eq!(error.raw_os_error(), Some(libc::ENOTTY));
}

#[test]
fn stale_readiness_is_ignored_but_short_and_failed_reads_end_session() {
    use std::{io::Write, os::unix::net::UnixStream};
    let (mut writer, reader) = UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    dispatch_event(libc::POLLIN, reader.as_raw_fd(), &mut |_| {
        panic!("empty input")
    })
    .unwrap();
    dispatch_event(0, -1, &mut |_| panic!("not readable")).unwrap();
    writer.write_all(&[0; 3]).unwrap();
    let error = dispatch_event(libc::POLLIN, reader.as_raw_fd(), &mut |_| {
        panic!("short event")
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    let error = dispatch_event(libc::POLLIN, -1, &mut |_| panic!("invalid fd")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
}
#[test]
fn every_session_failure_is_recoverable_and_same_error_is_reported_after_recovery() {
    for kind in [
        io::ErrorKind::NotConnected,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::UnexpectedEof,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::Other,
    ] {
        let mut fake = Fake {
            connections: [
                Ok(()),
                Err(io::Error::from(kind)),
                Ok(()),
                Err(io::Error::from(kind)),
            ]
            .into(),
            failures: [Err(io::Error::from(kind)), Err(io::Error::from(kind))].into(),
            stop_at: Duration::from_secs(3),
            ..Default::default()
        };
        assert_eq!(run_runtime(&mut fake, false).unwrap(), 0);
        let log = fake.log.borrow();
        assert_eq!(
            log.iter()
                .filter(|s| s.contains("controller session ended"))
                .count(),
            2
        );
        let message = format!("waiting for controller: {}", io::Error::from(kind));
        assert_eq!(log.iter().filter(|s| **s == message).count(), 2);
        assert_eq!(log.iter().filter(|s| *s == "launch").count(), 1);
        assert!(log.contains(&"drop:1".into()) && log.contains(&"drop:2".into()));
    }
}
#[test]
fn child_exit_while_attached_releases_controller_and_preserves_status() {
    let mut fake = Fake {
        connections: [Ok(())].into(),
        exit_at: Some((Duration::from_millis(1000), 42)),
        ..Default::default()
    };
    assert_eq!(run_runtime(&mut fake, false).unwrap(), 42);
    assert_eq!(fake.log.borrow().last().unwrap(), "drop:1");
    assert_eq!(fake.now, Duration::from_secs(1));
}
