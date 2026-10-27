//! Linux only. Requires read/write evdev and uinput permission.
mod app;
mod cli;
mod device;
mod feedback;
mod linux;
mod mapping;
mod process;
mod relay;

/// Report runtime errors and return the wrapper or child exit status.
fn main() {
    match app::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("dsxinput: {error}");
            std::process::exit(1);
        }
    }
}
