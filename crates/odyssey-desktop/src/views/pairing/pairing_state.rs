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
    pub(super) command_pending: Signal<bool>,
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
            command_pending: use_signal(|| false),
        }
    }

    /// Reserve the row before spawning: a cancel or clear must not overtake
    /// an in-flight start, and repeated clicks must not send duplicate commands.
    pub(super) fn run_command(mut self, command: impl Future<Output = ()> + 'static) {
        if *self.command_pending.peek() {
            return;
        }
        self.command_pending.set(true);
        spawn(async move {
            command.await;
            self.command_pending.set(false);
        });
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
