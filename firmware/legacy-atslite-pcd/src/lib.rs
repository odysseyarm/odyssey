#![expect(
    unsafe_code,
    reason = "shared-memory accessors form references to the cross-core PCD SRAM region"
)]
#![no_std]

//! Peripheral CPU DFU (PCD) library for nRF5340 network core firmware updates
//!
//! The application core uses it to hand network core firmware update
//! instructions to the network core bootloader through shared SRAM (ICMSG region).
//!
//! Protocol flow:
//! 1. App core writes netcore firmware to DFU partition (in app flash)
//! 2. App core writes PcdCmd with update instruction to shared SRAM
//! 3. App core resets network core
//! 4. Netcore bootloader reads PcdCmd from shared SRAM
//! 5. If update pending, netcore copies firmware and writes PcdStatus
//! 6. Netcore boots application
//! 7. Netcore app confirms boot by writing PcdStatus
//!
//! Shared access uses atomics, including diagnostic words. Neither core creates
//! an exclusive Rust reference to SRAM that the other core may still access.

use core::sync::atomic::{AtomicU32, Ordering};

/// Marks a valid PCD command structure.
///
/// Since: Odyssey Desktop 0.7.0.
pub const PCD_CMD_MAGIC: u32 = 0x50434431; // "PCD1"

/// Marks a valid PCD status structure.
///
/// Since: Odyssey Desktop 0.7.0.
pub const PCD_STATUS_MAGIC: u32 = 0x50434453; // "PCDS"

/// How many times the application core starts the network core bootloader for
/// one update before it gives up and lets the unit back into DFU. A copy takes
/// seconds, so a command still unanswered after a few tries is not going to be.
///
/// Since: Odyssey Desktop 0.7.0.
pub const MAX_COPY_ATTEMPTS: u32 = 3;

/// The attempt count to persist for this start, given the count stored in
/// flash (erased flash means none yet), or `None` once the attempts are spent.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn next_copy_attempt(stored: u32) -> Option<u32> {
    let used = if stored == u32::MAX { 0 } else { stored };
    (used < MAX_COPY_ATTEMPTS).then_some(used + 1)
}

/// PCD command written by application core
///
/// Since: Odyssey Desktop 0.7.0.
#[repr(C)]
pub struct PcdCmd {
    /// Magic number (PCD_CMD_MAGIC when valid)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub magic: AtomicU32,

    /// Address of firmware image in app core flash
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub image_addr: AtomicU32,

    /// Size of firmware image in bytes
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub image_size: AtomicU32,

    /// SHA-256 hash of firmware image (optional, first 32 bytes)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub image_sha: [u8; 32],
}

/// PCD status written by network core
///
/// Since: Odyssey Desktop 0.7.0.
#[repr(C)]
pub struct PcdStatus {
    /// Magic number (PCD_STATUS_MAGIC when valid)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub magic: AtomicU32,

    /// Status code
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub status: AtomicU32,

    /// Not part of the protocol. The app core bootloader stores the page-padded
    /// image size in `[0]` and a progress marker ('NTP'/'NTD') in `[1]`, for
    /// debugging without RTT.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub reserved: [AtomicU32; 2],
}

/// Status codes for PcdStatus
///
/// Since: Odyssey Desktop 0.7.0.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcdStatusCode {
    /// No update performed
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Idle = 0,

    /// Update in progress
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Updating = 1,

    /// Update completed successfully
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Success = 2,

    /// Update failed - copy error
    ///
    /// Since: Odyssey Desktop 0.7.0.
    FailedCopy = 3,

    /// Update failed - verification error
    ///
    /// Since: Odyssey Desktop 0.7.0.
    FailedVerify = 4,

    /// Application booted and confirmed
    ///
    /// Since: Odyssey Desktop 0.7.0.
    AppConfirmed = 5,
}

impl PcdCmd {
    /// Get the command structure at the given address.
    ///
    /// # Safety
    ///
    /// `addr` must be aligned for `Self` and point to initialized, readable and
    /// writable SRAM that remains mapped for `'static`. Concurrent access must
    /// use atomics.
    /// The non-atomic `image_sha` must not be mutated while this reference lives.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub unsafe fn at(addr: usize) -> &'static Self {
        unsafe { &*(addr as *const Self) }
    }

    /// Check if command is valid
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == PCD_CMD_MAGIC
    }

    /// Get image address
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn get_image_addr(&self) -> u32 {
        self.image_addr.load(Ordering::Acquire)
    }

    /// Get image size
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn get_image_size(&self) -> u32 {
        self.image_size.load(Ordering::Acquire)
    }

    /// Set update command (app core)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn set_update(&self, addr: u32, size: u32) {
        self.image_addr.store(addr, Ordering::Release);
        self.image_size.store(size, Ordering::Release);
        // Write the magic last, after the rest of the command is written
        self.magic.store(PCD_CMD_MAGIC, Ordering::Release);
    }

    /// Clear command (netcore after processing)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn clear(&self) {
        self.magic.store(0, Ordering::Release);
    }
}

impl PcdStatus {
    /// Get the status structure at the given address.
    ///
    /// # Safety
    ///
    /// `addr` must be aligned for `Self` and point to initialized, readable and
    /// writable SRAM that remains mapped for `'static`. All concurrent access
    /// must use atomics.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub unsafe fn at(addr: usize) -> &'static Self {
        unsafe { &*(addr as *const Self) }
    }

    /// Check if status is valid
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == PCD_STATUS_MAGIC
    }

    /// Get status code
    ///
    /// Since: Odyssey Desktop 0.7.0.
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

    /// Set status (netcore)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn set_status(&self, status: PcdStatusCode) {
        self.status.store(status as u32, Ordering::Release);
        // Write the magic last, after the rest of the status is written
        self.magic.store(PCD_STATUS_MAGIC, Ordering::Release);
    }

    /// Clear status
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn clear(&self) {
        self.magic.store(0, Ordering::Release);
    }
}

/// Memory layout of PCD region in shared SRAM
///
/// Since: Odyssey Desktop 0.7.0.
#[repr(C)]
pub struct PcdRegion {
    /// Since: Odyssey Desktop 0.7.0.
    pub cmd: PcdCmd,
    /// Since: Odyssey Desktop 0.7.0.
    pub status: PcdStatus,
}

impl PcdRegion {
    /// Get the PCD region at the given address.
    ///
    /// # Safety
    ///
    /// `addr` must satisfy the requirements of [`PcdCmd::at`] and
    /// [`PcdStatus::at`] for the corresponding fields.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
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
    ///
    /// Since: Odyssey Desktop 0.7.0.
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
