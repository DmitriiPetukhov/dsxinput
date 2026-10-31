use std::{env, io};

#[derive(Debug, Clone, Copy)]
pub struct EnvConfig {
    pub stick_deadzone_l: f32,
    pub stick_deadzone_r: f32,
}

impl EnvConfig {
    pub fn from_env() -> io::Result<Self> {
        Ok(Self {
            stick_deadzone_l: parse_deadzone("DSXINPUT_STICK_DEADZONE_L")?,
            stick_deadzone_r: parse_deadzone("DSXINPUT_STICK_DEADZONE_R")?,
        })
    }
}

fn parse_deadzone(name: &str) -> io::Result<f32> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a finite number from 0 (inclusive) to 1 (exclusive)"),
        )
    };
    let raw = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(0.1),
        Err(env::VarError::NotUnicode(_)) => return Err(invalid()),
    };
    let value: f32 = raw.trim().parse().map_err(|_| invalid())?;
    if !value.is_finite() || !(0.0..1.0).contains(&value) {
        return Err(invalid());
    }
    Ok(value)
}
