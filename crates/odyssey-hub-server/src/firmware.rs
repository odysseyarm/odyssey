//! Firmware manifest fetching and DFU update support.

use std::collections::HashMap;
use std::time::Duration;

use dfu_core::DfuIo;
use dfu_nusb::DfuNusb;
use futures::io::Cursor;
use nusb::MaybeFuture;
use odyssey_protocol::BootloaderRelease;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Where firmware manifests are published. Promotion from alpha to stable is a
/// deliberate copy of an entry between the two files.
const MANIFEST_BASE_URL: &str =
    "https://raw.githubusercontent.com/odysseyarm/odyssey/master/firmware/manifests";

/// Which manifest a host follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FirmwareChannel {
    /// Entries someone promoted by hand. What customers follow.
    #[default]
    Stable,
    /// Every release as it is published.
    Alpha,
}

impl FirmwareChannel {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Alpha => "alpha",
        }
    }

    /// An unrecognised name falls back to stable: the safe channel is the one
    /// to land on when a settings file is stale or hand-edited.
    pub fn from_name(name: &str) -> Self {
        match name {
            "alpha" => Self::Alpha,
            _ => Self::Stable,
        }
    }

    /// The channel the user chose, stable if they never did.
    pub async fn load() -> Self {
        odyssey_hub_common::config::firmware_channel_async()
            .await
            .map_or(Self::Stable, |name| Self::from_name(&name))
    }

    pub async fn save(self) -> Result<(), Box<dyn std::error::Error>> {
        odyssey_hub_common::config::firmware_channel_save_async(self.name()).await
    }

    fn manifest_url(self) -> String {
        format!("{MANIFEST_BASE_URL}/releases.{}.json", self.name())
    }
}

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

/// What a device's bcdDevice says about the bootloader it is running.
pub fn bootloader_release(bcd_device: u16) -> BootloaderRelease {
    BootloaderRelease::from_bcd_device(bcd_device)
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

/// A single firmware version entry.
///
/// `requires_bootloader` and `requires_netcore` are the lowest bootloader and
/// net core releases this application can run on, `0` when it has no
/// requirement. `min_desktop` is the oldest Odyssey Desktop that is actually
/// required and tested with this firmware, which is not necessarily the desktop
/// that shipped alongside it: coupling a hotfix to a desktop upgrade it does not
/// need would strand users for no reason.
///
/// All three are required, not optional: an entry that omitted one would read as
/// unrestricted, so `gen-release` always writes them and a manifest missing one
/// is rejected whole. Older hosts ignore fields they do not know, so adding one
/// never stops them parsing.
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareEntry {
    pub version: String,
    pub requires_bootloader: u16,
    pub requires_netcore: u16,
    pub min_desktop: String,
    pub files: HashMap<String, FileEntry>,
    #[serde(default)]
    pub released: Option<String>,
}

/// A single file within a firmware entry.
///
/// `release` is only present on `stage2-bootloader` and `net-core`: both are
/// versioned independently of the application, and the same image is
/// republished unchanged across application releases.
#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub release: Option<u16>,
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

/// The highest manifest version this build understands.
///
/// Additive changes keep this number: unknown fields are ignored, so an older
/// host simply does not act on them. Bump it only for a change an older host
/// must not act on, which makes those hosts stop offering updates rather than
/// act on a manifest they would misread.
pub const SUPPORTED_MANIFEST_VERSION: u32 = 1;

/// Fetch the firmware manifest for a channel from GitHub
pub async fn fetch_manifest(channel: FirmwareChannel) -> anyhow::Result<FirmwareManifest> {
    let client = reqwest::Client::new();
    let manifest: FirmwareManifest = client
        .get(channel.manifest_url())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if manifest.version > SUPPORTED_MANIFEST_VERSION {
        anyhow::bail!(
            "firmware manifest is version {}, newer than the {} this build understands; update the application",
            manifest.version,
            SUPPORTED_MANIFEST_VERSION
        );
    }
    Ok(manifest)
}

/// What a manifest offers one device, given the desktop doing the offering.
#[derive(Debug, Clone, Copy)]
pub struct FirmwareOffer<'a> {
    /// The newest entry this desktop can install.
    pub offered: Option<&'a FirmwareEntry>,
    /// A newer entry that exists but needs a newer desktop. The user is told so
    /// and chooses whether to update; nothing updates the desktop for them.
    pub held_back: Option<&'a FirmwareEntry>,
}

/// Choose firmware for a device type.
///
/// Entries are published newest-first. Cross-major and cross-minor updates are
/// allowed: the manifest says what a device should run, and refusing to cross
/// a minor would strand devices on a line that is no longer built. An entry
/// whose `min_desktop` is above `desktop`, or cannot be read, is not
/// installable here.
pub fn select_firmware<'a>(
    manifest: &'a FirmwareManifest,
    device_type: &str,
    desktop: [u16; 3],
) -> FirmwareOffer<'a> {
    let mut offer = FirmwareOffer {
        offered: None,
        held_back: None,
    };
    let Some(device) = manifest.devices.get(device_type) else {
        return offer;
    };
    for entry in &device.firmwares {
        let installable = match parse_version(&entry.min_desktop) {
            Some(min) => !version_newer(desktop, min),
            None => {
                tracing::warn!(
                    "Firmware {} has an unreadable min_desktop {:?}; not offering it",
                    entry.version,
                    entry.min_desktop
                );
                false
            }
        };
        if installable {
            offer.offered = Some(entry);
            return offer;
        }
        offer.held_back.get_or_insert(entry);
    }
    offer
}

/// Which GitHub release a firmware file is published under.
///
/// The bootloader and the net core are released on their own, because they
/// change far less often than the application and a bad one cannot be fixed
/// without a probe. An application release entry pins the exact component
/// releases it was tested with, so the combination stays recorded in the
/// manifest while each image lives in the release that built it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseSource<'a> {
    /// `<device>-v<version>`
    Application { version: &'a str },
    /// `<device>-bootloader-v<release>`
    Bootloader { release: u16 },
    /// `<device>-netcore-v<release>`
    NetCore { release: u16 },
}

impl ReleaseSource<'_> {
    /// The git tag of the release.
    pub fn tag(&self, device_type: &str) -> String {
        match self {
            Self::Application { version } => format!("{device_type}-v{version}"),
            Self::Bootloader { release } => format!("{device_type}-bootloader-v{release}"),
            Self::NetCore { release } => format!("{device_type}-netcore-v{release}"),
        }
    }
}

/// Build the download URL for a firmware file
pub fn build_download_url(device_type: &str, source: ReleaseSource<'_>, filename: &str) -> String {
    format!(
        "{}/{}/{}.bin",
        RELEASES_BASE_URL,
        source.tag(device_type),
        filename
    )
}

/// Download a firmware file and verify its SHA256 hash
pub async fn download_firmware(
    device_type: &str,
    source: ReleaseSource<'_>,
    filename: &str,
    expected_sha256: &str,
) -> anyhow::Result<Vec<u8>> {
    let url = build_download_url(device_type, source, filename);
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

/// Read the release of the bootloader a device is currently running.
///
/// The bootloader publishes it in bcdDevice, so this costs no transfers and
/// needs neither an open device nor a claimed interface.
pub fn read_bootloader_release_for_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<BootloaderRelease, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    Ok(bootloader_release(info.device_version()))
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

/// Find the alt setting that writes the nRF5340 net core image.
///
/// The stage2 bootloader serves it, so the device must already be in DFU mode.
pub fn find_netcore_alt_for_device(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<u8, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface_num = find_dfu_interface(&device)?;
    find_alt_for_slot(&device, interface_num, NETCORE_ALT_LABEL)
}

/// The application cannot run on any bootloader this device will end up with.
#[derive(Debug, PartialEq, Eq)]
pub struct BootloaderTooOld {
    pub required: u16,
}

impl std::fmt::Display for BootloaderTooOld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this firmware needs bootloader release {} or newer and the release publishes none",
            self.required
        )
    }
}

/// Decide what the bootloader needs before the application is written.
///
/// Returns whether to flash `published`. Two rules, kept apart because they
/// answer different questions: the fleet should converge on the published
/// bootloader, so a superseded one is replaced; and the application has a floor
/// below which it must not be installed, so the device is refused if neither the
/// running nor the published bootloader reaches `requires`.
pub fn plan_bootloader(
    running: BootloaderRelease,
    published: Option<u16>,
    requires: u16,
) -> Result<bool, BootloaderTooOld> {
    let flash = published.is_some_and(|p| running.superseded_by(p));
    let ending_at = match (flash, published, running) {
        (true, Some(published), _) => published,
        (_, _, BootloaderRelease::Counter(running)) => running,
        (_, _, BootloaderRelease::Legacy) => 0,
    };
    if ending_at < requires {
        return Err(BootloaderTooOld { required: requires });
    }
    Ok(flash)
}

/// Whether a bootloader reads back as the image just written.
///
/// Exact, not "at least": the artifact declares the release it carries, so a
/// higher counter is as much a mismatch between image and manifest as a lower
/// one or a bootloader that kept its old image.
pub fn bootloader_took_update(now: BootloaderRelease, published: u16) -> bool {
    now == BootloaderRelease::Counter(published)
}

/// DFU alt setting label under which the stage2 bootloader exposes the net core.
const NETCORE_ALT_LABEL: &str = "NET";

/// The application cannot run on any net core this device will end up with.
#[derive(Debug, PartialEq, Eq)]
pub struct NetCoreTooOld {
    pub required: u16,
}

impl std::fmt::Display for NetCoreTooOld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this firmware needs net core release {} or newer and the release publishes none",
            self.required
        )
    }
}

/// Decide what the net core needs before the application is written.
///
/// `running` is what the device reports, `0` when it reports nothing: firmware
/// that predates the property, or a net core that did not answer. Unknown is
/// treated as the oldest possible, so it is replaced whenever a release is
/// published and never counts towards `requires`.
///
/// Returns whether to flash `published`. As for the bootloader, replacing a
/// superseded image and refusing below the floor are separate rules: the device
/// is refused if neither the running nor the published net core reaches
/// `requires`.
pub fn plan_netcore(
    running: u16,
    published: Option<u16>,
    requires: u16,
) -> Result<bool, NetCoreTooOld> {
    let flash = published.is_some_and(|p| running < p);
    let ending_at = match (flash, published) {
        (true, Some(published)) => published,
        _ => running,
    };
    if ending_at < requires {
        return Err(NetCoreTooOld { required: requires });
    }
    Ok(flash)
}

/// Whether a net core reads back as the image just written.
///
/// Exact, not "at least", for the same reason as the bootloader: a higher
/// counter is as much a mismatch between image and manifest as a lower one. A
/// net core that reports nothing (`0`) never matches, since a copy that failed
/// leaves it silent.
pub fn netcore_took_update(now: u16, published: u16) -> bool {
    now == published
}

/// One stage of a firmware update, in the order a host performs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStep {
    /// Write the published net core image through the bootloader and let the
    /// device copy it into net flash.
    FlashNetCore,
    /// Read the net core release back from the running application and require
    /// it to equal the published one ([`netcore_took_update`]).
    VerifyNetCore,
    /// The bootloader and application session: detach, optional bootloader
    /// update, application image, and the check that the device returns.
    WriteApplication,
}

/// Order the stages of an update for one device.
///
/// `can_report` is whether the application running now relays the net core
/// release, which only applications speaking device protocol 0.1.5 or newer do.
/// It decides where the read-back can happen, never whether the net core is
/// updated first: the net core is always written before the application, so an
/// application is never installed on top of a net core step that has not run.
///
/// - A running application that can report verifies the net core immediately,
///   so a copy that failed stops the update before the application is touched.
/// - One that cannot report leaves nothing on the device able to read the net
///   core back, so the verification comes after the new application is
///   installed and relays the release. On this path the `requires` floor is
///   enforced after the application is written, and a silently failed copy is
///   only detected then. That is the residual risk of updating from firmware
///   that predates the property.
///
/// An unreported net core counts as release 0 either way, so it is replaced
/// whenever a release is published and never satisfies `requires`.
pub fn plan_update_steps(
    can_report: bool,
    running: u16,
    published: Option<u16>,
    requires: u16,
) -> Result<Vec<UpdateStep>, NetCoreTooOld> {
    let running = if can_report { running } else { 0 };
    if !plan_netcore(running, published, requires)? {
        return Ok(vec![UpdateStep::WriteApplication]);
    }
    Ok(if can_report {
        vec![
            UpdateStep::FlashNetCore,
            UpdateStep::VerifyNetCore,
            UpdateStep::WriteApplication,
        ]
    } else {
        vec![
            UpdateStep::FlashNetCore,
            UpdateStep::WriteApplication,
            UpdateStep::VerifyNetCore,
        ]
    })
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
    // so this is the one point in the update where its release is readable.
    match read_bootloader_release_for_device(uuid, vid, pid) {
        Ok(release) => tracing::info!("Bootloader reports {:?}", release),
        Err(e) => tracing::warn!("Could not read the bootloader release: {e}"),
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
    fn bootloader_plan_follows_published_release() {
        use BootloaderRelease::{Counter, Legacy};
        // Legacy always updates once something is published.
        assert_eq!(plan_bootloader(Legacy, Some(1), 0), Ok(true));
        assert_eq!(plan_bootloader(Counter(1), Some(2), 0), Ok(true));
        // Current, newer, or nothing published: leave it alone.
        assert_eq!(plan_bootloader(Counter(2), Some(2), 0), Ok(false));
        assert_eq!(plan_bootloader(Counter(3), Some(2), 0), Ok(false));
        assert_eq!(plan_bootloader(Legacy, None, 0), Ok(false));
    }

    #[test]
    fn only_the_published_counter_counts_as_written() {
        use BootloaderRelease::{Counter, Legacy};
        assert!(bootloader_took_update(Counter(2), 2));
        assert!(!bootloader_took_update(Counter(1), 2));
        assert!(!bootloader_took_update(Counter(3), 2));
        assert!(!bootloader_took_update(Legacy, 2));
    }

    #[test]
    fn netcore_plan_follows_published_release() {
        // Unknown always updates once something is published.
        assert_eq!(plan_netcore(0, Some(1), 0), Ok(true));
        assert_eq!(plan_netcore(1, Some(2), 0), Ok(true));
        // Current, newer, or nothing published: leave it alone.
        assert_eq!(plan_netcore(2, Some(2), 0), Ok(false));
        assert_eq!(plan_netcore(3, Some(2), 0), Ok(false));
        assert_eq!(plan_netcore(0, None, 0), Ok(false));
        assert_eq!(plan_netcore(5, None, 0), Ok(false));
    }

    #[test]
    fn netcore_requirement_is_a_floor() {
        // The published net core reaches the floor, so flashing it satisfies it.
        assert_eq!(plan_netcore(0, Some(2), 2), Ok(true));
        assert_eq!(plan_netcore(1, Some(3), 2), Ok(true));
        // Already at or above the floor.
        assert_eq!(plan_netcore(2, None, 2), Ok(false));
        assert_eq!(plan_netcore(4, Some(3), 2), Ok(false));
        // Neither the running nor the published net core reaches it.
        assert_eq!(plan_netcore(1, Some(1), 2), Err(NetCoreTooOld { required: 2 }));
        assert_eq!(plan_netcore(0, None, 1), Err(NetCoreTooOld { required: 1 }));
        assert_eq!(plan_netcore(1, None, 2), Err(NetCoreTooOld { required: 2 }));
        // A floor above a published release the device already exceeds is met.
        assert_eq!(plan_netcore(5, Some(3), 4), Ok(false));
    }

    #[test]
    fn only_the_published_netcore_counts_as_written() {
        assert!(netcore_took_update(2, 2));
        assert!(!netcore_took_update(1, 2));
        assert!(!netcore_took_update(3, 2));
        assert!(!netcore_took_update(0, 2));
    }

    #[test]
    fn net_core_is_written_before_the_application_and_read_back_where_possible() {
        use UpdateStep::{FlashNetCore, VerifyNetCore, WriteApplication};
        // An application that relays the release proves the net core first.
        assert_eq!(
            plan_update_steps(true, 1, Some(2), 0),
            Ok(vec![FlashNetCore, VerifyNetCore, WriteApplication])
        );
        // A reporting application whose net core answered nothing is replaced too.
        assert_eq!(
            plan_update_steps(true, 0, Some(2), 0),
            Ok(vec![FlashNetCore, VerifyNetCore, WriteApplication])
        );
        // One that cannot relay is verified only through the application it installs.
        assert_eq!(
            plan_update_steps(false, 0, Some(2), 0),
            Ok(vec![FlashNetCore, WriteApplication, VerifyNetCore])
        );
        // A value the device could not have reported is not trusted.
        assert_eq!(
            plan_update_steps(false, 9, Some(2), 0),
            Ok(vec![FlashNetCore, WriteApplication, VerifyNetCore])
        );
    }

    #[test]
    fn current_or_unpublished_net_core_leaves_only_the_application() {
        use UpdateStep::WriteApplication;
        assert_eq!(plan_update_steps(true, 2, Some(2), 0), Ok(vec![WriteApplication]));
        assert_eq!(plan_update_steps(true, 3, Some(2), 0), Ok(vec![WriteApplication]));
        assert_eq!(plan_update_steps(true, 0, None, 0), Ok(vec![WriteApplication]));
        assert_eq!(plan_update_steps(false, 0, None, 0), Ok(vec![WriteApplication]));
    }

    #[test]
    fn net_core_floor_refuses_before_any_step_runs() {
        use UpdateStep::{FlashNetCore, VerifyNetCore, WriteApplication};
        // Flashing the published release reaches the floor.
        assert_eq!(
            plan_update_steps(true, 1, Some(3), 3),
            Ok(vec![FlashNetCore, VerifyNetCore, WriteApplication])
        );
        // Already at the floor, nothing published.
        assert_eq!(plan_update_steps(true, 3, None, 3), Ok(vec![WriteApplication]));
        // Neither the device nor the release reaches it.
        assert_eq!(
            plan_update_steps(true, 1, Some(2), 3),
            Err(NetCoreTooOld { required: 3 })
        );
        // Without a way to read the device, nothing but a publication can meet the floor.
        assert_eq!(
            plan_update_steps(false, 0, None, 1),
            Err(NetCoreTooOld { required: 1 })
        );
        assert_eq!(
            plan_update_steps(false, 7, None, 1),
            Err(NetCoreTooOld { required: 1 })
        );
    }

    #[test]
    fn bootloader_requirement_is_a_floor() {
        use BootloaderRelease::{Counter, Legacy};
        // The published bootloader reaches the floor, so flashing it satisfies it.
        assert_eq!(plan_bootloader(Legacy, Some(2), 2), Ok(true));
        // Already at the floor.
        assert_eq!(plan_bootloader(Counter(2), None, 2), Ok(false));
        // Neither the running nor the published bootloader reaches it.
        assert_eq!(
            plan_bootloader(Counter(1), Some(1), 2),
            Err(BootloaderTooOld { required: 2 })
        );
        assert_eq!(
            plan_bootloader(Legacy, None, 1),
            Err(BootloaderTooOld { required: 1 })
        );
    }

    const ALPHA: &str = include_str!("../../../firmware/manifests/releases.alpha.json");
    const STABLE: &str = include_str!("../../../firmware/manifests/releases.stable.json");

    /// Every host parses the manifest whole or not at all, so one entry missing
    /// a required field would hide every update from every device.
    #[test]
    fn committed_manifests_parse() {
        for (name, text) in [("alpha", ALPHA), ("stable", STABLE)] {
            let manifest: FirmwareManifest =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} parses: {e}"));
            assert!(manifest.version <= SUPPORTED_MANIFEST_VERSION);
            for (device, entries) in &manifest.devices {
                assert!(!entries.firmwares.is_empty(), "{name} {device} has no firmware");
                let mut previous: Option<[u16; 3]> = None;
                for entry in &entries.firmwares {
                    let version = entry.version_parts().unwrap_or_else(|| {
                        panic!("{name} {device} {} is not a version", entry.version)
                    });
                    assert!(
                        parse_version(&entry.min_desktop).is_some(),
                        "{name} {device} {} has min_desktop {:?}",
                        entry.version,
                        entry.min_desktop
                    );
                    // Hosts take the first installable entry, so order is semantics.
                    if let Some(newer) = previous {
                        assert!(version_newer(version, newer), "{name} {device} is not newest-first");
                    }
                    previous = Some(version);
                }
            }
        }
    }

    /// Stable is promoted from alpha by copying, so it can never hold an entry
    /// alpha does not, or one that differs from it.
    #[test]
    fn stable_only_holds_promoted_alpha_entries() {
        let alpha: serde_json::Value = serde_json::from_str(ALPHA).unwrap();
        let stable: serde_json::Value = serde_json::from_str(STABLE).unwrap();
        for (device, entries) in stable["devices"].as_object().unwrap() {
            for entry in entries["firmwares"].as_array().unwrap() {
                let in_alpha = alpha["devices"][device]["firmwares"]
                    .as_array()
                    .is_some_and(|all| all.contains(entry));
                assert!(in_alpha, "stable {device} {} is not an alpha entry", entry["version"]);
            }
        }
    }

    fn manifest_with(entries: &[(&str, &str)]) -> FirmwareManifest {
        let firmwares: Vec<_> = entries
            .iter()
            .map(|(version, min_desktop)| {
                serde_json::json!({
                    "version": version,
                    "requires_bootloader": 0,
                    "requires_netcore": 0,
                    "min_desktop": min_desktop,
                    "files": {},
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "devices": { "atslite1": { "firmwares": firmwares } },
        }))
        .unwrap()
    }

    fn selected(offer: FirmwareOffer<'_>) -> (Option<&str>, Option<&str>) {
        (
            offer.offered.map(|e| e.version.as_str()),
            offer.held_back.map(|e| e.version.as_str()),
        )
    }

    #[test]
    fn firmware_needing_a_newer_desktop_is_held_back() {
        let manifest = manifest_with(&[("0.3.0", "0.8.0"), ("0.2.0", "0.6.0"), ("0.1.0", "0.6.0")]);
        // Below the newest entry's floor: offer the next one, mention the newest.
        assert_eq!(
            selected(select_firmware(&manifest, "atslite1", [0, 7, 1])),
            (Some("0.2.0"), Some("0.3.0"))
        );
        // Exactly at the floor counts as meeting it.
        assert_eq!(
            selected(select_firmware(&manifest, "atslite1", [0, 8, 0])),
            (Some("0.3.0"), None)
        );
        // Older than every floor: nothing installable, but say what exists.
        assert_eq!(
            selected(select_firmware(&manifest, "atslite1", [0, 5, 0])),
            (None, Some("0.3.0"))
        );
        // Unknown device.
        assert_eq!(selected(select_firmware(&manifest, "nope", [9, 9, 9])), (None, None));
    }

    #[test]
    fn channel_names_round_trip_and_unknown_falls_back_to_stable() {
        for channel in [FirmwareChannel::Stable, FirmwareChannel::Alpha] {
            assert_eq!(FirmwareChannel::from_name(channel.name()), channel);
        }
        assert_eq!(FirmwareChannel::from_name(""), FirmwareChannel::Stable);
        assert_eq!(FirmwareChannel::from_name("nightly"), FirmwareChannel::Stable);
    }

    #[test]
    fn unreadable_min_desktop_is_never_offered() {
        let manifest = manifest_with(&[("0.2.0", "soon"), ("0.1.0", "0.6.0")]);
        assert_eq!(
            selected(select_firmware(&manifest, "atslite1", [1, 0, 0])),
            (Some("0.1.0"), Some("0.2.0"))
        );
    }

    #[test]
    fn download_urls_follow_each_components_own_release() {
        let url = |source| build_download_url("atslite1", source, "app-a");
        assert_eq!(
            url(ReleaseSource::Application { version: "0.1.15" }),
            "https://github.com/odysseyarm/odyssey/releases/download/atslite1-v0.1.15/app-a.bin"
        );
        assert_eq!(
            ReleaseSource::Bootloader { release: 3 }.tag("legacy-atslite1"),
            "legacy-atslite1-bootloader-v3"
        );
        assert_eq!(
            ReleaseSource::NetCore { release: 2 }.tag("atslite1"),
            "atslite1-netcore-v2"
        );
    }
}
