mod layout {
    include!(concat!(env!("OUT_DIR"), "/dongle_layout.rs"));
}

mod nordic;
mod package;

use std::time::Duration;

use futures::StreamExt as _;
use thiserror::Error;

use super::{FirmwareEntry, FirmwareSourceError, ReleaseSource};

const VID: u16 = 0x1915;
const APP_PID: u16 = 0x5212;
const BOOTLOADER_PID: u16 = 0x521f;
const BOOTLOADER_TIMEOUT: Duration = Duration::from_secs(120);
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PACKAGE: u64 = 2 * 1024 * 1024;

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DongleTarget {
    /// Since: Odyssey Desktop 0.7.5.
    pub id: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub in_bootloader: bool,
    bus: String,
    ports: Vec<u8>,
}

impl DongleTarget {
    /// Since: Odyssey Desktop 0.7.5.
    pub fn same_port(&self, other: &Self) -> bool {
        self.bus == other.bus && self.ports == other.ports
    }

    fn from_device(device: &nusb::DeviceInfo) -> Option<Self> {
        if device.vendor_id() != VID || ![APP_PID, BOOTLOADER_PID].contains(&device.product_id()) {
            return None;
        }
        let id = device.serial_number()?.to_owned();
        if id.is_empty() || device.port_chain().is_empty() {
            return None;
        }
        Some(Self {
            id,
            in_bootloader: device.product_id() == BOOTLOADER_PID,
            bus: device.bus_id().to_owned(),
            ports: device.port_chain().to_vec(),
        })
    }
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug)]
pub enum DongleUpdateProgress {
    /// Since: Odyssey Desktop 0.7.5.
    Downloading,
    /// Since: Odyssey Desktop 0.7.5.
    WaitingForBootloader,
    /// Since: Odyssey Desktop 0.7.5.
    Flashing {
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        written: usize,
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        total: usize,
    },
    /// Since: Odyssey Desktop 0.7.5.
    Reconnecting,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Debug, Error)]
pub enum DongleUpdateError {
    /// Since: Odyssey Desktop 0.7.5.
    #[error(transparent)]
    Source(#[from] FirmwareSourceError),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("USB enumeration failed: {0}")]
    Usb(#[from] nusb::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Invalid signed dongle package: {0}")]
    InvalidPackage(&'static str),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Cannot read signed dongle ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Invalid dongle package manifest: {0}")]
    Json(#[from] serde_json::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Cannot read dongle package: {0}")]
    Io(#[from] std::io::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error(transparent)]
    Transport(#[from] nordic::Error),
    /// Since: Odyssey Desktop 0.7.5.
    #[error("Dongle update worker stopped: {0}")]
    Worker(#[from] tokio::task::JoinError),
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "Dongle did not enter bootloader mode. Press its reset/DFU button, keeping it in the same USB port, then retry."
    )]
    BootloaderTimeout,
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "Dongle did not reconnect after flashing. Unplug/replug it in the same USB port. If it stays in bootloader mode, use Recover dongle."
    )]
    ReconnectTimeout,
    /// Since: Odyssey Desktop 0.7.5.
    #[error(
        "Cannot safely identify the dongle's serial port. Unplug other bootloader devices, check the USB serial driver, then retry."
    )]
    SerialPortUnavailable,
    /// Since: Odyssey Desktop 0.7.5.
    #[error("A different dongle appeared in the selected USB port; update stopped")]
    IdentityChanged,
}

/// Enumerates connected application- and bootloader-mode dongles.
///
/// # Errors
/// Returns [`DongleUpdateError::Usb`] if USB device enumeration fails.
///
/// Since: Odyssey Desktop 0.7.5.
pub async fn connected_targets() -> Result<Vec<DongleTarget>, DongleUpdateError> {
    Ok(nusb::list_devices()
        .await?
        .filter_map(|device| DongleTarget::from_device(&device))
        .collect())
}

async fn wait_for_mode(
    target: &DongleTarget,
    bootloader: bool,
) -> Result<DongleTarget, DongleUpdateError> {
    let mut watch = nusb::watch_devices()?;
    let wait = async {
        loop {
            if let Some(found) = connected_targets().await?.into_iter().find(|candidate| {
                candidate.same_port(target) && candidate.in_bootloader == bootloader
            }) {
                if !bootloader
                    && !target.in_bootloader
                    && !found.id.eq_ignore_ascii_case(&target.id)
                {
                    return Err(DongleUpdateError::IdentityChanged);
                }
                return Ok(found);
            }
            watch.next().await;
        }
    };
    tokio::time::timeout(
        if bootloader {
            BOOTLOADER_TIMEOUT
        } else {
            RECONNECT_TIMEOUT
        },
        wait,
    )
    .await
    .map_err(|_elapsed| {
        if bootloader {
            DongleUpdateError::BootloaderTimeout
        } else {
            DongleUpdateError::ReconnectTimeout
        }
    })?
}

fn serial_port(bootloader: &DongleTarget) -> Result<String, DongleUpdateError> {
    let ports = serialport::available_ports().map_err(nordic::Error::from)?;
    let mut matches = ports.into_iter().filter(|port| {
        matches!(&port.port_type, serialport::SerialPortType::UsbPort(info)
            if info.vid == VID && info.pid == BOOTLOADER_PID
            && info.serial_number.as_deref().is_some_and(|id| id.eq_ignore_ascii_case(&bootloader.id)))
    });
    match (matches.next(), matches.next()) {
        (Some(port), None) => Ok(port.port_name),
        _ => Err(DongleUpdateError::SerialPortUnavailable),
    }
}

/// Update a dongle's firmware.
///
/// Download and validate both vendor packages before waiting for the stock bootloader.
/// Returns the reconnected application's full USB serial. The caller must confirm
/// its firmware version. Interrupted uploads restart from the beginning. The
/// bootloader checks signatures.
///
/// # Errors
/// Returns [`DongleUpdateError::Source`] for firmware download failures.
/// [`DongleUpdateError::InvalidPackage`], [`DongleUpdateError::Zip`],
/// [`DongleUpdateError::Json`], or [`DongleUpdateError::Io`] for invalid or
/// unreadable packages. [`DongleUpdateError::Usb`] or
/// [`DongleUpdateError::IdentityChanged`] when USB enumeration or identity checks
/// fail. [`DongleUpdateError::BootloaderTimeout`],
/// [`DongleUpdateError::ReconnectTimeout`], or
/// [`DongleUpdateError::SerialPortUnavailable`] when bootloader or serial-port
/// discovery times out. [`DongleUpdateError::Transport`] for rejected DFU
/// operations. [`DongleUpdateError::Worker`] if a blocking update task fails.
///
/// # Since
/// Odyssey Desktop 0.7.5.
pub async fn update_dongle(
    target: DongleTarget,
    entry: FirmwareEntry,
    on_progress: impl Fn(DongleUpdateProgress) + Send + Sync + 'static,
) -> Result<String, DongleUpdateError> {
    on_progress(DongleUpdateProgress::Downloading);
    if entry.version_parts().is_none() {
        return Err(DongleUpdateError::InvalidPackage(
            "unreadable release version",
        ));
    }
    let mut packages = Vec::with_capacity(2);
    for key in ["dfu-fanstel", "dfu-laird"] {
        let file = entry
            .files
            .get(key)
            .ok_or(DongleUpdateError::InvalidPackage("missing vendor package"))?;
        if file.size == 0 || file.size > MAX_PACKAGE {
            return Err(DongleUpdateError::InvalidPackage("invalid ZIP size"));
        }
        let bytes = super::download_firmware(
            "dongle",
            ReleaseSource::Application {
                version: &entry.version,
            },
            key,
            &file.sha256,
            file.size,
        )
        .await?;
        packages.push(package::parse(&bytes)?);
    }
    if packages[0].image != packages[1].image {
        return Err(DongleUpdateError::InvalidPackage(
            "vendor packages contain different applications",
        ));
    }
    on_progress(DongleUpdateProgress::WaitingForBootloader);
    let bootloader = wait_for_mode(&target, true).await?;
    let selected = bootloader.clone();
    let port = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(port) = serial_port(&selected) {
                return Ok::<_, DongleUpdateError>(port);
            }
            if std::time::Instant::now() >= deadline {
                return Err(DongleUpdateError::SerialPortUnavailable);
            }
            // USB arrival precedes the OS creating its CDC serial port.
            std::thread::sleep(Duration::from_millis(100));
        }
    })
    .await??;
    if !connected_targets().await?.contains(&bootloader) {
        return Err(DongleUpdateError::IdentityChanged);
    }
    let returned = target.clone();
    let result = tokio::task::spawn_blocking(move || {
        nordic::flash(&port, &packages, |written, total| {
            on_progress(DongleUpdateProgress::Flashing { written, total });
        })?;
        on_progress(DongleUpdateProgress::Reconnecting);
        Ok::<_, DongleUpdateError>(())
    })
    .await?;
    result?;
    Ok(wait_for_mode(&returned, false).await?.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_target_includes_bus_and_entire_port_chain() {
        let target = DongleTarget {
            id: "original".into(),
            in_bootloader: false,
            bus: "1".into(),
            ports: vec![2, 3],
        };
        let mut other = target.clone();
        other.id = "bootloader".into();
        other.in_bootloader = true;
        assert!(target.same_port(&other));
        other.bus = "2".into();
        assert!(!target.same_port(&other));
        other.bus = "1".into();
        other.ports = vec![3];
        assert!(!target.same_port(&other));
    }
}
