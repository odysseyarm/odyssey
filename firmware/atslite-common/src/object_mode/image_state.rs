use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Packet publication and settings exclusion have different lifetimes: capture ends before
/// cleanup, while settings stay owned until all pending restoration has succeeded.
pub(super) struct ImageOwnership {
    active: AtomicBool,
    settings_busy: AtomicBool,
    usb_generation: AtomicU32,
    generation: AtomicU32,
}

impl ImageOwnership {
    pub(super) fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            settings_busy: AtomicBool::new(false),
            usb_generation: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }

    pub(super) fn request_is_current(request_generation: u32, usb_generation: u32) -> bool {
        request_generation == usb_generation
    }

    pub(super) fn accepts(&self, generation: u32, usb_generation: u32) -> bool {
        self.active.load(Ordering::Acquire)
            && self.generation() == generation
            && self.usb_generation() == usb_generation
    }

    pub(super) fn generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(super) fn usb_generation(&self) -> u32 {
        self.usb_generation.load(Ordering::Relaxed)
    }

    pub(super) fn begin(&self, generation: u32, usb_generation: u32) {
        self.generation.store(generation, Ordering::Relaxed);
        self.usb_generation.store(usb_generation, Ordering::Relaxed);
        self.settings_busy.store(true, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    pub(super) fn finish_capture(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub(super) fn finish_restore(&self, restored: bool) {
        self.settings_busy.store(!restored, Ordering::Release);
    }

    pub(super) fn settings_writable(&self) -> bool {
        !self.settings_busy.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::ImageOwnership;

    #[test]
    fn settings_writes_stay_excluded_until_failed_cleanup_recovers() {
        let state = ImageOwnership::new();
        assert!(state.settings_writable());
        state.begin(7, 10);
        state.finish_capture();
        state.finish_restore(false);
        // Packet delivery stopped, but accepting writes here would let a retry overwrite
        // an acknowledged live register change with the saved pre-image settings.
        assert!(!state.accepts(7, 10));
        assert!(!state.settings_writable());
        state.finish_restore(false);
        assert!(!state.settings_writable());
        state.finish_restore(true);
        assert!(state.settings_writable());
    }

    #[test]
    fn reconnect_invalidates_queued_start_and_owner_despite_same_wire_generation() {
        let state = ImageOwnership::new();
        let queued_usb_generation = 20;
        state.begin(1, queued_usb_generation);
        // A brief disconnect changes the retained epoch even when already configured again
        // by the time an in-flight SPI operation completes. A new host may reuse wire id 1.
        let reconnected_usb_generation = 21;
        assert!(!ImageOwnership::request_is_current(
            queued_usb_generation,
            reconnected_usb_generation,
        ));
        assert!(!state.accepts(1, reconnected_usb_generation));
        state.finish_capture();
        state.finish_restore(true);
        state.begin(1, reconnected_usb_generation);
        assert!(state.accepts(1, reconnected_usb_generation));
        assert!(!state.accepts(1, queued_usb_generation));
    }
}
