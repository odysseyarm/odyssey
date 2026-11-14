
use odyssey_firmware_manifest::{FileEntry, FirmwareEntry};
use odyssey_protocol::ota::{self, Code, Image, Request, Status};
use sha2::Digest as _;

const VENDOR_DATA_CHUNK: usize = 92;
const MARKER_DATA_CHUNK: usize = 12;

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
    #[error("radio OTA cannot update stage2 bootloader; use USB")]
    Stage2RequiresUsb,
    #[error("target bootloader floor {floor} exceeds known compatible floor {known}")]
    BootloaderFloor { floor: String, known: String },
    #[error("device staged bitmap does not include {0:?}")]
    NotStaged(Image),
    #[error("device did not acknowledge expected OTA offset {expected}; got {actual}")]
    Offset { expected: u32, actual: u32 },
    #[error("device reported unsupported image {0:?}")]
    ImageUnsupported(Image),
    #[error("device did not return expected application/net-core versions after reboot")]
    VersionMismatch,
}

pub trait OtaTransport {
    fn exchange(&mut self, frame: &[u8]) -> impl Future<Output = Result<[u8; ota::STATUS_LEN], String>>;
    fn verify_reconnected(
        &mut self,
        app_version: [u32; 3],
        netcore_release: u16,
    ) -> impl Future<Output = Result<bool, String>>;
}

#[derive(Clone, Copy, Debug)]
pub struct VerifiedImage<'a> {
    pub bytes: &'a [u8],
    pub manifest: &'a FileEntry,
}

#[derive(Clone, Copy, Debug)]
pub struct Images<'a> {
    pub application: Option<VerifiedImage<'a>>,
    pub app_a: Option<VerifiedImage<'a>>,
    pub app_b: Option<VerifiedImage<'a>>,
    pub netcore: Option<VerifiedImage<'a>>,
    pub app_version: [u32; 3],
    pub known_bootloader_floor: &'a semver::Version,
    pub stage2_bootloader_update: bool,
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

fn ensure_image(image: VerifiedImage<'_>, name: &'static str) -> Result<(), OtaError> {
    if image.bytes.is_empty()
        || u64::try_from(image.bytes.len()).ok() != Some(image.manifest.size)
        || hex::encode(sha2::Sha256::digest(image.bytes)) != image.manifest.sha256
    {
        return Err(OtaError::InvalidImage(name));
    }
    Ok(())
}


async fn transfer<T: OtaTransport>(
    transport: &mut T,
    image: Image,
    bytes: &[u8],
    link: LinkMtu,
) -> Result<(), OtaError> {
    let size = u32::try_from(bytes.len()).map_err(|_| OtaError::InvalidImage("image"))?;
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
        let offset = u32::try_from(offset).map_err(|_| OtaError::InvalidImage("image"))?;
        let status = exchange(
            transport,
            Request::Data {
                offset,
                data: chunk,
            },
        )
        .await?;
        let expected = offset
            .checked_add(u32::try_from(chunk.len()).map_err(|_| OtaError::InvalidImage("image"))?)
            .ok_or(OtaError::InvalidImage("image"))?;
        let final_chunk_staged = expected == size
            && status.receiving.is_none()
            && status.staged & image.bit() != 0;
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
    Ok(())
}
async fn transfer_safely<T: OtaTransport>(
    transport: &mut T,
    image: Image,
    bytes: &[u8],
    link: LinkMtu,
) -> Result<(), OtaError> {
    match transfer(transport, image, bytes, link).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = exchange(transport, Request::Abort).await;
            Err(error)
        }
    }
}

/// Sends verified images through an acknowledged accessory or vendor transport.
pub async fn send<T: OtaTransport>(
    transport: &mut T,
    link: LinkMtu,
    entry: &FirmwareEntry,
    images: Images<'_>,
) -> Result<(), OtaError> {
    if images.stage2_bootloader_update {
        return Err(OtaError::Stage2RequiresUsb);
    }
    if entry.requires_bootloader > *images.known_bootloader_floor {
        return Err(OtaError::BootloaderFloor {
            floor: entry.requires_bootloader.to_string(),
            known: images.known_bootloader_floor.to_string(),
        });
    }

    let status = exchange(transport, Request::Query).await?;
    if status.supported == 0 {
        return Err(OtaError::Unsupported);
    }
    if status.app_slot > 2 {
        return Err(OtaError::Status(Code::State));
    }
    let (app_image, application) = match status.app_slot {
        0 => (Image::Application, images.application),
        1 => (Image::Application, images.app_a),
        2 => (Image::Application, images.app_b),
        _ => return Err(OtaError::Status(Code::State)),
    };
    let application = application.ok_or(OtaError::MissingImage(match status.app_slot {
        0 => "app",
        1 => "app-a",
        _ => "app-b",
    }))?;
    ensure_image(application, "application")?;
    if status.supported & app_image.bit() == 0 {
        return Err(OtaError::ImageUnsupported(app_image));
    }

    if let Some(netcore) = images.netcore {
        ensure_image(netcore, "net-core")?;
    }
    let published_netcore = images
        .netcore
        .and_then(|image| image.manifest.release);
    let netcore_floor = entry.requires_netcore;
    if status.netcore_release < netcore_floor
        && published_netcore.is_none_or(|release| release < netcore_floor)
    {
        return Err(OtaError::MissingImage("net-core meeting required floor"));
    }
    let netcore_target = published_netcore.unwrap_or(status.netcore_release);
    let needs_netcore = netcore_target > status.netcore_release && netcore_target >= netcore_floor;
    if needs_netcore {
        if status.supported & Image::NetCore.bit() == 0 {
            return Err(OtaError::ImageUnsupported(Image::NetCore));
        }
        let netcore = images.netcore.ok_or(OtaError::MissingImage("net-core"))?;
        if let Err(error) =
            transfer_safely(transport, Image::NetCore, netcore.bytes, link).await
        {
            return Err(error);
        }
    }
    if let Err(error) = transfer_safely(transport, app_image, application.bytes, link).await {
        return Err(error);
    }
    let status = match exchange(transport, Request::Query).await {
        Ok(status) => status,
        Err(error) => {
            let _ = exchange(transport, Request::Abort).await;
            return Err(error);
        }
    };
    let staged = app_image.bit() | if needs_netcore { Image::NetCore.bit() } else { 0 };
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
    use super::{Images, LinkMtu, OtaError, OtaTransport, VerifiedImage, send};
    use better_ota::{Action, Receiver, Slots};
    use odyssey_firmware_manifest::{FileEntry, FirmwareEntry};
    use odyssey_protocol::ota::{self, Code, Image};
    use sha2::Digest as _;

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

        async fn write(
            &mut self,
            image: Image,
            offset: u32,
            data: &[u8],
        ) -> Result<(), Code> {
            let memory = match image {
                Image::Application => &mut self.app,
                Image::NetCore => &mut self.netcore,
            };
            let start = usize::try_from(offset).map_err(|_| Code::Size)?;
            let end = start.checked_add(data.len()).ok_or(Code::Size)?;
            memory
                .get_mut(start..end)
                .ok_or(Code::Size)?
                .copy_from_slice(data);
            Ok(())
        }

        async fn read(
            &mut self,
            image: Image,
            offset: u32,
            data: &mut [u8],
        ) -> Result<(), Code> {
            let memory = match image {
                Image::Application => &self.app,
                Image::NetCore => &self.netcore,
            };
            let start = usize::try_from(offset).map_err(|_| Code::Size)?;
            let end = start.checked_add(data.len()).ok_or(Code::Size)?;
            data.copy_from_slice(memory.get(start..end).ok_or(Code::Size)?);
            Ok(())
        }

        async fn valid(&mut self, _image: Image, _size: u32) -> Result<(), Code> {
            Ok(())
        }

        async fn commit(
            &mut self,
            _app_size: Option<u32>,
            _netcore_size: Option<u32>,
        ) -> Result<(), Code> {
            self.commits += 1;
            Ok(())
        }
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
            match self.receiver.handle(&mut self.flash, &frame).await {
                Ok(Action::Continue) => {}
                Ok(Action::Reset) => self.reset = true,
                Err(_) => {}
            }
            Ok(self
                .receiver
                .status(
                    self.slot,
                    Image::Application.bit() | Image::NetCore.bit(),
                    self.app_version,
                    self.current_netcore,
                )
                .encode())
        }

        async fn verify_reconnected(
            &mut self,
            app_version: [u32; 3],
            netcore_release: u16,
        ) -> Result<bool, String> {
            Ok(self.reset
                && app_version == self.app_version
                && netcore_release == self.target_netcore)
        }
    }

    fn file(bytes: &[u8], release: Option<u16>) -> FileEntry {
        FileEntry {
            sha256: hex::encode(sha2::Sha256::digest(bytes)),
            size: bytes.len() as u64,
            version: None,
            release,
        }
    }

    fn entry() -> FirmwareEntry {
        FirmwareEntry {
            version: "1.2.3".into(),
            requires_bootloader: semver::Version::new(0, 0, 0),
            requires_netcore: 1,
            min_desktop: "0.0.0".into(),
            files: Default::default(),
            released: None,
        }
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
                stage2_bootloader_update: false,
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
                stage2_bootloader_update: false,
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
}
