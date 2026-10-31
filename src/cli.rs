use std::ffi::OsString;

use clap::{ArgGroup, Parser};

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .multiple(false)
        .args(["command", "standalone"])
))]
pub struct Cli {
    /// Executable followed by its arguments.
    #[arg(
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub command: Vec<OsString>,

    /// Run without launching a game.
    #[arg(long)]
    pub standalone: bool,
}
