//! "Save details for support": saves the latest calibration attempt, on the failure
//! screen and the review, as one zip a person can send (see `attempt_bundle`).

use dioxus::prelude::*;

use super::ui::SECONDARY_BUTTON;
use super::wizard::Wizard;

/// A size for people: "46.3 MB".
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

#[component]
pub fn SaveAttempt(wizard: Wizard) -> Element {
    let saving = *wizard.attempt_saving.read();
    let status = wizard.attempt_status.read().clone();
    rsx! {
        div {
            class: "flex flex-col gap-2",
            div {
                class: "flex flex-wrap items-center gap-3",
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    disabled: saving,
                    onclick: move |_| wizard.save_attempt(),
                    if saving { "Saving\u{2026}" } else { "Save details for support\u{2026}" }
                }
            }
            p {
                class: "text-sm text-gray-600 dark:text-gray-300",
                "A zip of what this attempt used: every camera frame (color, depth and infrared, which show the room), the calibration lines of the app's log, the camera's serial number and the settings. It is tens of megabytes. Nothing is sent anywhere; send it yourself if you want help."
            }
            match status {
                Some(Ok(saved)) => rsx! {
                    p {
                        role: "status",
                        class: "text-sm text-gray-700 dark:text-gray-200",
                        "Saved {saved.path.display()} ({megabytes(saved.bytes)})."
                    }
                },
                Some(Err(error)) => rsx! {
                    p { role: "alert", class: "text-sm text-red-700 dark:text-red-300", "Couldn't save the details: {error}" }
                },
                None => rsx! {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_in_megabytes() {
        assert_eq!(megabytes(46_300_000), "46.3 MB");
        assert_eq!(megabytes(0), "0.0 MB");
    }
}
