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
impl From<Device> for common::device::Device {
    fn from(d: Device) -> Self {
        Self {
            uuid: address_from_bytes(&d.uuid),
            transport: Transport::try_from(d.transport)
                .expect("transport is a defined Transport value")
                .into(),
            capabilities: common::device::DeviceCapabilities::new(d.capabilities as u8),
            firmware_version: d
                .firmware_version
                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                .unwrap_or([0, 0, 0]),
            events_transport: match EventsTransport::try_from(d.events_transport)
                .expect("events transport is a defined EventsTransport value")
            {
                EventsTransport::Wired => common::device::EventsTransport::Wired,
                EventsTransport::Bluetooth => common::device::EventsTransport::Bluetooth,
            },
            events_connected: d.events_connected,
            product_id: d.product_id as u16,
            name: Self::name_bytes(&d.name),
            netcore_release: u16::try_from(d.netcore_release).unwrap_or(0),
        }
    }
}
