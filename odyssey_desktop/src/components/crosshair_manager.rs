// src/components/crosshair_manager.rs

use std::sync::atomic::AtomicU64;

use dioxus::{
    desktop::{window, DesktopContext},
    html::geometry::euclid::Rect,
    logger::tracing,
    prelude::UnboundedReceiver,
    prelude::*,
};
use futures::StreamExt;
use odyssey_hub_common::{device::Device, events::TrackingEvent};
use tokio::sync::broadcast;

/// Wrapper for broadcast::Sender that implements PartialEq (always false, triggers re-render)
/// This is needed because Dioxus #[component] requires PartialEq on all props
#[derive(Clone)]
pub struct TrackingSender(pub broadcast::Sender<(Device, TrackingEvent)>);

impl PartialEq for TrackingSender {
    fn eq(&self, _other: &Self) -> bool {
        // Always return false to ensure component updates when sender changes
        false
    }
}

const CROSSHAIR_IMAGES: [Asset; 6] = [
    asset!("/assets/images/crosshairs/crosshair-red.png"),
    asset!("/assets/images/crosshairs/crosshair-blue.png"),
    asset!("/assets/images/crosshairs/crosshair-yellow.png"),
    asset!("/assets/images/crosshairs/crosshair-green.png"),
    asset!("/assets/images/crosshairs/crosshair-orange.png"),
    asset!("/assets/images/crosshairs/crosshair-gray.png"),
];
const BOOTSTRAP_SCRIPT: &str = include_str!("crosshair_canvas.js");

#[derive(Clone, Debug)]
struct CrosshairUpdate {
    key: usize,
    x: f64,
    y: f64,
    image_src: String,
}

fn eval_script(desktop: &DesktopContext, script: &str) {
    if let Err(err) = desktop.webview.evaluate_script(script) {
        tracing::warn!("Crosshair canvas eval failed: {err}");
    }
}

fn bootstrap(desktop: &DesktopContext) {
    eval_script(desktop, BOOTSTRAP_SCRIPT);
}

fn resize_canvas(desktop: &DesktopContext, width: f64, height: f64) {
    eval_script(
        desktop,
        &format!(
            "window.__odysseyCrosshair?.resize({:.2}, {:.2});",
            width, height
        ),
    );
}

/// Unique ID counter for CrosshairManager instances to ensure fresh subscriptions
static CROSSHAIR_MANAGER_ID: AtomicU64 = AtomicU64::new(0);

#[component]
pub fn CrosshairManager(
    hub: Signal<crate::hub::HubContext>,
    /// Pass the broadcast sender directly to avoid Signal issues across VirtualDoms
    #[props(optional)]
    tracking_sender: Option<TrackingSender>,
) -> Element {
    // Generate a unique ID for this component instance
    let instance_id =
        use_hook(|| CROSSHAIR_MANAGER_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));

    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);
    let known_devices = use_signal(Vec::<usize>::new);

    let desktop = window();

    // Bootstrap the crosshair canvas JS once per component instance (i.e., per window)
    use_hook(|| {
        bootstrap(&desktop);
    });

    let crosshair_updates = use_coroutine({
        let desktop = desktop.clone();
        move |mut rx: UnboundedReceiver<CrosshairUpdate>| {
            let desktop = desktop.clone();
            async move {
                while let Some(update) = rx.next().await {
                    let script = format!(
                        "window.__odysseyCrosshair?.draw({key}, {x:.2}, {y:.2}, {src:?});",
                        key = update.key,
                        x = update.x,
                        y = update.y,
                        src = update.image_src
                    );
                    eval_script(&desktop, &script);
                }
            }
        }
    });

    {
        // Use the directly passed sender if available, otherwise try to get from hub Signal
        // The direct sender is needed for cross-VirtualDom scenarios (like new windows)
        let sender = tracking_sender
            .clone()
            .map(|ts| ts.0)
            .unwrap_or_else(|| hub.peek().tracking_events.clone());
        let hub = hub.clone();
        let rect_signal = rect_signal.clone();
        let crosshair_updates = crosshair_updates.clone();
        use_future(move || {
            let mut receiver = sender.subscribe();
            tracing::info!(
                "CrosshairManager[{}]: subscribed to tracking events",
                instance_id
            );
            async move {
                tracing::info!("CrosshairManager[{}]: entering event loop", instance_id);
                loop {
                    match receiver.recv().await {
                        Ok((device, tracking)) => {
                            tracing::debug!("CrosshairManager[{}] received tracking event for device {:?}: aimpoint=({}, {})", instance_id, device.uuid, tracking.aimpoint.x, tracking.aimpoint.y);
                            let rect = rect_signal.read();
                            let width = rect.size.width as f64;
                            let height = rect.size.height as f64;
                            if width <= 0.0 || height <= 0.0 {
                                tracing::warn!("Canvas size invalid: {}x{}", width, height);
                                continue;
                            }

                            if let Some(key) = hub.peek().device_key(&device) {
                                tracing::trace!("Device key: {}", key);
                                let image_src = CROSSHAIR_IMAGES
                                    [key.min(CROSSHAIR_IMAGES.len() - 1)]
                                .to_string();
                                let x = tracking.aimpoint.x.clamp(0.0, 1.0) as f64 * width;
                                let y = tracking.aimpoint.y.clamp(0.0, 1.0) as f64 * height;
                                crosshair_updates.send(CrosshairUpdate {
                                    key,
                                    x,
                                    y,
                                    image_src,
                                });
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::warn!(
                                "CrosshairManager[{}]: tracking channel closed",
                                instance_id
                            );
                            break;
                        }
                    }
                }
            }
        });
    }

    {
        let desktop = desktop.clone();
        let hub = hub.clone();
        let mut known_devices = known_devices.clone();
        use_effect(move || {
            let devices = (hub().devices)();
            let current_keys: Vec<usize> = devices.iter().map(|(key, _)| key).collect();
            let previous = known_devices.peek().clone();
            for key in previous.iter().filter(|k| !current_keys.contains(k)) {
                eval_script(
                    &desktop,
                    &format!("window.__odysseyCrosshair?.clearKey({key});"),
                );
            }
            known_devices.set(current_keys);
        });
    }

    let desktop_for_mount = desktop.clone();
    let desktop_for_resize = desktop.clone();
    rsx! {
        div {
            class: "w-full h-full",
            onmounted: move |cx| {
                let desktop = desktop_for_mount.clone();
                async move {
                    let cx_data = cx.data();
                    if let Ok(rect) = cx_data.as_ref().get_client_rect().await {
                        resize_canvas(&desktop, rect.size.width as f64, rect.size.height as f64);
                        rect_signal.set(rect);
                    }
                    root_div.set(Some(cx_data));
                }
            },
            onresize: move |_| {
                let desktop = desktop_for_resize.clone();
                async move {
                    if let Some(root) = root_div() {
                        if let Ok(rect) = root.as_ref().get_client_rect().await {
                            resize_canvas(&desktop, rect.size.width as f64, rect.size.height as f64);
                            rect_signal.set(rect);
                        }
                    }
                }
            },
            canvas {
                id: "crosshair-layer",
                class: "absolute inset-0 pointer-events-none",
            }
        }
    }
}
