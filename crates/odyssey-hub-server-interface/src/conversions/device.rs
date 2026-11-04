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
            firmware_version: if d.firmware_version == [0, 0, 0] {
                None
            } else {
                Some(FirmwareVersion {
                    major: u32::from(d.firmware_version[0]),
                    minor: u32::from(d.firmware_version[1]),
                    patch: u32::from(d.firmware_version[2]),
                })
            },
            capabilities: u32::from(d.capabilities.bits()),
            events_transport: match d.events_transport {
                common::device::EventsTransport::Wired => EventsTransport::Wired as i32,
                common::device::EventsTransport::Bluetooth => EventsTransport::Bluetooth as i32,
            },
            events_connected: d.events_connected,
            product_id: u32::from(d.product_id),
            name: d.name().to_string(),
            netcore_release: u32::from(d.netcore_release),
        }
    }
}
impl TryFrom<Device> for common::device::Device {
    type Error = &'static str;

    fn try_from(d: Device) -> Result<Self, Self::Error> {
        let firmware_version = if let Some(version) = d.firmware_version {
            [
                u16::try_from(version.major)
                    .map_err(|_conversion_error| "firmware major exceeds u16")?,
                u16::try_from(version.minor)
                    .map_err(|_conversion_error| "firmware minor exceeds u16")?,
                u16::try_from(version.patch)
                    .map_err(|_conversion_error| "firmware patch exceeds u16")?,
            ]
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
        assert!(
            common::device::Device::try_from(Device {
                uuid: vec![0; 6],
                transport: i32::MAX,
                ..Device::default()
            })
            .is_err()
        );
        assert!(
            common::device::Device::try_from(Device {
                uuid: vec![0; 6],
                events_transport: i32::MAX,
                ..Device::default()
            })
            .is_err()
        );
    }
}
