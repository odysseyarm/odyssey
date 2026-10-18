//! Command line. Every argument is optional: a plain launch opens the app as
//! always, and `calibrate` opens the automatic calibration wizard with the
//! given choices filled in.
//!
//! The parser is hand-written because the whole grammar is one subcommand with
//! six flags; a parsing crate would cost more to build than this costs to read.

use std::fmt;

use parking_lot::Mutex;

use crate::views::calibrate::automatic::settings::{
    COLOR_EXPOSURE_RANGE_US, DIAGONAL_RANGE_IN,
};

pub const USAGE: &str = "\
Usage:
  odyssey-desktop                     Open the app
  odyssey-desktop calibrate [options] Open the automatic screen calibration

Calibrate options (each one pre-fills a setup question; the wizard asks for
anything left out):
  --monitor <n>          Display to calibrate, numbered as in the app (1 = first)
  --screen-id <n>        Screen id to save as (0-5; shown as Screen 1-6)
  --camera <id>          Camera id or serial number
  --diagonal <inches>    Screen diagonal, if known (otherwise it is measured)
  --rgb-exposure <us>    Starting color exposure in microseconds
  --demo                 Use a simulated camera instead of hardware
  -h, --help             Show this help";

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Open the app normally.
    Run,
    Calibrate(CalibrateArgs),
    Help,
}

/// Answers to the wizard's setup questions given on the command line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CalibrateArgs {
    /// Display number as the screen picker shows it, starting at 1.
    pub monitor: Option<usize>,
    pub screen_id: Option<u8>,
    /// Matches a camera's id or serial number.
    pub camera: Option<String>,
    pub diagonal_in: Option<f64>,
    pub rgb_exposure_us: Option<u32>,
    pub demo: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CliError(String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

fn err<T>(message: impl Into<String>) -> Result<T, CliError> {
    Err(CliError(message.into()))
}

/// Parses the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    match args.next().as_deref() {
        None => Ok(Command::Run),
        Some("-h" | "--help" | "help") => Ok(Command::Help),
        Some("calibrate") => parse_calibrate(args),
        Some(other) => err(format!("unknown command '{other}'")),
    }
}

fn parse_calibrate(mut args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let mut out = CalibrateArgs::default();
    while let Some(arg) = args.next() {
        // `--flag=value` and `--flag value` mean the same.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_owned(), Some(value.to_owned())),
            _ => (arg, None),
        };
        let mut value = |flag: &str| -> Result<String, CliError> {
            match inline.clone().or_else(|| args.next()) {
                Some(v) => Ok(v),
                None => err(format!("{flag} needs a value")),
            }
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--demo" => {
                if inline.is_some() {
                    return err("--demo takes no value");
                }
                out.demo = true;
            }
            "--monitor" => {
                let v = value("--monitor")?;
                match v.parse::<usize>() {
                    Ok(n) if n >= 1 => out.monitor = Some(n),
                    _ => return err(format!("--monitor must be a display number starting at 1, got '{v}'")),
                }
            }
            "--screen-id" => {
                let v = value("--screen-id")?;
                match v.parse::<u8>() {
                    Ok(n) if n <= odyssey_common::MAX_SCREEN_ID => out.screen_id = Some(n),
                    _ => {
                        return err(format!(
                            "--screen-id must be between 0 and {}, got '{v}'",
                            odyssey_common::MAX_SCREEN_ID
                        ))
                    }
                }
            }
            "--camera" => {
                let v = value("--camera")?;
                if v.is_empty() {
                    return err("--camera needs a camera id or serial number");
                }
                out.camera = Some(v);
            }
            "--diagonal" => {
                let v = value("--diagonal")?;
                match v.parse::<f64>() {
                    Ok(d) if DIAGONAL_RANGE_IN.contains(&d) => out.diagonal_in = Some(d),
                    _ => {
                        return err(format!(
                            "--diagonal must be between {} and {} inches, got '{v}'",
                            DIAGONAL_RANGE_IN.start(),
                            DIAGONAL_RANGE_IN.end()
                        ))
                    }
                }
            }
            "--rgb-exposure" => {
                let v = value("--rgb-exposure")?;
                match v.parse::<u32>() {
                    Ok(us) if COLOR_EXPOSURE_RANGE_US.contains(&us) => out.rgb_exposure_us = Some(us),
                    _ => {
                        return err(format!(
                            "--rgb-exposure must be between {} and {} microseconds, got '{v}'",
                            COLOR_EXPOSURE_RANGE_US.start(),
                            COLOR_EXPOSURE_RANGE_US.end()
                        ))
                    }
                }
            }
            other => return err(format!("unknown option '{other}' for 'calibrate'")),
        }
    }
    Ok(Command::Calibrate(out))
}

/// The calibrate request from the command line, handed to the wizard once.
static PENDING_CALIBRATE: Mutex<Option<CalibrateArgs>> = Mutex::new(None);

pub fn set_pending_calibrate(args: CalibrateArgs) {
    *PENDING_CALIBRATE.lock() = Some(args);
}

/// Whether the command line asked for the wizard and nothing has taken the
/// request yet.
pub fn has_pending_calibrate() -> bool {
    PENDING_CALIBRATE.lock().is_some()
}

/// Takes the command-line request. The first wizard to open gets it; later
/// openings start from a blank setup.
pub fn take_pending_calibrate() -> Option<CalibrateArgs> {
    PENDING_CALIBRATE.lock().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().copied())
    }

    fn calibrate(args: &[&str]) -> CalibrateArgs {
        let mut all = vec!["calibrate"];
        all.extend_from_slice(args);
        match run(&all).unwrap() {
            Command::Calibrate(a) => a,
            other => panic!("expected calibrate, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_runs_the_app_as_before() {
        assert_eq!(run(&[]).unwrap(), Command::Run);
    }

    #[test]
    fn calibrate_without_options_leaves_everything_to_the_wizard() {
        assert_eq!(calibrate(&[]), CalibrateArgs::default());
    }

    #[test]
    fn every_option_is_read() {
        let a = calibrate(&[
            "--monitor", "2", "--screen-id", "3", "--camera", "CP1234", "--diagonal", "55.5",
            "--rgb-exposure", "2000", "--demo",
        ]);
        assert_eq!(
            a,
            CalibrateArgs {
                monitor: Some(2),
                screen_id: Some(3),
                camera: Some("CP1234".into()),
                diagonal_in: Some(55.5),
                rgb_exposure_us: Some(2000),
                demo: true,
            }
        );
    }

    #[test]
    fn equals_form_matches_space_form() {
        assert_eq!(
            calibrate(&["--monitor=2", "--diagonal=27"]),
            calibrate(&["--monitor", "2", "--diagonal", "27"])
        );
    }

    #[test]
    fn unknown_input_is_an_error_not_ignored() {
        assert!(run(&["frobnicate"]).unwrap_err().to_string().contains("unknown command"));
        assert!(run(&["calibrate", "--bogus"]).unwrap_err().to_string().contains("--bogus"));
        assert!(run(&["calibrate", "stray"]).unwrap_err().to_string().contains("stray"));
        assert!(run(&["calibrate", "--demo=1"]).is_err());
    }

    #[test]
    fn values_are_validated() {
        for bad in [
            &["--monitor", "0"][..],
            &["--monitor", "x"],
            &["--monitor"],
            &["--screen-id", "6"],
            &["--screen-id", "-1"],
            &["--diagonal", "0"],
            &["--diagonal", "nan"],
            &["--diagonal", "5000"],
            &["--rgb-exposure", "0"],
            &["--rgb-exposure", "99999999"],
            &["--camera", ""],
        ] {
            let mut all = vec!["calibrate"];
            all.extend_from_slice(bad);
            assert!(run(&all).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn a_flag_value_is_not_mistaken_for_the_next_flag() {
        let command = run(&["calibrate", "--camera", "--demo"]).unwrap();
        assert!(matches!(
            &command,
            Command::Calibrate(CalibrateArgs { camera: Some(id), demo: false, .. }) if id == "--demo"
        ));
    }

    #[test]
    fn help_is_recognised() {
        assert_eq!(run(&["--help"]).unwrap(), Command::Help);
        assert_eq!(run(&["calibrate", "--help"]).unwrap(), Command::Help);
    }
}
