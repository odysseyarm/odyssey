use gpui::{App, AppContext as _, Context, Window};
use odyssey_hub_common::device::DeviceCapabilities;

use super::device_controls::DeviceControls;
use super::{TRACKING_MAX_AGE, ZeroView};

impl ZeroView {
    pub(super) fn sync_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let devices = self.hub.read(cx).devices.clone();
        for controls in self.controls.values() {
            let device = devices
                .iter()
                .find(|device| device.uuid == controls.read(cx).device.uuid)
                .cloned();
            controls.update(cx, |controls, cx| controls.sync_connection(device, cx));
        }
        for device in devices
            .iter()
            .filter(|device| device.capabilities.contains(DeviceCapabilities::EVENTS))
        {
            if self.controls.contains_key(&device.uuid) {
                continue;
            }
            let hub = self.hub.clone();
            let controls = cx.new(|cx| DeviceControls::new(device.clone(), hub, window, cx));
            self.control_changes
                .push(cx.observe(&controls, |zero, _, cx| {
                    zero.refresh_readiness(cx);
                    cx.notify();
                }));
            self.controls.insert(device.uuid, controls);
        }
        self.history.retain(|uuid, _| {
            devices
                .iter()
                .any(|device| device.uuid == *uuid && device.events_connected)
        });
        self.tracking.retain(|uuid, _| {
            devices
                .iter()
                .any(|device| device.uuid == *uuid && device.events_connected)
        });
        self.devices = devices;
        self.sync_selection(cx);
        self.refresh_readiness(cx);
        cx.notify();
    }

    pub(super) fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.workspace.read(cx).selected_device;
        if self.selected_device != selected {
            if let Some(controls) = self
                .selected_device
                .and_then(|uuid| self.controls.get(&uuid))
            {
                controls.update(cx, |controls, cx| {
                    controls
                        .state
                        .cancel("Tracker selection changed. Waiting for shot cancelled.");
                    cx.notify();
                });
            }
            self.selected_device = selected;
        }
    }

    pub(super) fn refresh_readiness(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.read(cx);
        let screen_id = workspace
            .calibrations
            .get(&self.monitor)
            .copied()
            .filter(|id| {
                !workspace.calibration_loading
                    && *id <= odyssey_common::MAX_SCREEN_ID
                    && workspace.saved_calibration_ids.contains(id)
            });
        let screen_changed = self.tracking_screen.send_if_modified(|current| {
            if *current == screen_id {
                return false;
            }
            *current = screen_id;
            true
        });
        if screen_changed {
            self.history.clear();
            self.tracking.clear();
        }
        for (&uuid, controls) in &self.controls {
            let controls_state = controls.read(cx);
            let ready = self.selected_device == Some(uuid)
                && controls_state.connected
                && controls_state
                    .device
                    .capabilities
                    .contains(DeviceCapabilities::EVENTS)
                && controls_state.device.events_connected
                && controls_state.delay_loaded
                && !controls_state.pending
                && self.tracking.get(&uuid).is_some_and(|(event, received)| {
                    received.elapsed() <= TRACKING_MAX_AGE
                        && screen_id.is_some_and(|screen| u32::from(screen) == event.screen_id)
                        && event.aimpoint.iter().all(|value| value.is_finite())
                        && event.pose.rotation.iter().all(|value| value.is_finite())
                        && event.pose.translation.iter().all(|value| value.is_finite())
                });
            if ready != controls_state.ready {
                controls.update(cx, |controls, cx| {
                    controls.ready = ready;
                    if !ready {
                        controls.state.cancel(
                            "Tracking unavailable or tracker changed. Waiting for shot cancelled.",
                        );
                    }
                    cx.notify();
                });
                cx.notify();
            }
        }
    }

    pub(super) fn readiness_message(&self, cx: &App) -> &'static str {
        let Some(uuid) = self.selected_device else {
            return if self
                .devices
                .iter()
                .any(|device| device.capabilities.contains(DeviceCapabilities::EVENTS))
            {
                "Select one tracker above. Capture only affects the selected tracker."
            } else {
                "Connect a tracker to begin. The center target stays ready while you connect."
            };
        };
        let Some(device) = self.devices.iter().find(|device| device.uuid == uuid) else {
            return "Selected tracker disconnected. Reconnect it, or explicitly select another tracker. Waiting for shot is cancelled.";
        };
        if !device.capabilities.contains(DeviceCapabilities::EVENTS) {
            return "Selected device cannot stream tracking. Choose a tracker, not a control board or bootloader.";
        }
        if !device.events_connected {
            return "Tracker connected, event stream unavailable. Restore tracking connection before capturing.";
        }
        let workspace = self.workspace.read(cx);
        if workspace.calibration_loading {
            return "Loading screen calibration…";
        }
        let Some(screen) = workspace.calibrations.get(&self.monitor) else {
            return if workspace.saved_calibration_ids.is_empty() {
                "No saved screen calibration. Return to workspace and finish screen calibration before aligning."
            } else {
                "Saved calibrations exist, but none is assigned to this display. Return to workspace and check the selected display."
            };
        };
        if let Some(controls) = self.controls.get(&uuid) {
            let controls = controls.read(cx);
            if controls.pending {
                return "Loading or updating shot delay…";
            }
            if !controls.delay_loaded {
                return "Shot delay unavailable. Reload it below before capturing.";
            }
            if controls.state.pending.is_some() {
                return "Command in flight. Wait for its acknowledgement before capturing again.";
            }
            if controls.ready {
                return "Tracking ready. Aim at the center, then press Z once. Click the target to restore keyboard focus after editing delay.";
            }
        }
        if self.tracking.get(&uuid).is_some_and(|(event, received)| {
            received.elapsed() <= TRACKING_MAX_AGE && event.screen_id != u32::from(*screen)
        }) {
            "Tracking another screen. Aim at this screen's markers before capturing."
        } else {
            "Waiting for fresh tracking on this screen. Keep screen markers visible and aim toward the center."
        }
    }
}
