#![windows_subsystem = "windows"]

use dioxus::{
    desktop::{window, Config, WindowBuilder, WindowCloseBehaviour},
    logger::tracing,
    prelude::*,
};
use dioxus_router::{Routable, Router};
use odyssey_hub_server::Message;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use velopack::VelopackApp;

use components::Navbar;
use views::Accessories;
use views::Home;

use components::update::UpdateBanner;

mod components;
mod hub;
mod styles;
mod tray;
mod views;

fn main() {
    VelopackApp::build().run();

    if cfg!(target_os = "windows") {
        let user_data_dir = std::env::var("LOCALAPPDATA").expect("env var LOCALAPPDATA not found");
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_data_directory(user_data_dir)
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses)
                    .with_window(WindowBuilder::new().with_title("Odyssey")),
            )
            .launch(app);
    } else {
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses)
                    .with_window(WindowBuilder::new().with_title("Odyssey")),
            )
            .launch(app);
    }
}

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
    #[route("/accessories")]
    Accessories {},
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn app() -> Element {
    let cancel_token = CancellationToken::new();
    let config_corruptions =
        use_signal(|| Vec::<odyssey_hub_common::config::ConfigCorruptionEvent>::new());
    let server_ready = use_signal(|| false);

    use_hook(|| {
        // Hide window to tray on close
        window().set_close_behavior(WindowCloseBehaviour::WindowHides);
    });

    tray::use_tray_menu(cancel_token.clone());

    // Run server + status handling
    use_future({
        let cancel_token = cancel_token.clone();
        let config_corruptions = config_corruptions.clone();
        let server_ready = server_ready.clone();
        move || {
            let cancel_token = cancel_token.clone();
            async move {
                let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

                // Spawn server in background
                let server_handle = tokio::spawn({
                    let sender = sender.clone();
                    let cancel_token = cancel_token.clone();
                    async move {
                        let _ = odyssey_hub_server::run_server(sender, cancel_token).await;
                    }
                });

                // Handle status messages (this future has access to Signal)
                let should_exit =
                    handle_server_status(receiver, config_corruptions, server_ready).await;

                // Wait for server task to complete
                let _ = server_handle.await;
                tracing::info!("Server task ended");

                // If we detected another instance, close the app
                if should_exit {
                    tracing::info!("Closing app due to another instance running");
                    use dioxus::desktop::window;
                    window().close();
                }
            }
        }
    });

    let hub = use_context_provider(|| Signal::new(hub::HubContext::new()));
    use_future({
        let server_ready = server_ready.clone();
        move || {
            let mut hub = hub();
            async move {
                // Wait for server to be ready
                while !*server_ready.read() {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                hub.run().await
            }
        }
    });

    // --- Update banner signals + one-shot check ---
    let update_available = use_signal(|| false);
    let update_busy = use_signal(|| false);
    let update_error = use_signal(|| Option::<String>::None);

    // One-shot update check
    use_effect({
        let mut available = update_available.clone();
        let mut error = update_error.clone();
        move || {
            dioxus::prelude::spawn(async move {
                use velopack::{self as vp, sources};

                let source = sources::HttpSource::new(
                    "https://github.com/odysseyarm/odyssey-desktop/releases/latest/download",
                );
                match vp::UpdateManager::new(source, None, None) {
                    Ok(um) => {
                        match um.check_for_updates_async().await {
                            Ok(vp::UpdateCheck::UpdateAvailable(_)) => {
                                tracing::info!("Update available");
                                available.set(true);
                            }
                            Ok(_) => {
                                tracing::info!("No update available");
                                // no update available
                            }
                            Err(e) => {
                                tracing::error!("Error checking for updates: {e}");
                                error.set(Some(format!("{e}")));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error creating UpdateManager: {e}");
                        error.set(Some(format!("{e}")));
                    }
                }
            });
        }
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        UpdateBanner {
            available: update_available,
            busy: update_busy,
            error: update_error,
            cancel_token: Signal::new(cancel_token.clone()),
        }

        ConfigCorruptionModal {
            corruptions: config_corruptions,
        }

        div {
            Router::<Route> {}
        }
    }
}

#[component]
fn ConfigCorruptionModal(
    corruptions: Signal<Vec<odyssey_hub_common::config::ConfigCorruptionEvent>>,
) -> Element {
    let mut show_modal = use_signal(|| false);

    // Show modal when corruptions are detected
    use_effect(move || {
        if !corruptions.read().is_empty() && !*show_modal.read() {
            show_modal.set(true);
        }
    });

    if !*show_modal.read() || corruptions.read().is_empty() {
        return rsx! {};
    }

    let corruption_count = corruptions.read().len();

    rsx! {
        // Modal backdrop
        div {
            class: "fixed inset-0 bg-black bg-opacity-50 z-50 flex items-center justify-center p-4",
            onclick: move |_| show_modal.set(false),

            // Modal content
            div {
                class: "bg-white dark:bg-gray-800 rounded-lg shadow-xl max-w-2xl w-full max-h-[80vh] flex flex-col overflow-hidden",
                onclick: move |e| e.stop_propagation(),

                // Header
                div {
                    class: "bg-yellow-500 dark:bg-yellow-600 px-6 py-4 flex-shrink-0",
                    h2 {
                        class: "text-xl font-bold text-white flex items-center gap-2",
                        "⚠️ Configuration File Warning"
                    }
                }

                // Body (scrollable)
                div {
                    class: "p-6 overflow-y-auto flex-1 min-h-0",
                    p {
                        class: "text-gray-700 dark:text-gray-300 mb-4",
                        {
                            if corruption_count == 1 {
                                "The following configuration file was corrupted and could not be loaded. Default values are being used instead:"
                            } else {
                                "The following configuration files were corrupted and could not be loaded. Default values are being used instead:"
                            }
                        }
                    }

                    ul {
                        class: "space-y-3",
                        for corruption in corruptions.read().iter() {
                            li {
                                class: "bg-gray-100 dark:bg-gray-700 p-4 rounded",
                                div {
                                    class: "font-semibold text-gray-900 dark:text-gray-100 mb-1",
                                    "{corruption.file_type:?}"
                                }
                                div {
                                    class: "text-sm text-gray-600 dark:text-gray-400 mb-1",
                                    "Path: {corruption.file_path}"
                                }
                                div {
                                    class: "text-sm text-red-600 dark:text-red-400",
                                    "Error: {corruption.error_message}"
                                }
                            }
                        }
                    }

                    div {
                        class: "mt-4 p-4 bg-blue-50 dark:bg-blue-900/30 rounded",
                        p {
                            class: "text-sm text-gray-700 dark:text-gray-300",
                            "You can continue using the application with default settings. To fix this issue, you can either:"
                        }
                        ul {
                            class: "list-disc list-inside text-sm text-gray-600 dark:text-gray-400 mt-2 space-y-1",
                            li { "Delete the corrupted file(s) to reset to defaults" }
                            li { "Manually fix the JSON syntax errors in the file(s)" }
                            li { "Restore from a backup if available" }
                        }
                    }
                }

                // Footer
                div {
                    class: "bg-gray-50 dark:bg-gray-900 px-6 py-4 flex justify-end gap-3 flex-shrink-0",
                    button {
                        class: "px-4 py-2 bg-red-600 hover:bg-red-700 text-white rounded font-medium transition-colors",
                        onclick: move |_| {
                            // Delete corrupted files
                            let corruptions_to_delete = corruptions.read().clone();
                            spawn(async move {
                                for corruption in corruptions_to_delete {
                                    if let Err(e) = tokio::fs::remove_file(&corruption.file_path).await {
                                        tracing::error!("Failed to delete {}: {}", corruption.file_path, e);
                                    } else {
                                        tracing::info!("Deleted corrupted file: {}", corruption.file_path);
                                    }
                                }
                            });
                            corruptions.write().clear();
                            show_modal.set(false);
                        },
                        "Delete Files"
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white rounded font-medium transition-colors",
                        onclick: move |_| {
                            corruptions.write().clear();
                            show_modal.set(false);
                        },
                        "Continue with Defaults"
                    }
                }
            }
        }
    }
}

async fn handle_server_status(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>,
    mut config_corruptions: Signal<Vec<odyssey_hub_common::config::ConfigCorruptionEvent>>,
    mut server_ready: Signal<bool>,
) -> bool {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(corruptions))) => {
                if corruptions.is_empty() {
                    tracing::info!("Server started successfully");
                } else {
                    tracing::warn!(
                        "Server started with {} corrupted config file(s)",
                        corruptions.len()
                    );
                    for corruption in &corruptions {
                        tracing::warn!(
                            "Config file corrupted: {:?} at '{}': {}. Using default values.",
                            corruption.file_type,
                            corruption.file_path,
                            corruption.error_message
                        );
                    }
                    // Update signal to show modal dialog
                    config_corruptions.set(corruptions);
                }
                // Signal that server is ready for connections
                server_ready.set(true);
            }
            Some(Message::ServerInit(Err(e))) => {
                // Check if it's an "address already in use" error (another instance running)
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    tracing::warn!("Another instance is already running. Bringing it to front...");

                    // Try to connect to the existing instance and call BringToFront
                    let mut client = odyssey_hub_client::client::Client::default();
                    if let Ok(()) = client.connect().await {
                        if let Ok(()) = client.bring_to_front().await {
                            tracing::info!("Successfully brought existing instance to front");
                        } else {
                            tracing::warn!("Failed to bring existing instance to front");
                        }
                    }

                    // Return true to signal we should exit due to another instance
                    return true;
                } else {
                    tracing::error!("Server start error: {}", e);
                    return false;
                }
            }
            Some(Message::BringToFront) => {
                tracing::info!("Received BringToFront request");
                // Bring window to front using Dioxus window API
                use dioxus::desktop::window;
                window().set_focus();
                window().set_visible(true);
            }
            Some(Message::Stop) => {
                tracing::info!("Server stopped");
                return false;
            }
            None => {
                tracing::info!("Server channel closed");
                return false;
            }
        }
    }
}
