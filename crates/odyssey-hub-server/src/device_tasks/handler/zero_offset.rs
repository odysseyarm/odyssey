//! Zeroing: the per-unit rotation that makes the unit point at a chosen screen target.

use nalgebra::{Isometry3, Point2, Translation3, UnitQuaternion};
use odyssey_cv::helpers as ats_helpers;
use odyssey_hub_common::events::{DeviceEventKind, TrackingEvent};
use tracing::{debug, info, warn};

use super::DeviceHandler;
use crate::device_tasks::tracking::calculate_zero_offset_from_tracking_event;

impl DeviceHandler {
    pub(super) async fn on_reset_zero(&mut self) {
        debug!("ResetZero requested for {:02x?}", self.device.uuid);
        self.forget_stored_offset().await;
        self.context
            .emit_device_event(&self.device, DeviceEventKind::ZeroResult(true));
    }

    pub(super) async fn on_clear_zero(&mut self) {
        debug!(
            "ClearZero - removing stored device offset for {:02x?}",
            self.device.uuid
        );
        self.forget_stored_offset().await;
        self.context
            .emit_device_event(&self.device, DeviceEventKind::SaveZeroResult(true));
    }

    pub(super) async fn on_save_zero(&mut self) {
        debug!("SaveZero requested for {:02x?}", self.device.uuid);
        // Keep the guard through the write: concurrent saves must not persist an
        // older snapshot after a newer one.
        let offsets = self.context.device_offsets.lock().await;
        #[cfg(feature = "sim")]
        let offsets = persisted(&offsets);
        let result = odyssey_hub_common::config::device_offsets_save_async(&offsets).await;
        if let Err(error) = &result {
            warn!("Failed to save zero offsets: {error}");
        }
        self.context.emit_device_event(
            &self.device,
            DeviceEventKind::SaveZeroResult(result.is_ok()),
        );
    }

    pub(super) async fn on_zero(
        &mut self,
        translation: Translation3<f32>,
        target: Point2<f32>,
        tracking_event: Option<TrackingEvent>,
    ) {
        let uuid = self.device.uuid;
        info!(
            "Zero received for {uuid:02x?}, translation: {:?}, target: {:?}, has_tracking_event: {}",
            translation.vector,
            target,
            tracking_event.is_some()
        );
        let Some(rotation) = self.zero_rotation(translation, target, tracking_event.as_ref())
        else {
            warn!("Zero offset computation failed for {uuid:02x?} (no valid pose)");
            self.context
                .emit_device_event(&self.device, DeviceEventKind::ZeroResult(false));
            return;
        };
        let offset = Isometry3::from_parts(translation, rotation);
        info!("Zero offset computed for {uuid:02x?}: {offset:?}");
        self.context
            .device_offsets
            .lock()
            .await
            .insert(uuid, offset);
        self.context
            .emit_device_event(&self.device, DeviceEventKind::ZeroResult(true));
    }

    /// The client may supply the tracking event it was looking at when the user zeroed;
    /// otherwise the pose comes from this unit's own tracker state.
    fn zero_rotation(
        &mut self,
        translation: Translation3<f32>,
        target: Point2<f32>,
        tracking_event: Option<&TrackingEvent>,
    ) -> Option<UnitQuaternion<f32>> {
        let screen_calibrations = self.context.screen_calibrations.load();
        match tracking_event {
            Some(tracking_event) => calculate_zero_offset_from_tracking_event(
                translation,
                target,
                &screen_calibrations,
                tracking_event,
            ),
            None => ats_helpers::calculate_zero_offset_quat(
                translation,
                target,
                &screen_calibrations,
                &self.tracker.foveated,
            ),
        }
    }

    async fn forget_stored_offset(&mut self) {
        self.context
            .device_offsets
            .lock()
            .await
            .remove(&self.device.uuid);
    }
}

/// The offsets that belong in the configuration file.
#[cfg(feature = "sim")]
fn persisted(
    offsets: &std::collections::HashMap<[u8; 6], Isometry3<f32>>,
) -> std::borrow::Cow<'_, std::collections::HashMap<[u8; 6], Isometry3<f32>>> {
    if offsets.keys().all(crate::sim::persists) {
        return std::borrow::Cow::Borrowed(offsets);
    }
    std::borrow::Cow::Owned(
        offsets
            .iter()
            .filter(|(uuid, _)| crate::sim::persists(uuid))
            .map(|(uuid, offset)| (*uuid, *offset))
            .collect(),
    )
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use super::*;

    #[test]
    fn simulated_offsets_are_not_written_to_the_users_configuration() {
        let real = [0xd2, 0xa6, 0x07, 0xd5, 0xdc, 0xea];
        let offsets = std::collections::HashMap::from([
            (real, Isometry3::identity()),
            (odyssey_sim::UNIT_UUID, Isometry3::identity()),
        ]);
        assert_eq!(persisted(&offsets).keys().collect::<Vec<_>>(), [&real]);
    }
}
