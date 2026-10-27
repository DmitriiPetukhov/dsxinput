//! Parse wrapper flags without interpreting the child command's arguments.
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) device: Option<PathBuf>,
    pub(crate) diagnostic: bool,
    pub(crate) command: Vec<OsString>,
}

impl Options {
    /// Parse leading wrapper flags, then pass the command and all remaining arguments through.
    /// An optional `--` is accepted for compatibility.
    pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut args = args.into_iter().peekable();
        let mut options = Self::default();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--device") if args.peek().is_some() => {
                    options.device = args.next().map(PathBuf::from);
                }
                Some("--diagnose") => options.diagnostic = true,
                Some("--") => {
                    options.command.extend(args);
                    break;
                }
                _ => {
                    options.command.push(arg);
                    options.command.extend(args);
                    break;
                }
            }
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Options {
        Options::parse(args.iter().map(|arg| OsString::from(arg)))
    }

    #[test]
    fn parses_all_wrapper_options() {
        assert_eq!(
            parse(&[
                "--device",
                "/dev/input/event7",
                "--diagnose",
                "game",
                "--flag"
            ]),
            Options {
                device: Some(PathBuf::from("/dev/input/event7")),
                diagnostic: true,
                command: vec!["game".into(), "--flag".into()],
            }
        );
    }

    #[test]
    fn preserves_child_options_after_command() {
        let options = parse(&["game", "--fullscreen", "--device", "file"]);
        assert_eq!(
            options.command,
            ["game", "--fullscreen", "--device", "file"]
        );
        assert_eq!(options.device, None);
    }

    #[test]
    fn launcher_arguments_after_wrapper_options_are_unchanged() {
        let options = parse(&[
            "--device",
            "/dev/input/event25",
            "umu-run",
            "/games/My Game.exe",
            "--diagnose",
            "--",
            "--device",
            "file",
        ]);
        assert_eq!(options.device, Some(PathBuf::from("/dev/input/event25")));
        assert!(!options.diagnostic);
        assert_eq!(
            options.command,
            [
                "umu-run",
                "/games/My Game.exe",
                "--diagnose",
                "--",
                "--device",
                "file",
            ]
        );
    }

    #[test]
    fn separator_allows_command_named_like_wrapper_flag() {
        let options = parse(&["--", "--diagnose"]);
        assert!(!options.diagnostic);
        assert_eq!(options.command, ["--diagnose"]);
    }

    #[test]
    fn last_device_option_wins() {
        assert_eq!(
            parse(&["--device", "first", "--device", "second", "game"]).device,
            Some(PathBuf::from("second"))
        );
    }

    #[test]
    fn missing_device_value_preserves_existing_behavior() {
        assert_eq!(parse(&["--device"]).command, ["--device"]);
    }

    #[test]
    fn unknown_option_starts_child_command() {
        assert_eq!(
            parse(&["--unknown", "--diagnose"]).command,
            ["--unknown", "--diagnose"]
        );
    }

    #[test]
    fn diagnose_does_not_require_command() {
        let options = parse(&["--diagnose"]);
        assert!(options.diagnostic);
        assert!(options.command.is_empty());
    }

    #[test]
    fn repeated_boolean_options_are_idempotent() {
        let options = parse(&["--diagnose", "--diagnose"]);
        assert!(options.diagnostic);
        assert!(options.command.is_empty());
    }

    #[test]
    fn empty_input_has_default_options() {
        assert_eq!(parse(&[]), Options::default());
        assert_eq!(parse(&["--"]), Options::default());
    }
}

#[cfg(test)]
mod native_argument_tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn preserves_non_utf8_device_paths_and_child_arguments() {
        let raw = OsString::from_vec(vec![b'/', b'x', 0xff]);
        let options = Options::parse([
            "--device".into(),
            raw.clone(),
            "game".into(),
            raw.clone(),
            "".into(),
        ]);
        assert_eq!(options.device, Some(PathBuf::from(&raw)));
        assert_eq!(
            options.command,
            [OsString::from("game"), raw.clone(), OsString::new()]
        );
        assert_eq!(
            Options::parse([raw.clone(), "--diagnose".into()]).command,
            [raw, "--diagnose".into()]
        );
    }
}
