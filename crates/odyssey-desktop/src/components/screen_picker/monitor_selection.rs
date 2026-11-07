use std::rc::Rc;

use gpui::{
    App, Context, DisplayId, PlatformDisplay, Subscription, WindowBounds, WindowKind, WindowOptions,
};
use odyssey_hub_common::config::{self, MonitorKey};

use super::monitor_layout::{MonitorInfo, resolve_selected};
use crate::runtime;
use crate::ui_state::UiStateWriter;

/// Identity of a display, from the logical bounds GPUI reports.
///
/// GPUI exposes neither a display's name nor its scale factor, so the key is in
/// logical pixels with no name. Physical pixels would need the scale of a window
/// already on that display, which would make the key change when a window
/// opens there. Callers that need physical pixels use their own window's scale.
pub(crate) fn monitor_key_of(display: &dyn PlatformDisplay) -> MonitorKey {
    let bounds = display.bounds();
    MonitorKey {
        name: None,
        x: bounds.origin.x.as_f32().round() as i32,
        y: bounds.origin.y.as_f32().round() as i32,
        width: bounds.size.width.as_f32().round().max(0.0) as u32,
        height: bounds.size.height.as_f32().round().max(0.0) as u32,
    }
}

/// The attached display that `key` identifies, if it is still attached.
pub(crate) fn display_for_key(key: &MonitorKey, cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
    cx.displays()
        .into_iter()
        .find(|display| monitor_key_of(display.as_ref()) == *key)
}

/// Options for a borderless window covering `display`.
pub(crate) fn fullscreen_options(display: &dyn PlatformDisplay, kind: WindowKind) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(display.bounds())),
        titlebar: None,
        kind,
        display_id: Some(display.id()),
        ..WindowOptions::default()
    }
}

fn enumerate_monitors(cx: &App) -> Vec<MonitorInfo> {
    let primary = cx
        .primary_display()
        .map(|display| monitor_key_of(display.as_ref()));
    cx.displays()
        .iter()
        .map(|display| {
            let key = monitor_key_of(display.as_ref());
            let primary = primary.as_ref() == Some(&key);
            MonitorInfo { key, primary }
        })
        .collect()
}

/// The attached displays and the one the user picked, plus the app's ordered
/// persistence queue. Share it as an `Entity`.
pub(crate) struct MonitorSelection {
    monitors: Vec<MonitorInfo>,
    chosen: Option<MonitorKey>,
    writer: UiStateWriter,
    _displays: Option<Subscription>,
}

impl MonitorSelection {
    /// Tracks the attached displays (including hot-plug) and restores the
    /// display the user picked last time.
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        let displays = cx.observe_displays(move |_, cx| {
            this.update(cx, Self::refresh).ok();
        });

        let remembered =
            runtime::spawn(async { config::ui_state_async().await.into_value().selected_monitor });
        cx.spawn(async move |this, cx| {
            let remembered = match remembered.await {
                Ok(remembered) => remembered,
                Err(error) => {
                    tracing::warn!("Failed to load the remembered display: {error}");
                    return;
                }
            };
            this.update(cx, |selection, cx| {
                // A click that lands before the file loads wins over the stale value.
                if selection.chosen.is_none() && remembered.is_some() {
                    selection.chosen = remembered;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();

        Self {
            monitors: enumerate_monitors(cx),
            chosen: None,
            writer: cx.global::<UiStateWriter>().clone(),
            _displays: Some(displays),
        }
    }

    #[cfg(all(test, feature = "sim"))]
    pub(crate) fn for_test(key: MonitorKey, writer: UiStateWriter) -> Self {
        Self {
            monitors: vec![MonitorInfo { key, primary: true }],
            chosen: None,
            writer,
            _displays: None,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let latest = enumerate_monitors(cx);
        if self.monitors != latest {
            self.monitors = latest;
            cx.notify();
        }
    }

    pub(crate) fn monitors(&self) -> &[MonitorInfo] {
        &self.monitors
    }

    /// Index into [`Self::monitors`] of the selected monitor, if any are attached.
    pub(crate) fn selected(&self) -> Option<usize> {
        resolve_selected(&self.monitors, self.chosen.as_ref())
    }

    pub(crate) fn selected_key(&self) -> Option<MonitorKey> {
        Some(self.monitors.get(self.selected()?)?.key.clone())
    }

    /// The GPUI display of the selected monitor, for placing windows on it.
    pub(crate) fn selected_display(&self, cx: &App) -> Option<DisplayId> {
        Some(display_for_key(&self.selected_key()?, cx)?.id())
    }

    /// Selects the monitor at `index` and remembers it across launches.
    pub(crate) fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(key) = self.monitors.get(index).map(|m| m.key.clone()) else {
            return;
        };
        self.chosen = Some(key.clone());
        cx.notify();

        // Queue at the click, before spawning a waiter. The shared writer
        // serializes edits and preserves fields changed by other views.
        let save = self
            .writer
            .update(move |state| state.selected_monitor = Some(key));
        cx.spawn(async move |_, _| {
            if let Err(error) = save.await {
                tracing::warn!("Failed to save UI state: {error:?}");
            }
        })
        .detach();
    }
}
