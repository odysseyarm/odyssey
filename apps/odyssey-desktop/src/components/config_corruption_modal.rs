use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::config::ConfigCorruptionEvent;

/// Removes corrupted files and returns the entries that could not be deleted.
/// A file already removed outside the app no longer needs a warning.
async fn delete_corrupted_files(
    corruptions: Vec<ConfigCorruptionEvent>,
) -> Vec<ConfigCorruptionEvent> {
    let mut remaining = Vec::new();
    for corruption in corruptions {
        match tokio::fs::remove_file(&corruption.file_path).await {
            Ok(()) => tracing::info!("Deleted corrupted file: {}", corruption.file_path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::error!("Failed to delete {}: {error}", corruption.file_path);
                remaining.push(corruption);
            }
        }
    }
    remaining
}

/// Tells the user which config files were unreadable at startup and offers to
/// delete them or carry on with defaults.
#[component]
pub(crate) fn ConfigCorruptionModal(
    mut corruptions: Signal<Vec<ConfigCorruptionEvent>>,
) -> Element {
    let mut deleting = use_signal(|| false);
    let mut delete_failed = use_signal(|| false);

    if corruptions.read().is_empty() {
        return rsx! {};
    }

    let corruption_count = corruptions.read().len();

    rsx! {
        // Modal backdrop
        div {
            class: "fixed inset-0 bg-black bg-opacity-50 z-50 flex items-center justify-center p-4",
            onclick: move |_| {
                if !deleting() {
                    corruptions.write().clear();
                }
            },

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

                if delete_failed() {
                    p {
                        class: "px-6 py-3 text-sm text-red-700 dark:text-red-300",
                        role: "alert",
                        "Some files could not be deleted. Check their permissions and try again, or continue with defaults."
                    }
                }

                // Footer
                div {
                    class: "bg-gray-50 dark:bg-gray-900 px-6 py-4 flex justify-end gap-3 flex-shrink-0",
                    button {
                        class: "btn-danger",
                        disabled: deleting(),
                        onclick: move |_| async move {
                            deleting.set(true);
                            let reported = corruptions.peek().clone();
                            let remaining = delete_corrupted_files(reported).await;
                            delete_failed.set(!remaining.is_empty());
                            corruptions.set(remaining);
                            deleting.set(false);
                        },
                        if deleting() { "Deleting…" } else { "Delete Files" }
                    }
                    button {
                        class: "btn-primary",
                        disabled: deleting(),
                        onclick: move |_| corruptions.write().clear(),
                        "Continue with Defaults"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::config::ConfigFileType;

    use super::*;

    #[tokio::test]
    async fn deletion_keeps_only_failed_files_for_retry() {
        let root =
            std::env::temp_dir().join(format!("odyssey-corruption-delete-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("scratch directory");
        let removable = root.join("corrupt.json");
        std::fs::write(&removable, "{").expect("corrupt file");
        let missing = root.join("already-removed.json");
        let blocked = root.join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("directory cannot be removed as a file");
        let reports = [&removable, &missing, &blocked]
            .into_iter()
            .map(|path| ConfigCorruptionEvent {
                file_type: ConfigFileType::DeviceOffsets,
                file_path: path.to_string_lossy().into_owned(),
                error_message: "invalid JSON".into(),
                using_default: true,
            })
            .collect();

        let remaining = delete_corrupted_files(reports).await;

        assert_eq!(
            remaining
                .iter()
                .map(|event| event.file_path.as_str())
                .collect::<Vec<_>>(),
            [blocked.to_string_lossy().as_ref()]
        );
        assert!(
            !removable.exists(),
            "successfully deleted file stays deleted"
        );
        assert_eq!(remaining[0].error_message, "invalid JSON");
        std::fs::remove_dir_all(root).expect("clean scratch directory");
    }
}
