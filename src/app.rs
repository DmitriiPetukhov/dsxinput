//! Runtime orchestration, event polling, and shutdown.
use crate::{
    cli,
    device::{gamepad_path, open_physical, require_rumble, virtual_pad},
    feedback::forward_feedback,
    linux::{ioctl_val, read_event, write_event, EVIOCGRAB, EVIOCRMFF},
    process::{spawn_game, Game},
    relay::{Relay, Snapshot},
};
use std::{
    env, io,
    os::fd::{AsRawFd, RawFd},
    thread,
    time::{Duration, Instant},
};

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Request shutdown without doing I/O in the signal handler.
extern "C" fn handle_signal(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Set up controllers and relay events until shutdown or child exit.
pub(crate) fn run() -> io::Result<i32> {
    let cli::Options {
        device: path,
        diagnostic,
        command: args,
    } = cli::Options::parse(env::args().skip(1));
    if args.is_empty() && !diagnostic {
        eprintln!("dsxinput v{}\nUsage: dsxinput [--device /dev/input/eventN] [--diagnose] COMMAND [ARGS...]\n       dsxinput --diagnose", env!("CARGO_PKG_VERSION"));
        return Ok(2);
    }
    // Declared first: controller resources drop before child shutdown.
    let mut game: Option<Game> = None;
    let path = match path {
        Some(p) => p,
        None => gamepad_path()?,
    };
    let physical = open_physical(&path)?;
    let pfd = physical.as_raw_fd();
    eprintln!("dsxinput: source {}", path.display());
    require_rumble(pfd)?;
    eprintln!("dsxinput: rumble forwarding enabled");
    let virtual_pad = virtual_pad()?;
    let vfd = virtual_pad.as_raw_fd();
    // Exclusive evdev grab prevents most native apps seeing the original buttons; HIDRAW remains independently accessible.
    ioctl_val(pfd, EVIOCGRAB, 1)?;
    struct Grab(RawFd);
    impl Drop for Grab {
        fn drop(&mut self) {
            let _ = ioctl_val(self.0, EVIOCGRAB, 0);
        }
    }
    let _grab = Grab(pfd);
    eprintln!("dsxinput: virtual Xbox-style controller ready (evdev grabbed)");
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
    let mut relay = Relay::new(Snapshot::read(pfd)?, &mut |kind, code, value| {
        write_event(vfd, kind, code, value)
    })?;
    if !diagnostic {
        thread::sleep(Duration::from_millis(800));
        if !STOP.load(std::sync::atomic::Ordering::SeqCst) {
            game = Some(Game::new(spawn_game(&args)?));
        }
    }
    let mut ids = [-1i16; 16];
    let started = Instant::now();
    let mut status = 0;
    loop {
        if STOP.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if let Some(ref mut child) = game {
            if let Some(exit) = child.try_wait()? {
                status = exit.code().unwrap_or(128);
                break;
            }
        }
        let mut fds = [
            libc::pollfd {
                fd: pfd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: vfd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 100) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        check_poll(fds[0].revents, "physical controller")?;
        check_poll(fds[1].revents, "virtual controller")?;
        if fds[0].revents & libc::POLLIN != 0 {
            let ev = read_event(pfd)?;
            relay.process(ev, &mut || Snapshot::read(pfd), &mut |kind, code, value| {
                write_event(vfd, kind, code, value)
            })?;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let ev = read_event(vfd)?;
            forward_feedback(vfd, pfd, ev, &mut ids)?;
        }
        if diagnostic && started.elapsed() > Duration::from_secs(30) {
            break;
        }
    }
    for id in ids {
        if id >= 0 {
            let _ = ioctl_val(pfd, EVIOCRMFF, id as i32);
        }
    }
    Ok(status)
}

/// Check terminal poll conditions before attempting potentially blocking I/O.
fn check_poll(events: i16, device: &str) -> io::Result<()> {
    if events & libc::POLLNVAL != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{device}: invalid descriptor"),
        ));
    }
    if events & (libc::POLLHUP | libc::POLLERR) != 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            format!("{device}: disconnected or failed"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_poll_conditions_override_readability_for_both_devices() {
        for device in ["physical controller", "virtual controller"] {
            for flag in [libc::POLLHUP, libc::POLLERR, libc::POLLNVAL] {
                for readable in [0, libc::POLLIN] {
                    let error = check_poll(flag | readable, device).unwrap_err();
                    assert!(error.to_string().contains(device));
                    assert_eq!(
                        error.kind(),
                        if flag == libc::POLLNVAL {
                            io::ErrorKind::InvalidInput
                        } else {
                            io::ErrorKind::NotConnected
                        }
                    );
                }
            }
            for flag in [0, libc::POLLIN, libc::POLLOUT] {
                check_poll(flag, device).unwrap();
            }
        }
    }
}
