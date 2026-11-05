use std::time::Duration;

use eros::Context as _;
use odyssey_hub_server::firmware;

use super::dfu_session::DfuSession;
use super::downloads::ReleaseImage;
use super::update_state::FirmwareUpdateState;

/// How long the old bootloader gets to stop enumerating after it was written.
const BOOTLOADER_RESET_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the unit gets to come back, and be taken back into DFU, after the
/// bootloader update.
const BOOTLOADER_RETURN_TIMEOUT: Duration = Duration::from_secs(30);

impl DfuSession<'_> {
    /// Bring the bootloader to the release the update carries, when it is behind or
    /// the application needs a newer one, and leave the device in DFU mode.
    ///
    /// This runs before anything else is written. The bootloader performs the
    /// net-core copy and owns the application bank writes, so a fix to either only
    /// helps an update that comes after it. It is also the only window where the
    /// bootloader is the thing enumerating, which is both why its release is
    /// readable and why it can be written at all. Calling it again once the
    /// bootloader is current changes nothing.
    pub(super) async fn ensure_bootloader(
        &self,
        bootloader_image: Option<&ReleaseImage>,
        requires_bootloader: u16,
    ) -> eros::Result<()> {
        self.enter_dfu().await?;

        let running = firmware::read_bootloader_release_for_device(&self.uuid, self.vid, self.pid)
            .context("Could not read the bootloader release")?;
        let flash_bootloader = firmware::plan_bootloader(
            running,
            bootloader_image.map(|image| image.release),
            requires_bootloader,
        )?;
        let Some(image) = bootloader_image.filter(|_| flash_bootloader) else {
            tracing::info!("Bootloader {:?} stays as it is", running);
            return Ok(());
        };

        let alt = firmware::find_bootloader_alt_for_device(&self.uuid, self.vid, self.pid)
            .context("Bootloader cannot be updated over DFU")?;
        tracing::info!(
            "Updating bootloader {:?} -> release {} on alt {}",
            running,
            image.release,
            alt
        );
        self.flash_with_progress(
            "BOOTLOADER",
            alt,
            &image.data,
            "Bootloader flash",
            |progress, total| FirmwareUpdateState::Flashing { progress, total },
        )
        .await?;

        // The write resets the device and the new bootloader boots the
        // application, so the unit comes back running it, not in DFU. Wait
        // out the old bootloader's enumeration first: it would otherwise
        // satisfy the wait below and be read as if it were the new one.
        self.set_state(FirmwareUpdateState::Rebooting);
        if !self
            .wait_for_bootloader_reset(BOOTLOADER_RESET_TIMEOUT)
            .await
        {
            eros::bail!("Device did not reset after the bootloader update");
        }
        self.return_to_dfu(BOOTLOADER_RETURN_TIMEOUT)
            .await
            .context("Device did not return after the bootloader update")?;

        // A completed write and a returning device do not show the new
        // image is the one running. Nothing may go onto a bootloader that
        // never took the update, so read it back.
        let now = firmware::read_bootloader_release_for_device(&self.uuid, self.vid, self.pid)
            .context("Could not read the bootloader after updating it")?;
        if !firmware::bootloader_took_update(now, image.release) {
            eros::bail!(
                "Bootloader reports {now:?} after updating to release {}",
                image.release
            );
        }
        tracing::info!("Bootloader now reports {:?}", now);
        Ok(())
    }
}
