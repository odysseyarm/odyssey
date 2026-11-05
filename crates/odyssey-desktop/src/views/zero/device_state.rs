//! Per-device zeroing state shared by the sidebar controls and the event feeds.

/// `None` while idle, `Some(Ok(()))` once saved, `Some(Err(msg))` when saving failed.
pub(super) type SaveStatus = Option<Result<(), String>>;

/// The largest shot delay the controls offer, in milliseconds.
pub(super) const MAX_SHOT_DELAY_MS: u16 = 1023;

#[derive(Default)]
pub(super) struct DeviceState {
    /// Zero on the device's next shot.
    pub(super) shooting: bool,
    /// Zero on the next press of the Z key.
    pub(super) zeroing_on_key: bool,
    shot_delay: u16,
    /// Bumped by every edit of the shot delay, so a late reply can tell that
    /// the user has edited it since the request went out.
    shot_delay_revision: u64,
    pub(super) save_zero_status: SaveStatus,
}

impl DeviceState {
    pub(super) fn shot_delay_ms(&self) -> u16 {
        self.shot_delay
    }

    pub(super) fn set_shot_delay_ms(&mut self, delay_ms: u16) {
        self.shot_delay = delay_ms;
        self.shot_delay_revision = self.shot_delay_revision.wrapping_add(1);
    }

    pub(super) fn shot_delay_revision(&self) -> u64 {
        self.shot_delay_revision
    }
}
