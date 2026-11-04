//! What a pairing row shows: the last thing that happened, and whether a
//! pairing is running.

use dioxus::prelude::*;

use crate::views::device_label::format_address;

/// How long the hub keeps a pairing open waiting for the other side.
pub(super) const PAIRING_TIMEOUT_MS: u32 = 120_000;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct PairingState {
    pub(super) status: Signal<String>,
    pub(super) pairing: Signal<bool>,
}

/// The status line for a finished pairing.
pub(super) fn result_status(
    success: bool,
    paired_address: &[u8; 6],
    paired_name: &str,
    error: &str,
) -> String {
    if !success {
        return error.to_string();
    }
    let label = if paired_name.is_empty() {
        format_address(paired_address)
    } else {
        format!("{paired_name} ({})", format_address(paired_address))
    };
    format!("Paired with {label}. Please unplug and replug the dongle.")
}

impl PairingState {
    pub(super) fn use_new() -> Self {
        Self {
            status: use_signal(String::new),
            pairing: use_signal(|| false),
        }
    }

    pub(super) fn is_pairing(self) -> bool {
        *self.pairing.peek()
    }

    pub(super) fn started(mut self) {
        self.pairing.set(true);
        self.status.set("Pairing...".into());
    }

    pub(super) fn start_failed(mut self, error: impl std::fmt::Display) {
        self.error(error);
        self.pairing.set(false);
    }

    pub(super) fn cancelled(mut self) {
        self.status.set("Cancelled".into());
        self.pairing.set(false);
    }

    /// Stops showing a pairing as running, e.g. before its bonds are cleared.
    pub(super) fn stopped(mut self) {
        self.pairing.set(false);
    }

    pub(super) fn error(mut self, error: impl std::fmt::Display) {
        self.status.set(format!("Error: {error}"));
    }

    pub(super) fn bonds_cleared(mut self, message: &str) {
        self.status.set(message.into());
    }

    pub(super) fn finished(mut self, status: String) {
        self.pairing.set(false);
        self.status.set(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: [u8; 6] = [0x0A, 0xBC, 0, 1, 2, 0xFF];

    #[test]
    fn a_named_pairing_shows_the_name_and_the_address() {
        assert_eq!(
            result_status(true, &ADDRESS, "Trigger", ""),
            "Paired with Trigger (0A:BC:00:01:02:FF). Please unplug and replug the dongle."
        );
    }

    #[test]
    fn an_unnamed_pairing_shows_only_the_address() {
        assert_eq!(
            result_status(true, &ADDRESS, "", ""),
            "Paired with 0A:BC:00:01:02:FF. Please unplug and replug the dongle."
        );
    }

    #[test]
    fn a_failed_pairing_shows_the_reason() {
        assert_eq!(result_status(false, &ADDRESS, "", "timed out"), "timed out");
    }
}
