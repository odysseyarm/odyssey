use odyssey_hub_common as common;

use super::hardware_address::address_from_bytes;
use crate::{Device, EventsTransport, FirmwareVersion, Transport};

impl From<common::device::Transport> for Transport {
    fn from(t: common::device::Transport) -> Self {
        match t {
            common::device::Transport::Usb => Self::Usb,
            common::device::Transport::UsbMux => Self::UsbMux,
            common::device::Transport::UdpMux => Self::UdpMux,
        }
    }
}
impl From<Transport> for common::device::Transport {
    fn from(t: Transport) -> Self {
        match t {
            Transport::Usb => Self::Usb,
            Transport::UsbMux => Self::UsbMux,
            Transport::UdpMux => Self::UdpMux,
        }
    }
}

impl From<common::device::Device> for Device {
    fn from(d: common::device::Device) -> Self {
        Self {
            uuid: d.uuid.to_vec(),
            transport: Transport::from(d.transport) as i32,
            name: d.name().to_owned(),
            firmware_version: if d.firmware_version == [0, 0, 0] {
                None
            } else {
                Some(FirmwareVersion {
                    major: d.firmware_version[0],
                    minor: d.firmware_version[1],
                    patch: d.firmware_version[2],
                })
            },
            capabilities: u32::from(d.capabilities.bits()),
            events_transport: match d.events_transport {
                common::device::EventsTransport::Wired => EventsTransport::Wired as i32,
                common::device::EventsTransport::Bluetooth => EventsTransport::Bluetooth as i32,
            },
            events_connected: d.events_connected,
            product_id: u32::from(d.product_id),
            firmware_version_string: d
                .firmware_identity
                .known()
                .map(|version| version.to_string()),
            netcore_release: u32::from(d.netcore_release),
        }
    }
}
impl TryFrom<Device> for common::device::Device {
    type Error = &'static str;

    fn try_from(d: Device) -> Result<Self, Self::Error> {
        let firmware_version = if let Some(version) = d.firmware_version {
            [version.major, version.minor, version.patch]
        } else {
            [0; 3]
        };
        Ok(Self {
            uuid: address_from_bytes(&d.uuid)?,
            transport: Transport::try_from(d.transport)
                .map_err(|_conversion_error| "unknown device transport")?
                .into(),
            capabilities: common::device::DeviceCapabilities::new(
                u8::try_from(d.capabilities)
                    .map_err(|_conversion_error| "device capabilities exceed u8")?,
            ),
            firmware_version,
            events_transport: match EventsTransport::try_from(d.events_transport)
                .map_err(|_conversion_error| "unknown device events transport")?
            {
                EventsTransport::Wired => common::device::EventsTransport::Wired,
                EventsTransport::Bluetooth => common::device::EventsTransport::Bluetooth,
            },
            events_connected: d.events_connected,
            product_id: u16::try_from(d.product_id)
                .map_err(|_conversion_error| "product id exceeds u16")?,
            name: Self::name_bytes(&d.name),
            firmware_identity: d
                .firmware_version_string
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_conversion_error| "firmware version is not an Odyssey release")?
                .unwrap_or_default(),
            netcore_release: u16::try_from(d.netcore_release)
                .map_err(|_conversion_error| "net core release exceeds u16")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_wire_transports_are_errors() {
        common::device::Device::try_from(Device {
            uuid: vec![0; 6],
            transport: i32::MAX,
            ..Device::default()
        })
        .expect_err("unknown device transport must be rejected");
        common::device::Device::try_from(Device {
            uuid: vec![0; 6],
            events_transport: i32::MAX,
            ..Device::default()
        })
        .expect_err("unknown events transport must be rejected");
    }

    #[test]
    fn firmware_identity_crosses_the_wire_as_release_text() {
        let text = "1.2.4-nightly.20261111.t201500.gabcdef012345";
        let device = common::device::Device::try_from(Device {
            uuid: vec![0; 6],
            firmware_version_string: Some(text.to_owned()),
            ..Device::default()
        })
        .expect("a release version converts");
        assert_eq!(
            Device::from(device).firmware_version_string.as_deref(),
            Some(text)
        );
        common::device::Device::try_from(Device {
            uuid: vec![0; 6],
            firmware_version_string: Some("1.2.3-rc.1".to_owned()),
            ..Device::default()
        })
        .expect_err("other version text must be rejected");
    }
}
