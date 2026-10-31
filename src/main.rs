use clap::Parser;

use crate::app::App;

mod app;
mod cli;
mod controller_tracker;
mod env_config;
mod game_process;
mod gamepad_session;
mod gamepad_session_manager;
mod physical_gamepad;
mod stick_axis_filter;
mod virtual_gamepad;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = crate::cli::Cli::parse();

    let config = env_config::EnvConfig::from_env()?;
    App::new(cli, config).run();
    Ok(())
}
