//! Task transitions are independent of the window and device transport.

use crate::workspace::Alignment;

pub(super) const MAX_SHOT_DELAY_MS: u16 = 1023;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Apply,
    Save,
    Reset,
    Clear,
}

impl Operation {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Apply => "Applying zero…",
            Self::Save => "Saving zero…",
            Self::Reset => "Resetting zero…",
            Self::Clear => "Clearing zero…",
        }
    }
}

pub(super) struct Failure {
    pub(super) message: String,
    pub(super) unknown: bool,
}

pub(super) struct DeviceState {
    pub(super) armed: bool,
    pub(super) pending: Option<Operation>,
    pub(super) alignment: Alignment,
    pub(super) unsaved: bool,
    pub(super) unknown: bool,
    pub(super) error: Option<String>,
    pub(super) retry: Option<Operation>,
    pub(super) notice: Option<&'static str>,
    shot_delay: u16,
    shot_delay_revision: u64,
}

impl DeviceState {
    pub(super) fn new(alignment: Alignment) -> Self {
        Self {
            armed: false,
            pending: None,
            alignment,
            unsaved: alignment == Alignment::Applied,
            unknown: false,
            error: None,
            retry: None,
            notice: None,
            shot_delay: 0,
            shot_delay_revision: 0,
        }
    }

    pub(super) fn arm(&mut self, ready: bool) {
        if ready && self.pending.is_none() {
            self.armed = true;
            self.error = None;
            self.retry = None;
            self.notice = None;
        }
    }

    pub(super) fn cancel(&mut self, message: &'static str) {
        if self.armed {
            self.armed = false;
            self.notice = Some(message);
        }
    }

    pub(super) fn begin(&mut self, operation: Operation) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.armed = false;
        self.pending = Some(operation);
        self.error = None;
        self.retry = None;
        self.notice = None;
        true
    }

    pub(super) fn finish(&mut self, operation: Operation, result: Result<(), Failure>) {
        self.pending = None;
        match result {
            Ok(()) => {
                self.unknown = false;
                self.alignment = match operation {
                    Operation::Apply => Alignment::Applied,
                    Operation::Save => Alignment::Saved,
                    Operation::Reset | Operation::Clear => Alignment::NotVerified,
                };
                self.unsaved = operation != Operation::Save;
                self.notice = Some(match operation {
                    Operation::Apply => {
                        "Applied, not saved. Aim at the center again and check the live crosshair before saving."
                    }
                    Operation::Save => "Saved in Odyssey for this tracker.",
                    Operation::Reset | Operation::Clear => {
                        "Active zero removed. Save zero to persist this removal; capture again to realign."
                    }
                });
            }
            Err(failure) => {
                if failure.unknown {
                    self.alignment = Alignment::NotVerified;
                    self.unknown = true;
                    self.unsaved = true;
                }
                self.error = Some(failure.message);
                self.retry = Some(operation);
            }
        }
    }

    pub(super) fn needs_exit_warning(&self) -> bool {
        self.unsaved || self.unknown || self.pending.is_some()
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_apply_never_advances_to_save() {
        let mut state = DeviceState::new(Alignment::NotVerified);
        state.arm(true);
        assert!(state.begin(Operation::Apply));
        assert!(!state.armed);
        state.finish(
            Operation::Apply,
            Err(Failure {
                message: "No valid pose".into(),
                unknown: false,
            }),
        );
        assert_eq!(state.alignment, Alignment::NotVerified);
        assert!(!state.unsaved);
        assert_eq!(state.retry, Some(Operation::Apply));
    }

    #[test]
    fn apply_requires_acknowledgement_and_failed_save_preserves_unsaved_zero() {
        let mut state = DeviceState::new(Alignment::NotVerified);
        state.begin(Operation::Apply);
        assert_eq!(state.alignment, Alignment::NotVerified);
        assert!(state.needs_exit_warning());
        state.finish(Operation::Apply, Ok(()));
        assert_eq!(state.alignment, Alignment::Applied);
        state.begin(Operation::Save);
        state.finish(
            Operation::Save,
            Err(Failure {
                message: "Write failed".into(),
                unknown: false,
            }),
        );
        assert_eq!(state.alignment, Alignment::Applied);
        assert!(state.needs_exit_warning());
        assert!(state.error.is_some());
        state.begin(Operation::Save);
        state.finish(Operation::Save, Ok(()));
        assert_eq!(state.alignment, Alignment::Saved);
        assert!(!state.needs_exit_warning());
    }

    #[test]
    fn cancellation_prevents_capture_and_unready_tracker_cannot_arm() {
        let mut state = DeviceState::new(Alignment::NotVerified);
        state.arm(false);
        assert!(!state.armed);
        state.arm(true);
        state.cancel("Tracker disconnected. Capture cancelled.");
        assert!(!state.armed);
        assert!(state.pending.is_none());
    }

    #[test]
    fn unknown_result_invalidates_saved_alignment_and_protects_exit() {
        let mut state = DeviceState::new(Alignment::Saved);
        state.begin(Operation::Reset);
        state.finish(
            Operation::Reset,
            Err(Failure {
                message: "Disconnected".into(),
                unknown: true,
            }),
        );
        assert_eq!(state.alignment, Alignment::NotVerified);
        assert!(state.unknown);
        assert!(state.needs_exit_warning());
    }

    #[test]
    fn reset_removal_needs_save_even_without_verified_alignment() {
        let mut state = DeviceState::new(Alignment::Saved);
        state.begin(Operation::Clear);
        state.finish(Operation::Clear, Ok(()));
        assert_eq!(state.alignment, Alignment::NotVerified);
        assert!(state.needs_exit_warning());
        state.begin(Operation::Save);
        state.finish(Operation::Save, Ok(()));
        assert!(!state.needs_exit_warning());
    }
}
