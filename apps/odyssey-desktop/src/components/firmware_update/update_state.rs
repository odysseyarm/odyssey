use super::update_notes::{HeldBack, NetCoreBlocked};

/// State for a single device's firmware update
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FirmwareUpdateState {
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
