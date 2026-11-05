//! Sending a zero to the hub, from a key press or from a detected shot.

use gpui::{Context, Entity, Task};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use tokio::sync::broadcast::error::RecvError;

use super::history::closest_sample;
use super::{ZERO_TARGET, ZeroView};
use crate::hub::Hub;
use crate::runtime;

/// The translation sent with every zero request, in meters.
const ZERO_TRANSLATION_M: nalgebra::Vector3<f32> = nalgebra::Vector3::new(0., -0.0635, 0.);

/// The key that zeroes every device waiting for it.
const ZERO_KEY: &str = "z";
/// The key that gives up waiting without zeroing.
const CANCEL_KEY: &str = "Escape";

impl ZeroView {
    /// Zeroes `device` against the screen center. `sample` is the tracking event
    /// from the moment of the shot, when one is known.
    fn zero_device(&self, device: Device, sample: Option<TrackingEvent>, cx: &Context<Self>) {
        let Some(client) = self.hub.read(cx).client() else {
            tracing::warn!(
                "Failed to zero device {:x?}: not connected to the hub",
                device.uuid
            );
            return;
        };
        runtime::spawn(async move {
            if let Err(e) = client
                .device(device.clone())
                .zero(ZERO_TRANSLATION_M.into(), ZERO_TARGET.into(), sample)
                .await
            {
                tracing::error!("Failed to zero device {:x?}: {}", device.uuid, e);
            }
        });
    }

    /// Handles a key press: Z zeroes every device that is waiting for it, Escape
    /// stops them waiting.
    pub(super) fn zero_on_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if key != ZERO_KEY && key != CANCEL_KEY {
            return;
        }
        let devices = self.hub.read(cx).devices.clone();
        for device in devices {
            let Some(controls) = self.controls.get(&device.uuid).cloned() else {
                continue;
            };
            if !controls.read(cx).state.zeroing_on_key {
                continue;
            }
            controls.update(cx, |controls, cx| {
                controls.state.zeroing_on_key = false;
                cx.notify();
            });
            if key == CANCEL_KEY {
                continue;
            }
            let sample = closest_sample(
                &self.history,
                &device.uuid,
                controls.read(cx).state.shot_delay_ms(),
                None,
            );
            self.zero_device(device, sample, cx);
        }
    }

    fn zero_if_armed(&mut self, device: Device, impact_timestamp: u32, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.get(&device.uuid).cloned() else {
            tracing::warn!("ImpactEvent: no zeroing state for {:02x?}", device.uuid);
            return;
        };
        let shooting = controls.read(cx).state.shooting;
        tracing::info!(
            "ImpactEvent for {:02x?}: shooting={}",
            device.uuid,
            shooting
        );
        if !shooting {
            return;
        }
        let sample = closest_sample(
            &self.history,
            &device.uuid,
            controls.read(cx).state.shot_delay_ms(),
            Some(impact_timestamp),
        );
        controls.update(cx, |controls, cx| {
            controls.state.shooting = false;
            cx.notify();
        });
        self.zero_device(device, sample, cx);
    }
}

/// Zeroes on every shot of a device that is waiting for one.
pub(super) fn zero_on_impact(hub: &Entity<Hub>, cx: &mut Context<ZeroView>) -> Task<()> {
    let mut receiver = hub.read(cx).subscribe_impact();
    cx.spawn(async move |this, cx| {
        loop {
            match receiver.recv().await {
                Ok((device, impact)) => {
                    let handled = this.update(cx, |zero, cx| {
                        zero.zero_if_armed(device, impact.timestamp, cx);
                    });
                    if handled.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    })
}
