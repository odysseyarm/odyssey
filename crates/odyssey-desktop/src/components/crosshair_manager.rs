use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::html::geometry::Pixels;
use dioxus::html::geometry::euclid::Rect;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use slab::Slab;
use tokio::sync::broadcast;

use crate::hub::HubContext;

/// A `broadcast::Sender` usable as a prop: Dioxus `#[component]` props must be
/// `PartialEq`, and this one always compares unequal so a change re-renders.
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

/// Gives each `CrosshairManager` instance its own id so it gets a fresh subscription.
static CROSSHAIR_MANAGER_ID: AtomicU64 = AtomicU64::new(0);

/// A crosshair's place in the container, in pixels, and its palette slot.
type CrosshairPlacement = (f64, f64, usize);
type CrosshairPositions = HashMap<[u8; 6], CrosshairPlacement>;

/// Image for the device at slab slot `key`; slots past the palette share the last.
fn crosshair_image(key: usize) -> Asset {
    CROSSHAIR_IMAGES[key.min(CROSSHAIR_IMAGES.len() - 1)]
}

/// Slab slots can be reused or reassigned on a device-list refresh. Keep
/// positions by UUID so a new occupant never inherits the old device's aim.
fn retain_device_crosshairs(positions: &mut CrosshairPositions, devices: &Slab<Device>) {
    positions.retain(|uuid, (_, _, palette)| {
        if let Some((slot, _)) = devices.iter().find(|(_, device)| device.uuid == *uuid) {
            *palette = slot;
            true
        } else {
            false
        }
    });
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
    mut crosshair_positions: Signal<CrosshairPositions>,
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
        crosshair_positions.write().insert(device.uuid, (x, y, key));
    }
}

#[component]
pub fn CrosshairManager(
    hub: Signal<HubContext>,
    /// Passed as a prop rather than a `Signal`, which does not work across VirtualDoms.
    #[props(optional)]
    tracking_sender: Option<TrackingSender>,
) -> Element {
    let instance_id = use_hook(|| CROSSHAIR_MANAGER_ID.fetch_add(1, Ordering::SeqCst));

    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);
    let mut crosshair_positions = use_signal(CrosshairPositions::new);

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
        retain_device_crosshairs(&mut crosshair_positions.write(), &devices);
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
            for (uuid, (x, y, palette)) in crosshair_positions.read().iter() {
                img {
                    key: "{uuid:?}",
                    src: crosshair_image(*palette),
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
    fn refreshed_slots_do_not_transfer_positions_between_devices() {
        use odyssey_hub_common::device::{DeviceCapabilities, EventsTransport, Transport};

        let surviving = Device {
            uuid: [2; 6],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::new(DeviceCapabilities::EVENTS),
            firmware_version: [1, 0, 0],
            events_transport: EventsTransport::Wired,
            events_connected: true,
            product_id: 0,
            name: [0; 33],
            netcore_release: 0,
        };
        // Before the refresh, the departed device occupied slot zero and this
        // survivor occupied slot one. A full list refresh reuses slot zero.
        let mut positions = CrosshairPositions::from([
            ([1; 6], (50.0, 60.0, 0)),
            (surviving.uuid, (70.0, 80.0, 1)),
        ]);
        let mut devices = Slab::new();
        devices.insert(surviving);

        retain_device_crosshairs(&mut positions, &devices);

        assert_eq!(
            positions,
            CrosshairPositions::from([([2; 6], (70.0, 80.0, 0))])
        );
    }

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
