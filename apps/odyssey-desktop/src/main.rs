#![windows_subsystem = "windows"]

use std::io::Write as _;
use std::rc::Rc;
use std::time::Duration;

use components::{ConfigCorruptionModal, FirmwareUpdateManager, Navbar};
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour, window};
use dioxus::history::MemoryHistory;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use dioxus_router::components::HistoryProvider;
use dioxus_router::{Routable, Router};
use odyssey_hub_common::config::ConfigCorruptionEvent;
use server_status::{ServerExit, handle_server_status};
use tokio_util::sync::CancellationToken;
use views::{Accessories, Calibrate, CalibrateAutomatic, Home, Pairing, Settings};

mod cli;
mod components;
mod hub;
mod logging;
mod paths;
mod scale_board;
mod sdk_install;
mod server_status;
mod tray;
mod ui_state;
mod views;

/// How often the hub connection checks whether the server is listening yet.
const SERVER_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Set once the hub server is listening. Views that must not act in a second
/// app instance, which exits as soon as it finds the first, wait for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerReady(pub Signal<bool>);

fn main() {
    // Log to the terminal and a file before Dioxus launches.
    logging::init();

    match cli::parse(std::env::args().skip(1)) {
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
    // loaded is dealt with now, before anything can load it again.
    sdk_install::finish_pending();

    LaunchBuilder::new().with_cfg(desktop_config()).launch(app);
}

fn desktop_config() -> Config {
    let config = Config::default()
        .with_menu(None)
        .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
        .with_window(WindowBuilder::new().with_title("Odyssey"));
    if cfg!(target_os = "windows") {
        let user_data_dir = std::env::var("LOCALAPPDATA").expect("env var LOCALAPPDATA not found");
        config.with_data_directory(user_data_dir)
    } else {
        config
    }
}

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
    #[route("/pairing")]
    Pairing {},
    #[route("/accessories")]
    Accessories {},
    #[route("/settings")]
    Settings {},
    #[route("/calibrate")]
    Calibrate {},
    #[end_layout]
    // The wizard fills the window, so it sits outside the navigation layout.
    #[route("/calibrate/automatic")]
    CalibrateAutomatic {},
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

/// Runs the hub server for the life of the window and reports its status through
/// the two signals. Closes the window when another instance already owns the hub.
fn use_server_task(
    cancel_token: CancellationToken,
    config_corruptions: Signal<Vec<ConfigCorruptionEvent>>,
    server_ready: Signal<bool>,
) {
    use_future(move || {
        let cancel_token = cancel_token.clone();
        async move {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

            // Spawn server in background. Its failures reach the UI as
            // status messages, so the result itself carries nothing more.
            let server_handle = tokio::spawn({
                let cancel_token = cancel_token.clone();
                async move {
                    let _ = odyssey_hub_server::run_server(sender, cancel_token).await;
                }
            });

            // Handle status messages (this future has access to Signal)
            let exit = handle_server_status(receiver, config_corruptions, server_ready).await;

            // Wait for server task to complete
            let _ = server_handle.await;
            tracing::info!("Server task ended");

            // If we detected another instance, close the app
            if exit == ServerExit::AnotherInstanceRunning {
                tracing::info!("Closing app due to another instance running");
                window().close();
            }
        }
    });
}

/// Connects the hub context to the server once it is listening, and keeps it
/// connected.
fn use_hub_connection(hub: Signal<hub::HubContext>, server_ready: Signal<bool>) {
    use_future(move || {
        let mut hub = hub();
        async move {
            while !*server_ready.read() {
                tokio::time::sleep(SERVER_READY_POLL_INTERVAL).await;
            }
            hub.run().await;
        }
    });
}

#[component]
fn app() -> Element {
    let cancel_token = CancellationToken::new();
    let config_corruptions = use_signal(Vec::<ConfigCorruptionEvent>::new);
    let server_ready = use_signal(|| false);

    use_hook(|| {
        // Hide window to tray on close
        window().set_close_behavior(WindowCloseBehaviour::WindowHides);
    });

    tray::use_tray_menu(cancel_token.clone());
    use_server_task(cancel_token, config_corruptions, server_ready);

    let hub = use_context_provider(|| Signal::new(hub::HubContext::new()));
    // Firmware work survives page navigation, so its claims and progress must too.
    use_context_provider(FirmwareUpdateManager::new);
    let ui_state_writer = ui_state::use_ui_state_writer();
    use_context_provider(|| ui_state_writer);
    use_hub_connection(hub, server_ready);

    // Decided once: the wizard takes the command-line request when it opens,
    // and a later re-render must not move the router.
    let open_wizard = use_hook(cli::has_pending_calibrate);
    use_context_provider(|| ServerReady(server_ready));

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        ConfigCorruptionModal {
            corruptions: config_corruptions,
        }

        div {
            if open_wizard {
                // The command line asked for the wizard, so start on it.
                HistoryProvider {
                    history: |()| {
                        Rc::new(MemoryHistory::with_initial_path(
                            Route::CalibrateAutomatic {}.to_string(),
                        )) as Rc<dyn History>
                    },
                    Router::<Route> {}
                }
            } else {
                Router::<Route> {}
            }
        }
    }
}
