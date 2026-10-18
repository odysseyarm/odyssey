//! Command line. Every argument is optional: a plain launch opens the app as
//! always, and `calibrate` opens the automatic calibration wizard with the
//! given choices filled in.
//!
//! The parser is hand-written because the whole grammar is one subcommand with
//! six flags; a parsing crate would cost more to build than this costs to read.

use std::fmt;

use odyssey_hub_common::config::DisplayKind;
use parking_lot::Mutex;

use crate::views::calibrate::automatic::settings::{COLOR_EXPOSURE_RANGE_US, DIAGONAL_RANGE_IN};

/// Largest overscan the demo can simulate, in percent. Real TVs crop 2-5%.
pub const DEMO_OVERSCAN_MAX_PERCENT: f64 = 15.0;

pub const USAGE: &str = "\
Usage:
  odyssey-desktop                     Open the app
  odyssey-desktop calibrate [options] Open the automatic screen calibration

Calibrate options (each one pre-fills a setup question; the wizard asks for
anything left out):
  --monitor <n>          Display to calibrate, numbered as in the app (1 = first)
  --screen-id <n>        Screen id to save as (0-5; shown as Screen 1-6)
  --kind <flat|projector>
                         What is being calibrated: a TV or monitor, or a projector
  --camera <id>          Camera id or serial number
  --diagonal <inches>    Screen diagonal, if known (otherwise the TV's own size or
                         the camera's measurement is used)
  --rgb-exposure <us>    Starting color exposure in microseconds
  --demo                 Use a simulated camera instead of hardware
  --demo-overscan <percent>
                         With --demo: make the simulated TV crop its picture by this
                         much (0-15), to try the 'picture is cropped' advice
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
    pub kind: Option<DisplayKind>,
    pub camera: Option<String>,
    pub diagonal_in: Option<f64>,
    pub rgb_exposure_us: Option<u32>,
    pub demo: bool,
    /// How much the simulated TV overscans, in percent. Only with `demo`.
    pub demo_overscan_percent: Option<f64>,
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
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
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
                    _ => {
                        return err(format!(
                            "--monitor must be a display number starting at 1, got '{v}'"
                        ));
                    }
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
                        ));
                    }
                }
            }
            "--kind" => {
                let v = value("--kind")?;
                out.kind = Some(match v.as_str() {
                    "flat" => DisplayKind::Flat,
                    "projector" => DisplayKind::Projector,
                    _ => return err(format!("--kind must be 'flat' or 'projector', got '{v}'")),
                });
            }
            "--demo-overscan" => {
                let v = value("--demo-overscan")?;
                match v.parse::<f64>() {
                    Ok(p) if (0.0..=DEMO_OVERSCAN_MAX_PERCENT).contains(&p) => {
                        out.demo_overscan_percent = Some(p)
                    }
                    _ => {
                        return err(format!(
                            "--demo-overscan must be a percentage between 0 and {DEMO_OVERSCAN_MAX_PERCENT}, got '{v}'"
                        ));
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
                        ));
                    }
                }
            }
            "--rgb-exposure" => {
                let v = value("--rgb-exposure")?;
                match v.parse::<u32>() {
                    Ok(us) if COLOR_EXPOSURE_RANGE_US.contains(&us) => {
                        out.rgb_exposure_us = Some(us)
                    }
                    _ => {
                        return err(format!(
                            "--rgb-exposure must be between {} and {} microseconds, got '{v}'",
                            COLOR_EXPOSURE_RANGE_US.start(),
                            COLOR_EXPOSURE_RANGE_US.end()
                        ));
                    }
                }
            }
            other => return err(format!("unknown option '{other}' for 'calibrate'")),
        }
    }
    if out.demo_overscan_percent.is_some() && !out.demo {
        return err("--demo-overscan only works together with --demo");
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
            "--monitor",
            "2",
            "--screen-id",
            "3",
            "--kind",
            "projector",
            "--camera",
            "CP1234",
            "--diagonal",
            "55.5",
            "--rgb-exposure",
            "2000",
            "--demo",
            "--demo-overscan",
            "3.5",
        ]);
        assert_eq!(
            a,
            CalibrateArgs {
                monitor: Some(2),
                screen_id: Some(3),
                kind: Some(DisplayKind::Projector),
                camera: Some("CP1234".into()),
                diagonal_in: Some(55.5),
                rgb_exposure_us: Some(2000),
                demo: true,
                demo_overscan_percent: Some(3.5),
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
        assert!(
            run(&["frobnicate"])
                .unwrap_err()
                .to_string()
                .contains("unknown command")
        );
        assert!(
            run(&["calibrate", "--bogus"])
                .unwrap_err()
                .to_string()
                .contains("--bogus")
        );
        assert!(
            run(&["calibrate", "stray"])
                .unwrap_err()
                .to_string()
                .contains("stray")
        );
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
            &["--kind", "tv"],
            &["--kind"],
            &["--demo", "--demo-overscan", "-1"],
            &["--demo", "--demo-overscan", "40"],
            &["--demo", "--demo-overscan", "lots"],
            &["--demo-overscan", "3"],
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
    fn kind_is_flat_or_projector() {
        assert_eq!(calibrate(&["--kind", "flat"]).kind, Some(DisplayKind::Flat));
        assert_eq!(
            calibrate(&["--kind=projector"]).kind,
            Some(DisplayKind::Projector)
        );
        assert_eq!(calibrate(&[]).kind, None);
    }

    #[test]
    fn demo_overscan_needs_the_demo_and_is_documented() {
        assert_eq!(
            calibrate(&["--demo", "--demo-overscan", "3"]).demo_overscan_percent,
            Some(3.0)
        );
        assert!(
            run(&["calibrate", "--demo-overscan", "3"])
                .unwrap_err()
                .to_string()
                .contains("--demo")
        );
        for flag in ["--kind", "--demo-overscan", "--diagonal"] {
            assert!(USAGE.contains(flag), "{flag} missing from --help");
        }
    }

    #[test]
    fn help_is_recognised() {
        assert_eq!(run(&["--help"]).unwrap(), Command::Help);
        assert_eq!(run(&["calibrate", "--help"]).unwrap(), Command::Help);
    }
}
