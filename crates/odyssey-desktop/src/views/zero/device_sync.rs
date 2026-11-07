//! Keeps the per-device zeroing state in step with the devices that are
//! connected.

use gpui::{AppContext as _, Context, Window};
use odyssey_hub_common::device::Device;

use super::ZeroView;
use super::device_controls::DeviceControls;
use crate::runtime;

impl ZeroView {
    /// Drops state when devices leave and loads the stored shot delay only for
    /// newly connected devices. Late replies never recreate removed state or
    /// overwrite a delay the user has already edited.
    pub(super) fn sync_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let devices = self.hub.read(cx).devices.clone();
        let snapshot: Vec<[u8; 6]> = devices.iter().map(|device| device.uuid).collect();

        // The sidebar draws every connected device, so each one has state from
        // the first paint on, and a renamed device shows its new label.
        for device in &devices {
            match self.controls.get(&device.uuid) {
                Some(controls) => {
                    controls.update(cx, |controls, cx| {
                        controls.set_device(device.clone());
                        cx.notify();
                    });
                }
                None => {
                    let hub = self.hub.clone();
                    let controls =
                        cx.new(|cx| DeviceControls::new(device.clone(), hub, window, cx));
                    self.controls.insert(device.uuid, controls);
                }
            }
        }
        self.devices = devices.clone();

        if self.last_snapshot == snapshot {
            cx.notify();
            return;
        }
        let previous = std::mem::replace(&mut self.last_snapshot, snapshot.clone());
        self.controls.retain(|uuid, _| snapshot.contains(uuid));

        for device in devices {
            if previous.contains(&device.uuid) {
                continue;
            }
            self.load_shot_delay(device, cx);
        }
        cx.notify();
    }

    fn load_shot_delay(&self, device: Device, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.get(&device.uuid) else {
            return;
        };
        let revision = controls.read(cx).state.shot_delay_revision();
        let controls = controls.downgrade();
        let Some(client) = self.hub.read(cx).client() else {
            tracing::warn!("Failed to get shot delay: not connected to the hub");
            return;
        };
        let reply = runtime::spawn({
            let device = device.clone();
            async move { client.device(device).get_shot_delay().await }
        });
        cx.spawn(async move |this, cx| {
            let delay = match reply.await {
                Ok(Ok(delay)) => delay,
                Ok(Err(e)) => {
                    tracing::error!("Failed to get shot delay: {e}");
                    return;
                }
                Err(e) => {
                    tracing::error!("Failed to get shot delay: {e}");
                    return;
                }
            };
            this.update(cx, |zero, cx| {
                let still_present = zero
                    .controls
                    .get(&device.uuid)
                    .is_some_and(|current| current.entity_id() == controls.entity_id());
                let Some(controls) = controls.upgrade() else {
                    return;
                };
                if !still_present {
                    return;
                }
                controls.update(cx, |controls, cx| {
                    if controls.state.shot_delay_revision() == revision {
                        controls.state.set_shot_delay_ms(delay);
                        cx.notify();
                    }
                });
            })
            .ok();
        })
        .detach();
    }
}
