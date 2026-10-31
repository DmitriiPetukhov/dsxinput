use clap::Parser;

use crate::app::App;

mod app;
mod cli;
mod controller_tracker;
mod game_process;
mod gamepad_session;
mod gamepad_session_manager;
mod physical_gamepad;
mod stick_axis_filter;
mod virtual_gamepad;

fn main() {
    let cli = crate::cli::Cli::parse();

    App::new(cli).run();
}
