//! Radio OTA session shared by every device that stages images over a link.
//!
//! [`send`] runs the whole update against an [`OtaTransport`]: check the
//! bootloader and net-core floors, stage the net core (when it has to change)
//! and the application, commit, then wait for the unit to come back reporting
//! the new versions. Frame and status layouts are in `odyssey_protocol::ota`;
//! the device side is `better_ota`'s receiver.
//!
//! The session is identical for every device. Only the way a frame reaches the
//! unit differs, and that is what each [`OtaTransport`] supplies:
//!
//! - ATS Lite: `RadioOtaTransport` in odyssey-desktop's
//!   `firmware_update/update_task.rs`. Frames go through the hub's
//!   `exchange_vendor` RPC and out over the device's selected packet link, BLE
//!   through a mux dongle. Chunking is [`LinkMtu::Vendor`].
//! - Marker control board: `MarkerOtaTransport` in `accessories/ota.rs`. Frames
//!   go to the marker's OTA GATT characteristic, either on the host's own
//!   Bluetooth adapter or relayed by an Odyssey dongle (`accessories/transport.rs`
//!   picks which). Chunking is [`LinkMtu::Marker`].
//!
//! The dongle's own firmware never takes this path; it is updated over USB DFU.
//! Bootloaders never take it either: only an application and a net core are
//! sent, so a release that needs a newer bootloader than the unit is known to
//! run is refused up front and has to be installed over USB.

use odyssey_firmware_manifest::{FileEntry, FirmwareEntry};
use odyssey_protocol::ota::{self, Code, Image, Request, Status};
use sha2::Digest as _;

const VENDOR_DATA_CHUNK: usize = 92;
const MARKER_DATA_CHUNK: usize = 12;

/// Data bytes per [`Request::Data`] frame. The frame adds a 5-byte header, so
/// the vendor chunk plus header stays within [`ota::MAX_FRAME`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMtu {
    Vendor,
    Marker,
}

impl LinkMtu {
    const fn data_chunk(self) -> usize {
        match self {
            Self::Vendor => VENDOR_DATA_CHUNK,
            Self::Marker => MARKER_DATA_CHUNK,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OtaError {
    #[error("OTA transport failed: {0}; recover this unit over USB")]
    Transport(String),
    #[error("device does not support radio OTA; update over USB first")]
    Unsupported,
    #[error("device reported OTA error {0}; recover this unit over USB")]
    Device(Code),
    #[error("invalid OTA status: {0}")]
    Status(Code),
    #[error("manifest image {0} is missing")]
    MissingImage(&'static str),
    #[error("manifest image {0} was not validated")]
    InvalidImage(&'static str),
    #[error("target bootloader floor {floor} exceeds known compatible floor {known}")]
    BootloaderFloor { floor: String, known: String },
    #[error(
        "ATS Lite image format is incompatible with installed bootloader; SWD reprovisioning required"
    )]
    IncompatibleBootloaderFormat,
    #[error("device staged bitmap does not include {0:?}")]
    NotStaged(Image),
    #[error("device did not acknowledge expected OTA offset {expected}; got {actual}")]
    Offset { expected: u32, actual: u32 },
    #[error("device reported unsupported image {0:?}")]
    ImageUnsupported(Image),
    #[error("device did not return expected application/net-core versions after reboot")]
    VersionMismatch,
    #[error("device only installs signed firmware but manifest image {0} has no signature")]
    MissingSignature(&'static str),
    #[error("manifest signature for {0} is not 128 hex characters")]
    InvalidSignature(&'static str),
    #[error("secure ATS Lite signature does not match release metadata")]
    SignatureRejected,
    #[error("secure ATS Lite device did not require signature verification")]
    SignatureModeMismatch,
}

/// Carries one OTA request frame to the device and returns its status reply.
///
/// The transport owns the link, including the reconnect after the commit
/// reboots the unit. It reports failures as text; [`send`] wraps them in
/// [`OtaError::Transport`].
pub trait OtaTransport {
    fn exchange(
        &mut self,
        frame: &[u8],
    ) -> impl Future<Output = Result<[u8; ota::STATUS_LEN], String>>;
    fn verify_reconnected(
        &mut self,
        app_version: [u32; 3],
        netcore_release: u16,
    ) -> impl Future<Output = Result<bool, String>>;
}

/// An image that [`send`] re-checks against its manifest size and SHA-256
/// before transferring a byte.
#[derive(Clone, Copy, Debug)]
pub struct VerifiedImage<'a> {
    pub bytes: &'a [u8],
    pub manifest: &'a FileEntry,
}

/// The images a release provides, plus what the host knows about the unit.
///
/// Which application field is used depends on the bank the device reports:
/// `application` for a single image, `app_a`/`app_b` for dual-bank units.
#[derive(Clone, Copy, Debug)]
pub struct Images<'a> {
    pub application: Option<VerifiedImage<'a>>,
    pub app_a: Option<VerifiedImage<'a>>,
    pub app_b: Option<VerifiedImage<'a>>,
    pub netcore: Option<VerifiedImage<'a>>,
    pub app_version: [u32; 3],
    /// Bootloader version the unit is known to run. A release whose
    /// `requires_bootloader` is above it is refused before any radio traffic.
    /// ATS Lite reports it over the control channel (`Device::bootloader_version`); see
    /// `RADIO_OTA_MIN_BOOTLOADER` in odyssey-desktop's
    /// `firmware_update/update_manager.rs` for why. The marker has one
    /// bootloader version; see `accessories/ota.rs`.
    pub known_bootloader_floor: &'a semver::Version,
}

async fn exchange<T: OtaTransport>(
    transport: &mut T,
    request: Request<'_>,
) -> Result<Status, OtaError> {
    let mut frame = [0; ota::MAX_FRAME];
    let length = request.encode(&mut frame).map_err(OtaError::Status)?;
    let reply = transport
        .exchange(&frame[..length])
        .await
        .map_err(OtaError::Transport)?;
    let status = Status::decode(&reply).map_err(OtaError::Status)?;
    if status.error != Code::Ok {
        return Err(OtaError::Device(status.error));
    }
    Ok(status)
}

/// Builds the signature record for `file` when the device requires signatures: `version` as
/// three little-endian words, then the signature. `version` is what the signature binds the
/// image to: the application semver, or `[release, 0, 0]` for the net core. Devices that do
/// not require signatures never get the frames, so older firmware that does not know the
/// opcode is not sent it.
fn signature_record(
    status: &Status,
    name: &'static str,
    file: &FileEntry,
    version: [u32; 3],
) -> Result<Option<[u8; ota::SIGNATURE_RECORD_LEN]>, OtaError> {
    if !status.signature_required {
        return Ok(None);
    }
    let hex = file
        .signature
        .as_deref()
        .ok_or(OtaError::MissingSignature(name))?;
    let signature = hex::decode(hex)
        .ok()
        .and_then(|bytes| <[u8; ota::SIGNATURE_LEN]>::try_from(bytes).ok())
        .ok_or(OtaError::InvalidSignature(name))?;
    let mut record = [0; ota::SIGNATURE_RECORD_LEN];
    for (index, word) in version.into_iter().enumerate() {
        record[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    record[12..].copy_from_slice(&signature);
    Ok(Some(record))
}

fn ensure_image(image: VerifiedImage<'_>, name: &'static str) -> Result<(), OtaError> {
    if image.bytes.is_empty()
        || u64::try_from(image.bytes.len()).ok() != Some(image.manifest.size)
        || hex::encode(sha2::Sha256::digest(image.bytes)) != image.manifest.sha256
    {
        return Err(OtaError::InvalidImage(name));
    }
    Ok(())
}
fn verify_secure_signature(
    image: VerifiedImage<'_>,
    component: release_signing::Component,
    version: [u32; 3],
    name: &'static str,
) -> Result<(), OtaError> {
    let signature: [u8; release_signing::SIGNATURE_LEN] = image
        .manifest
        .signature
        .as_deref()
        .ok_or(OtaError::MissingSignature(name))
        .and_then(|signature| {
            hex::decode(signature)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(OtaError::InvalidSignature(name))
        })?;
    let size = u32::try_from(image.bytes.len()).map_err(|_| OtaError::InvalidImage(name))?;
    let hash: [u8; 32] = sha2::Sha256::digest(image.bytes).into();
    let message = release_signing::message(component, size, version, &hash);
    if release_signing::verify_production(&message, &signature) {
        Ok(())
    } else {
        Err(OtaError::SignatureRejected)
    }
}

async fn transfer<T: OtaTransport>(
    transport: &mut T,
    image: Image,
    bytes: &[u8],
    link: LinkMtu,
    signature: Option<[u8; ota::SIGNATURE_RECORD_LEN]>,
) -> Result<(), OtaError> {
    let size =
        u32::try_from(bytes.len()).map_err(|_conversion_error| OtaError::InvalidImage("image"))?;
    let status = exchange(
        transport,
        Request::Begin {
            image,
            size,
            crc32: ota::crc32(bytes),
        },
    )
    .await?;
    if status.receiving != Some(image) || status.offset != 0 {
        return Err(OtaError::Offset {
            expected: 0,
            actual: status.offset,
        });
    }

    let chunk_size = link.data_chunk();
    for (index, chunk) in bytes.chunks(chunk_size).enumerate() {
        let offset = index * chunk_size;
        let offset =
            u32::try_from(offset).map_err(|_conversion_error| OtaError::InvalidImage("image"))?;
        let status = exchange(
            transport,
            Request::Data {
                offset,
                data: chunk,
            },
        )
        .await?;
        let expected = offset
            .checked_add(
                u32::try_from(chunk.len())
                    .map_err(|_conversion_error| OtaError::InvalidImage("image"))?,
            )
            .ok_or(OtaError::InvalidImage("image"))?;
        let final_chunk_staged =
            expected == size && status.receiving.is_none() && status.staged & image.bit() != 0;
        if status.offset != expected && !final_chunk_staged {
            return Err(OtaError::Offset {
                expected,
                actual: status.offset,
            });
        }
    }

    let status = exchange(transport, Request::Query).await?;
    if status.staged & image.bit() == 0 {
        return Err(OtaError::NotStaged(image));
    }
    if let Some(record) = signature {
        for (index, bytes) in record.chunks(chunk_size).enumerate() {
            let offset = u8::try_from(index * chunk_size)
                .map_err(|_conversion_error| OtaError::InvalidImage("signature"))?;
            exchange(
                transport,
                Request::Signature {
                    image,
                    offset,
                    bytes,
                },
            )
            .await?;
        }
    }
    Ok(())
}
async fn transfer_safely<T: OtaTransport>(
    transport: &mut T,
    image: Image,
    bytes: &[u8],
    link: LinkMtu,
    signature: Option<[u8; ota::SIGNATURE_RECORD_LEN]>,
) -> Result<(), OtaError> {
    match transfer(transport, image, bytes, link, signature).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = exchange(transport, Request::Abort).await;
            Err(error)
        }
    }
}

/// Sends verified images through an acknowledged accessory or vendor transport.
///
/// Both images are staged before a single commit, and any failure before it
/// aborts the staged data, so the running firmware is untouched. The net core
/// is staged first, and only when the release carries a newer one than the
/// unit runs.
///
/// # Errors
///
/// Returns an [`OtaError`] when the transport fails, the device rejects or does not
/// support the update, an image is missing or fails validation, a transfer is not
/// acknowledged at the expected offset, or the device does not report the new versions
/// after reboot. A failed transfer is aborted on the device before the error returns.
pub async fn send<T: OtaTransport>(
    transport: &mut T,
    link: LinkMtu,
    entry: &FirmwareEntry,
    images: Images<'_>,
) -> Result<(), OtaError> {
    if link == LinkMtu::Vendor
        && !odyssey_firmware_manifest::ats_lite_target_compatible(
            "atslite1",
            entry,
            Some(images.known_bootloader_floor),
        )
    {
        return Err(OtaError::IncompatibleBootloaderFormat);
    }
    if entry.requires_bootloader > *images.known_bootloader_floor {
        return Err(OtaError::BootloaderFloor {
            floor: entry.requires_bootloader.to_string(),
            known: images.known_bootloader_floor.to_string(),
        });
    }
    let secure_format =
        link == LinkMtu::Vendor && entry.requires_bootloader >= semver::Version::new(2, 0, 0);
    if secure_format {
        let component = release_signing::Component::Lite1SecureApp;
        for image in [images.application, images.app_a, images.app_b]
            .into_iter()
            .flatten()
        {
            ensure_image(image, "application")?;
            verify_secure_signature(image, component, images.app_version, "application")?;
        }
        if let Some(netcore) = images.netcore {
            let release = netcore
                .manifest
                .release
                .ok_or(OtaError::InvalidImage("net-core"))?;
            ensure_image(netcore, "net-core")?;
            verify_secure_signature(
                netcore,
                release_signing::Component::Lite1SecureNetCore,
                [u32::from(release), 0, 0],
                "net-core",
            )?;
        }
    }

    let status = exchange(transport, Request::Query).await?;
    if status.supported == 0 {
        return Err(OtaError::Unsupported);
    }
    if secure_format && !status.signature_required {
        return Err(OtaError::SignatureModeMismatch);
    }
    let (slot_name, application) = match status.app_slot {
        0 => ("app", images.application),
        1 => ("app-a", images.app_a),
        2 => ("app-b", images.app_b),
        _ => return Err(OtaError::Status(Code::State)),
    };
    let app_image = Image::Application;
    let application = application.ok_or(OtaError::MissingImage(slot_name))?;
    ensure_image(application, "application")?;
    if status.supported & app_image.bit() == 0 {
        return Err(OtaError::ImageUnsupported(app_image));
    }
    let app_signature = signature_record(
        &status,
        "application",
        application.manifest,
        images.app_version,
    )?;

    if let Some(netcore) = images.netcore {
        ensure_image(netcore, "net-core")?;
    }
    // Unlike `plan_update_steps` (DFU), a net core this release carries counts
    // toward `requires_netcore` here; the reconnect check reads it back.
    let published_netcore = images.netcore.and_then(|image| image.manifest.release);
    let netcore_floor = entry.requires_netcore;
    if status.netcore_release < netcore_floor
        && published_netcore.is_none_or(|release| release < netcore_floor)
    {
        return Err(OtaError::MissingImage("net-core meeting required floor"));
    }
    let netcore_target = published_netcore.unwrap_or(status.netcore_release);
    let needs_netcore = netcore_target > status.netcore_release && netcore_target >= netcore_floor;
    // The device keeps its session across BLE drops; a stale one answers Begin with Busy.
    if status.receiving.is_some() || status.staged != 0 {
        exchange(transport, Request::Abort).await?;
    }
    if needs_netcore {
        if status.supported & Image::NetCore.bit() == 0 {
            return Err(OtaError::ImageUnsupported(Image::NetCore));
        }
        let netcore = images.netcore.ok_or(OtaError::MissingImage("net-core"))?;
        let netcore_signature = signature_record(
            &status,
            "net-core",
            netcore.manifest,
            [u32::from(netcore_target), 0, 0],
        )?;
        transfer_safely(
            transport,
            Image::NetCore,
            netcore.bytes,
            link,
            netcore_signature,
        )
        .await?;
    }
    transfer_safely(transport, app_image, application.bytes, link, app_signature).await?;
    let status = match exchange(transport, Request::Query).await {
        Ok(status) => status,
        Err(error) => {
            let _ = exchange(transport, Request::Abort).await;
            return Err(error);
        }
    };
    let staged = app_image.bit()
        | if needs_netcore {
            Image::NetCore.bit()
        } else {
            0
        };
    if status.staged & staged != staged {
        let error = OtaError::NotStaged(if needs_netcore {
            Image::NetCore
        } else {
            app_image
        });
        let _ = exchange(transport, Request::Abort).await;
        return Err(error);
    }

    let expected_netcore = if needs_netcore {
        netcore_target
    } else {
        status.netcore_release
    };
    // The commit reboots the unit, so its acknowledgement can be lost with the
    // link. The versions seen after reconnecting are the authority.
    let committed = exchange(transport, Request::Commit).await;
    let verified = transport
        .verify_reconnected(images.app_version, expected_netcore)
        .await
        .map_err(OtaError::Transport)?;
    if !verified {
        return Err(OtaError::VersionMismatch);
    }
    if let Err(error) = committed {
        tracing::debug!(?error, "OTA commit acknowledgement lost during reconnect");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use better_ota::{Action, Receiver, Slots};
    use odyssey_firmware_manifest::{FileEntry, FirmwareEntry};
    use odyssey_protocol::ota::{self, Code, Image, Request};
    use release_signing::Component;
    use sha2::Digest as _;

    use super::{Images, LinkMtu, OtaError, OtaTransport, VerifiedImage, exchange, send, transfer};

    struct Flash {
        app: Vec<u8>,
        netcore: Vec<u8>,
        commits: usize,
    }

    impl Flash {
        fn new() -> Self {
            Self {
                app: vec![0xff; 256],
                netcore: vec![0xff; 256],
                commits: 0,
            }
        }
    }

    impl Slots for Flash {
        fn capacity(&self, _image: Image) -> u32 {
            256
        }

        fn write(
            &mut self,
            image: Image,
            offset: u32,
            data: &[u8],
        ) -> impl Future<Output = Result<(), Code>> {
            let memory = match image {
                Image::Application => &mut self.app,
                Image::NetCore => &mut self.netcore,
            };
            std::future::ready(range(memory.len(), offset, data.len()).map(|range| {
                memory[range].copy_from_slice(data);
            }))
        }

        fn read(
            &mut self,
            image: Image,
            offset: u32,
            data: &mut [u8],
        ) -> impl Future<Output = Result<(), Code>> {
            let memory = match image {
                Image::Application => &self.app,
                Image::NetCore => &self.netcore,
            };
            std::future::ready(range(memory.len(), offset, data.len()).map(|range| {
                data.copy_from_slice(&memory[range]);
            }))
        }

        fn component(&self, image: Image) -> Component {
            match image {
                Image::Application => Component::Lite1App,
                Image::NetCore => Component::Lite1NetCore,
            }
        }

        fn valid(&mut self, _image: Image, _size: u32) -> impl Future<Output = Result<(), Code>> {
            std::future::ready(Ok(()))
        }

        fn commit(
            &mut self,
            _app_size: Option<u32>,
            _netcore_size: Option<u32>,
        ) -> impl Future<Output = Result<(), Code>> {
            self.commits += 1;
            std::future::ready(Ok(()))
        }
    }

    fn range(memory_len: usize, offset: u32, len: usize) -> Result<std::ops::Range<usize>, Code> {
        let start = usize::try_from(offset).map_err(|_conversion_error| Code::Size)?;
        let end = start.checked_add(len).ok_or(Code::Size)?;
        if end > memory_len {
            return Err(Code::Size);
        }
        Ok(start..end)
    }

    struct Device {
        receiver: Receiver,
        flash: Flash,
        slot: u8,
        current_netcore: u16,
        app_version: [u32; 3],
        target_netcore: u16,
        corrupt_next_data: bool,
        reset: bool,
        require_signature: bool,
        signatures: Vec<Vec<u8>>,
    }

    impl Device {
        fn new(corrupt_next_data: bool) -> Self {
            Self {
                receiver: Receiver::new(),
                flash: Flash::new(),
                slot: 2,
                current_netcore: 1,
                app_version: [1, 2, 3],
                target_netcore: 2,
                corrupt_next_data,
                reset: false,
                require_signature: false,
                signatures: Vec::new(),
            }
        }
    }

    impl OtaTransport for Device {
        async fn exchange(&mut self, frame: &[u8]) -> Result<[u8; ota::STATUS_LEN], String> {
            let mut frame = frame.to_vec();
            if self.corrupt_next_data && frame.first() == Some(&2) && frame.len() > 5 {
                frame[5] ^= 1;
                self.corrupt_next_data = false;
            }
            if frame.first() == Some(&5) {
                self.signatures.push(frame.clone());
            }
            match self.receiver.handle(&mut self.flash, &frame).await {
                Ok(Action::Reset) => self.reset = true,
                Ok(Action::Continue) | Err(_) => {}
            }
            let mut status = self.receiver.status(
                self.slot,
                Image::Application.bit() | Image::NetCore.bit(),
                self.app_version,
                self.current_netcore,
            );
            status.signature_required = self.require_signature;
            Ok(status.encode())
        }

        fn verify_reconnected(
            &mut self,
            app_version: [u32; 3],
            netcore_release: u16,
        ) -> impl Future<Output = Result<bool, String>> {
            std::future::ready(Ok(self.reset
                && app_version == self.app_version
                && netcore_release == self.target_netcore))
        }
    }

    fn file(bytes: &[u8], release: Option<u16>) -> FileEntry {
        FileEntry {
            sha256: hex::encode(sha2::Sha256::digest(bytes)),
            size: bytes.len() as u64,
            version: None,
            release,
            signature: None,
        }
    }

    fn entry() -> FirmwareEntry {
        FirmwareEntry {
            version: "1.2.3".into(),
            requires_bootloader: semver::Version::new(0, 0, 0),
            requires_netcore: 1,
            min_desktop: "0.0.0".into(),
            files: std::collections::HashMap::default(),
            released: None,
        }
    }

    #[tokio::test]
    async fn secure_radio_target_requires_component_bound_signature_before_query() {
        let app = b"secure application";
        let app_file = file(app, None);
        let mut target = entry();
        target.requires_bootloader = semver::Version::new(2, 0, 0);
        let mut device = Device::new(false);

        let result = send(
            &mut device,
            LinkMtu::Vendor,
            &target,
            Images {
                application: None,
                app_a: None,
                app_b: Some(VerifiedImage {
                    bytes: app,
                    manifest: &app_file,
                }),
                netcore: None,
                app_version: [2, 0, 0],
                known_bootloader_floor: &semver::Version::new(2, 0, 0),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(OtaError::MissingSignature("application"))
        ));
        assert_eq!(device.flash.commits, 0);
    }
    #[tokio::test]
    async fn stages_target_bank_and_netcore_before_one_commit() {
        let app_a = b"wrong bank application image";
        let app_b = b"target bank application image";
        let netcore = b"new network core image";
        let app_a_file = file(app_a, None);
        let app_b_file = file(app_b, None);
        let netcore_file = file(netcore, Some(2));
        let mut device = Device::new(false);

        send(
            &mut device,
            LinkMtu::Marker,
            &entry(),
            Images {
                application: None,
                app_a: Some(VerifiedImage {
                    bytes: app_a,
                    manifest: &app_a_file,
                }),
                app_b: Some(VerifiedImage {
                    bytes: app_b,
                    manifest: &app_b_file,
                }),
                netcore: Some(VerifiedImage {
                    bytes: netcore,
                    manifest: &netcore_file,
                }),
                app_version: [1, 2, 3],
                known_bootloader_floor: &semver::Version::new(0, 0, 0),
            },
        )
        .await
        .expect("complete radio update");

        assert_eq!(device.flash.commits, 1);
        assert!(device.reset);
        assert_eq!(&device.flash.app[..app_b.len()], app_b);
        assert_eq!(&device.flash.netcore[..netcore.len()], netcore);
    }

    #[tokio::test]
    async fn checksum_failure_never_commits_and_can_recover_over_usb() {
        let app = b"valid application image";
        let app_file = file(app, None);
        let mut device = Device::new(true);

        let result = send(
            &mut device,
            LinkMtu::Vendor,
            &entry(),
            Images {
                application: None,
                app_a: None,
                app_b: Some(VerifiedImage {
                    bytes: app,
                    manifest: &app_file,
                }),
                netcore: None,
                app_version: [1, 2, 3],
                known_bootloader_floor: &semver::Version::new(0, 0, 0),
            },
        )
        .await;

        let error = result.expect_err("corrupted checksum must stop before commit");
        assert!(
            matches!(&error, OtaError::Device(Code::Checksum)),
            "unexpected OTA failure: {error:?}"
        );
        assert!(error.to_string().contains("USB"));
        assert_eq!(device.flash.commits, 0);
        assert!(!device.reset);
        let status = device.receiver.status(
            device.slot,
            Image::Application.bit() | Image::NetCore.bit(),
            device.app_version,
            device.current_netcore,
        );
        assert_eq!(status.receiving, None);
        assert_eq!(status.staged, 0);
    }

    #[tokio::test]
    async fn interrupted_receiving_session_is_aborted_before_retry() {
        let stale = b"interrupted application image";
        let app = b"retried application image";
        let app_file = file(app, None);
        let mut device = Device::new(false);
        device.target_netcore = device.current_netcore;
        let begin = Request::Begin {
            image: Image::Application,
            size: u32::try_from(stale.len()).expect("size"),
            crc32: ota::crc32(stale),
        };
        exchange(&mut device, begin).await.expect("stale begin");
        assert_eq!(
            device.receiver.status(2, 0, [0; 3], 0).receiving,
            Some(Image::Application)
        );

        update(&mut device, LinkMtu::Vendor, (app, &app_file), None)
            .await
            .expect("retry after interrupted transfer");

        assert_eq!(device.flash.commits, 1);
        assert_eq!(&device.flash.app[..app.len()], app);
    }

    #[tokio::test]
    async fn staged_session_is_aborted_before_retry() {
        let stale = b"staged but never committed";
        let app = b"retried application image";
        let app_file = file(app, None);
        let mut device = Device::new(false);
        device.target_netcore = device.current_netcore;
        transfer(
            &mut device,
            Image::Application,
            stale,
            LinkMtu::Vendor,
            None,
        )
        .await
        .expect("stage stale image");
        assert_ne!(device.receiver.status(2, 0, [0; 3], 0).staged, 0);

        update(&mut device, LinkMtu::Vendor, (app, &app_file), None)
            .await
            .expect("retry after staged session");

        assert_eq!(device.flash.commits, 1);
        assert_eq!(&device.flash.app[..app.len()], app);
    }

    fn signed_file(bytes: &[u8], release: Option<u16>, fill: u8) -> FileEntry {
        FileEntry {
            signature: Some(hex::encode([fill; ota::SIGNATURE_LEN])),
            ..file(bytes, release)
        }
    }

    async fn update(
        device: &mut Device,
        link: LinkMtu,
        app: (&[u8], &FileEntry),
        netcore: Option<(&[u8], &FileEntry)>,
    ) -> Result<(), OtaError> {
        let bootloader = match link {
            LinkMtu::Vendor => semver::Version::new(1, 0, 0),
            LinkMtu::Marker => semver::Version::new(0, 0, 0),
        };
        send(
            device,
            link,
            &entry(),
            Images {
                application: None,
                app_a: None,
                app_b: Some(VerifiedImage {
                    bytes: app.0,
                    manifest: app.1,
                }),
                netcore: netcore.map(|(bytes, manifest)| VerifiedImage { bytes, manifest }),
                app_version: [1, 2, 3],
                known_bootloader_floor: &bootloader,
            },
        )
        .await
    }

    /// Signature frames for a record of `version` and a signature of `fill` bytes, split into
    /// chunks of at most `chunk` bytes.
    fn frames(image: u8, version: [u32; 3], fill: u8, chunk: usize) -> Vec<Vec<u8>> {
        let mut record = Vec::new();
        for word in version {
            record.extend(word.to_le_bytes());
        }
        record.extend([fill; ota::SIGNATURE_LEN]);
        record
            .chunks(chunk)
            .enumerate()
            .map(|(index, bytes)| {
                let mut frame = vec![5, image, u8::try_from(index * chunk).expect("offset")];
                frame.extend(bytes);
                frame
            })
            .collect()
    }

    #[tokio::test]
    async fn historical_signature_follows_each_staged_image_on_one_x_bootloader() {
        let app = b"signed application image";
        let netcore = b"signed network core image";
        let app_file = signed_file(app, None, 0xa1);
        let netcore_file = signed_file(netcore, Some(2), 0xb2);
        let mut device = Device::new(false);
        device.require_signature = true;

        update(
            &mut device,
            LinkMtu::Vendor,
            (app, &app_file),
            Some((netcore, &netcore_file)),
        )
        .await
        .expect("signed update");

        let expected = [
            frames(1, [2, 0, 0], 0xb2, ota::MAX_FRAME),
            frames(0, [1, 2, 3], 0xa1, ota::MAX_FRAME),
        ]
        .concat();
        assert_eq!(device.signatures, expected);
        assert_eq!(device.signatures.len(), 2);
        assert_eq!(device.flash.commits, 1);
    }

    #[tokio::test]
    async fn device_requiring_signatures_refuses_unsigned_manifest_before_any_transfer() {
        let app = b"unsigned application image";
        let app_file = file(app, None);
        let mut device = Device::new(false);
        device.require_signature = true;

        let result = update(&mut device, LinkMtu::Vendor, (app, &app_file), None).await;

        assert!(matches!(
            result,
            Err(OtaError::MissingSignature("application"))
        ));
        assert_eq!(device.flash.app, vec![0xff; 256]);
        assert_eq!(device.flash.commits, 0);
    }

    #[tokio::test]
    async fn malformed_signature_is_rejected() {
        let app = b"application image";
        let mut app_file = file(app, None);
        app_file.signature = Some("abcd".into());
        let mut device = Device::new(false);
        device.require_signature = true;

        let result = update(&mut device, LinkMtu::Vendor, (app, &app_file), None).await;

        assert!(matches!(
            result,
            Err(OtaError::InvalidSignature("application"))
        ));
    }

    #[tokio::test]
    async fn device_not_requiring_signatures_is_never_sent_one() {
        let app = b"signed application image";
        let app_file = signed_file(app, None, 0xa1);
        let mut device = Device::new(false);
        device.target_netcore = device.current_netcore;

        update(&mut device, LinkMtu::Vendor, (app, &app_file), None)
            .await
            .expect("update");

        assert_eq!(device.signatures.len(), 0);
    }

    #[tokio::test]
    async fn marker_link_sends_the_signature_record_in_chunks_that_fit_one_gatt_write() {
        let app = b"signed application image";
        let app_file = signed_file(app, None, 0xa1);
        let mut device = Device::new(false);
        device.require_signature = true;
        device.target_netcore = device.current_netcore;

        update(&mut device, LinkMtu::Marker, (app, &app_file), None)
            .await
            .expect("signed marker update");

        assert_eq!(device.signatures, frames(0, [1, 2, 3], 0xa1, 12));
        assert_eq!(device.signatures.len(), 7);
        assert!(device.signatures.iter().all(|frame| frame.len() <= 20));
        assert_eq!(device.flash.commits, 1);
    }
}
