use std::rc::Rc;
use std::time::Duration;

use dioxus::desktop::tao::monitor::MonitorHandle;
use dioxus::desktop::{DesktopContext, use_window};
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::config::{self, MonitorKey};

use super::monitor_layout::{MonitorInfo, monitor_key, resolve_selected};

/// How often to look for monitors being plugged in or removed.
const MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn enumerate_monitors(window: &DesktopContext) -> Vec<MonitorInfo> {
    let primary = window.primary_monitor().map(|m| monitor_key(&m));
    window
        .available_monitors()
        .map(|m| MonitorInfo::from_handle(&m, primary.as_ref()))
        .collect()
}

/// The attached monitors and the one the user picked. Copy it freely: it is a
/// pair of signal handles.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct MonitorSelection {
    monitors: Signal<Vec<MonitorInfo>>,
    chosen: Signal<Option<MonitorKey>>,
}

impl MonitorSelection {
    pub(crate) fn monitors(&self) -> Vec<MonitorInfo> {
        self.monitors.read().clone()
    }

    /// Index into [`Self::monitors`] of the selected monitor, if any are attached.
    pub(crate) fn selected(&self) -> Option<usize> {
        resolve_selected(&self.monitors.read(), self.chosen.read().as_ref())
    }

    /// The OS handle of the selected monitor, for placing windows on it.
    pub(crate) fn selected_handle(&self) -> Option<MonitorHandle> {
        let key = self.monitors.read().get(self.selected()?)?.key.clone();
        dioxus::desktop::window()
            .available_monitors()
            .find(|m| monitor_key(m) == key)
    }

    /// Selects the monitor at `index` and remembers it across launches.
    pub(crate) fn select(&self, index: usize) {
        let Some(key) = self.monitors.peek().get(index).map(|m| m.key.clone()) else {
            return;
        };
        let mut chosen = self.chosen;
        chosen.set(Some(key.clone()));

        spawn(async move {
            // Reload rather than reuse an earlier snapshot so a field some
            // other part of the app saved in the meantime is not overwritten.
            let mut state = config::ui_state_async().await.into_value();
            state.selected_monitor = Some(key);
            if let Err(e) = config::ui_state_save_async(&state).await {
                tracing::warn!("Failed to save UI state: {e}");
            }
        });
    }
}

/// Tracks the attached monitors (including hot-plug) and restores the
/// monitor the user picked last time.
pub(crate) fn use_monitor_selection() -> MonitorSelection {
    let window = use_window();
    let mut monitors = use_signal({
        let window = Rc::clone(&window);
        move || enumerate_monitors(&window)
    });
    let mut chosen = use_signal(|| None::<MonitorKey>);

    use_future(move || async move {
        let remembered = config::ui_state_async().await.into_value().selected_monitor;
        // A click that lands before the file loads wins over the stale value.
        if chosen.peek().is_none() {
            chosen.set(remembered);
        }
    });

    use_future(move || {
        let window = Rc::clone(&window);
        async move {
            loop {
                tokio::time::sleep(MONITOR_POLL_INTERVAL).await;
                let latest = enumerate_monitors(&window);
                if *monitors.peek() != latest {
                    monitors.set(latest);
                }
            }
        }
    });

    MonitorSelection { monitors, chosen }
}
