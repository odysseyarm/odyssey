// src/components/crosshair_manager.rs

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::html::geometry::Pixels;
use dioxus::html::geometry::euclid::Rect;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use tokio::sync::broadcast;

use crate::hub::HubContext;

/// Wrapper for broadcast::Sender that implements PartialEq (always false, triggers re-render)
/// This is needed because Dioxus #[component] requires PartialEq on all props
#[derive(Clone)]
pub(crate) struct TrackingSender(pub broadcast::Sender<(Device, TrackingEvent)>);

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

/// A crosshair's place in the container, in pixels, and the image to draw.
type CrosshairPlacement = (f64, f64, String);

/// Image for the device at slab slot `key`; slots past the palette share the last.
fn crosshair_image(key: usize) -> String {
    CROSSHAIR_IMAGES[key.min(CROSSHAIR_IMAGES.len() - 1)].to_string()
}

/// Pixel position of a normalised aimpoint inside a container of `width` x
/// `height`; aimpoints off the screen stick to its edge.
fn aimpoint_pixels(aim_x: f32, aim_y: f32, width: f64, height: f64) -> (f64, f64) {
    (
        f64::from(aim_x.clamp(0.0, 1.0)) * width,
        f64::from(aim_y.clamp(0.0, 1.0)) * height,
    )
}

/// Moves crosshairs as tracking events arrive, until the channel closes.
async fn follow_tracking(
    instance_id: u64,
    mut receiver: broadcast::Receiver<(Device, TrackingEvent)>,
    hub: Signal<HubContext>,
    rect_signal: Signal<Rect<f64, Pixels>>,
    mut crosshair_positions: Signal<HashMap<usize, CrosshairPlacement>>,
) {
    tracing::info!("CrosshairManager[{}]: entering event loop", instance_id);
    loop {
        let (device, tracking) = match receiver.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => {
                tracing::warn!("CrosshairManager[{}]: tracking channel closed", instance_id);
                break;
            }
        };
        tracing::debug!(
            "CrosshairManager[{}] received tracking event for device {:?}: aimpoint=({}, {})",
            instance_id,
            device.uuid,
            tracking.aimpoint.x,
            tracking.aimpoint.y
        );
        let size = rect_signal.peek().size;
        if size.width <= 0.0 || size.height <= 0.0 {
            tracing::warn!("Container size invalid: {}x{}", size.width, size.height);
            continue;
        }

        let Some(key) = hub.peek().device_key(&device) else {
            continue;
        };
        let (x, y) = aimpoint_pixels(
            tracking.aimpoint.x,
            tracking.aimpoint.y,
            size.width,
            size.height,
        );
        crosshair_positions
            .write()
            .insert(key, (x, y, crosshair_image(key)));
    }
}

#[component]
pub fn CrosshairManager(
    hub: Signal<HubContext>,
    /// Pass the broadcast sender directly to avoid Signal issues across VirtualDoms
    #[props(optional)]
    tracking_sender: Option<TrackingSender>,
) -> Element {
    let instance_id = use_hook(|| CROSSHAIR_MANAGER_ID.fetch_add(1, Ordering::SeqCst));

    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);
    // key → (x_px, y_px, image_src)
    let mut crosshair_positions = use_signal(HashMap::<usize, CrosshairPlacement>::new);

    let sender = tracking_sender.map_or_else(|| hub.peek().tracking_events.clone(), |ts| ts.0);
    use_future(move || {
        let receiver = sender.subscribe();
        tracing::info!(
            "CrosshairManager[{}]: subscribed to tracking events",
            instance_id
        );
        follow_tracking(instance_id, receiver, hub, rect_signal, crosshair_positions)
    });

    use_effect(move || {
        let devices = (hub().devices)();
        let current_keys: Vec<usize> = devices.iter().map(|(key, _)| key).collect();
        crosshair_positions
            .write()
            .retain(|k, _| current_keys.contains(k));
    });

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
                    if let Some(root) = root_div()
                        && let Ok(rect) = root.as_ref().get_client_rect().await
                    {
                        rect_signal.set(rect);
                    }
                }
            },
            for (key, (x, y, src)) in crosshair_positions.read().clone().into_iter() {
                img {
                    key: "{key}",
                    src,
                    class: "absolute pointer-events-none",
                    style: "left: {x}px; top: {y}px; transform: translate(-50%, -50%);",
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aimpoint_scales_to_the_container() {
        assert_eq!(aimpoint_pixels(0.5, 0.25, 800.0, 400.0), (400.0, 100.0));
    }

    #[test]
    fn aimpoint_off_screen_sticks_to_the_edge() {
        assert_eq!(aimpoint_pixels(-0.3, 1.7, 800.0, 400.0), (0.0, 400.0));
    }

    #[test]
    fn crosshair_images_past_the_palette_share_the_last() {
        assert_eq!(
            crosshair_image(99),
            crosshair_image(CROSSHAIR_IMAGES.len() - 1)
        );
    }
}
