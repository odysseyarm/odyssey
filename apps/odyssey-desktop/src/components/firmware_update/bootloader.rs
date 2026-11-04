use dioxus::logger::tracing;
use odyssey_hub_server::firmware;

use super::dfu_steps::{enter_dfu, flash_with_progress, return_to_dfu, wait_for_bootloader_reset};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

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
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    bootloader_image: Option<&(Vec<u8>, u16)>,
    requires_bootloader: u16,
) -> Result<(), String> {
    enter_dfu(manager, device_uuid, vid, pid).await?;

    let running = firmware::read_bootloader_release_for_device(&device_uuid, vid, pid)
        .map_err(|e| format!("Could not read the bootloader release: {e}"))?;
    let flash_bootloader = firmware::plan_bootloader(
        running,
        bootloader_image.map(|(_, release)| *release),
        requires_bootloader,
    )
    .map_err(|e| e.to_string())?;
    match bootloader_image {
        Some((bootloader_data, published)) if flash_bootloader => {
            let published = *published;
            let alt = firmware::find_bootloader_alt_for_device(&device_uuid, vid, pid)
                .map_err(|e| format!("Bootloader cannot be updated over DFU: {e}"))?;
            tracing::info!(
                "Updating bootloader {:?} -> release {} on alt {}",
                running,
                published,
                alt
            );
            flash_with_progress(
                manager,
                device_uuid,
                vid,
                pid,
                "BOOTLOADER",
                alt,
                bootloader_data,
                "Bootloader flash",
                |progress, total| FirmwareUpdateState::Flashing { progress, total },
            )
            .await?;

            // The write resets the device and the new bootloader boots the
            // application, so the unit comes back running it, not in DFU. Wait
            // out the old bootloader's enumeration first: it would otherwise
            // satisfy the wait below and be read as if it were the new one.
            manager.set_state(device_uuid, FirmwareUpdateState::Rebooting);
            if !wait_for_bootloader_reset(
                &device_uuid,
                vid,
                pid,
                std::time::Duration::from_secs(15),
            )
            .await
            {
                return Err("Device did not reset after the bootloader update".to_string());
            }
            return_to_dfu(
                manager,
                device_uuid,
                vid,
                pid,
                std::time::Duration::from_secs(30),
            )
            .await
            .map_err(|e| format!("Device did not return after the bootloader update: {e}"))?;

            // A completed write and a returning device do not show the new
            // image is the one running. Nothing may go onto a bootloader that
            // never took the update, so read it back.
            let now = firmware::read_bootloader_release_for_device(&device_uuid, vid, pid)
                .map_err(|e| format!("Could not read the bootloader after updating it: {e}"))?;
            if !firmware::bootloader_took_update(now, published) {
                return Err(format!(
                    "Bootloader reports {now:?} after updating to release {published}"
                ));
            }
            tracing::info!("Bootloader now reports {:?}", now);
        }
        _ => tracing::info!("Bootloader {:?} stays as it is", running),
    }
    Ok(())
}
