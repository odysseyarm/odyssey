//! Selected-tracker capture and acknowledged zero operations.

use std::time::Duration;

use futures::StreamExt as _;
use gpui::{Context, Entity, Task};
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event, TrackingEvent};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use super::device_state::{Failure, Operation};
use super::history::closest_sample;
use super::{ZERO_TARGET, ZeroView};
use crate::hub::Hub;

/// Keep the physical translation and target unchanged.
const ZERO_TRANSLATION_M: nalgebra::Vector3<f32> = nalgebra::Vector3::new(0., -0.0635, 0.);
const ZERO_RESULT_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn run_operation(
    client: Client,
    device: Device,
    operation: Operation,
    sample: Option<TrackingEvent>,
    disconnected: CancellationToken,
) -> Result<(), Failure> {
    let mut dispatched = false;
    let result = tokio::time::timeout(ZERO_RESULT_TIMEOUT, async {
        let mut events = client.subscribe_events().await.map_err(|error| error.to_string())?;
        let tracker = client.device(device.clone());
        // RPC success only means queued. Subscribe first so a fast result cannot be lost.
        dispatched = true;
        match operation {
            Operation::Apply => tracker.zero(ZERO_TRANSLATION_M.into(), ZERO_TARGET.into(), sample).await,
            Operation::Save => tracker.save_zero().await,
            Operation::Reset => tracker.reset_zero().await,
            Operation::Clear => tracker.clear_zero().await,
        }.map_err(|error| error.to_string())?;
        loop {
            tokio::select! {
                () = disconnected.cancelled() => return Err("Tracker disconnected. Result unknown; reconnect and retry before trusting alignment.".to_owned()),
                event = events.next() => {
                    let event = event.ok_or("Hub result stream closed. Result unknown; retry.".to_owned())?
                        .map_err(|error| error.to_string())?;
                    if let Event::DeviceEvent(DeviceEvent(source, kind)) = event
                        && source.uuid == device.uuid
                        && let Some(success) = acknowledged_result(operation, &kind)
                    {
                        return Ok(success);
                    }
                }
            }
        }
    }).await;
    match result {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err(Failure {
            message: match operation {
                Operation::Apply => "Zero was not applied. Check tracking and screen calibration, then capture again.",
                Operation::Save => "Zero was not saved. Keep this task open and retry Save zero.",
                Operation::Reset | Operation::Clear => "Zero was not removed. Retry when the tracker is connected.",
            }.to_owned(),
            unknown: false,
        }),
        Ok(Err(message)) => Err(Failure { message, unknown: dispatched }),
        Err(_) => Err(Failure {
            message: "No acknowledgement within 10 seconds. Result unknown; reconnect and retry before trusting alignment.".to_owned(),
            unknown: dispatched,
        }),
    }
}

fn acknowledged_result(operation: Operation, event: &DeviceEventKind) -> Option<bool> {
    match (operation, event) {
        (Operation::Apply | Operation::Reset, DeviceEventKind::ZeroResult(success))
        | (Operation::Save | Operation::Clear, DeviceEventKind::SaveZeroResult(success)) => {
            Some(*success)
        }
        _ => None,
    }
}

impl ZeroView {
    pub(super) fn capture_selected(
        &mut self,
        impact_timestamp: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.refresh_readiness(cx);
        let Some(uuid) = self.selected_device else {
            return;
        };
        let Some(controls) = self.controls.get(&uuid).cloned() else {
            return;
        };
        if !controls.read(cx).state.armed || !controls.read(cx).ready {
            return;
        }
        let sample = closest_sample(
            &self.history,
            &uuid,
            controls.read(cx).state.shot_delay_ms(),
            impact_timestamp,
        );
        controls.update(cx, |controls, cx| {
            controls.start_operation(Operation::Apply, sample, cx);
        });
    }

    fn zero_if_armed(&mut self, device: Device, impact_timestamp: u32, cx: &mut Context<Self>) {
        if self.selected_device == Some(device.uuid) {
            self.capture_selected(Some(impact_timestamp), cx);
        }
    }
}

pub(super) fn zero_on_impact(hub: &Entity<Hub>, cx: &mut Context<ZeroView>) -> Task<()> {
    let mut receiver = hub.read(cx).subscribe_impact();
    cx.spawn(async move |this, cx| {
        loop {
            match receiver.recv().await {
                Ok((device, impact)) => {
                    if this
                        .update(cx, |zero, cx| {
                            zero.zero_if_armed(device, impact.timestamp, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_acceptance_cannot_stand_in_for_device_result() {
        assert_eq!(
            acknowledged_result(Operation::Apply, &DeviceEventKind::SaveZeroResult(true)),
            None
        );
        assert_eq!(
            acknowledged_result(Operation::Save, &DeviceEventKind::ZeroResult(true)),
            None
        );
        assert_eq!(
            acknowledged_result(Operation::Apply, &DeviceEventKind::ZeroResult(false)),
            Some(false)
        );
        assert_eq!(
            acknowledged_result(Operation::Save, &DeviceEventKind::SaveZeroResult(false)),
            Some(false)
        );
    }
}
