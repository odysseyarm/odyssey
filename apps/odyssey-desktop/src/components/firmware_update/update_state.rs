use super::update_notes::{HeldBack, NetCoreBlocked};

/// State for a single device's firmware update
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FirmwareUpdateState {
    /// No update available or not yet checked
    Idle,
    /// Update is available but device lacks USB control (needs to be plugged in via USB)
    NeedsUsbConnection {
        current: [u16; 3],
        available: String,
    },
    /// Update is available
    Available {
        current: [u16; 3],
        available: String,
        device_type: String,
        vid: u16,
        pid: u16,
        held_back: Option<HeldBack>,
        net_core_blocked: Option<NetCoreBlocked>,
    },
    /// A newer firmware exists but this desktop is too old to install any of it
    NeedsNewerDesktop(HeldBack),
    /// A newer firmware exists but the unit's net core is too old to take any of it
    NetCoreBlockedNoPath(NetCoreBlocked),
    /// Currently downloading firmware
    Downloading,
    /// Sending DFU detach command
    Detaching,
    /// Waiting for device to enter DFU mode
    WaitingForDfu,
    /// Flashing firmware with progress (bytes_written, total_bytes)
    Flashing { progress: usize, total: usize },
    /// Writing the net core (BLE controller) image with progress (bytes_written, total_bytes)
    FlashingNetCore { progress: usize, total: usize },
    /// Waiting for the net core to report the release that was just written
    VerifyingNetCore,
    /// Waiting for the net core bootloader to copy the staged net core image and
    /// the device to return to DFU mode
    InstallingNetCore,
    /// Waiting for device to reboot after flash
    Rebooting,
    /// Verifying the new firmware version
    Verifying,
    /// Update completed successfully
    Complete,
    /// Update failed
    Failed(String),
}

pub(super) const BYTES_PER_KB: usize = 1024;

/// Whole percent of `total` that `progress` covers; an empty image counts as 0.
pub(super) fn percent_complete(progress: usize, total: usize) -> usize {
    (progress * 100).checked_div(total).unwrap_or(0)
}

impl FirmwareUpdateState {
    /// The states before any update has started, which the availability check
    /// owns and may overwrite on every render. Every other state belongs to the
    /// running update.
    pub(super) fn is_before_update(&self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::Available { .. }
                | Self::NeedsUsbConnection { .. }
                | Self::NeedsNewerDesktop(_)
                | Self::NetCoreBlockedNoPath(_)
        )
    }

    /// Bytes written and total bytes of an image being flashed.
    pub(super) fn flash_progress(&self) -> Option<(usize, usize)> {
        match self {
            Self::Flashing { progress, total } | Self::FlashingNetCore { progress, total } => {
                Some((*progress, *total))
            }
            _ => None,
        }
    }

    /// The fixed wording of the state, shared by every place that shows one.
    /// States that carry data show it next to this text.
    pub(super) fn status_text(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::NeedsUsbConnection { .. } => "Plug in USB to update",
            Self::NeedsNewerDesktop(_) => "Needs a newer Odyssey Desktop",
            Self::NetCoreBlockedNoPath(_) => "No update path from this net core",
            Self::Available { .. } => "Update available",
            Self::Downloading => "Downloading...",
            Self::Detaching => "Preparing device...",
            Self::WaitingForDfu => "Waiting for DFU mode...",
            Self::Flashing { .. } => "Flashing...",
            Self::FlashingNetCore { .. } => "Updating radio firmware...",
            Self::VerifyingNetCore => "Checking radio firmware...",
            Self::InstallingNetCore => "Installing radio firmware...",
            Self::Rebooting => "Rebooting device...",
            Self::Verifying => "Verifying update...",
            Self::Complete => "Update complete!",
            Self::Failed(_) => "Update failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_handles_empty_and_partial_images() {
        assert_eq!(percent_complete(0, 0), 0);
        assert_eq!(percent_complete(512, 2048), 25);
        assert_eq!(percent_complete(2048, 2048), 100);
    }

    #[test]
    fn only_flashing_states_report_progress() {
        let flashing = FirmwareUpdateState::Flashing {
            progress: 3,
            total: 9,
        };
        assert_eq!(flashing.flash_progress(), Some((3, 9)));
        assert_eq!(FirmwareUpdateState::Rebooting.flash_progress(), None);
    }

    #[test]
    fn a_running_update_is_not_before_update() {
        assert!(FirmwareUpdateState::Idle.is_before_update());
        assert!(!FirmwareUpdateState::Downloading.is_before_update());
        assert!(!FirmwareUpdateState::Failed("x".into()).is_before_update());
    }
}
