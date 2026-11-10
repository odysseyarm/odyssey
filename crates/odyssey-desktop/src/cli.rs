//! Command line. Every argument is optional: a plain launch opens the app as
//! always, and `calibrate` opens the automatic calibration wizard with the
//! given choices filled in.
//!
//! The parser is hand-written because the whole grammar is one subcommand with
//! a small set of flags; a parsing crate would cost more to build than this costs to read.

use std::fmt;

use odyssey_hub_common::config::DisplayKind;
use parking_lot::Mutex;

use crate::views::calibrate::automatic::settings::{COLOR_EXPOSURE_RANGE_US, DIAGONAL_RANGE_IN};

/// Largest overscan the demo can simulate, in percent. Real TVs crop 2-5%.
pub(crate) const DEMO_OVERSCAN_MAX_PERCENT: f64 = 15.0;

pub(crate) const USAGE: &str = "\
Usage:
  odyssey-desktop                     Open the app
  odyssey-desktop calibrate [options] Open the automatic screen calibration
  odyssey-desktop --sim               Open with the transport simulator (sim builds)

Simulation requires --features sim at build time and --sim or ODYSSEY_SIM=1
at runtime. Physical unit/dongle/accessory discovery is disabled; settings and
synthetic screen 0 live in an isolated temporary directory.

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
pub(crate) enum Command {
    /// Open the app normally.
    Run,
    Calibrate(CalibrateArgs),
    Help,
}

/// Answers to the wizard's setup questions given on the command line.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CalibrateArgs {
    /// Display number as the screen picker shows it, starting at 1.
    pub monitor: Option<usize>,
    pub screen_id: Option<u8>,
    pub kind: Option<DisplayKind>,
    /// Matches a camera's id or serial number.
    pub camera: Option<String>,
    pub diagonal_in: Option<f64>,
    pub rgb_exposure_us: Option<u32>,
    pub demo: bool,
    /// How much the simulated TV overscans, in percent. Only with `demo`.
    pub demo_overscan_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CliError(String);

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
pub(crate) fn parse<I, S>(args: I) -> Result<Command, CliError>
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

fn parse_monitor(value: &str) -> Result<usize, CliError> {
    match value.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => err(format!(
            "--monitor must be a display number starting at 1, got '{value}'"
        )),
    }
}

fn parse_screen_id(value: &str) -> Result<u8, CliError> {
    match value.parse::<u8>() {
        Ok(n) if n <= odyssey_common::MAX_SCREEN_ID => Ok(n),
        _ => err(format!(
            "--screen-id must be between 0 and {}, got '{value}'",
            odyssey_common::MAX_SCREEN_ID
        )),
    }
}

fn parse_kind(value: &str) -> Result<DisplayKind, CliError> {
    match value {
        "flat" => Ok(DisplayKind::Flat),
        "projector" => Ok(DisplayKind::Projector),
        _ => err(format!(
            "--kind must be 'flat' or 'projector', got '{value}'"
        )),
    }
}

fn parse_demo_overscan(value: &str) -> Result<f64, CliError> {
    match value.parse::<f64>() {
        Ok(percent) if (0.0..=DEMO_OVERSCAN_MAX_PERCENT).contains(&percent) => Ok(percent),
        _ => err(format!(
            "--demo-overscan must be a percentage between 0 and {DEMO_OVERSCAN_MAX_PERCENT}, got '{value}'"
        )),
    }
}

fn parse_camera(value: String) -> Result<String, CliError> {
    if value.is_empty() {
        return err("--camera needs a camera id or serial number");
    }
    Ok(value)
}

fn parse_diagonal(value: &str) -> Result<f64, CliError> {
    match value.parse::<f64>() {
        Ok(inches) if DIAGONAL_RANGE_IN.contains(&inches) => Ok(inches),
        _ => err(format!(
            "--diagonal must be between {} and {} inches, got '{value}'",
            DIAGONAL_RANGE_IN.start(),
            DIAGONAL_RANGE_IN.end()
        )),
    }
}

fn parse_rgb_exposure(value: &str) -> Result<u32, CliError> {
    match value.parse::<u32>() {
        Ok(us) if COLOR_EXPOSURE_RANGE_US.contains(&us) => Ok(us),
        _ => err(format!(
            "--rgb-exposure must be between {} and {} microseconds, got '{value}'",
            COLOR_EXPOSURE_RANGE_US.start(),
            COLOR_EXPOSURE_RANGE_US.end()
        )),
    }
}

fn parse_calibrate(mut args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let mut out = CalibrateArgs::default();
    while let Some(arg) = args.next() {
        // `--flag=value` and `--flag value` mean the same.
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (arg.as_str(), None),
        };
        let mut value = |flag: &str| -> Result<String, CliError> {
            match inline.map(str::to_owned).or_else(|| args.next()) {
                Some(v) => Ok(v),
                None => err(format!("{flag} needs a value")),
            }
        };
        match flag {
            "-h" | "--help" => return Ok(Command::Help),
            "--demo" => {
                if inline.is_some() {
                    return err("--demo takes no value");
                }
                out.demo = true;
            }
            "--monitor" => out.monitor = Some(parse_monitor(&value("--monitor")?)?),
            "--screen-id" => out.screen_id = Some(parse_screen_id(&value("--screen-id")?)?),
            "--kind" => out.kind = Some(parse_kind(&value("--kind")?)?),
            "--demo-overscan" => {
                out.demo_overscan_percent = Some(parse_demo_overscan(&value("--demo-overscan")?)?);
            }
            "--camera" => out.camera = Some(parse_camera(value("--camera")?)?),
            "--diagonal" => out.diagonal_in = Some(parse_diagonal(&value("--diagonal")?)?),
            "--rgb-exposure" => {
                out.rgb_exposure_us = Some(parse_rgb_exposure(&value("--rgb-exposure")?)?);
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

pub(crate) fn set_pending_calibrate(args: CalibrateArgs) {
    *PENDING_CALIBRATE.lock() = Some(args);
}

/// Whether the command line asked for the wizard and nothing has taken the
/// request yet.
pub(crate) fn has_pending_calibrate() -> bool {
    PENDING_CALIBRATE.lock().is_some()
}

/// Takes the command-line request. The first wizard to open gets it; later
/// openings start from a blank setup.
pub(crate) fn take_pending_calibrate() -> Option<CalibrateArgs> {
    PENDING_CALIBRATE.lock().take()
}

/// How the hub inside this app finds its devices; the default unless the
/// command line said otherwise.
static SERVER_OPTIONS: std::sync::OnceLock<odyssey_hub_server::ServerOptions> =
    std::sync::OnceLock::new();

#[cfg(feature = "sim")]
pub(crate) fn set_server_options(options: odyssey_hub_server::ServerOptions) {
    let _ = SERVER_OPTIONS.set(options);
}

pub(crate) fn server_options() -> odyssey_hub_server::ServerOptions {
    SERVER_OPTIONS.get().copied().unwrap_or_default()
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
            &["--demo=1"],
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
    fn help_is_recognised() {
        assert_eq!(run(&["--help"]).unwrap(), Command::Help);
        assert_eq!(run(&["calibrate", "--help"]).unwrap(), Command::Help);
    }
}
