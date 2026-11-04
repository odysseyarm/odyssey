#![no_std]

//! Peripheral CPU DFU (PCD): how the nRF5340 application core tells the
//! network core bootloader to install a network core image, through shared
//! SRAM in the ICMSG region.
//!
//! 1. The app core writes the netcore image to the DFU partition in app flash.
//! 2. It writes a [`PcdCmd`] to shared SRAM and resets the network core.
//! 3. The netcore bootloader reads the command, copies the image if one is
//!    pending, and reports the outcome in [`PcdStatus`].
//! 4. The netcore application boots and confirms itself through [`PcdStatus`].
//!
//! Both cores hold shared references to this region. Every mutable word is atomic; never
//! create an exclusive reference while the other core may access it.

use core::sync::atomic::{AtomicU32, Ordering};

/// Marks a valid [`PcdCmd`].
pub const PCD_CMD_MAGIC: u32 = 0x50434431; // "PCD1"

/// Marks a valid [`PcdStatus`].
pub const PCD_STATUS_MAGIC: u32 = 0x50434453; // "PCDS"

/// How many times the application core starts the network core bootloader for
/// one update before it gives up and lets the unit back into DFU. A copy takes
/// seconds, so a command still unanswered after a few tries is not going to be.
pub const MAX_COPY_ATTEMPTS: u32 = 3;

/// The attempt count to persist for this start, given the count stored in
/// flash (erased flash means none yet), or `None` once the attempts are spent.
pub fn next_copy_attempt(stored: u32) -> Option<u32> {
    let used = if stored == u32::MAX { 0 } else { stored };
    (used < MAX_COPY_ATTEMPTS).then_some(used + 1)
}

/// Written by the application core.
#[repr(C)]
pub struct PcdCmd {
    /// [`PCD_CMD_MAGIC`] when the command is valid.
    pub magic: AtomicU32,

    /// Image address in app core flash.
    pub image_addr: AtomicU32,

    /// Size of firmware image in bytes
    pub image_size: AtomicU32,

    /// SHA-256 of the image; optional.
    pub image_sha: [u8; 32],
}

/// Written by the network core.
#[repr(C)]
pub struct PcdStatus {
    /// [`PCD_STATUS_MAGIC`] when the status is valid.
    pub magic: AtomicU32,

    /// A [`PcdStatusCode`].
    pub status: AtomicU32,

    pub reserved: [AtomicU32; 2],
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcdStatusCode {
    /// No update performed
    Idle = 0,

    /// Copy in progress.
    Updating = 1,

    /// Copy finished and verified.
    Success = 2,

    FailedCopy = 3,

    /// The copied image did not match the source.
    FailedVerify = 4,

    /// The netcore application booted and confirmed itself.
    AppConfirmed = 5,
}

impl PcdCmd {
    /// # Safety
    ///
    /// `addr` must point to a `PcdCmd` that stays mapped for `'static`.
    pub unsafe fn at(addr: usize) -> &'static Self {
        unsafe { &*(addr as *const Self) }
    }

    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == PCD_CMD_MAGIC
    }

    pub fn get_image_addr(&self) -> u32 {
        self.image_addr.load(Ordering::Acquire)
    }

    pub fn get_image_size(&self) -> u32 {
        self.image_size.load(Ordering::Acquire)
    }

    /// Called by the app core.
    pub fn set_update(&self, addr: u32, size: u32) {
        self.image_addr.store(addr, Ordering::Release);
        self.image_size.store(size, Ordering::Release);
        // Magic goes last so the netcore never sees a valid but half-written command.
        self.magic.store(PCD_CMD_MAGIC, Ordering::Release);
    }

    /// Called by the netcore once the command is handled.
    pub fn clear(&self) {
        self.magic.store(0, Ordering::Release);
    }
}

impl PcdStatus {
    /// # Safety
    ///
    /// `addr` must point to a `PcdStatus` that stays mapped for `'static`.
    pub unsafe fn at(addr: usize) -> &'static Self {
        unsafe { &*(addr as *const Self) }
    }

    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == PCD_STATUS_MAGIC
    }

    /// `None` for a value outside [`PcdStatusCode`].
    pub fn get_status(&self) -> Option<PcdStatusCode> {
        let val = self.status.load(Ordering::Acquire);
        match val {
            0 => Some(PcdStatusCode::Idle),
            1 => Some(PcdStatusCode::Updating),
            2 => Some(PcdStatusCode::Success),
            3 => Some(PcdStatusCode::FailedCopy),
            4 => Some(PcdStatusCode::FailedVerify),
            5 => Some(PcdStatusCode::AppConfirmed),
            _ => None,
        }
    }

    /// Called by the netcore.
    pub fn set_status(&self, status: PcdStatusCode) {
        self.status.store(status as u32, Ordering::Release);
        // Magic goes last so the app core never sees a valid but half-written status.
        self.magic.store(PCD_STATUS_MAGIC, Ordering::Release);
    }

    pub fn clear(&self) {
        self.magic.store(0, Ordering::Release);
    }
}

/// Layout of the PCD region in shared SRAM.
#[repr(C)]
pub struct PcdRegion {
    pub cmd: PcdCmd,
    pub status: PcdStatus,
}

impl PcdRegion {
    /// # Safety
    ///
    /// `addr` must point to a `PcdRegion` that stays mapped for `'static`.
    pub unsafe fn at(addr: usize) -> &'static Self {
        unsafe { &*(addr as *const Self) }
    }

    /// Whether the application core still has to run the network core
    /// bootloader: a command is waiting and no terminal status answers it.
    ///
    /// A freshly written command has no status at all (the bootloader clears it
    /// when it writes the command), so a missing status counts as pending. Only
    /// the status the network core bootloader writes after it finishes, or
    /// fails, settles a command.
    pub fn copy_pending(&self) -> bool {
        if !self.cmd.is_valid() {
            return false;
        }
        if !self.status.is_valid() {
            return true;
        }
        !matches!(
            self.status.get_status(),
            Some(
                PcdStatusCode::Success
                    | PcdStatusCode::FailedCopy
                    | PcdStatusCode::FailedVerify
                    | PcdStatusCode::AppConfirmed
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(cmd: bool, status: Option<PcdStatusCode>) -> PcdRegion {
        // SAFETY: every field is plain integers or atomics, for which all-zero is valid.
        let r: PcdRegion = unsafe { core::mem::zeroed() };
        if cmd {
            r.cmd.set_update(0x1000, 4096);
        }
        if let Some(code) = status {
            r.status.magic.store(PCD_STATUS_MAGIC, Ordering::Release);
            r.status.status.store(code as u32, Ordering::Release);
        }
        r
    }

    #[test]
    fn a_fresh_command_without_status_is_pending() {
        assert!(region(true, None).copy_pending());
    }

    #[test]
    fn no_command_is_never_pending() {
        assert!(!region(false, None).copy_pending());
        assert!(!region(false, Some(PcdStatusCode::Updating)).copy_pending());
    }

    #[test]
    fn an_interrupted_copy_is_resumed() {
        assert!(region(true, Some(PcdStatusCode::Updating)).copy_pending());
        assert!(region(true, Some(PcdStatusCode::Idle)).copy_pending());
    }

    #[test]
    fn a_terminal_status_settles_the_command() {
        for code in [
            PcdStatusCode::Success,
            PcdStatusCode::FailedCopy,
            PcdStatusCode::FailedVerify,
            PcdStatusCode::AppConfirmed,
        ] {
            assert!(!region(true, Some(code)).copy_pending(), "{code:?}");
        }
    }

    #[test]
    fn the_copy_is_attempted_a_bounded_number_of_times() {
        // Erased flash means nothing was tried yet.
        assert_eq!(next_copy_attempt(u32::MAX), Some(1));
        assert_eq!(next_copy_attempt(0), Some(1));
        assert_eq!(next_copy_attempt(1), Some(2));
        assert_eq!(
            next_copy_attempt(MAX_COPY_ATTEMPTS - 1),
            Some(MAX_COPY_ATTEMPTS)
        );
        assert_eq!(next_copy_attempt(MAX_COPY_ATTEMPTS), None);
        // A value that is neither erased nor a plausible count is not retried.
        assert_eq!(next_copy_attempt(0xDEAD_BEEF), None);
    }
}
