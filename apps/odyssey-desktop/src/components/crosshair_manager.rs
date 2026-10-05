// src/components/crosshair_manager.rs

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

use dioxus::{
    html::geometry::euclid::Rect,
    logger::tracing,
    prelude::*,
};
use odyssey_hub_common::{device::Device, events::TrackingEvent};
use tokio::sync::broadcast;

/// Wrapper for broadcast::Sender that implements PartialEq (always false, triggers re-render)
/// This is needed because Dioxus #[component] requires PartialEq on all props
#[derive(Clone)]
pub struct TrackingSender(pub broadcast::Sender<(Device, TrackingEvent)>);

impl PartialEq for TrackingSender {
    fn eq(&self, _other: &Self) -> bool {
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

/// Unique ID counter for CrosshairManager instances to ensure fresh subscriptions
static CROSSHAIR_MANAGER_ID: AtomicU64 = AtomicU64::new(0);

#[component]
pub fn CrosshairManager(
    hub: Signal<crate::hub::HubContext>,
    /// Pass the broadcast sender directly to avoid Signal issues across VirtualDoms
    #[props(optional)]
    tracking_sender: Option<TrackingSender>,
) -> Element {
    let instance_id =
        use_hook(|| CROSSHAIR_MANAGER_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst));

    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);
    // key → (x_px, y_px, image_src)
    let mut crosshair_positions = use_signal(HashMap::<usize, (f64, f64, String)>::new);

    {
        let sender = tracking_sender
            .clone()
            .map(|ts| ts.0)
            .unwrap_or_else(|| hub.peek().tracking_events.clone());
        let hub = hub.clone();
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
                            let rect = rect_signal.peek();
                            let width = rect.size.width as f64;
                            let height = rect.size.height as f64;
                            if width <= 0.0 || height <= 0.0 {
                                tracing::warn!("Container size invalid: {}x{}", width, height);
                                continue;
                            }

                            if let Some(key) = hub.peek().device_key(&device) {
                                let image_src = CROSSHAIR_IMAGES
                                    [key.min(CROSSHAIR_IMAGES.len() - 1)]
                                .to_string();
                                let x = tracking.aimpoint.x.clamp(0.0, 1.0) as f64 * width;
                                let y = tracking.aimpoint.y.clamp(0.0, 1.0) as f64 * height;
                                crosshair_positions.write().insert(key, (x, y, image_src));
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
        let hub = hub.clone();
        use_effect(move || {
            let devices = (hub().devices)();
            let current_keys: Vec<usize> = devices.iter().map(|(key, _)| key).collect();
            crosshair_positions.write().retain(|k, _| current_keys.contains(k));
        });
    }

    rsx! {
        div {
            class: "w-full h-full relative overflow-hidden",
            onmounted: move |cx| {
                async move {
                    let cx_data = cx.data();
                    if let Ok(rect) = cx_data.as_ref().get_client_rect().await {
                        rect_signal.set(rect);
                    }
                    root_div.set(Some(cx_data));
                }
            },
            onresize: move |_| {
                async move {
                    if let Some(root) = root_div() {
                        if let Ok(rect) = root.as_ref().get_client_rect().await {
                            rect_signal.set(rect);
                        }
                    }
                }
            },
            for (key, (x, y, src)) in crosshair_positions.read().clone().into_iter() {
                img {
                    key: "{key}",
                    src: "{src}",
                    class: "absolute pointer-events-none",
                    style: "left: {x}px; top: {y}px; transform: translate(-50%, -50%);",
                }
            }
        }
    }
}
