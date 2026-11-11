//! Pausing, restarting and consuming a unit's sensor streams.

use odyssey_usb::device::VmDevice;
use tracing::{info, warn};

use super::DeviceHandler;
use crate::device_tasks::sensor_streams::{DeviceStreamEvent, SensorStreams};

impl DeviceHandler {
    /// Stop the sensor streams ahead of a register or config round trip.
    ///
    /// The firmware's `obj_loop` holds the PAG mutex during `get_objects()`. If a register
    /// access arrives while that lock is held, `recv_loop` blocks for the whole call. So the
    /// streams are cleared first (`clear_all_streams` sends `DisableAll` and waits for the `Ack`,
    /// which guarantees the firmware has processed it before the register access reaches it).
    /// The standby link's streams stop too: they are the same subscriptions on the unit.
    ///
    /// Only the first access of a batch pauses. Callers then
    /// [`arm_resume`](super::super::sensor_streams::StreamRestartTimers::arm_resume) so
    /// the streams restart once no access has arrived for a short while, instead of
    /// cycling around every individual access.
    pub(super) async fn pause_sensor_streams(&mut self, links: &[VmDevice], operation: &str) {
        let standby_running = self
            .standby
            .as_ref()
            .is_some_and(|link| link.sensors.is_running());
        if !self.sensors.is_running() && !standby_running {
            return;
        }
        self.stream_timers.paused = true;
        self.sensors.stop();
        if let Some(link) = &mut self.standby {
            link.sensors.stop();
        }
        // A cable pulled mid-batch fails the DisableAll over USB. The next link reaches
        // the same subscriptions.
        for link in links {
            match link.clear_all_streams().await {
                Ok(()) => return,
                Err(e) => warn!(
                    "clear_all_streams failed before {operation} for {:02x?}: {e}",
                    self.device.uuid
                ),
            }
        }
    }

    /// The debounce after a batch of register accesses expired: the firmware's PAG mutex
    /// is free again, so restart the streams.
    pub(super) async fn on_streams_resume_due(&mut self) {
        self.stream_timers.resume_armed = false;
        self.stream_timers.paused = false;
        info!(
            "Register access batch done, restarting sensor streams for {:02x?}",
            self.device.uuid
        );
        self.start_stopped_streams().await;
    }

    /// Streams that are not running (e.g. a broken protocol, or none ever started) are
    /// retried on a fixed interval.
    pub(super) async fn on_streams_retry_due(&mut self) {
        info!("Retrying sensor streams for {:02x?}", self.device.uuid);
        self.start_stopped_streams().await;
        self.stream_timers.delay_retry();
    }

    /// Subscribe again on every attached link whose streams are not running. The events
    /// device goes first: it is the link the reports come over.
    async fn start_stopped_streams(&mut self) {
        if self.image_reboot_required {
            return;
        }
        let uuid = self.device.uuid;
        if !self.sensors.is_running()
            && let Some(events_device) = self.connections.events_device.as_ref()
        {
            self.sensors = SensorStreams::start(events_device, uuid, self.protocol_version).await;
        }
        if let Some(link) = &mut self.standby
            && !link.sensors.is_running()
        {
            link.sensors = SensorStreams::start(&link.device, uuid, self.protocol_version).await;
        }
    }

    /// `None` means every stream ended.
    pub(super) async fn on_sensor_event(&mut self, event: Option<DeviceStreamEvent>) {
        let Some(event) = event else {
            info!(
                "Sensor streams ended for device {:02x?}, will retry in 5s",
                self.device.uuid
            );
            // The handler stays alive so the device stays listed and can still be
            // flashed over DFU. The USB scan loop sends RemoveControlDevice once the
            // device is unplugged.
            self.sensors.stop();
            self.stream_timers.delay_retry();
            return;
        };
        self.tracker
            .handle_event(&self.device, event, &self.context)
            .await;
    }

    /// Reports reach the standby link once the firmware has moved off USB, which can be
    /// before the USB scan sees the cable go. They are forwarded like any other.
    pub(super) async fn on_standby_event(&mut self, event: Option<DeviceStreamEvent>) {
        let Some(event) = event else {
            info!(
                "Standby BLE streams ended for device {:02x?}, will retry in 5s",
                self.device.uuid
            );
            if let Some(link) = &mut self.standby {
                link.sensors.stop();
            }
            self.stream_timers.delay_retry();
            return;
        };
        self.tracker
            .handle_event(&self.device, event, &self.context)
            .await;
    }
}
