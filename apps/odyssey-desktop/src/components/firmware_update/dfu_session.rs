use std::time::{Duration, Instant};

use dioxus::logger::tracing;
use odyssey_hub_server::firmware;

use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// How long a detached device gets to show up as a DFU bootloader.
const DFU_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the flashing task publishes the byte count it has seen.
const PROGRESS_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
const APP_MODE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const BOOTLOADER_RESET_POLL_INTERVAL: Duration = Duration::from_millis(100);
const RETURN_TO_DFU_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// One device being driven through DFU, and where its progress is published.
#[derive(Clone, Copy)]
pub(super) struct DfuSession<'a> {
    pub(super) manager: &'a FirmwareUpdateManager,
    pub(super) uuid: [u8; 6],
    pub(super) vid: u16,
    pub(super) pid: u16,
}

impl DfuSession<'_> {
    pub(super) fn set_state(&self, state: FirmwareUpdateState) {
        self.manager.set_state(self.uuid, state);
    }

    /// Poll until the device enumerates in application mode, `false` on timeout.
    pub(super) async fn wait_for_app_mode(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            tokio::time::sleep(APP_MODE_POLL_INTERVAL).await;
            if firmware::is_device_in_app_mode(&self.uuid, self.vid, self.pid) {
                return true;
            }
        }
        false
    }

    /// Put the device into DFU mode, detaching it first when it is running its
    /// application.
    pub(super) async fn enter_dfu(&self) -> Result<(), String> {
        // The application also exposes a DFU interface (for the runtime descriptor
        // that takes the detach), so only the DFU-mode protocol means the bootloader
        // is already running and waiting for a download. It enumerates under its own
        // product id, which is why the application's id alone cannot find it.
        let already_in_dfu = firmware::is_device_in_dfu_mode(&self.uuid, self.vid, self.pid);

        if already_in_dfu {
            tracing::info!(
                "Device {:02x?} is already in DFU mode, skipping detach",
                self.uuid
            );
            return Ok(());
        }

        // Send DFU detach
        tracing::info!(
            "Sending DFU detach to {:04x}:{:04x} (uuid {:02x?})",
            self.vid,
            self.pid,
            self.uuid
        );
        self.set_state(FirmwareUpdateState::Detaching);
        firmware::send_dfu_detach(&self.uuid, self.vid, self.pid)
            .await
            .map_err(|e| format!("DFU detach failed: {e}"))?;

        // Wait for device to enter DFU mode
        tracing::info!("Waiting for device to enter DFU mode");
        self.set_state(FirmwareUpdateState::WaitingForDfu);
        firmware::wait_for_dfu_device(&self.uuid, self.vid, self.pid, DFU_ENUMERATION_TIMEOUT)
            .await
            .map_err(|e| format!("Device did not enter DFU mode: {e}"))
    }

    /// Write one image to a DFU alt setting, publishing progress as it goes.
    pub(super) async fn flash_with_progress(
        &self,
        slot: &str,
        alt: u8,
        data: &[u8],
        what: &str,
        progress_state: fn(usize, usize) -> FirmwareUpdateState,
    ) -> Result<(), String> {
        let total_size = data.len();
        self.set_state(progress_state(0, total_size));

        // Publish from the transfer callback itself. A detached polling task can
        // outlive cancellation or race the next phase even after being aborted.
        let mut final_progress = 0;
        let mut last_publish = Instant::now();
        let progress_callback = |bytes_written: usize, _total: usize| {
            final_progress = bytes_written;
            if bytes_written >= total_size || last_publish.elapsed() >= PROGRESS_PUBLISH_INTERVAL {
                self.set_state(progress_state(bytes_written, total_size));
                last_publish = Instant::now();
            }
        };

        let flash_result = firmware::flash_firmware_with_progress(
            &self.uuid,
            self.vid,
            self.pid,
            slot,
            alt,
            data,
            progress_callback,
        )
        .await;

        flash_result.map_err(|e| format!("{what} failed: {e}"))?;

        // Verify flash was complete
        if final_progress < total_size {
            return Err(format!(
                "{what} incomplete: {final_progress}/{total_size} bytes written"
            ));
        }
        Ok(())
    }

    /// Poll until the device stops enumerating as a bootloader, `false` on timeout.
    pub(super) async fn wait_for_bootloader_reset(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !firmware::is_device_in_dfu_mode(&self.uuid, self.vid, self.pid) {
                return true;
            }
            tokio::time::sleep(BOOTLOADER_RESET_POLL_INTERVAL).await;
        }
        false
    }

    /// Wait for a device that has just reset to come back and put it in DFU mode,
    /// whichever way it returns: still in the bootloader, or running the
    /// application, which then gets detached again. A bootloader that finishes its
    /// work (swapping itself in, copying the net core) boots the application unless
    /// something asks for DFU, so the unit has to be taken back in rather than
    /// waited for.
    pub(super) async fn return_to_dfu(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            tokio::time::sleep(RETURN_TO_DFU_POLL_INTERVAL).await;
            match self.enter_dfu().await {
                Ok(()) => return Ok(()),
                Err(e) if Instant::now() < deadline => {
                    tracing::debug!("Device not back yet: {e}");
                }
                Err(e) => return Err(e),
            }
        }
    }
}
