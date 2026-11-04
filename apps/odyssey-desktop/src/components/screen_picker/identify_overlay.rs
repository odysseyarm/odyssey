use std::time::Duration;

use dioxus::desktop::tao::window::Fullscreen;
use dioxus::desktop::{Config, DesktopContext, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;

use super::monitor_layout::{MonitorInfo, monitor_key};

/// How long the numbered overlay stays on each monitor.
pub(super) const IDENTIFY_DURATION: Duration = Duration::from_millis(1800);

/* -------------------------------- Identify -------------------------------- */

/// Puts a numbered overlay on every physical monitor for a moment. The numbers
/// match the ones on the picker's tiles because both follow the OS's order.
pub(super) fn identify_monitors(window: &DesktopContext) {
    let primary = window.primary_monitor().map(|m| monitor_key(&m));
    for (i, monitor) in window.available_monitors().enumerate() {
        let info = MonitorInfo::from_handle(&monitor, primary.as_ref());
        let props = IdentifyOverlayProps {
            number: i + 1,
            label: format!("{} · {}", info.display_name(), info.resolution()),
        };
        let dom = VirtualDom::new_with_props(IdentifyOverlay, props);
        let config = Config::default()
            .with_menu(None)
            .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
            .with_window(
                WindowBuilder::new()
                    .with_position(monitor.position())
                    .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor)))),
            );
        window.new_window(dom, config);
    }
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn IdentifyOverlay(number: usize, label: String) -> Element {
    use_future(|| async {
        tokio::time::sleep(IDENTIFY_DURATION).await;
        dioxus::desktop::window().close();
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            class: "h-screen flex flex-col items-center justify-center gap-6 bg-blue-700 text-white select-none cursor-pointer",
            onclick: move |_| dioxus::desktop::window().close(),
            div { class: "text-[16rem] leading-none font-bold", "{number}" }
            div { class: "text-2xl opacity-90", {label} }
        }
    }
}
