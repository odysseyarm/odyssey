//! Firmware manifest fetching and DFU update support.

use std::collections::HashMap;
use std::time::Duration;

use dfu_core::DfuIo;
use dfu_nusb::DfuNusb;
use futures::io::Cursor;
use nusb::MaybeFuture;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Manifest URL for alpha channel firmware releases
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/odysseyarm/odyssey/master/firmware/manifests/releases.alpha.json";

/// GitHub releases download base URL
const RELEASES_BASE_URL: &str = "https://github.com/odysseyarm/odyssey/releases/download";

/// Known device types mapped by VID:PID
pub const DEVICE_MAP: &[(u16, u16, &str)] = &[
    (0x1915, 0x5210, "legacy-atslite1"),
    (0x1915, 0x5211, "atslite1"),
    (0x1915, 0x5212, "dongle"),
];

/// The product id a device enumerates under while its bootloader is running.
///
/// The bootloaders add 0x10 to the application's id, so the two modes are
/// distinguishable from the device descriptor alone and bcdDevice can mean the
/// bootloader's own version while it is in charge. An offset rather than a bit:
/// 0x5210 and 0x5211 already have bit 4 set, so setting it would not be
/// reversible.
pub const fn bootloader_pid(app_pid: u16) -> u16 {
    app_pid + 0x10
}

/// The application's product id for a device currently in its bootloader.
pub const fn app_pid(pid: u16) -> u16 {
    pid - 0x10
}

/// Decode the version a bootloader publishes in bcdDevice.
///
/// Four bits of major, six of minor, six of patch, matching what the bootloader
/// encodes. Bootloaders that predate this leave the field at zero, which reads
/// back as `None` — the host must treat that as older than anything published,
/// since it is the one thing those builds cannot report.
pub fn bootloader_version(bcd_device: u16) -> Option<[u16; 3]> {
    if bcd_device == 0 {
        return None;
    }
    Some([
        (bcd_device >> 12) & 0xf,
        (bcd_device >> 6) & 0x3f,
        bcd_device & 0x3f,
    ])
}

/// Top-level firmware manifest
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareManifest {
    pub version: u32,
    pub devices: HashMap<String, DeviceManifest>,
}

/// Per-device firmware information
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceManifest {
    pub firmwares: Vec<FirmwareEntry>,
}

/// A single firmware version entry
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareEntry {
    pub version: String,
    pub files: HashMap<String, FileEntry>,
    pub released: String,
}

/// A single file within a firmware entry.
///
/// `version` is optional and only meaningful for `stage2-bootloader`: the
/// bootloader is versioned independently of the application, and the same image
/// is republished unchanged across application releases.
#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub version: Option<String>,
}

impl FileEntry {
    /// Parse `version` into `[major, minor, patch]`.
    pub fn version_parts(&self) -> Option<[u16; 3]> {
        parse_version(self.version.as_deref()?)
    }
}

impl FirmwareEntry {
    /// Parse version string into [major, minor, patch]
    pub fn version_parts(&self) -> Option<[u16; 3]> {
        parse_version(&self.version)
    }
}

/// Check if manifest version is newer than device version
pub fn version_newer(device: [u16; 3], manifest: [u16; 3]) -> bool {
    if manifest[0] != device[0] {
        return manifest[0] > device[0];
    }
    if manifest[1] != device[1] {
        return manifest[1] > device[1];
    }
    manifest[2] > device[2]
}

/// Get device type name from VID/PID
pub fn device_type_from_vid_pid(vid: u16, pid: u16) -> Option<&'static str> {
    DEVICE_MAP
        .iter()
        .find(|(v, p, _)| *v == vid && *p == pid)
        .map(|(_, _, name)| *name)
}

/// Get device type name and VID/PID from product ID alone
pub fn device_info_from_pid(pid: u16) -> Option<(u16, u16, &'static str)> {
    DEVICE_MAP
        .iter()
        .find(|(_, p, _)| *p == pid)
        .map(|&(v, p, name)| (v, p, name))
}

/// Fetch the firmware manifest from GitHub
pub async fn fetch_manifest() -> anyhow::Result<FirmwareManifest> {
    let client = reqwest::Client::new();
    let manifest: FirmwareManifest = client.get(MANIFEST_URL).send().await?.json().await?;
    Ok(manifest)
}

/// Find the latest firmware for a device type.
/// Cross-major and cross-minor updates are allowed: the manifest publishes what
/// a device should be running, and refusing to cross a minor would strand
/// devices on a line that is no longer built.
/// Entries are published newest-first.
pub fn find_latest_firmware<'a>(
    manifest: &'a FirmwareManifest,
    device_type: &str,
) -> Option<&'a FirmwareEntry> {
    // Firmwares are sorted newest-first in manifest
    manifest.devices.get(device_type)?.firmwares.first()
}

/// Build the download URL for a firmware file
pub fn build_download_url(device_type: &str, version: &str, filename: &str) -> String {
    format!(
        "{}/{}-v{}/{}.bin",
        RELEASES_BASE_URL, device_type, version, filename
    )
}

/// Download a firmware file and verify its SHA256 hash
pub async fn download_firmware(
    device_type: &str,
    version: &str,
    filename: &str,
    expected_sha256: &str,
) -> anyhow::Result<Vec<u8>> {
    let url = build_download_url(device_type, version, filename);
    tracing::info!("Downloading firmware from {}", url);

    tracing::debug!("Creating reqwest client...");
    let client = reqwest::Client::new();
    tracing::debug!("Sending GET request...");
    let response = client.get(&url).send().await?;
    tracing::debug!("Got response, status: {}", response.status());

    if !response.status().is_success() {
        anyhow::bail!("Failed to download firmware: HTTP {}", response.status());
    }

    tracing::debug!("Reading response bytes...");
    let data = response.bytes().await?.to_vec();
    tracing::debug!("Got {} bytes", data.len());

    // Verify SHA256
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = hasher.finalize();
    let hash_hex = hex::encode(hash);

    if hash_hex != expected_sha256 {
        anyhow::bail!(
            "SHA256 mismatch: expected {}, got {}",
            expected_sha256,
            hash_hex
        );
    }

    tracing::info!("Downloaded {} bytes, SHA256 verified", data.len());
    Ok(data)
}

/// Find a USB device by its UUID (MAC address) and return its VID/PID.
/// Compares the UUID against USB serial number descriptors, which encode the
/// BLE MAC address as a hex string (e.g. "AABBCCDDEEFF").
pub fn find_device_vid_pid_by_uuid(uuid: &[u8; 6]) -> Option<(u16, u16)> {
    let uuid_hex = format!(
        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
    );
    let devices = nusb::list_devices().wait().ok()?;
    for dev in devices {
        for &(vid, pid, _device_type) in DEVICE_MAP {
            if dev.vendor_id() == vid && dev.product_id() == pid {
                if let Some(serial) = dev.serial_number() {
                    // Compare case-insensitively, ignoring colons/dashes
                    let serial_clean: String =
                        serial.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                    if serial_clean.eq_ignore_ascii_case(&uuid_hex) {
                        return Some((vid, pid));
                    }
                }
            }
        }
    }
    None
}

/// Get device type and VID/PID for any connected device matching known types
pub fn find_any_known_device() -> Option<(u16, u16, &'static str)> {
    for &(vid, pid, device_type) in DEVICE_MAP {
        if find_device(vid, pid).is_ok() {
            return Some((vid, pid, device_type));
        }
    }
    None
}

/// Check if a specific device (by UUID) is present and in application mode (not DFU mode).
/// Returns true if device is found and is in app mode, false otherwise.
pub fn is_device_in_app_mode(uuid: &[u8; 6], vid: u16, pid: u16) -> bool {
    let info = match find_device_by_uuid(uuid, vid, pid) {
        Ok(info) => info,
        Err(_) => return false,
    };
    let device = match info.open().wait() {
        Ok(device) => device,
        Err(_) => return false,
    };

    // Check if device has a DFU interface in DFU mode
    if let Ok(interface_num) = find_dfu_interface(&device) {
        for config in device.configurations() {
            for interface in config.interfaces() {
                if interface.interface_number() != interface_num {
                    continue;
                }
                for alt in interface.alt_settings() {
                    if alt.class() == DFU_CLASS
                        && alt.subclass() == DFU_SUBCLASS
                        && alt.protocol() == DFU_PROTOCOL_DFU_MODE
                    {
                        // Device is in DFU mode, not application mode
                        return false;
                    }
                }
            }
        }
    }

    // Device is present and not in DFU mode = app mode
    true
}

/// DFU-related errors
#[derive(Debug, Error)]
pub enum DfuError {
    #[error("Device not found")]
    DeviceNotFound,
    #[error("No suitable alt interface found for slot: {0}")]
    AltInterfaceNotFound(String),
    #[error("DFU interface not found on device")]
    DfuInterfaceNotFound,
    #[error("DFU error: {0}")]
    Dfu(#[from] dfu_nusb::Error),
    #[error("USB error: {0}")]
    Usb(#[from] nusb::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// DFU interface class/subclass, and the protocol byte that distinguishes the
/// bootloader from the runtime interface an application also exposes.
pub const DFU_CLASS: u8 = 0xFE;
pub const DFU_SUBCLASS: u8 = 0x01;
pub const DFU_PROTOCOL_DFU_MODE: u8 = 0x02;

/// Find a USB device by VID/PID
pub fn find_device(vid: u16, pid: u16) -> Result<nusb::DeviceInfo, DfuError> {
    nusb::list_devices()
        .wait()
        .map_err(|e| DfuError::Usb(e))?
        .find(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .ok_or(DfuError::DeviceNotFound)
}

/// Find a USB device by VID/PID and UUID, matched against the USB serial number.
///
/// Both the application and the bootloader publish the UUID as the serial, so
/// this is the only safe way to pick one of several identical devices: flashing
/// resolves the device several times over, and the wrong match writes an image
/// into a device the user did not choose.
pub fn find_device_by_uuid(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<nusb::DeviceInfo, DfuError> {
    let uuid_hex = format!(
        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
    );
    let candidates: Vec<_> = nusb::list_devices()
        .wait()
        .map_err(|e| DfuError::Usb(e))?
        .filter(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .collect();

    if let Some(dev) = candidates.iter().find(|dev| {
        dev.serial_number()
            .map(|s| {
                let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                clean.eq_ignore_ascii_case(&uuid_hex)
            })
            .unwrap_or(false)
    }) {
        return Ok(dev.clone());
    }

    crate::migration::single_unidentifiable_device(candidates).ok_or(DfuError::DeviceNotFound)
}

/// Find the DFU interface number on a device
pub fn find_dfu_interface(device: &nusb::Device) -> Result<u8, DfuError> {
    for config in device.configurations() {
        for interface in config.interfaces() {
            for alt in interface.alt_settings() {
                if alt.class() == DFU_CLASS && alt.subclass() == DFU_SUBCLASS {
                    return Ok(interface.interface_number());
                }
            }
        }
    }
    Err(DfuError::DfuInterfaceNotFound)
}

/// Find the alt setting for a given slot name (e.g., "APP_A" or "APP_B")
/// Returns the alt setting number if found
pub fn find_alt_for_slot(
    device: &nusb::Device,
    interface_num: u8,
    slot: &str,
) -> Result<u8, DfuError> {
    for config in device.configurations() {
        for interface in config.interfaces() {
            if interface.interface_number() != interface_num {
                continue;
            }
            for alt in interface.alt_settings() {
                if alt.class() == DFU_CLASS && alt.subclass() == DFU_SUBCLASS {
                    // Get the string descriptor for this alt setting
                    if let Some(string_index) = alt.string_index() {
                        let lang = device
                            .get_string_descriptor_supported_languages(Duration::from_secs(3))
                            .wait()
                            .ok()
                            .and_then(|mut langs| langs.next())
                            .unwrap_or_default();
                        if let Ok(desc) = device
                            .get_string_descriptor(string_index, lang, Duration::from_secs(3))
                            .wait()
                        {
                            // Check if this alt interface matches the requested slot
                            // The string descriptor should contain the slot name (e.g., "APP_A" or "APP_B")
                            if desc.contains(slot) {
                                return Ok(alt.alternate_setting());
                            }
                        }
                    }
                }
            }
        }
    }
    Err(DfuError::AltInterfaceNotFound(slot.to_string()))
}

/// Determine which slot to flash by reading the DFU interface alt settings.
/// The bootloader exposes the inactive bank, so we read the string descriptors
/// to find which bank (APP_A or APP_B) is available for flashing.
/// Returns (slot_name for DFU "APP_A"/"APP_B", firmware_file "app-a"/"app-b", alt_setting)
pub fn determine_target_slot(
    device: &nusb::Device,
    interface_num: u8,
) -> Result<(&'static str, &'static str, u8), DfuError> {
    // Check which slots are available by reading alt interface string descriptors
    let app_a_result = find_alt_for_slot(device, interface_num, "APP_A");
    let app_b_result = find_alt_for_slot(device, interface_num, "APP_B");

    match (app_a_result, app_b_result) {
        (Ok(alt), _) => {
            // APP_A is exposed (meaning bank B is active, we flash to A)
            tracing::info!(
                "DFU target: APP_A (alt {}), downloading app-a firmware",
                alt
            );
            Ok(("APP_A", "app-a", alt))
        }
        (_, Ok(alt)) => {
            // APP_B is exposed (meaning bank A is active, we flash to B)
            tracing::info!(
                "DFU target: APP_B (alt {}), downloading app-b firmware",
                alt
            );
            Ok(("APP_B", "app-b", alt))
        }
        _ => {
            // No recognized slot names
            Err(DfuError::AltInterfaceNotFound("APP_A or APP_B".to_string()))
        }
    }
}

/// Determine which slot to flash for a specific device.
/// Opens the device, reads the DFU interface, and determines the target slot.
/// Returns (slot_name "APP_A"/"APP_B", firmware_file "app-a"/"app-b", alt_setting)
pub fn determine_target_slot_for_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<(&'static str, &'static str, u8), DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface_num = find_dfu_interface(&device)?;
    determine_target_slot(&device, interface_num)
}

/// Parse a `major.minor.patch` string from the manifest.
pub fn parse_version(version: &str) -> Option<[u16; 3]> {
    let mut parts = version.split('.');
    let parsed = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    if parts.next().is_some() {
        return None;
    }
    Some(parsed)
}

/// Read the version of the bootloader a device is currently running.
///
/// The bootloader publishes it in bcdDevice, so this costs no transfers and
/// needs neither an open device nor a claimed interface.
pub fn read_bootloader_version_for_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<Option<[u16; 3]>, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    Ok(bootloader_version(info.device_version()))
}

/// Find a device that is currently running its bootloader.
///
/// Callers pass the application's product id throughout; the bootloader
/// enumerates under its own, so the translation lives here rather than at every
/// call site.
pub fn find_bootloader_by_uuid(
    uuid: &[u8; 6],
    vid: u16,
    app_pid: u16,
) -> Result<nusb::DeviceInfo, DfuError> {
    match find_device_by_uuid(uuid, vid, bootloader_pid(app_pid)) {
        Ok(info) => Ok(info),
        // Bootloaders flashed before the split enumerate under the
        // application's id instead.
        Err(_) => crate::migration::find_bootloader_sharing_app_pid(uuid, vid, app_pid),
    }
}

/// Find the alt setting that writes the stage2 bootloader itself.
pub fn find_bootloader_alt_for_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<u8, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface_num = find_dfu_interface(&device)?;
    find_alt_for_slot(&device, interface_num, "BOOTLOADER")
}

/// Whether the published bootloader is newer than the one running.
///
/// A bootloader that reports no version predates version reporting, so any
/// published version supersedes it.
pub fn bootloader_update_needed(running: Option<[u16; 3]>, published: [u16; 3]) -> bool {
    match running {
        Some(running) => version_newer(running, published),
        None => true,
    }
}

/// Open a DFU device and prepare it for flashing
pub fn open_dfu_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    interface_num: u8,
    alt: u8,
) -> Result<DfuNusb, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface = device.claim_interface(interface_num).wait()?;
    let dfu = DfuNusb::open(device, interface, alt)?;
    Ok(dfu)
}

/// Send DFU detach command to a device in runtime mode.
/// After detach, the device will reset and re-enumerate in DFU mode.
///
/// Note: For devices with WILL_DETACH capability (like ATS Lite), the device
/// resets immediately upon receiving the DETACH request, before the USB control
/// transfer completes. This causes an endpoint stall from the host's perspective,
/// which is expected behavior per the DFU 1.1 spec.
pub async fn send_dfu_detach(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<(), DfuError> {
    let info = find_device_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;

    // Find the DFU runtime interface
    let interface_num = find_dfu_interface(&device)?;
    let interface = device.claim_interface(interface_num).wait()?;

    // Open with alt 0 (doesn't matter for detach)
    let dfu = DfuNusb::open(device, interface, 0)?;
    let dfu_async = dfu.into_async_dfu();

    // Send detach - for WILL_DETACH devices, the device resets immediately
    // and the control transfer will fail with a stall. This is expected behavior.
    match dfu_async.detach().await {
        Ok(()) => {
            // Device didn't reset immediately, send USB reset to trigger DFU mode
            let _ = dfu_async.usb_reset().await;
        }
        Err(e) => {
            // Check if this is an endpoint stall - expected for WILL_DETACH devices
            // The device has already reset, so we don't need to do anything else
            let is_stall = matches!(
                &e,
                dfu_nusb::Error::Transfer(nusb::transfer::TransferError::Stall)
            );
            if !is_stall {
                return Err(e.into());
            }
            tracing::debug!(
                "DFU detach received stall (device reset immediately, as expected for WILL_DETACH)"
            );
        }
    }

    Ok(())
}

/// Flash firmware to a device already in DFU mode with progress logging.
///
/// # Arguments
/// * `vid` - Vendor ID
/// * `pid` - Product ID
/// * `slot` - Target slot ("APP_A" or "APP_B")
/// * `alt` - Alt setting number for the target slot
/// * `firmware` - Firmware binary data
///
/// # Returns
/// Ok(()) on success
pub async fn flash_firmware(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    slot: &str,
    alt: u8,
    firmware: &[u8],
) -> Result<(), DfuError> {
    // Use the progress version with a no-op callback
    flash_firmware_with_progress(uuid, vid, pid, slot, alt, firmware, |_, _| {}).await
}

/// A wrapper reader that logs progress and optionally calls a callback
struct ProgressReader<R, F> {
    inner: R,
    total_size: usize,
    bytes_read: usize,
    last_logged_percent: usize,
    callback: Option<F>,
}

impl<R, F> ProgressReader<R, F> {
    fn new(inner: R, total_size: usize, callback: Option<F>) -> Self {
        Self {
            inner,
            total_size,
            bytes_read: 0,
            last_logged_percent: 0,
            callback,
        }
    }
}

impl<R: futures::io::AsyncRead + Unpin, F: FnMut(usize, usize) + Unpin> futures::io::AsyncRead
    for ProgressReader<R, F>
{
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let inner = std::pin::Pin::new(&mut self.inner);
        match inner.poll_read(cx, buf) {
            std::task::Poll::Ready(Ok(n)) => {
                self.bytes_read += n;
                let percent = if self.total_size > 0 {
                    (self.bytes_read * 100) / self.total_size
                } else {
                    100
                };

                // Copy values for use in logging and callback
                let bytes_read = self.bytes_read;
                let total_size = self.total_size;

                // Log every 10%
                if percent >= self.last_logged_percent + 10 || percent == 100 {
                    tracing::info!(
                        "DFU progress: {}/{} bytes ({}%)",
                        bytes_read,
                        total_size,
                        percent
                    );
                    self.last_logged_percent = (percent / 10) * 10;
                }

                // Call progress callback if provided
                if let Some(ref mut cb) = self.callback {
                    cb(bytes_read, total_size);
                }

                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }
}

/// Flash firmware with progress callback.
///
/// # Arguments
/// * `vid` - Vendor ID
/// * `pid` - Product ID
/// * `slot` - Target slot ("APP_A" or "APP_B")
/// * `alt` - Alt setting number for the target slot
/// * `firmware` - Firmware binary data
/// * `progress_callback` - Called with (bytes_written, total_bytes) during flash
///
/// # Returns
/// Ok(()) on success
pub async fn flash_firmware_with_progress<F>(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    slot: &str,
    alt: u8,
    firmware: &[u8],
    progress_callback: F,
) -> Result<(), DfuError>
where
    F: FnMut(usize, usize) + Unpin,
{
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;

    let interface_num = find_dfu_interface(&device)?;

    let interface = device.claim_interface(interface_num).wait()?;
    let dfu = DfuNusb::open(device, interface, alt)?;
    let transfer_size = dfu.functional_descriptor().transfer_size as usize;
    let mut dfu_async = dfu.into_async_dfu();

    let total_size = firmware.len();

    tracing::info!(
        "Flashing {} bytes to slot {} (alt {}), transfer_size={}",
        total_size,
        slot,
        alt,
        transfer_size
    );

    // Download the firmware with progress callback
    let cursor = ProgressReader::new(Cursor::new(firmware), total_size, Some(progress_callback));

    // The download may return a "cancelled" error at the end if the device
    // resets immediately after receiving all data (before the final status check).
    // This is expected behavior for devices with WILL_DETACH or manifestation tolerant.
    match dfu_async.download(cursor, total_size as u32).await {
        Ok(()) => {}
        Err(dfu_nusb::Error::Transfer(nusb::transfer::TransferError::Cancelled)) => {
            tracing::info!("Download completed (device reset during final handshake, as expected)");
        }
        Err(e) => return Err(e.into()),
    }

    tracing::info!("Firmware flash complete");

    // Reset to boot into new firmware
    // The device may reset automatically after the download completes,
    // so errors here are expected and can be ignored.
    if let Err(e) = dfu_async.detach().await {
        tracing::debug!(
            "Post-flash detach error (expected if device auto-reset): {}",
            e
        );
    }
    if let Err(e) = dfu_async.usb_reset().await {
        tracing::debug!(
            "Post-flash USB reset error (expected if device auto-reset): {}",
            e
        );
    }

    Ok(())
}

/// Wait for a device to appear in DFU mode after detach.
///
/// # Arguments
/// * `vid` - Vendor ID
/// * `pid` - Product ID
/// * `timeout` - Maximum time to wait
///
/// # Returns
/// Ok(()) when device is found, Err on timeout
pub async fn wait_for_dfu_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    timeout: Duration,
) -> Result<(), DfuError> {
    let start = std::time::Instant::now();
    tracing::debug!(
        "wait_for_dfu_device: starting poll for {:04x}:{:04x}",
        vid,
        pid
    );

    loop {
        if start.elapsed() > timeout {
            tracing::debug!("wait_for_dfu_device: timeout after {:?}", start.elapsed());
            return Err(DfuError::DeviceNotFound);
        }

        if let Ok(info) = find_bootloader_by_uuid(uuid, vid, pid) {
            tracing::debug!("wait_for_dfu_device: found device, checking DFU mode");
            // Check if device is in DFU mode
            if let Ok(device) = info.open().wait() {
                if let Ok(interface_num) = find_dfu_interface(&device) {
                    // Check if it's in DFU mode (protocol 0x02) not runtime (0x01)
                    for config in device.configurations() {
                        for interface in config.interfaces() {
                            if interface.interface_number() != interface_num {
                                continue;
                            }
                            for alt in interface.alt_settings() {
                                tracing::debug!(
                                    "wait_for_dfu_device: alt {} class={:02x} subclass={:02x} protocol={:02x}",
                                    alt.alternate_setting(),
                                    alt.class(),
                                    alt.subclass(),
                                    alt.protocol()
                                );
                                if alt.class() == DFU_CLASS
                                    && alt.subclass() == DFU_SUBCLASS
                                    && alt.protocol() == DFU_PROTOCOL_DFU_MODE
                                {
                                    tracing::debug!("wait_for_dfu_device: device is in DFU mode!");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// High-level firmware update function.
///
/// This performs the full update sequence:
/// 1. Send DFU detach to device in runtime mode
/// 2. Wait for device to re-enumerate in DFU mode
/// 3. Flash the firmware to the appropriate slot
/// 4. Device resets and boots new firmware
///
/// # Arguments
/// * `vid` - Vendor ID
/// * `pid` - Product ID
/// * `firmware` - Firmware binary data
///
/// Note: This function automatically determines which slot to flash by reading
/// the DFU interface alt settings after the device enters DFU mode.
pub async fn update_firmware(uuid: &[u8; 6], vid: u16, pid: u16, firmware: &[u8]) -> Result<(), DfuError> {
    tracing::info!("Starting firmware update for {:04x}:{:04x}", vid, pid);

    // Step 1: Send DFU detach
    tracing::info!("Sending DFU detach...");
    send_dfu_detach(uuid, vid, pid).await?;

    // Step 2: Wait for device to re-enumerate in DFU mode
    tracing::info!("Waiting for device to enter DFU mode...");
    wait_for_dfu_device(uuid, vid, pid, Duration::from_secs(10)).await?;

    // The bootloader only identifies itself while it is the thing enumerating,
    // so this is the one point in the update where its version is readable.
    match read_bootloader_version_for_device(uuid, vid, pid) {
        Ok(Some([major, minor, patch])) => {
            tracing::info!("Bootloader version {}.{}.{}", major, minor, patch)
        }
        Ok(None) => tracing::info!("Bootloader reports no version (predates version reporting)"),
        Err(e) => tracing::warn!("Could not read the bootloader version: {e}"),
    }

    // Step 3: Determine target slot by reading alt interface strings
    let (slot_name, _slot_file, alt) = determine_target_slot_for_device(uuid, vid, pid)?;
    tracing::info!("Target slot: {} (alt {})", slot_name, alt);

    // Step 4: Flash firmware
    tracing::info!("Flashing firmware...");
    flash_firmware(uuid, vid, pid, slot_name, alt, firmware).await?;

    tracing::info!("Firmware update complete!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer() {
        assert!(version_newer([0, 1, 0], [0, 1, 1]));
        assert!(version_newer([0, 1, 0], [0, 2, 0]));
        assert!(version_newer([0, 1, 0], [1, 0, 0]));
        assert!(!version_newer([0, 1, 1], [0, 1, 0]));
        assert!(!version_newer([0, 1, 0], [0, 1, 0]));
    }

    #[test]
    fn parses_only_well_formed_versions() {
        assert_eq!(parse_version("0.2.0"), Some([0, 2, 0]));
        assert_eq!(parse_version("1.20.300"), Some([1, 20, 300]));
        assert_eq!(parse_version("0.2"), None);
        assert_eq!(parse_version("0.2.0.1"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("0.2.x"), None);
    }

    #[test]
    fn file_version_is_optional() {
        let without: FileEntry =
            serde_json::from_str(r#"{"sha256":"ab","size":1}"#).expect("parses");
        assert_eq!(without.version_parts(), None);

        let with: FileEntry =
            serde_json::from_str(r#"{"sha256":"ab","size":1,"version":"0.2.0"}"#).expect("parses");
        assert_eq!(with.version_parts(), Some([0, 2, 0]));
    }

    #[test]
    fn test_build_download_url() {
        assert_eq!(
            build_download_url("atslite", "0.1.1", "app-a"),
            "https://github.com/odysseyarm/odyssey/releases/download/atslite-v0.1.1/app-a.bin"
        );
    }
}
