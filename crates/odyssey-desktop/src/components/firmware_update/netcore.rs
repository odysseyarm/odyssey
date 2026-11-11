use std::future;
use std::time::Duration;

use backon::Retryable as _;
use eros::Context as _;
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_server::firmware;

use super::dfu_session::DfuSession;
use super::poll_window::{NotYet, poll_window};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// How long the net core gets to report the release that was just written.
///
/// The application only learns the release once its BLE controller has
/// answered, and the server's own probe retries for about half a minute after
/// the device reconnects, so this has to outlast both.
const NETCORE_REPORT_TIMEOUT: Duration = Duration::from_secs(60);

const NETCORE_REPORT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long the net core bootloader and the stage2 bootloader get to finish a
/// net core update and bring the device back into DFU mode. Stage2 itself gives
/// the copy up to two minutes before it resets to retry, so this has to outlast
/// that.
const NETCORE_INSTALL_TIMEOUT: Duration = Duration::from_secs(150);

/// How long the device may keep enumerating as a bootloader after the net core
/// download completes before it counts as never having reset to start the copy.
const NETCORE_RESET_TIMEOUT: Duration = Duration::from_secs(20);

fn netcore_not_reported(published: u16) -> eros::ErrorUnion {
    eros::error!(
        "the net core did not report release {published}; the device may have no radio, \
         run the update again"
    )
}

/// The net core release the hub last read from a device that is in application
/// mode, `None` while the device is absent or has no control connection.
fn reported_netcore_release(devices: &[Device], uuid: [u8; 6]) -> Option<u16> {
    devices
        .iter()
        .find(|d| d.uuid == uuid && d.capabilities.contains(DeviceCapabilities::CONTROL))
        .map(|d| d.netcore_release)
}

/// Poll the hub's device list until the net core reports exactly `published`.
///
/// The hub rereads the release each time the device reconnects, so a reading
/// from before the reset can only be a mismatch, never a false match: the old
/// release differs from the one just written, and a device with no answer reads 0.
pub(super) async fn wait_for_netcore_release(
    manager: &FirmwareUpdateManager,
    uuid: [u8; 6],
    published: u16,
) -> eros::Result<()> {
    let mut last = None;
    let backoff = poll_window(NETCORE_REPORT_TIMEOUT, NETCORE_REPORT_POLL_INTERVAL);
    tokio::time::sleep(NETCORE_REPORT_POLL_INTERVAL).await;
    let reported = (|| {
        let reading = reported_netcore_release(&manager.devices(), uuid);
        if let Some(now) = reading {
            last = Some(now);
        }
        future::ready(match reading {
            Some(now) if firmware::netcore_took_update(now, published) => Ok(now),
            _ => Err(NotYet),
        })
    })
    .retry(backoff)
    .await;
    if let Ok(now) = reported {
        tracing::info!("Net core now reports release {}", now);
        return Ok(());
    }
    tracing::warn!(
        "Net core did not report release {} (last reading: {:?})",
        published,
        last
    );
    Err(netcore_not_reported(published))
}

impl DfuSession<'_> {
    /// Write the net core image and wait for the device to be back in DFU mode.
    ///
    /// Completing the download only means the bootloader has the image staged. The
    /// device resets, and the stage2 bootloader starts the net core bootloader,
    /// which copies the image into net flash. Stage2 treats every outcome of that
    /// copy alike. The application's DFU request is still pending afterwards, so the
    /// device returns to DFU mode and stays there until an application is written.
    /// The caller continues in that same DFU session without detaching again. Nothing
    /// reports the net core while the device is in the bootloader, so whether the
    /// copy took is only readable once the new application runs.
    pub(super) async fn flash_netcore(&self, image: &[u8], release: u16) -> eros::Result<()> {
        self.enter_dfu().await?;

        let alt = firmware::find_netcore_alt_for_device(&self.uuid, self.vid, self.pid)
            .context("The net core cannot be updated over DFU")?;
        tracing::info!("Updating net core to release {} on alt {}", release, alt);
        self.flash_with_progress("NET", alt, image, "Net core flash", |progress, total| {
            FirmwareUpdateState::FlashingNetCore { progress, total }
        })
        .await?;

        self.set_state(FirmwareUpdateState::InstallingNetCore);
        tracing::info!(
            "Waiting for the device to copy the net core image and return to DFU mode..."
        );
        // The bootloader may still be enumerating for a moment after the download,
        // and that stale enumeration would satisfy the wait below at once, sending
        // the application image into a device that is about to reset.
        if !self.wait_for_bootloader_reset(NETCORE_RESET_TIMEOUT).await {
            eros::bail!(
                "Device did not reset within {}s after the net core download. Leave it \
                 connected and run the update again.",
                NETCORE_RESET_TIMEOUT.as_secs()
            );
        }
        self.return_to_dfu(NETCORE_INSTALL_TIMEOUT)
            .await
            .with_context(|| {
                format!(
                    "Device did not come back within {}s after the net core update. \
                     Leave it connected and run the update again.",
                    NETCORE_INSTALL_TIMEOUT.as_secs()
                )
            })?;
        tracing::info!("Device is back in DFU mode");
        Ok(())
    }
}
