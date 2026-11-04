//! Per-device zeroing state shared by the sidebar controls and the event feeds.

use std::collections::HashMap;

use dioxus::prelude::*;

/// `None` while idle, `Some(Ok(()))` once saved, `Some(Err(msg))` when saving failed.
pub(super) type SaveStatus = Option<Result<(), String>>;

/// Zeroing state by device uuid. Slab slots are reused as devices come and
/// go, so the key is the uuid, never the slot.
pub(super) type SignalsByDevice = HashMap<[u8; 6], DeviceSignals>;

/// The largest shot delay the controls offer, in milliseconds.
pub(super) const MAX_SHOT_DELAY_MS: u16 = 1023;

/// Signals are handles into the runtime's arena, so copying this bundle hands
/// out another reference to the same state rather than duplicating it.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct DeviceSignals {
    /// Zero on the device's next shot.
    pub(super) shooting: Signal<bool>,
    /// Zero on the next press of the Z key.
    pub(super) zeroing_on_key: Signal<bool>,
    pub(super) shot_delay: Signal<u16>,
    shot_delay_revision: Signal<u64>,
    pub(super) save_zero_status: Signal<SaveStatus>,
}

impl DeviceSignals {
    pub(super) fn set_shooting(mut self, on: bool) {
        self.shooting.set(on);
    }

    pub(super) fn set_zeroing_on_key(mut self, on: bool) {
        self.zeroing_on_key.set(on);
    }

    pub(super) fn toggle_zeroing_on_key(mut self) {
        let armed = *self.zeroing_on_key.peek();
        self.zeroing_on_key.set(!armed);
    }

    pub(super) fn shot_delay_ms(self) -> u16 {
        *self.shot_delay.peek()
    }

    pub(super) fn set_shot_delay_ms(mut self, delay_ms: u16) {
        self.shot_delay.set(delay_ms);
        let revision = self.shot_delay_revision();
        self.shot_delay_revision.set(revision.wrapping_add(1));
    }

    pub(super) fn shot_delay_revision(self) -> u64 {
        *self.shot_delay_revision.peek()
    }

    pub(super) fn set_save_status(mut self, status: SaveStatus) {
        self.save_zero_status.set(status);
    }
}
