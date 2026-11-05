#![windows_subsystem = "windows"]
#![expect(
    elided_lifetimes_in_paths,
    reason = "GPUI writes its borrowed contexts as `Context<Self>` in every signature"
)]

use std::io::Write as _;

mod cli;
mod config_corruption;
mod components;
mod hub;
mod logging;
mod paths;
mod runtime;
mod scale_board;
mod sdk_install;
mod server_status;
mod shell;
mod theme;
mod tray;
mod ui_state;
mod views;

/// Renders diagnostics only when an error becomes a cloneable UI-state message.
fn error_message(error: &eros::ErrorUnion) -> String {
    use std::fmt::Write as _;

    let mut message = String::new();
    for context in error.contexts().rev() {
        let _ = write!(message, "{}: ", context.value());
    }
    let _ = write!(message, "{error}");
    message
}

fn main() {
    logging::init();
    #[cfg(feature = "sim")]
    if odyssey_hub_server::sim::enabled()
        && let Err(error) = odyssey_hub_server::sim::initialize_config()
    {
        tracing::error!("Could not initialize isolated simulator settings: {error}");
        std::process::exit(1);
    }

    let args = std::env::args().skip(1);
    #[cfg(feature = "sim")]
    let args = args.filter(|argument| !matches!(argument.as_str(), "--sim" | "--sim-legacy"));
    match cli::parse(args) {
        Ok(cli::Command::Run) => {}
        Ok(cli::Command::Calibrate(args)) => cli::set_pending_calibrate(args),
        Ok(cli::Command::Help) => {
            // The Windows build has no console; ignore a closed stdout.
            let _ = writeln!(std::io::stdout(), "{}", cli::USAGE);
            return;
        }
        Err(e) => {
            tracing::error!("{e}");
            let _ = writeln!(std::io::stderr(), "odyssey-desktop: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    }

    // A downloaded camera SDK that could not be removed or replaced while it was
    // loaded is dealt with here, before anything can load it again.
    sdk_install::finish_pending();

    gpui::application()
        .with_assets(gpui::assets::Assets)
        .run(shell::launch);
}
