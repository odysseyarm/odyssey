use dioxus::prelude::*;
use velopack::{self as vp, sources};

#[derive(Props, Clone, PartialEq)]
pub struct UpdateBannerProps {
    pub available: Signal<bool>,
    pub busy: Signal<bool>,
    pub error: Signal<Option<String>>,
}

#[component]
pub fn UpdateBanner(props: UpdateBannerProps) -> Element {
    // If no update, render nothing
    if !(props.available)() {
        return rsx! {};
    }

    let on_update_click = {
        let mut available = props.available;
        let mut busy = props.busy;
        let mut error = props.error;

        move |_| {
            if (busy)() {
                return;
            }
            busy.set(true);
            error.set(None);

            dioxus::prelude::spawn(async move {
                // This is sync/blocking, but short; fine on the UI thread for now.
                let result = (|| -> Result<(), vp::Error> {
                    let source = sources::AutoSource::new("github:odysseyarm/odyssey-desktop");
                    let um = vp::UpdateManager::new(source, None, None)?;
                    match um.check_for_updates()? {
                        vp::UpdateCheck::UpdateAvailable(info) => {
                            um.download_updates(&info, None)?;
                            // Will relaunch the app:
                            um.apply_updates_and_restart(&info)?;
                        }
                        _ => {
                            // No update anymore; hide the banner
                            available.set(false);
                        }
                    }
                    Ok(())
                })();

                if let Err(e) = result {
                    error.set(Some(format!("{e}")));
                    busy.set(false);
                }
                // On success, process restarts; no further UI updates
            });
        }
    };

    rsx! {
        div { class: "fixed top-0 left-0 right-0 z-50",
            div { class: "bg-red-600 text-white px-4 py-2 flex items-center justify-between gap-3",
                span { class: "font-medium",
                    "Odyssey is out of date. Update and restart?"
                }
                div { class: "flex items-center gap-2",
                    if let Some(err) = (props.error)() {
                        span { class: "text-white/90 text-sm", "{err}" }
                    }
                    button {
                        class: "bg-white text-red-700 rounded px-3 py-1 text-sm font-semibold hover:bg-red-50 disabled:opacity-60",
                        disabled: (props.busy)(),
                        onclick: on_update_click,
                        if (props.busy)() { "Updating…" } else { "Update & Restart" }
                    }
                    button {
                        class: "bg-transparent border border-white/70 text-white rounded px-3 py-1 text-sm hover:bg-white/10",
                        disabled: (props.busy)(),
                        onclick: {
                            let mut available = props.available;
                            move |_| available.set(false)
                        },
                        "Later"
                    }
                }
            }
            // Spacer so content isn't hidden by fixed banner
            div { class: "h-9" }
        }
    }
}
