//! What a pairing row shows: the last thing that happened, and whether a
//! pairing is running; plus what a row pairs, which decides the hub calls.

use odyssey_hub_client::client::{Client, ClientError};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};

use crate::views::device_label::format_address;

/// How long the hub keeps a pairing open waiting for the other side.
pub(super) const PAIRING_TIMEOUT_MS: u32 = 120_000;

/// The UI-facing reason a call could not be made without a hub connection.
pub(super) const NOT_CONNECTED: &str = "not connected to the hub";

#[derive(Default)]
pub(super) struct PairingState {
    pub(super) status: String,
    pub(super) pairing: bool,
    pub(super) command_pending: bool,
}

/// The status line for a finished pairing.
pub(super) fn result_status(
    success: bool,
    paired_address: &[u8; 6],
    paired_name: &str,
    error: &str,
) -> String {
    if !success {
        return format!(
            "Pairing failed: {error}. Check both devices are nearby and in pairing mode, then choose Pair again."
        );
    }
    let label = if paired_name.is_empty() {
        format_address(paired_address)
    } else {
        format!("{paired_name} ({})", format_address(paired_address))
    };
    format!("Paired with {label}. Please unplug and replug the dongle.")
}

impl PairingState {
    pub(super) fn started(&mut self) {
        self.pairing = true;
        self.status = "Pairing… keep both devices nearby (up to two minutes).".into();
    }

    pub(super) fn start_failed(&mut self, error: impl std::fmt::Display) {
        self.error(error);
        self.pairing = false;
    }

    pub(super) fn cancelled(&mut self) {
        self.status = "Cancelled".into();
        self.pairing = false;
    }

    /// Stops showing a pairing as running, e.g. before its bonds are cleared.
    pub(super) fn stopped(&mut self) {
        self.pairing = false;
    }

    pub(super) fn error(&mut self, error: impl std::fmt::Display) {
        self.status = format!("Error: {error}");
    }

    pub(super) fn bonds_cleared(&mut self, message: &str) {
        self.status = message.into();
    }

    pub(super) fn finished(&mut self, status: String) {
        self.pairing = false;
        self.status = status;
    }
}

/// What a row pairs: a dongle by id, or a device that pairs with a controller.
#[derive(Clone)]
pub(super) enum PairTarget {
    Dongle(String),
    Device(Device),
}

impl PairTarget {
    /// How log lines name the pairing.
    pub(super) fn pairing_noun(&self) -> &'static str {
        match self {
            Self::Dongle(_) => "dongle pairing",
            Self::Device(_) => "pairing",
        }
    }

    /// How log lines name the bonds that clearing forgets.
    pub(super) fn bonds_noun(&self) -> &'static str {
        match self {
            Self::Dongle(_) => "dongle bonds",
            Self::Device(_) => "bond",
        }
    }

    pub(super) fn clear_label(&self) -> &'static str {
        match self {
            Self::Dongle(_) => "Clear Bonds",
            Self::Device(_) => "Clear Bond",
        }
    }

    /// The question the clear button asks before it acts.
    pub(super) fn clear_prompt(&self) -> &'static str {
        match self {
            Self::Dongle(_) => "Forget every device this dongle is bonded to?",
            Self::Device(_) => "Forget this device's bond?",
        }
    }

    pub(super) fn cleared_message(&self) -> &'static str {
        match self {
            Self::Dongle(_) => "Bonds cleared",
            Self::Device(_) => "Bond cleared",
        }
    }

    pub(super) async fn start(&self, client: &Client) -> Result<(), ClientError> {
        match self {
            Self::Dongle(id) => {
                client
                    .start_dongle_pairing(id.clone(), PAIRING_TIMEOUT_MS)
                    .await
            }
            Self::Device(device) => {
                client
                    .device(device.clone())
                    .start_pairing(PAIRING_TIMEOUT_MS)
                    .await
            }
        }
    }

    pub(super) async fn cancel(&self, client: &Client) -> Result<(), ClientError> {
        match self {
            Self::Dongle(id) => client.cancel_dongle_pairing(id.clone()).await,
            Self::Device(device) => client.device(device.clone()).cancel_pairing().await,
        }
    }

    pub(super) async fn clear(&self, client: &Client) -> Result<(), ClientError> {
        match self {
            Self::Dongle(id) => client.clear_dongle_bonds(id.clone()).await,
            Self::Device(device) => client.device(device.clone()).clear_bond().await,
        }
    }

    /// The status line for `event` if it reports this target's pairing result.
    pub(super) fn result_of(&self, event: &Event) -> Option<String> {
        match (self, event) {
            (
                Self::Dongle(id),
                Event::DonglePairingResult {
                    dongle_id,
                    success,
                    paired_address,
                    paired_name,
                    error,
                },
            ) if dongle_id == id => {
                Some(result_status(*success, paired_address, paired_name, error))
            }
            (
                Self::Device(target),
                Event::DeviceEvent(DeviceEvent(
                    device,
                    DeviceEventKind::PairingResult {
                        success,
                        paired_address,
                        paired_name,
                        error,
                    },
                )),
            ) if device.uuid == target.uuid => {
                Some(result_status(*success, paired_address, paired_name, error))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_start_can_retry_and_finish_without_stale_running_state() {
        let mut state = PairingState::default();
        state.started();
        assert!(state.pairing);
        state.start_failed("Disconnected");
        assert!(!state.pairing);
        state.started();
        assert!(state.pairing);
        state.finished(result_status(true, &[1; 6], "Tracker", ""));
        assert!(!state.pairing);
    }
}
