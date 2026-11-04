//! The notice shown when offline mode is on or the policy file has problems.

use dioxus::prelude::*;

/// Says so when offline mode is on and where firmware updates come from, and
/// shows what is wrong with the firmware pack or the policy file. Shows nothing
/// when everything is online and fine.
#[component]
pub(super) fn OfflineNotice(error: Option<String>) -> Element {
    let policy = odyssey_hub_server::policy::current();
    if !policy.offline && policy.issues.is_empty() {
        return rsx! {};
    }
    let dir = odyssey_hub_server::policy::firmware_pack_dir()
        .display()
        .to_string();

    rsx! {
        div {
            role: "note",
            class: "flex flex-col gap-1 rounded-lg border-2 border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
            if policy.offline {
                span { class: "font-semibold", "Offline mode is on" }
                span {
                    "Odyssey won't connect to the internet. Firmware updates come from the firmware pack in {dir}."
                }
                if let Some(error) = error {
                    span { role: "alert", class: "font-medium", {error} }
                }
            }
            for issue in policy.issues.iter() {
                span { key: "{issue}", role: "alert", class: "font-medium", {issue.as_str()} }
            }
        }
    }
}
