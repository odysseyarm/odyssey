use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use gpui::{
    Context, Entity, Image, ImageFormat, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Task, Window, div, img, px, relative,
};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

use crate::hub::{Hub, HubHandle};

macro_rules! crosshair_png {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/images/crosshairs/",
            $name
        ))
    };
}

const CROSSHAIR_IMAGES: [&[u8]; 6] = [
    crosshair_png!("crosshair-red.png"),
    crosshair_png!("crosshair-blue.png"),
    crosshair_png!("crosshair-yellow.png"),
    crosshair_png!("crosshair-green.png"),
    crosshair_png!("crosshair-orange.png"),
    crosshair_png!("crosshair-gray.png"),
];

/// A crosshair image and its natural size, in pixels.
struct Sprite {
    image: Arc<Image>,
    width: f32,
    height: f32,
}

fn load_sprites() -> Vec<Sprite> {
    CROSSHAIR_IMAGES
        .iter()
        .filter_map(|bytes| {
            let reader =
                image::ImageReader::with_format(Cursor::new(*bytes), image::ImageFormat::Png);
            match reader.into_dimensions() {
                Ok((width, height)) => Some(Sprite {
                    image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec())),
                    width: width as f32,
                    height: height as f32,
                }),
                Err(error) => {
                    tracing::error!("Failed to read a crosshair image: {error}");
                    None
                }
            }
        })
        .collect()
}

/// A crosshair's place in the container as fractions of its width and height,
/// and its palette slot.
type CrosshairPlacement = (f32, f32, usize);
type CrosshairPositions = HashMap<[u8; 6], CrosshairPlacement>;

/// Palette slot for the device at list position `slot`; positions past the
/// palette share the last.
fn palette_slot(slot: usize) -> usize {
    slot.min(CROSSHAIR_IMAGES.len() - 1)
}

/// Device-list positions change on a refresh. Keep positions by UUID so a new
/// occupant of a position never inherits the old device's aim.
fn retain_device_crosshairs(positions: &mut CrosshairPositions, devices: &[Device]) {
    positions.retain(|uuid, (_, _, palette)| {
        if let Some(slot) = devices.iter().position(|device| device.uuid == *uuid) {
            *palette = slot;
            true
        } else {
            false
        }
    });
}

/// A normalised aimpoint as container fractions; aimpoints off the screen
/// stick to its edge.
fn clamped_aimpoint(aim_x: f32, aim_y: f32) -> (f32, f32) {
    (aim_x.clamp(0.0, 1.0), aim_y.clamp(0.0, 1.0))
}

/// Draws one crosshair per tracked device over the whole of its container.
pub(crate) struct CrosshairManager {
    hub: Entity<Hub>,
    sprites: Vec<Sprite>,
    positions: CrosshairPositions,
    _hub_changed: Subscription,
    _follow_tracking: Task<()>,
}

impl CrosshairManager {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        let hub_changed = cx.observe(&hub, |manager, hub, cx| {
            retain_device_crosshairs(&mut manager.positions, &hub.read(cx).devices);
            cx.notify();
        });

        let mut receiver = hub.read(cx).subscribe_tracking();
        let follow_tracking = cx.spawn(async move |this, cx| {
            tracing::info!("CrosshairManager: subscribed to tracking events");
            loop {
                let first = match receiver.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => {
                        tracing::warn!("CrosshairManager: tracking channel closed");
                        break;
                    }
                };
                // One repaint per wake: later samples overwrite earlier ones.
                let mut batch = vec![first];
                loop {
                    match receiver.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(TryRecvError::Lagged(_)) => {}
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
                if this
                    .update(cx, |manager, cx| manager.track(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            hub,
            sprites: load_sprites(),
            positions: CrosshairPositions::new(),
            _hub_changed: hub_changed,
            _follow_tracking: follow_tracking,
        }
    }

    fn track(&mut self, batch: Vec<(Device, TrackingEvent)>, cx: &mut Context<Self>) {
        let devices = &self.hub.read(cx).devices;
        for (device, tracking) in batch {
            tracing::debug!(
                "CrosshairManager received tracking event for device {:?}: aimpoint=({}, {})",
                device.uuid,
                tracking.aimpoint.x,
                tracking.aimpoint.y
            );
            let Some(slot) = devices.iter().position(|known| known.uuid == device.uuid) else {
                continue;
            };
            let (x, y) = clamped_aimpoint(tracking.aimpoint.x, tracking.aimpoint.y);
            self.positions.insert(device.uuid, (x, y, slot));
        }
        cx.notify();
    }
}

impl Render for CrosshairManager {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let crosshairs = self.positions.values().filter_map(|&(x, y, slot)| {
            let sprite = self.sprites.get(palette_slot(slot))?;
            Some(
                div().absolute().left(relative(x)).top(relative(y)).child(
                    img(Arc::clone(&sprite.image))
                        .absolute()
                        .left(px(-sprite.width / 2.0))
                        .top(px(-sprite.height / 2.0))
                        .w(px(sprite.width))
                        .h(px(sprite.height)),
                ),
            )
        });
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .children(crosshairs)
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
            ([1; 6], (0.5, 0.25, 0)),
            (surviving.uuid, (0.75, 0.125, 1)),
        ]);

        retain_device_crosshairs(&mut positions, &[surviving]);

        assert_eq!(
            positions,
            CrosshairPositions::from([([2; 6], (0.75, 0.125, 0))])
        );
    }

    #[test]
    fn aimpoint_inside_the_screen_is_unchanged() {
        assert_eq!(clamped_aimpoint(0.5, 0.25), (0.5, 0.25));
    }

    #[test]
    fn aimpoint_off_screen_sticks_to_the_edge() {
        assert_eq!(clamped_aimpoint(-0.3, 1.7), (0.0, 1.0));
    }

    #[test]
    fn crosshair_slots_past_the_palette_share_the_last() {
        assert_eq!(palette_slot(99), CROSSHAIR_IMAGES.len() - 1);
        assert_eq!(palette_slot(2), 2);
    }
}
