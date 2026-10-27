//! Runtime orchestration, event polling, and shutdown.
use crate::{
    cli,
    device::{gamepad_path, open_physical, require_rumble, virtual_pad, VirtualController},
    feedback::forward_feedback,
    linux::{ioctl_val, read_event, write_event, EVIOCGRAB, EVIOCRMFF},
    process::{spawn_game, Game},
    relay::{Relay, Snapshot},
};
use std::{
    env,
    fs::File,
    io,
    os::fd::AsRawFd,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const RETRY_INTERVAL: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Request shutdown without doing I/O in the signal handler.
extern "C" fn handle_signal(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub(crate) fn run() -> io::Result<i32> {
    let options = cli::Options::parse(env::args_os().skip(1));
    if options.command.is_empty() && !options.diagnostic {
        eprintln!("dsxinput v{}\nUsage: dsxinput [--device /dev/input/eventN] [--diagnose] COMMAND [ARGS...]\n       dsxinput --diagnose", env!("CARGO_PKG_VERSION"));
        return Ok(2);
    }
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
    let diagnostic = options.diagnostic;
    run_runtime(
        &mut SystemRuntime {
            options,
            game: None,
            started: Instant::now(),
        },
        diagnostic,
    )
}

/// Keep the game independent of disposable controller sessions. The borrowed
/// runtime owns the child; this function drops its controller before returning.
trait Runtime {
    type Controller;
    fn elapsed(&self) -> Duration;
    fn stopped(&self) -> bool;
    fn connect(&mut self) -> io::Result<Self::Controller>;
    fn settle(&mut self);
    fn launch(&mut self) -> io::Result<()>;
    fn exit_status(&mut self) -> io::Result<Option<i32>>;
    fn wait(&mut self, controller: Option<&mut Self::Controller>) -> io::Result<()>;
    fn report(&mut self, message: &str);
}

fn run_runtime(runtime: &mut impl Runtime, diagnostic: bool) -> io::Result<i32> {
    let mut controller = None;
    let mut next_attempt = Duration::ZERO;
    let mut last_error = None;
    let mut launched = false;
    loop {
        if runtime.stopped() {
            return Ok(0);
        }
        if launched {
            if let Some(status) = runtime.exit_status()? {
                return Ok(status);
            }
        }
        if diagnostic && runtime.elapsed() >= Duration::from_secs(30) {
            return Ok(0);
        }
        if controller.is_none() && runtime.elapsed() >= next_attempt {
            match runtime.connect() {
                Ok(pad) => {
                    controller = Some(pad);
                    last_error = None;
                }
                Err(error) => {
                    let message = format!("waiting for controller: {error}");
                    if last_error.as_ref() != Some(&message) {
                        runtime.report(&message);
                        last_error = Some(message);
                    }
                }
            }
            next_attempt = runtime.elapsed() + RETRY_INTERVAL;
        }
        if !diagnostic && !launched {
            if controller.is_some() {
                runtime.settle();
            }
            if runtime.stopped() {
                return Ok(0);
            }
            runtime.launch()?;
            launched = true;
        }
        if let Err(error) = runtime.wait(controller.as_mut()) {
            // Drop the virtual device, grab, physical fd and rumble state now.
            controller = None;
            runtime.report(&format!(
                "controller session ended: {error}; waiting for reconnect"
            ));
            next_attempt = runtime.elapsed() + RETRY_INTERVAL;
        }
    }
}

struct SystemRuntime {
    options: cli::Options,
    game: Option<Game>,
    started: Instant,
}
impl Runtime for SystemRuntime {
    type Controller = Controller;
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn stopped(&self) -> bool {
        STOP.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn connect(&mut self) -> io::Result<Controller> {
        let path = match &self.options.device {
            Some(path) => path.clone(),
            None => gamepad_path()?,
        };
        Controller::connect(path)
    }
    fn settle(&mut self) {
        thread::sleep(Duration::from_millis(800));
    }
    fn launch(&mut self) -> io::Result<()> {
        self.game = Some(Game::new(spawn_game(&self.options.command)?));
        Ok(())
    }
    fn exit_status(&mut self) -> io::Result<Option<i32>> {
        self.game
            .as_mut()
            .expect("game launched")
            .try_wait()
            .map(|status| status.map(|exit| exit.code().unwrap_or(128)))
    }
    fn wait(&mut self, controller: Option<&mut Controller>) -> io::Result<()> {
        match controller {
            Some(controller) => controller.poll(),
            None => {
                thread::sleep(POLL_INTERVAL);
                Ok(())
            }
        }
    }
    fn report(&mut self, message: &str) {
        eprintln!("dsxinput: {message}");
    }
}

/// The grab owns its descriptor, including during partial session setup.
struct Grabbed(File);
impl Grabbed {
    fn new(file: File) -> io::Result<Self> {
        ioctl_val(file.as_raw_fd(), EVIOCGRAB, 1)?;
        Ok(Self(file))
    }
}
impl Drop for Grabbed {
    fn drop(&mut self) {
        let _ = ioctl_val(self.0.as_raw_fd(), EVIOCGRAB, 0);
    }
}

struct Controller {
    // Fields drop in declaration order: remove virtual device before releasing grab.
    virtual_pad: VirtualController,
    physical: Grabbed,
    relay: Relay,
    ids: [i16; 16],
}
impl Controller {
    fn connect(path: PathBuf) -> io::Result<Self> {
        let physical = open_physical(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        let pfd = physical.as_raw_fd();
        require_rumble(pfd)?;
        let physical = Grabbed::new(physical)?;
        let virtual_pad = virtual_pad()?;
        let vfd = virtual_pad.as_raw_fd();
        let relay = Relay::new(Snapshot::read(pfd)?, &mut |kind, code, value| {
            write_event(vfd, kind, code, value)
        })?;
        eprintln!("dsxinput: source {}; virtual Xbox-style controller ready (evdev grabbed, rumble enabled)", path.display());
        Ok(Self {
            virtual_pad,
            physical,
            relay,
            ids: [-1; 16],
        })
    }
    fn poll(&mut self) -> io::Result<()> {
        let pfd = self.physical.0.as_raw_fd();
        let vfd = self.virtual_pad.as_raw_fd();
        poll_events(
            pfd,
            vfd,
            |event| {
                self.relay.process(
                    event,
                    &mut || Snapshot::read(pfd),
                    &mut |kind, code, value| write_event(vfd, kind, code, value),
                )
            },
            |event| forward_feedback(vfd, pfd, event, &mut self.ids),
        )
    }
}

/// Check both descriptors before forwarding either direction.
fn poll_events(
    pfd: std::os::fd::RawFd,
    vfd: std::os::fd::RawFd,
    mut physical: impl FnMut(crate::linux::InputEvent) -> io::Result<()>,
    mut feedback: impl FnMut(crate::linux::InputEvent) -> io::Result<()>,
) -> io::Result<()> {
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
    let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, POLL_INTERVAL.as_millis() as i32) };
    if n < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(())
        } else {
            Err(error)
        };
    }
    check_poll(fds[0].revents, "physical controller")?;
    check_poll(fds[1].revents, "virtual controller")?;
    dispatch_event(fds[0].revents, pfd, &mut physical)?;
    dispatch_event(fds[1].revents, vfd, &mut feedback)?;
    Ok(())
}

fn dispatch_event(
    events: i16,
    fd: std::os::fd::RawFd,
    handle: &mut impl FnMut(crate::linux::InputEvent) -> io::Result<()>,
) -> io::Result<()> {
    if events & libc::POLLIN != 0 {
        match read_event(fd) {
            Ok(event) => handle(event)?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl Drop for Controller {
    fn drop(&mut self) {
        for id in self.ids {
            if id >= 0 {
                let _ = ioctl_val(self.physical.0.as_raw_fd(), EVIOCRMFF, id as i32);
            }
        }
    }
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

#[cfg(test)]
#[path = "app/tests.rs"]
mod lifecycle_tests;
