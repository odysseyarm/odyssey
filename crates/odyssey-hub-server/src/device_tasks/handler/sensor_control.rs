//! Pausing, restarting and consuming a unit's sensor streams.

use odyssey_usb::device::VmDevice;
use tracing::{info, warn};

use super::DeviceHandler;
use crate::device_tasks::sensor_streams::{DeviceStreamEvent, SensorStreams};

impl DeviceHandler {
    /// Stop the sensor streams ahead of a register or config round trip.
    ///
    /// The firmware's `obj_loop` holds the PAG mutex during `get_objects()`; if a register
    /// access arrives while that lock is held, `recv_loop` blocks for the whole call. So the
    /// streams are cleared first (`clear_all_streams` sends DisableAll and waits for the Ack,
    /// which guarantees the firmware has processed it before the register access reaches it).
    ///
    /// Only the first access of a batch pauses; callers then
    /// [`arm_resume`](super::super::sensor_streams::StreamRestartTimers::arm_resume) so
    /// the streams restart once no access has arrived for a short while, instead of
    /// cycling around every individual access.
    pub(super) async fn pause_sensor_streams(&mut self, device: &VmDevice, operation: &str) {
        if !self.sensors.is_running() {
            return;
        }
        self.stream_timers.paused = true;
        self.sensors.stop();
        if let Err(e) = device.clear_all_streams().await {
            warn!(
                "clear_all_streams failed before {operation} for {:02x?}: {e}",
                self.device.uuid
            );
        }
    }

    /// The debounce after a batch of register accesses expired: the firmware's PAG mutex
    /// is free again, so restart the streams.
    pub(super) async fn on_streams_resume_due(&mut self) {
        self.stream_timers.resume_armed = false;
        self.stream_timers.paused = false;
        if self.sensors.is_running() {
            return;
        }
        let Some(events_device) = self.connections.events_device.as_ref() else {
            return;
        };
        info!(
            "Register access batch done, restarting sensor streams for {:02x?}",
            self.device.uuid
        );
        self.sensors =
            SensorStreams::start(events_device, self.device.uuid, self.protocol_version).await;
    }

    /// Streams that are not running (e.g. a broken protocol, or none ever started) are
    /// retried on a fixed interval.
    pub(super) async fn on_streams_retry_due(&mut self) {
        info!("Retrying sensor streams for {:02x?}", self.device.uuid);
        // The select! guard only fires this branch while an events device exists.
        let Some(events_device) = self.connections.events_device.as_ref() else {
            return;
        };
        self.sensors =
            SensorStreams::start(events_device, self.device.uuid, self.protocol_version).await;
        self.stream_timers.delay_retry();
    }

    /// `None` means every stream ended.
    pub(super) async fn on_sensor_event(&mut self, event: Option<DeviceStreamEvent>) {
        let Some(event) = event else {
            info!(
                "Sensor streams ended for device {:02x?}, will retry in 5s",
                self.device.uuid
            );
            // Don't break — keep the handler alive so the device stays
            // in the list and the user can still trigger a DFU update.
            // The USB scan loop will send RemoveControlDevice when the
            // device is physically unplugged.
            self.sensors.stop();
            self.stream_timers.delay_retry();
            return;
        };
        self.tracker
            .handle_event(&self.device, event, &self.context)
            .await;
    }
}
