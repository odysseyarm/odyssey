use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::config::ConfigCorruptionEvent;

/// Removes the corrupted files from disk so the server writes fresh defaults.
async fn delete_corrupted_files(corruptions: Vec<ConfigCorruptionEvent>) {
    for corruption in corruptions {
        if let Err(e) = tokio::fs::remove_file(&corruption.file_path).await {
            tracing::error!("Failed to delete {}: {}", corruption.file_path, e);
        } else {
            tracing::info!("Deleted corrupted file: {}", corruption.file_path);
        }
    }
}

/// Forgets the reported corruptions and closes the dialog.
fn dismiss(mut corruptions: Signal<Vec<ConfigCorruptionEvent>>, mut show_modal: Signal<bool>) {
    corruptions.write().clear();
    show_modal.set(false);
}

/// Tells the user which config files were unreadable at startup and offers to
/// delete them or carry on with defaults.
#[component]
pub(crate) fn ConfigCorruptionModal(corruptions: Signal<Vec<ConfigCorruptionEvent>>) -> Element {
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
                        class: "btn-danger",
                        onclick: move |_| {
                            spawn(delete_corrupted_files(corruptions.read().clone()));
                            dismiss(corruptions, show_modal);
                        },
                        "Delete Files"
                    }
                    button {
                        class: "btn-primary",
                        onclick: move |_| dismiss(corruptions, show_modal),
                        "Continue with Defaults"
                    }
                }
            }
        }
    }
}
