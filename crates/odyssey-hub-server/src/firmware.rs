//! Firmware manifest fetching and DFU update support.

mod dfu_download;
/// Sends complete application and net-core images over acknowledged OTA transports.
pub mod ota;
/// Since: Odyssey Desktop 0.7.5.
pub mod dongle;
/// Since: Odyssey Desktop 0.6.6.
pub mod pack;
use std::num::NonZeroU8;
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use dfu_core::DfuIo as _;
use dfu_nusb::DfuNusb;
use futures::io::Cursor;
use nusb::MaybeFuture as _;
use odyssey_firmware_manifest::asset_filename;
pub use odyssey_firmware_manifest::{
    DeviceManifest, FileEntry, FirmwareEntry, FirmwareManifest, ReleaseSource,
};
use odyssey_version::legacy_bootloader_version;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::policy;

/// Where firmware manifests are published. Promotion from nightly to stable is a
/// deliberate copy of an entry between the two files.
const MANIFEST_BASE_URL: &str =
    "https://raw.githubusercontent.com/odysseyarm/odyssey/master/firmware/manifests";

/// Which manifest a host follows.
///
/// Since: Odyssey Desktop 0.6.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FirmwareChannel {
    /// Entries someone promoted by hand. What customers follow.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    #[default]
    Stable,
    /// Every release as it is published.
    ///
    /// Since: Odyssey Desktop (unreleased).
    Nightly,
}

impl FirmwareChannel {
    /// Since: Odyssey Desktop 0.6.4.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    /// An unrecognised name falls back to stable: the safe channel is the one
    /// to land on when a settings file is stale or hand-edited. `alpha` is the
    /// nightly channel's name in settings saved before it was renamed.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub fn from_name(name: &str) -> Self {
        match name {
            "nightly" | "alpha" => Self::Nightly,
            _ => Self::Stable,
        }
    }

    /// The channel the user chose, stable if they never did.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub async fn load() -> Self {
        odyssey_hub_common::config::firmware_channel_async()
            .await
            .map_or(Self::Stable, |name| Self::from_name(&name))
    }

    /// Save this channel as the user's firmware preference.
    ///
    /// # Errors
    ///
    /// Returns the configuration error if the preference cannot be written.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub async fn save(self) -> Result<(), odyssey_hub_common::config::ConfigError> {
        odyssey_hub_common::config::firmware_channel_save_async(self.name()).await
    }

    fn manifest_url(self) -> String {
        format!("{MANIFEST_BASE_URL}/releases.{}.json", self.name())
    }
}

/// Base URL of GitHub release downloads.
const RELEASES_BASE_URL: &str = "https://github.com/odysseyarm/odyssey/releases/download";

/// Upper bound for one firmware download, including connection, redirects and
/// the complete response body. A stalled transfer must release the update claim.
const FIRMWARE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// The manifest's device type name for each known (VID, PID).
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub const DEVICE_MAP: &[(u16, u16, &str)] = &[
    (0x1915, 0x5210, "legacy-atslite1"),
    (0x1915, 0x5211, "atslite1"),
    (0x1915, 0x5212, "dongle"),
    (0x1915, 0x5213, "marker-control-board"),
];

/// The product id a device enumerates under while its bootloader is running.
///
/// The bootloaders add 0x10 to the application's id. That tells the two modes
/// apart from the device descriptor alone, so bcdDevice can carry the
/// bootloader's own version while it is in charge. It is an offset because
/// 0x5210 and 0x5211 already have bit 4 set, so setting the bit would not be
/// reversible.
///
/// Since: Odyssey Desktop 0.6.4.
pub const fn bootloader_pid(app_pid: u16) -> u16 {
    app_pid + 0x10
}

/// The application's product id for a device currently in its bootloader.
///
/// Since: Odyssey Desktop 0.6.4.
pub const fn app_pid(pid: u16) -> u16 {
    pid - 0x10
}

fn legacy_bootloader_semver(bcd_device: u16) -> Option<semver::Version> {
    if bcd_device & 0xF000 == 0xF000 {
        let [major, minor, patch] = legacy_bootloader_version(bcd_device & 0x0FFF)?;
        return Some(semver::Version::new(
            major.into(),
            minor.into(),
            patch.into(),
        ));
    }
    None
}

/// Whether `manifest` is a newer version than `device`.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn version_newer(device: [u32; 3], manifest: [u32; 3]) -> bool {
    if manifest[0] != device[0] {
        return manifest[0] > device[0];
    }
    if manifest[1] != device[1] {
        return manifest[1] > device[1];
    }
    manifest[2] > device[2]
}

/// The manifest device type of a VID/PID, if it is a known device.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn device_type_from_vid_pid(vid: u16, pid: u16) -> Option<&'static str> {
    DEVICE_MAP
        .iter()
        .find(|(v, p, _)| *v == vid && *p == pid)
        .map(|(_, _, name)| *name)
}

/// The VID, PID and device type name of a known product id.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn device_info_from_pid(pid: u16) -> Option<(u16, u16, &'static str)> {
    DEVICE_MAP
        .iter()
        .find(|(_, p, _)| *p == pid)
        .map(|&(v, p, name)| (v, p, name))
}

/// The exact manifest schema version this build understands.
///
/// A schema change that alters field meaning makes this build reject older and
/// newer manifests alike, so a host never acts on entries it would misread.
///
/// Since: Odyssey Desktop 0.6.4.
pub const SUPPORTED_MANIFEST_VERSION: u32 = 2;

/// Network and offline-pack failures keep their concrete source errors.
/// Callers tell an incompatible manifest from an incomplete pack by variant,
/// without inspecting the user-facing message.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Error)]
pub enum FirmwareSourceError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("firmware request failed")]
    Network(#[from] reqwest::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error(
        "unsupported firmware manifest version {0}; this build requires {SUPPORTED_MANIFEST_VERSION}"
    )]
    ManifestVersion(u32),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("SHA256 mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        #[doc = "Since: Odyssey Desktop 0.7.0."]
        expected: String,
        #[doc = "Since: Odyssey Desktop 0.7.0."]
        actual: String,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error("firmware size mismatch: expected {expected} bytes, received {actual}")]
    ImageSize {
        #[doc = "Since: Odyssey Desktop 0.7.0."]
        expected: u64,
        #[doc = "Since: Odyssey Desktop 0.7.0."]
        actual: u64,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error(
        "There is no firmware pack in {dir}: {manifest} is missing. Unpack the Odyssey offline firmware pack into that folder."
    )]
    MissingPack {
        /// Since: Odyssey Desktop 0.7.0.
        dir: std::path::PathBuf,
        /// Since: Odyssey Desktop 0.7.0.
        manifest: String,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error("could not read firmware pack file {path}")]
    Read {
        /// Since: Odyssey Desktop 0.7.0.
        path: std::path::PathBuf,
        /// Since: Odyssey Desktop 0.7.0.
        #[source]
        source: std::io::Error,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error("{path} is not a firmware manifest")]
    Manifest {
        /// Since: Odyssey Desktop 0.7.0.
        path: std::path::PathBuf,
        /// Since: Odyssey Desktop 0.7.0.
        #[source]
        source: serde_json::Error,
    },
    /// Since: Odyssey Desktop 0.7.0.
    #[error("The firmware pack in {dir} has no complete release.{details}")]
    IncompletePack {
        /// Since: Odyssey Desktop 0.7.0.
        dir: std::path::PathBuf,
        /// Since: Odyssey Desktop 0.7.0.
        details: String,
    },
}

/// Fetch the channel manifest from GitHub, or complete releases from the offline pack.
///
/// # Errors
/// Returns [`FirmwareSourceError::Network`] for request, HTTP status, or JSON
/// decoding failures, [`FirmwareSourceError::ManifestVersion`] if the manifest
/// uses a newer version, and pack errors for a missing, unreadable, malformed,
/// unsupported, or incomplete offline pack.
///
/// # Since
/// Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn fetch_manifest(
    channel: FirmwareChannel,
) -> Result<FirmwareManifest, FirmwareSourceError> {
    if policy::offline() {
        let dir = policy::firmware_pack_dir();
        let loaded = pack::load_manifest(&dir, channel)?;
        for dropped in &loaded.dropped {
            tracing::warn!(
                "Firmware pack: not offering {} {}: {}",
                dropped.device,
                dropped.version,
                dropped.problems.join("; ")
            );
        }
        return Ok(loaded.manifest);
    }
    let client = reqwest::Client::new();
    let manifest: FirmwareManifest = client
        .get(channel.manifest_url())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if manifest.version != SUPPORTED_MANIFEST_VERSION {
        return Err(FirmwareSourceError::ManifestVersion(manifest.version));
    }
    Ok(manifest)
}

/// What a manifest offers one device, given the desktop doing the offering and
/// the net core the unit is running.
///
/// `held_back` and `net_core_blocked` only ever name entries newer than
/// `offered`: an older entry that was skipped is not news.
///
/// Since: Odyssey Desktop 0.6.4.
#[derive(Debug, Clone, Copy)]
pub struct FirmwareOffer<'a> {
    /// The newest entry this desktop can install on this unit.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub offered: Option<&'a FirmwareEntry>,
    /// A newer entry that exists but needs a newer desktop. The user is told so
    /// and chooses whether to update. Nothing updates the desktop for them.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub held_back: Option<&'a FirmwareEntry>,
    /// A newer entry the desktop could install, were the unit's net core
    /// further along. When `offered` is set it is normally the bridge that
    /// brings the net core up to what this entry requires. When it is unset, the
    /// unit has no path to this entry from the net core it runs.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    pub net_core_blocked: Option<&'a FirmwareEntry>,
}

/// Choose firmware for a device type.
///
/// Entries are published newest-first. Cross-major and cross-minor updates are
/// allowed: the manifest says what a device should run, and refusing to cross
/// a minor would strand devices on a line that is no longer built. An entry
/// whose `min_desktop` is above `desktop`, or cannot be read, is not
/// installable here. Neither is one whose `requires_netcore` is above
/// `running_netcore`, the release the unit reports (`0` when it reports none,
/// which never satisfies a floor above `0`). An entry skipped for both reasons
/// counts as held back by the desktop.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn select_firmware<'a>(
    manifest: &'a FirmwareManifest,
    device_type: &str,
    desktop: &semver::Version,
    running_netcore: u16,
) -> FirmwareOffer<'a> {
    let mut offer = FirmwareOffer {
        offered: None,
        held_back: None,
        net_core_blocked: None,
    };
    let Some(device) = manifest.devices.get(device_type) else {
        return offer;
    };
    for entry in &device.firmwares {
        let desktop_ok = if let Ok(min) = semver::Version::parse(&entry.min_desktop) {
            *desktop >= min
        } else {
            tracing::warn!(
                "Firmware {} has an unreadable min_desktop {:?}; not offering it",
                entry.version,
                entry.min_desktop
            );
            false
        };
        if !desktop_ok {
            offer.held_back.get_or_insert(entry);
        } else if entry.requires_netcore > running_netcore {
            offer.net_core_blocked.get_or_insert(entry);
        } else {
            offer.offered = Some(entry);
            return offer;
        }
    }
    offer
}

/// The net core release to judge a unit by, or `None` when the hub has no settled
/// reading of it (`DeviceCapabilities::NETCORE_KNOWN`).
///
/// The application reports 0 until its BLE controller has answered, which is
/// also what a unit shows for a while after every reboot or transport swap, so
/// an unsettled 0 must not be judged. A 0 that stays after the hub's last retry
/// is the net core's real answer: it predates release reporting.
///
/// Since: Odyssey Desktop 0.6.5.
pub fn known_netcore_release(settled: bool, reported: u16) -> Option<u16> {
    settled.then_some(reported)
}

/// Whether offering `entry` to a unit changes anything worth a click.
///
/// A newer application always does. The same version does only as a repair: the
/// entry carries a net core above the one the unit reports, which is what a
/// failed net-core copy leaves behind once the application write has gone
/// through. Without this the unit sits on the bridge with a stale net core,
/// nothing newer is installable past its floor, and "run the update again" has
/// nothing to offer.
///
/// `running_netcore` is `None` when the net core's release is not known, so
/// that "unknown" never counts as "behind" and offers a repair nobody needs.
/// [`known_netcore_release`] decides which readings are real.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn is_worth_installing(
    entry: &FirmwareEntry,
    installed: [u32; 3],
    running_netcore: Option<u16>,
) -> bool {
    let Ok(version) = semver::Version::parse(&entry.version) else {
        return false;
    };
    let installed = semver::Version::new(
        u64::from(installed[0]),
        u64::from(installed[1]),
        u64::from(installed[2]),
    );
    if version > installed {
        return true;
    }
    version == installed
        && entry
            .files
            .get("net-core")
            .and_then(|file| file.release)
            .zip(running_netcore)
            .is_some_and(|(carried, running)| carried > running)
}

/// The download URL of `filename` in the release `source` names.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn build_download_url(device_type: &str, source: ReleaseSource<'_>, filename: &str) -> String {
    format!(
        "{}/{}/{}",
        RELEASES_BASE_URL,
        source.tag(device_type),
        asset_filename(device_type, source, filename)
    )
}

/// Fails unless `data` hashes to `expected_sha256`.
fn verify_sha256(data: &[u8], expected_sha256: &str) -> Result<(), FirmwareSourceError> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash_hex = hex::encode(hasher.finalize());

    if hash_hex != expected_sha256 {
        return Err(FirmwareSourceError::HashMismatch {
            expected: expected_sha256.to_owned(),
            actual: hash_hex,
        });
    }
    Ok(())
}

/// Download and verify firmware bytes against the manifest's SHA256 hash.
///
/// Offline mode reads from the firmware pack. Online mode fetches from the release
/// server with a two-minute deadline covering headers and body.
///
/// The manifest's declared size bounds accumulation before hash validation.
///
/// # Errors
/// Returns [`FirmwareSourceError::Network`] for request, HTTP status, transfer,
/// or response-body failures, [`FirmwareSourceError::ImageSize`] when received
/// size differs from the manifest, [`FirmwareSourceError::HashMismatch`] when
/// SHA256 differs, and pack read, size, or hash errors in offline mode.
///
/// # Since
/// Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn download_firmware(
    device_type: &str,
    source: ReleaseSource<'_>,
    filename: &str,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<Vec<u8>, FirmwareSourceError> {
    if policy::offline() {
        let dir = policy::firmware_pack_dir();
        tracing::info!(
            "Reading {filename} from the firmware pack in {}",
            dir.display()
        );
        return pack::read_file(
            &dir,
            device_type,
            source,
            filename,
            expected_sha256,
            expected_size,
        );
    }

    let url = build_download_url(device_type, source, filename);
    tracing::info!("Downloading firmware from {}", url);

    tracing::debug!("Creating reqwest client...");
    let client = reqwest::Client::builder()
        .timeout(FIRMWARE_DOWNLOAD_TIMEOUT)
        .build()?;
    tracing::debug!("Sending GET request...");
    let response = client.get(&url).send().await?;
    tracing::debug!("Got response, status: {}", response.status());

    let mut response = response.error_for_status()?;

    tracing::debug!("Reading response bytes...");
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let actual = data.len() as u64 + chunk.len() as u64;
        if actual > expected_size {
            return Err(FirmwareSourceError::ImageSize {
                expected: expected_size,
                actual,
            });
        }
        data.extend_from_slice(&chunk);
    }
    if data.len() as u64 != expected_size {
        return Err(FirmwareSourceError::ImageSize {
            expected: expected_size,
            actual: data.len() as u64,
        });
    }
    tracing::debug!("Got {} bytes", data.len());

    verify_sha256(&data, expected_sha256)?;

    tracing::info!("Downloaded {} bytes, SHA256 verified", data.len());
    Ok(data)
}

/// The VID/PID of the connected known device whose USB serial is `uuid` (its BLE MAC).
/// Serials encode the MAC as hex (e.g. "AABBCCDDEEFF"). Case, colons and dashes
/// are ignored.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn find_device_vid_pid_by_uuid(uuid: &[u8; 6]) -> Option<(u16, u16)> {
    let uuid_hex = format!(
        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
    );
    let devices = nusb::list_devices().wait().ok()?;
    for dev in devices {
        for &(vid, pid, _device_type) in DEVICE_MAP {
            if dev.vendor_id() == vid
                && dev.product_id() == pid
                && let Some(serial) = dev.serial_number()
            {
                let serial_clean: String = serial.chars().filter(char::is_ascii_hexdigit).collect();
                if serial_clean.eq_ignore_ascii_case(&uuid_hex) {
                    return Some((vid, pid));
                }
            }
        }
    }
    None
}

/// The first known device type with a device connected, and its VID/PID.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn find_any_known_device() -> Option<(u16, u16, &'static str)> {
    for &(vid, pid, device_type) in DEVICE_MAP {
        if find_device(vid, pid).is_ok() {
            return Some((vid, pid, device_type));
        }
    }
    None
}

/// Whether the device with `uuid` is connected and running its application. `false` when
/// it is absent, cannot be opened, or exposes the DFU-mode interface.
///
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn is_device_in_app_mode(uuid: &[u8; 6], vid: u16, pid: u16) -> bool {
    let Ok(info) = find_device_by_uuid(uuid, vid, pid) else {
        return false;
    };
    let Ok(device) = info.open().wait() else {
        return false;
    };

    // A DFU interface in DFU mode means the bootloader is running.
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
                        return false;
                    }
                }
            }
        }
    }

    true
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug, Error)]
pub enum DfuError {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("Device not found")]
    DeviceNotFound,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("No suitable alt interface found for slot: {0}")]
    AltInterfaceNotFound(String),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("DFU interface not found on device")]
    DfuInterfaceNotFound,
    /// Since: Odyssey Desktop (unreleased).
    #[error("unrecognized legacy bootloader counter {0}")]
    UnknownBootloaderCounter(u16),
    /// Since: Odyssey Desktop (unreleased).
    #[error("bootloader has no readable SemVer in USB string descriptor 4")]
    UnreadableBootloaderVersion,
    /// Since: Odyssey Desktop (unreleased).
    #[error("could not read bootloader USB version descriptor")]
    BootloaderDescriptor(#[from] nusb::GetDescriptorError),
    /// Since: Odyssey Desktop (unreleased).
    #[error("invalid bootloader version")]
    InvalidBootloaderVersion(#[from] semver::Error),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("DFU error: {0}")]
    Dfu(#[from] dfu_nusb::Error),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("USB error: {0}")]
    Usb(#[from] nusb::Error),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// DFU interface class/subclass, and the protocol byte that distinguishes the
/// bootloader from the runtime interface an application also exposes.
///
/// Since: Odyssey Desktop 0.6.3.
pub const DFU_CLASS: u8 = 0xFE;
/// Since: Odyssey Desktop 0.6.3.
pub const DFU_SUBCLASS: u8 = 0x01;
/// Since: Odyssey Desktop 0.6.3.
pub const DFU_PROTOCOL_DFU_MODE: u8 = 0x02;

/// The first USB device with this VID/PID.
///
/// # Errors
/// Returns [`DfuError::Usb`] if USB enumeration fails, or
/// [`DfuError::DeviceNotFound`] if no device has the requested VID/PID.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn find_device(vid: u16, pid: u16) -> Result<nusb::DeviceInfo, DfuError> {
    nusb::list_devices()
        .wait()
        .map_err(DfuError::Usb)?
        .find(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .ok_or(DfuError::DeviceNotFound)
}

/// Find a USB device by VID/PID and UUID, matched against the USB serial number.
///
/// Both the application and the bootloader publish the UUID as the serial, so
/// this is the only safe way to pick one of several identical devices: flashing
/// resolves the device several times over, and the wrong match writes an image
/// into a device the user did not choose.
///
/// # Errors
/// Returns [`DfuError::Usb`] if USB enumeration fails, or
/// [`DfuError::DeviceNotFound`] if no matching serial is found.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn find_device_by_uuid(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<nusb::DeviceInfo, DfuError> {
    let uuid_hex = format!(
        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
    );
    let candidates: Vec<_> = nusb::list_devices()
        .wait()
        .map_err(DfuError::Usb)?
        .filter(|dev| dev.vendor_id() == vid && dev.product_id() == pid)
        .collect();

    if let Some(dev) = candidates.iter().find(|dev| {
        dev.serial_number().is_some_and(|s| {
            let clean: String = s.chars().filter(char::is_ascii_hexdigit).collect();
            clean.eq_ignore_ascii_case(&uuid_hex)
        })
    }) {
        return Ok(dev.clone());
    }

    crate::migration::single_unidentifiable_device(candidates).ok_or(DfuError::DeviceNotFound)
}

/// The number of the first interface with the DFU class and subclass.
///
/// # Errors
/// Returns [`DfuError::DfuInterfaceNotFound`] if the device has no DFU interface.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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

fn slot_label_matches(descriptor: &str, slot: &str) -> bool {
    descriptor
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|label| label == slot)
}

/// The alt setting whose string descriptor names `slot` (e.g. `APP`, `APP_A` or `APP_B`).
///
/// # Errors
/// Returns [`DfuError::AltInterfaceNotFound`] if no DFU alternate setting names `slot`.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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
                    // The alt setting's string descriptor names the slot it writes.
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
                            && slot_label_matches(&desc, slot)
                        {
                            return Ok(alt.alternate_setting());
                        }
                    }
                }
            }
        }
    }
    Err(DfuError::AltInterfaceNotFound(slot.to_string()))
}

/// Selects the inactive dual-bank slot or the single-bank staging image.
/// Returns USB slot name, manifest file name and alternate setting.
///
/// # Errors
/// Returns [`DfuError::AltInterfaceNotFound`] if no application slot is exposed.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub fn determine_target_slot(
    device: &nusb::Device,
    interface_num: u8,
) -> Result<(&'static str, &'static str, u8), DfuError> {
    let app_a_result = find_alt_for_slot(device, interface_num, "APP_A");
    let app_b_result = find_alt_for_slot(device, interface_num, "APP_B");

    match (app_a_result, app_b_result) {
        (Ok(alt), _) => {
            // APP_A is exposed, so bank B is active.
            tracing::info!(
                "DFU target: APP_A (alt {}), downloading app-a firmware",
                alt
            );
            Ok(("APP_A", "app-a", alt))
        }
        (_, Ok(alt)) => {
            // APP_B is exposed, so bank A is active.
            tracing::info!(
                "DFU target: APP_B (alt {}), downloading app-b firmware",
                alt
            );
            Ok(("APP_B", "app-b", alt))
        }
        _ => find_alt_for_slot(device, interface_num, "APP").map(|alt| ("APP", "app", alt)),
    }
}

/// [`determine_target_slot`] for the device with `uuid` while it runs its bootloader.
///
/// # Errors
/// Returns [`DfuError`] if the device cannot be found, opened, or inspected, or
/// does not expose a suitable application slot.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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

/// Parse `SemVer` with `u32` core components, retaining Nightly identity in its string field.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn parse_version(version: &str) -> Option<[u32; 3]> {
    let version = semver::Version::parse(version).ok()?;
    Some([
        u32::try_from(version.major).ok()?,
        u32::try_from(version.minor).ok()?,
        u32::try_from(version.patch).ok()?,
    ])
}

/// Find a device that is currently running its bootloader.
///
/// Callers pass the application's product id throughout. The bootloader
/// enumerates under its own id, so the translation lives here instead of at
/// every call site.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found by UUID.
/// Since: Odyssey Desktop 0.6.4.
pub fn find_bootloader_by_uuid(
    uuid: &[u8; 6],
    vid: u16,
    app_pid: u16,
) -> Result<nusb::DeviceInfo, DfuError> {
    match find_device_by_uuid(uuid, vid, bootloader_pid(app_pid)) {
        Ok(info) => Ok(info),
        // Bootloaders flashed before the split enumerate under the
        // application's id instead.
        Err(_) => crate::migration::find_bootloader_sharing_app_pid(*uuid, vid, app_pid),
    }
}

/// Read bootloader `SemVer`, preferring counter identity over historical
/// descriptor-4 Cargo versions. Unversioned USB default `0x0010` yields `None`.
///
/// # Errors
/// Returns [`DfuError`] when the device is missing, reports unknown legacy
/// counter, or lacks a readable modern `SemVer`.
/// Since: Odyssey Desktop (unreleased).
pub async fn read_bootloader_release_for_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
) -> Result<Option<semver::Version>, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let bcd_device = info.device_version();
    if bcd_device & 0xF000 == 0xF000 {
        return legacy_bootloader_semver(bcd_device)
            .map(Some)
            .ok_or(DfuError::UnknownBootloaderCounter(bcd_device & 0x0FFF));
    }

    if bcd_device == 0x0010 {
        return Ok(None);
    }
    let device = info.open().await?;
    let mut languages = device
        .get_string_descriptor_supported_languages(Duration::from_secs(3))
        .await?;
    let language = languages
        .next()
        .ok_or(DfuError::UnreadableBootloaderVersion)?;
    let descriptor = device
        .get_string_descriptor(
            const { NonZeroU8::new(4).expect("descriptor index 4 is nonzero") },
            language,
            Duration::from_secs(3),
        )
        .await?;
    Ok(Some(semver::Version::parse(&descriptor)?))
}

/// Whether the device is running its bootloader right now.
///
/// Only DFU-mode interfaces count. Applications expose a runtime DFU interface too.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn is_device_in_dfu_mode(uuid: &[u8; 6], vid: u16, pid: u16) -> bool {
    find_bootloader_by_uuid(uuid, vid, pid).is_ok_and(|info| {
        info.interfaces().any(|i| {
            i.class() == DFU_CLASS
                && i.subclass() == DFU_SUBCLASS
                && i.protocol() == DFU_PROTOCOL_DFU_MODE
        })
    })
}

/// Find the alt setting that writes the stage2 bootloader itself.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found, opened, or inspected,
/// or does not expose its bootloader alternate setting.
/// Since: Odyssey Desktop 0.6.4.
pub fn find_bootloader_alt_for_device(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<u8, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface_num = find_dfu_interface(&device)?;
    find_alt_for_slot(&device, interface_num, "BOOTLOADER")
}

/// Find the alt setting that writes the nRF5340 net core image.
///
/// The stage2 bootloader serves it, so the device must already be in DFU mode.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found, opened, or inspected,
/// or does not expose its net-core alternate setting.
/// Since: Odyssey Desktop 0.6.4.
pub fn find_netcore_alt_for_device(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<u8, DfuError> {
    let info = find_bootloader_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;
    let interface_num = find_dfu_interface(&device)?;
    find_alt_for_slot(&device, interface_num, NETCORE_ALT_LABEL)
}

/// The application cannot run on any bootloader this device will end up with.
///
/// Since: Odyssey Desktop 0.6.4.
#[derive(Debug, PartialEq, Eq)]
pub struct BootloaderTooOld {
    /// Since: Odyssey Desktop 0.6.4.
    pub required: semver::Version,
}

impl std::fmt::Display for BootloaderTooOld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no available bootloader meets required version {}",
            self.required
        )
    }
}

impl std::error::Error for BootloaderTooOld {}

/// Decide whether to flash published bootloader and whether application floor holds.
///
/// # Errors
/// Returns [`BootloaderTooOld`] if neither running nor published version reaches
/// the application's required minimum.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn plan_bootloader(
    running: Option<&semver::Version>,
    published: Option<&semver::Version>,
    requires: &semver::Version,
) -> Result<bool, BootloaderTooOld> {
    let flash = published.is_some_and(|published| {
        running.is_none_or(|running| published.cmp_precedence(running).is_gt())
    });
    let ending_at = if flash { published } else { running };
    if !requires
        .cmp_precedence(&semver::Version::new(0, 0, 0))
        .is_eq()
        && ending_at.is_none_or(|version| version.cmp_precedence(requires).is_lt())
    {
        return Err(BootloaderTooOld {
            required: requires.clone(),
        });
    }
    Ok(flash)
}

/// Whether bootloader reports exactly version written.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn bootloader_took_update(now: Option<&semver::Version>, published: &semver::Version) -> bool {
    now == Some(published)
}

/// DFU alt setting label under which the stage2 bootloader exposes the net core.
const NETCORE_ALT_LABEL: &str = "NET";

/// The unit is not running the net core the application needs before it may be
/// installed.
///
/// Since: Odyssey Desktop 0.6.4.
#[derive(Debug, PartialEq, Eq)]
pub struct NetCoreTooOld {
    /// Since: Odyssey Desktop 0.6.4.
    pub required: u16,
}

impl std::fmt::Display for NetCoreTooOld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this firmware needs the net core to be running release {} or newer before it is installed; \
             install the earlier firmware that carries it first",
            self.required
        )
    }
}

impl std::error::Error for NetCoreTooOld {}

/// Decide whether the net core has to be written, and whether the update is
/// allowed at all.
///
/// `running` is what the device last reported, `0` when it reported nothing:
/// firmware that predates the property, a net core that did not answer, or a
/// device that is in the bootloader. Unknown is treated as the oldest possible,
/// so it is replaced whenever a release is published and never satisfies a
/// `requires` above `0`.
///
/// Returns whether to flash `published`. As for the bootloader, replacing a
/// superseded image and refusing below the floor are separate rules, but the
/// floor is judged against `running` alone: nothing reports the net core between
/// writing it and the new application running, so the release this update
/// publishes can never vouch for its own `requires`. See [`plan_update_steps`].
///
/// # Errors
/// Returns [`NetCoreTooOld`] if the running net-core release is below the
/// application's required minimum.
/// Since: Odyssey Desktop 0.6.4.
pub fn plan_netcore(
    running: u16,
    published: Option<u16>,
    requires: u16,
) -> Result<bool, NetCoreTooOld> {
    if running < requires {
        return Err(NetCoreTooOld { required: requires });
    }
    Ok(published.is_some_and(|p| running < p))
}

/// Whether a net core reads back as the image just written.
///
/// An exact match is required, for the same reason as the bootloader: a higher
/// counter is as much a mismatch between image and manifest as a lower one. A
/// net core that reports nothing (`0`) never matches, since a copy that failed
/// leaves it silent.
///
/// Since: Odyssey Desktop 0.6.4.
pub fn netcore_took_update(now: u16, published: u16) -> bool {
    now == published
}

/// One stage of a firmware update, in the order a host performs it.
///
/// Since: Odyssey Desktop 0.6.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStep {
    /// Write the published net core image through the bootloader. The net core
    /// bootloader copies it into net flash afterwards, while the device sits in
    /// the stage2 bootloader awaiting an application.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    FlashNetCore,
    /// Read the net core release back from the running application and require
    /// it to equal the published one ([`netcore_took_update`]).
    ///
    /// Since: Odyssey Desktop 0.6.4.
    VerifyNetCore,
    /// The bootloader and application session: detach if needed, optional
    /// bootloader update, application image, and the check that the device
    /// returns.
    ///
    /// Since: Odyssey Desktop 0.6.4.
    WriteApplication,
}

/// Order the stages of an update for one device.
///
/// When the net core has to be written the order is always net core, then
/// application, then read-back. The stage2 bootloader leaves the device in DFU
/// mode after a net core update, so the only way on is writing the application,
/// and nothing reports the net core while the device is in the bootloader: the
/// read-back needs the new application running and relaying the release.
/// Consequently `requires` is enforced against the net core the device already
/// runs, never against the one this update installs, and the read-back after the
/// application is written only detects a copy that failed silently. A device
/// left in that state keeps its old net core and reports it (or nothing), and
/// running the update again retries.
///
/// When nothing needs flashing the update is only the application. An
/// unreported net core counts as release 0, so it is replaced whenever a release
/// is published and never satisfies a `requires` above `0`.
///
/// # Errors
/// Returns [`NetCoreTooOld`] if the running net-core release is below the
/// application's required minimum.
/// Since: Odyssey Desktop 0.6.4.
pub fn plan_update_steps(
    running: u16,
    published: Option<u16>,
    requires: u16,
) -> Result<Vec<UpdateStep>, NetCoreTooOld> {
    if !plan_netcore(running, published, requires)? {
        return Ok(vec![UpdateStep::WriteApplication]);
    }
    Ok(vec![
        UpdateStep::FlashNetCore,
        UpdateStep::WriteApplication,
        UpdateStep::VerifyNetCore,
    ])
}

/// Open the DFU interface of the device with `uuid`, in its bootloader, at alt `alt`.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found, opened, or claimed, or
/// if the DFU interface at `alt` cannot be opened.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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

/// Send DFU detach to a device running its application. It resets and
/// re-enumerates in its bootloader.
///
/// An immediate reset can interrupt the control transfer. Transfer errors count as
/// success only when the same device enumerates its DFU-mode interface within 10 s.
///
/// # Errors
/// Returns [`DfuError`] if the device cannot be found, opened, or claimed, if its
/// DFU interface cannot be opened, or if detach fails without subsequent DFU-mode
/// enumeration.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn send_dfu_detach(uuid: &[u8; 6], vid: u16, pid: u16) -> Result<(), DfuError> {
    let info = find_device_by_uuid(uuid, vid, pid)?;
    let device = info.open().wait()?;

    let interface_num = find_dfu_interface(&device)?;
    let interface = device.claim_interface(interface_num).wait()?;

    // The alt setting does not matter for detach.
    let dfu = DfuNusb::open(device, interface, 0)?;
    let dfu_async = dfu.into_async_dfu();

    // A WILL_DETACH device resets at once and the control transfer stalls.
    match dfu_async.detach().await {
        Ok(()) => {
            // The device did not reset by itself. A USB reset starts the bootloader.
            let _ = dfu_async.usb_reset().await;
        }
        Err(e) => {
            let reset_interrupted_transfer = matches!(
                &e,
                dfu_nusb::Error::Transfer(
                    nusb::transfer::TransferError::Stall
                        | nusb::transfer::TransferError::Fault
                        | nusb::transfer::TransferError::Disconnected
                )
            );
            if !reset_interrupted_transfer
                || wait_for_dfu_device(uuid, vid, pid, Duration::from_secs(10))
                    .await
                    .is_err()
            {
                return Err(e.into());
            }
            tracing::debug!("DFU bootloader enumerated after interrupted detach transfer: {e}");
        }
    }

    Ok(())
}

/// Flash `firmware` into `slot` (`APP_A` or `APP_B`, written through alt setting `alt`)
/// of the device with `uuid`, which must already be in its bootloader. Progress is
/// logged every 10%.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found, opened, or claimed, or
/// if the DFU transfer fails.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn flash_firmware(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    slot: &str,
    alt: u8,
    firmware: &[u8],
) -> Result<(), DfuError> {
    flash_firmware_with_progress(uuid, vid, pid, slot, alt, firmware, |_, _| {}).await
}

/// Reader that logs download progress every 10% and reports each read to `callback`.
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
                let percent = (self.bytes_read * 100)
                    .checked_div(self.total_size)
                    .unwrap_or(100);

                let bytes_read = self.bytes_read;
                let total_size = self.total_size;

                if percent >= self.last_logged_percent + 10 || percent == 100 {
                    tracing::info!(
                        "DFU progress: {}/{} bytes ({}%)",
                        bytes_read,
                        total_size,
                        percent
                    );
                    self.last_logged_percent = (percent / 10) * 10;
                }

                if let Some(ref mut cb) = self.callback {
                    cb(bytes_read, total_size);
                }

                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }
}

/// [`flash_firmware`], also calling `progress_callback` with (bytes written, total
/// bytes) during the download.
///
/// # Errors
/// Returns [`DfuError`] if the bootloader cannot be found, opened, or claimed, or
/// if the DFU transfer fails.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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

    let total_size = firmware.len();

    tracing::info!(
        "Flashing {} bytes to slot {} (alt {}), transfer_size={}",
        total_size,
        slot,
        alt,
        transfer_size
    );

    let cursor = ProgressReader::new(Cursor::new(firmware), total_size, Some(progress_callback));
    dfu_download::download(&dfu, cursor, total_size as u32).await?;

    Ok(())
}

/// The bootloader is not (yet) enumerated in DFU mode.
struct NotInDfuMode;

/// Whether the device with `uuid` currently shows its DFU-mode interface (protocol 0x02,
/// not the application's runtime 0x01).
fn is_in_dfu_mode(uuid: [u8; 6], vid: u16, pid: u16) -> bool {
    let Ok(info) = find_bootloader_by_uuid(&uuid, vid, pid) else {
        return false;
    };
    tracing::debug!("wait_for_dfu_device: found device, checking DFU mode");
    let Ok(device) = info.open().wait() else {
        return false;
    };
    let Ok(interface_num) = find_dfu_interface(&device) else {
        return false;
    };
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
                    return true;
                }
            }
        }
    }
    false
}

/// Poll every 100 ms until the device with `uuid` shows its DFU-mode interface (protocol
/// 0x02, not the application's runtime 0x01).
///
/// # Errors
/// Returns [`DfuError::DeviceNotFound`] once `timeout` has passed without the
/// device exposing its DFU-mode interface.
///
/// # Since
/// Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn wait_for_dfu_device(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    timeout: Duration,
) -> Result<(), DfuError> {
    tracing::debug!(
        "wait_for_dfu_device: starting poll for {:04x}:{:04x}",
        vid,
        pid
    );

    let poll = (|| async {
        if is_in_dfu_mode(*uuid, vid, pid) {
            Ok(())
        } else {
            Err(NotInDfuMode)
        }
    })
    .retry(
        ConstantBuilder::default()
            .with_delay(Duration::from_millis(100))
            .without_max_times(),
    );
    if let Ok(Ok(())) = tokio::time::timeout(timeout, poll).await {
        Ok(())
    } else {
        tracing::debug!("wait_for_dfu_device: timeout after {timeout:?}");
        Err(DfuError::DeviceNotFound)
    }
}

/// Update a device that runs its application.
///
/// Detach it into DFU mode, wait up to 10 seconds for bootloader enumeration, flash
/// the exposed slot, then allow it to reset into the new firmware.
///
/// # Errors
/// Returns [`DfuError`] if detach, DFU enumeration, slot selection, or flashing
/// fails.
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub async fn update_firmware(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    firmware: &[u8],
) -> Result<(), DfuError> {
    tracing::info!("Starting firmware update for {:04x}:{:04x}", vid, pid);

    tracing::info!("Sending DFU detach...");
    send_dfu_detach(uuid, vid, pid).await?;

    tracing::info!("Waiting for device to enter DFU mode...");
    wait_for_dfu_device(uuid, vid, pid, Duration::from_secs(10)).await?;

    // The bootloader only identifies itself while it is the thing enumerating,
    // so this is the one point in the update where its release is readable.
    match read_bootloader_release_for_device(uuid, vid, pid).await {
        Ok(release) => tracing::info!("Bootloader reports {:?}", release),
        Err(e) => tracing::warn!("Could not read the bootloader release: {e}"),
    }

    let (slot_name, _slot_file, alt) = determine_target_slot_for_device(uuid, vid, pid)?;
    tracing::info!("Target slot: {} (alt {})", slot_name, alt);

    tracing::info!("Flashing firmware...");
    flash_firmware(uuid, vid, pid, slot_name, alt, firmware).await?;

    tracing::info!("Firmware update complete!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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
        assert_eq!(
            parse_version("1.2.3-nightly.20261111.t201500.gabcdef012345"),
            Some([1, 2, 3])
        );
        assert_eq!(
            parse_version("4294967295.4294967295.4294967295"),
            Some([u32::MAX; 3])
        );
        assert_eq!(parse_version("4294967296.0.0"), None);
        assert_eq!(parse_version("0.4294967296.0"), None);
        assert_eq!(parse_version("0.0.4294967296"), None);
        assert_eq!(parse_version("0.2"), None);
        assert_eq!(parse_version("0.2.0.1"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("0.2.x"), None);
    }

    #[test]
    fn nightly_comparison_uses_semver_precedence() {
        let nightly = FirmwareEntry {
            version: "1.2.4-nightly.20261111.t201500.gabcdef".into(),
            requires_bootloader: semver::Version::new(0, 0, 0),
            requires_netcore: 0,
            min_desktop: "0.0.0".into(),
            files: HashMap::new(),
            released: None,
        };
        assert!(is_worth_installing(&nightly, [1, 2, 3], None));
        assert!(!is_worth_installing(&nightly, [1, 2, 4], None));
    }
    #[test]
    fn legacy_bootloader_marker_maps_only_reserved_counters() {
        for counter in 1..=5 {
            assert_eq!(
                legacy_bootloader_semver(0xF000 | counter),
                Some(semver::Version::new(0, 0, u64::from(counter)))
            );
        }
        assert_eq!(legacy_bootloader_semver(0xF000), None);
        assert_eq!(legacy_bootloader_semver(0xF006), None);
    }

    #[test]
    fn bootloader_plan_uses_semver_precedence_without_downgrade() {
        let v = |v| semver::Version::parse(v).unwrap();
        let published = v("1.2.0");
        assert_eq!(
            plan_bootloader(None, Some(&published), &v("0.0.0")),
            Ok(true)
        );
        assert_eq!(
            plan_bootloader(Some(&v("1.1.9")), Some(&published), &v("0.0.0")),
            Ok(true)
        );
        assert_eq!(
            plan_bootloader(Some(&v("1.2.0")), Some(&published), &v("0.0.0")),
            Ok(false)
        );
        assert_eq!(
            plan_bootloader(Some(&v("2.0.0")), Some(&published), &v("0.0.0")),
            Ok(false)
        );
        assert_eq!(plan_bootloader(None, None, &v("0.0.0")), Ok(false));
        assert_eq!(
            plan_bootloader(Some(&v("1.2.0-alpha.1")), None, &v("1.2.0")),
            Err(BootloaderTooOld {
                required: v("1.2.0")
            })
        );
        assert_eq!(
            plan_bootloader(
                Some(&v("1.2.0+build.1")),
                Some(&v("1.2.0+build.2")),
                &v("1.2.0+floor")
            ),
            Ok(false)
        );
    }

    #[test]
    fn bootloader_readback_requires_exact_semver() {
        let expected = semver::Version::parse("1.2.0").unwrap();
        assert!(bootloader_took_update(Some(&expected), &expected));
        assert!(!bootloader_took_update(None, &expected));
        assert!(!bootloader_took_update(
            Some(&semver::Version::parse("1.2.1").unwrap()),
            &expected,
        ));
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
    fn netcore_requirement_is_judged_against_the_running_release() {
        // Running at the floor (boundary) or above it.
        assert_eq!(plan_netcore(2, None, 2), Ok(false));
        assert_eq!(plan_netcore(2, Some(3), 2), Ok(true));
        assert_eq!(plan_netcore(4, Some(3), 2), Ok(false));
        // Running below it.
        assert_eq!(plan_netcore(1, None, 2), Err(NetCoreTooOld { required: 2 }));
        assert_eq!(plan_netcore(0, None, 1), Err(NetCoreTooOld { required: 1 }));
        // The published release never satisfies the entry's own floor.
        assert_eq!(
            plan_netcore(0, Some(2), 2),
            Err(NetCoreTooOld { required: 2 })
        );
        assert_eq!(
            plan_netcore(1, Some(3), 2),
            Err(NetCoreTooOld { required: 2 })
        );
        assert_eq!(
            plan_netcore(1, Some(1), 2),
            Err(NetCoreTooOld { required: 2 })
        );
        // Unknown (0) never satisfies a floor above 0, and always satisfies 0.
        assert_eq!(
            plan_netcore(0, Some(5), 1),
            Err(NetCoreTooOld { required: 1 })
        );
        assert_eq!(plan_netcore(0, Some(5), 0), Ok(true));
    }

    #[test]
    fn only_the_published_netcore_counts_as_written() {
        assert!(netcore_took_update(2, 2));
        assert!(!netcore_took_update(1, 2));
        assert!(!netcore_took_update(3, 2));
        assert!(!netcore_took_update(0, 2));
    }

    #[test]
    fn net_core_is_written_before_the_application_and_read_back_after_it() {
        use UpdateStep::{FlashNetCore, VerifyNetCore, WriteApplication};
        let flash = Ok(vec![FlashNetCore, WriteApplication, VerifyNetCore]);
        // Superseded, and unknown (0), both replace the net core.
        assert_eq!(plan_update_steps(1, Some(2), 0), flash);
        assert_eq!(plan_update_steps(0, Some(2), 0), flash);
    }

    #[test]
    fn current_or_unpublished_net_core_leaves_only_the_application() {
        use UpdateStep::WriteApplication;
        assert_eq!(plan_update_steps(2, Some(2), 0), Ok(vec![WriteApplication]));
        assert_eq!(plan_update_steps(3, Some(2), 0), Ok(vec![WriteApplication]));
        assert_eq!(plan_update_steps(0, None, 0), Ok(vec![WriteApplication]));
    }

    #[test]
    fn net_core_floor_refuses_before_any_step_runs() {
        use UpdateStep::{FlashNetCore, VerifyNetCore, WriteApplication};
        // Already at the floor, with and without a newer net core to write.
        assert_eq!(plan_update_steps(3, None, 3), Ok(vec![WriteApplication]));
        assert_eq!(
            plan_update_steps(2, Some(3), 2),
            Ok(vec![FlashNetCore, WriteApplication, VerifyNetCore])
        );
        // Flashing the published release does not reach the floor.
        assert_eq!(
            plan_update_steps(1, Some(3), 3),
            Err(NetCoreTooOld { required: 3 })
        );
        assert_eq!(
            plan_update_steps(1, Some(2), 3),
            Err(NetCoreTooOld { required: 3 })
        );
        // An unknown net core never meets a floor on its own.
        assert_eq!(
            plan_update_steps(0, None, 1),
            Err(NetCoreTooOld { required: 1 })
        );
        assert_eq!(
            plan_update_steps(0, Some(1), 1),
            Err(NetCoreTooOld { required: 1 })
        );
    }

    #[test]
    fn bootloader_requirement_is_a_semver_floor() {
        let v = |v| semver::Version::parse(v).unwrap();
        assert_eq!(
            plan_bootloader(None, Some(&v("1.2.0")), &v("1.2.0")),
            Ok(true)
        );
        assert_eq!(
            plan_bootloader(Some(&v("1.2.0")), None, &v("1.2.0")),
            Ok(false)
        );
        assert_eq!(
            plan_bootloader(Some(&v("1.1.9")), Some(&v("1.1.9")), &v("1.2.0")),
            Err(BootloaderTooOld {
                required: v("1.2.0")
            })
        );
        assert_eq!(plan_bootloader(None, None, &v("0.0.0")), Ok(false));
    }

    const NIGHTLY: &str = include_str!("../../../firmware/manifests/releases.nightly.json");
    const STABLE: &str = include_str!("../../../firmware/manifests/releases.stable.json");

    /// Every host parses the manifest whole or not at all, so one entry missing
    /// a required field would hide every update from every device.
    #[test]
    fn committed_manifests_parse() {
        for (name, text) in [("nightly", NIGHTLY), ("stable", STABLE)] {
            let manifest: FirmwareManifest =
                serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} parses: {e}"));
            assert!(manifest.version <= SUPPORTED_MANIFEST_VERSION);
            for (device, entries) in &manifest.devices {
                assert!(
                    !entries.firmwares.is_empty(),
                    "{name} {device} has no firmware"
                );
                let mut previous: Option<[u32; 3]> = None;
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
                        assert!(
                            version_newer(version, newer),
                            "{name} {device} is not newest-first"
                        );
                    }
                    previous = Some(version);
                }
            }
        }
    }

    /// Stable is promoted from nightly by copying, so it can never hold an entry
    /// nightly does not, or one that differs from it.
    #[test]
    fn stable_only_holds_promoted_nightly_entries() {
        let nightly: serde_json::Value = serde_json::from_str(NIGHTLY).unwrap();
        let stable: serde_json::Value = serde_json::from_str(STABLE).unwrap();
        for (device, entries) in stable["devices"].as_object().unwrap() {
            for entry in entries["firmwares"].as_array().unwrap() {
                let in_nightly = nightly["devices"][device]["firmwares"]
                    .as_array()
                    .is_some_and(|all| all.contains(entry));
                assert!(
                    in_nightly,
                    "stable {device} {} is not a nightly entry",
                    entry["version"]
                );
            }
        }
    }

    fn manifest_with(entries: &[(&str, &str)]) -> FirmwareManifest {
        let firmwares: Vec<_> = entries
            .iter()
            .map(|(version, min_desktop)| {
                serde_json::json!({
                    "version": version,
                    "requires_bootloader": "0.0.0",
                    "requires_netcore": 0,
                    "min_desktop": min_desktop,
                    "files": {},
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": 2,
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
            selected(select_firmware(
                &manifest,
                "atslite1",
                &semver::Version::new(0, 7, 1),
                0
            )),
            (Some("0.2.0"), Some("0.3.0"))
        );
        // Exactly at the floor counts as meeting it.
        assert_eq!(
            selected(select_firmware(
                &manifest,
                "atslite1",
                &semver::Version::new(0, 8, 0),
                0
            )),
            (Some("0.3.0"), None)
        );
        // Older than every floor: nothing installable, but say what exists.
        assert_eq!(
            selected(select_firmware(
                &manifest,
                "atslite1",
                &semver::Version::new(0, 5, 0),
                0
            )),
            (None, Some("0.3.0"))
        );
        // Unknown device.
        assert_eq!(
            selected(select_firmware(
                &manifest,
                "nope",
                &semver::Version::new(9, 9, 9),
                0
            )),
            (None, None)
        );
    }
    #[test]
    fn nightly_desktop_is_below_matching_stable_firmware_floor() {
        let manifest = manifest_with(&[("0.3.0", "0.7.6")]);
        let nightly = semver::Version::parse("0.7.6-nightly.20260619.t020000.gabcdef").unwrap();
        assert_eq!(
            selected(select_firmware(&manifest, "atslite1", &nightly, 0)),
            (None, Some("0.3.0"))
        );
        assert_eq!(
            selected(select_firmware(
                &manifest,
                "atslite1",
                &semver::Version::new(0, 7, 6),
                0
            )),
            (Some("0.3.0"), None)
        );
    }

    /// Entries are `(version, requires_netcore, net-core release carried)`,
    /// newest first, all installable by any desktop.
    fn netcore_manifest(entries: &[(&str, u16, Option<u16>)]) -> FirmwareManifest {
        let firmwares: Vec<_> = entries
            .iter()
            .map(|(version, requires, carries)| {
                let files = match carries {
                    Some(release) => serde_json::json!({
                        "net-core": { "sha256": "00", "size": 1, "release": release },
                    }),
                    None => serde_json::json!({}),
                };
                serde_json::json!({
                    "version": version,
                    "requires_bootloader": "0.0.0",
                    "requires_netcore": requires,
                    "min_desktop": "0.6.0",
                    "files": files,
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": 2,
            "devices": { "atslite1": { "firmwares": firmwares } },
        }))
        .unwrap()
    }

    fn netcore_selected(manifest: &FirmwareManifest, running: u16) -> (Option<&str>, Option<&str>) {
        let offer = select_firmware(
            manifest,
            "atslite1",
            &semver::Version::new(1, 0, 0),
            running,
        );
        (
            offer.offered.map(|e| e.version.as_str()),
            offer.net_core_blocked.map(|e| e.version.as_str()),
        )
    }

    #[test]
    fn net_core_bump_ships_through_a_bridge_entry() {
        let manifest = netcore_manifest(&[
            ("0.3.0", 2, Some(2)),
            ("0.2.1", 0, Some(2)),
            ("0.2.0", 0, Some(1)),
        ]);
        // Units below the new net core reach the bridge, and are told why not 0.3.0.
        for running in [0, 1] {
            assert_eq!(
                netcore_selected(&manifest, running),
                (Some("0.2.1"), Some("0.3.0")),
                "running {running}"
            );
        }
        // Once the bridge has run, the later entry opens up.
        assert_eq!(netcore_selected(&manifest, 2), (Some("0.3.0"), None));
        assert_eq!(netcore_selected(&manifest, 3), (Some("0.3.0"), None));
    }

    #[test]
    fn an_entry_never_satisfies_its_own_net_core_floor() {
        // The only entry carries the release it requires, so a unit below it has no path.
        let manifest = netcore_manifest(&[("0.3.0", 2, Some(2))]);
        assert_eq!(netcore_selected(&manifest, 1), (None, Some("0.3.0")));
        assert_eq!(netcore_selected(&manifest, 2), (Some("0.3.0"), None));
    }

    #[test]
    fn unknown_net_core_never_meets_a_floor_above_zero() {
        let manifest = netcore_manifest(&[("0.2.0", 1, Some(1)), ("0.1.0", 0, None)]);
        assert_eq!(
            netcore_selected(&manifest, 0),
            (Some("0.1.0"), Some("0.2.0"))
        );
        // Boundary: exactly the floor is enough.
        assert_eq!(netcore_selected(&manifest, 1), (Some("0.2.0"), None));
    }

    #[test]
    fn desktop_hold_takes_precedence_over_the_net_core_floor() {
        let mut manifest = netcore_manifest(&[("0.2.0", 1, Some(1)), ("0.1.0", 0, None)]);
        manifest.devices.get_mut("atslite1").unwrap().firmwares[0].min_desktop = "9.0.0".into();
        let offer = select_firmware(&manifest, "atslite1", &semver::Version::new(1, 0, 0), 0);
        assert_eq!(offer.offered.map(|e| e.version.as_str()), Some("0.1.0"));
        assert_eq!(offer.held_back.map(|e| e.version.as_str()), Some("0.2.0"));
        assert!(offer.net_core_blocked.is_none());
    }

    /// Walk a unit up through a manifest the way the desktop does: take the
    /// newest entry its running net core allows, skip it unless installing it
    /// is worth a click, and let the write raise the net core to the release
    /// that entry carries. Returns the version the unit ends on. The
    /// `min_desktop` axis is separate, so the desktop here is always new enough.
    fn walk(
        manifest: &FirmwareManifest,
        device: &str,
        mut app: [u32; 3],
        mut running: u16,
    ) -> [u32; 3] {
        for _ in 0..=2 * manifest.devices[device].firmwares.len() {
            let Some(entry) = select_firmware(
                manifest,
                device,
                &semver::Version::new(u64::MAX, u64::MAX, u64::MAX),
                running,
            )
            .offered
            else {
                break;
            };
            if !is_worth_installing(entry, app, Some(running)) {
                break;
            }
            app = entry.version_parts().unwrap();
            if let Some(release) = entry.files.get("net-core").and_then(|f| f.release) {
                running = running.max(release);
            }
        }
        app
    }

    /// A floor is only real if units can reach it. Selection is greedy, so an
    /// entry that carries a net core is useless as a bridge when a newer entry
    /// is installable without it: units take the newer one and never pick it up.
    /// Checking that older entries carry a release is not enough. Walk every
    /// starting point a unit can have and require it to arrive at the newest.
    fn unreachable_starts(manifest: &FirmwareManifest) -> Vec<String> {
        let mut stuck = Vec::new();
        for (device, entries) in &manifest.devices {
            let newest = entries.firmwares[0].version_parts().unwrap();
            let mut starts = vec![([0, 0, 0], 0u16)];
            for entry in &entries.firmwares {
                if let (Some(version), Some(release)) = (
                    entry.version_parts(),
                    entry.files.get("net-core").and_then(|f| f.release),
                ) {
                    // A unit that took the application but whose net-core copy
                    // failed sits at this version with any lower release.
                    for running in 0..=release {
                        starts.push((version, running));
                    }
                }
            }
            for (app, running) in starts {
                let ends = walk(manifest, device, app, running);
                if ends != newest {
                    stuck.push(format!(
                        "{device}: a unit at {app:?} with net core {running} ends on {ends:?}, not {newest:?}"
                    ));
                }
            }
        }
        stuck
    }

    #[test]
    fn committed_manifests_let_every_unit_reach_the_newest_entry() {
        for (name, text) in [("nightly", NIGHTLY), ("stable", STABLE)] {
            let manifest: FirmwareManifest = serde_json::from_str(text).unwrap();
            let stuck = unreachable_starts(&manifest);
            assert_eq!(stuck, Vec::<String>::new(), "{name}: {stuck:?}");
        }
    }

    #[test]
    fn a_proper_bridge_reaches_the_newest_entry_from_every_start() {
        let manifest = netcore_manifest(&[
            ("0.3.0", 2, Some(2)),
            ("0.2.1", 0, Some(2)),
            ("0.2.0", 0, Some(1)),
        ]);
        assert_eq!(unreachable_starts(&manifest), Vec::<String>::new());
    }

    /// The bridge below is older than an entry that needs no net core, so units
    /// pick the newer one, never receive the release, and cannot take 0.3.0.
    #[test]
    fn a_bridge_that_a_newer_entry_overtakes_leaves_units_stranded() {
        let manifest = netcore_manifest(&[
            ("0.3.0", 1, None),
            ("0.2.0", 0, None),
            ("0.1.0", 0, Some(1)),
        ]);
        assert_eq!(walk(&manifest, "atslite1", [0, 0, 0], 0), [0, 2, 0]);
        assert_ne!(unreachable_starts(&manifest), Vec::<String>::new());
    }

    /// The application write went through but the net-core copy did not, so the
    /// unit sits on the bridge with a stale net core, below the next floor. The
    /// only way forward is to install the bridge again.
    #[test]
    fn a_failed_net_core_copy_is_repaired_by_reinstalling_the_same_version() {
        let manifest = netcore_manifest(&[
            ("0.3.0", 2, Some(2)),
            ("0.2.1", 0, Some(2)),
            ("0.2.0", 0, Some(1)),
        ]);
        assert_eq!(walk(&manifest, "atslite1", [0, 2, 1], 1), [0, 3, 0]);
        assert_eq!(walk(&manifest, "atslite1", [0, 2, 1], 0), [0, 3, 0]);
    }

    #[test]
    fn a_net_core_reading_only_counts_once_it_has_settled() {
        // A 0 is a real answer once settled, and "not answered yet" before.
        assert_eq!(known_netcore_release(true, 0), Some(0));
        assert_eq!(known_netcore_release(true, 2), Some(2));
        assert_eq!(known_netcore_release(false, 0), None);
    }

    #[test]
    fn the_same_version_is_only_offered_as_a_repair() {
        let manifest = netcore_manifest(&[("0.2.1", 0, Some(2)), ("0.2.0", 0, None)]);
        let bridge = &manifest.devices["atslite1"].firmwares[0];
        let plain = &manifest.devices["atslite1"].firmwares[1];
        // Net core behind what the entry carries: repair.
        assert!(is_worth_installing(bridge, [0, 2, 1], Some(1)));
        assert!(is_worth_installing(bridge, [0, 2, 1], Some(0)));
        // Net core already there or ahead: nothing to do.
        assert!(!is_worth_installing(bridge, [0, 2, 1], Some(2)));
        assert!(!is_worth_installing(bridge, [0, 2, 1], Some(3)));
        // A net core that cannot be read (Bluetooth only) is not evidence of a failed copy.
        assert!(!is_worth_installing(bridge, [0, 2, 1], None));
        // An entry that carries no net core never repairs anything.
        assert!(!is_worth_installing(plain, [0, 2, 0], Some(0)));
        // Newer always counts, readable net core or not. Older never does.
        assert!(is_worth_installing(bridge, [0, 2, 0], Some(2)));
        assert!(is_worth_installing(bridge, [0, 2, 0], None));
        assert!(!is_worth_installing(plain, [0, 2, 1], Some(0)));
    }

    #[test]
    fn channel_names_round_trip_and_unknown_falls_back_to_stable() {
        for channel in [FirmwareChannel::Stable, FirmwareChannel::Nightly] {
            assert_eq!(FirmwareChannel::from_name(channel.name()), channel);
        }
        assert_eq!(FirmwareChannel::from_name(""), FirmwareChannel::Stable);
        assert_eq!(FirmwareChannel::from_name("beta"), FirmwareChannel::Stable);
    }

    #[test]
    fn a_channel_saved_as_alpha_reads_as_nightly() {
        assert_eq!(
            FirmwareChannel::from_name("alpha"),
            FirmwareChannel::Nightly
        );
    }

    #[test]
    fn unreadable_min_desktop_is_never_offered() {
        let manifest = manifest_with(&[("0.2.0", "soon"), ("0.1.0", "0.6.0")]);
        assert_eq!(
            selected(select_firmware(
                &manifest,
                "atslite1",
                &semver::Version::new(1, 0, 0),
                0
            )),
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
        let legacy = semver::Version::new(0, 0, 3);
        assert_eq!(
            ReleaseSource::Bootloader { version: &legacy }.tag("legacy-atslite1"),
            "legacy-atslite1-bootloader-v3"
        );
        let modern = semver::Version::new(1, 2, 3);
        assert_eq!(
            ReleaseSource::Bootloader { version: &modern }.tag("legacy-atslite1"),
            "legacy-atslite1-bootloader-v1.2.3"
        );
        assert_eq!(
            ReleaseSource::NetCore { release: 2 }.tag("atslite1"),
            "atslite1-netcore-v2"
        );
    }

    #[test]
    fn single_bank_label_never_selects_a_dual_bank_image() {
        for (descriptor, slot, matches) in [
            ("APP", "APP", true),
            ("APP_A", "APP", false),
            ("APP_B", "APP", false),
            ("@APP/0x11000", "APP", true),
            ("APP_A", "APP_A", true),
            ("APP_B", "APP_A", false),
            ("BOOTLOADER 1", "BOOTLOADER", true),
        ] {
            assert_eq!(
                slot_label_matches(descriptor, slot),
                matches,
                "{descriptor:?} {slot:?}"
            );
        }
    }

    #[test]
    fn application_layout_rejects_missing_mixed_and_single_core_auxiliary_images() {
        for (names, required_netcore, expected) in [
            (vec![], 0, None),
            (vec!["app"], 0, Some(vec!["app"])),
            (vec!["app-a"], 0, None),
            (vec!["app-b"], 0, None),
            (vec!["app-a", "app-b"], 0, Some(vec!["app-a", "app-b"])),
            (vec!["app", "app-a"], 0, None),
            (vec!["app", "app-b"], 0, None),
            (vec!["app", "app-a", "app-b"], 0, None),
            (vec!["app", "net-core"], 0, None),
            (vec!["app", "stage2-bootloader"], 0, None),
            (vec!["app"], 1, None),
        ] {
            let mut entry = FirmwareEntry {
                version: "0.1.0".into(),
                requires_bootloader: semver::Version::new(0, 0, 1),
                requires_netcore: required_netcore,
                min_desktop: "0.7.3".into(),
                files: HashMap::new(),
                released: None,
            };
            for name in &names {
                entry.files.insert(
                    (*name).into(),
                    FileEntry {
                        sha256: "unused".into(),
                        size: 1,
                        version: None,
                        release: None,
                    },
                );
            }
            assert_eq!(
                entry
                    .application_files()
                    .map(|files| files.map(|(name, _)| name).collect::<Vec<_>>()),
                expected,
                "{names:?} requires_netcore={required_netcore}",
            );
        }
    }
}
