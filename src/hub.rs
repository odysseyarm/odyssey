use heapless::Vec;

use crate::Packet;

pub const MAX_DEVICES: usize = 7;
type Uuid = [u8; 6];

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum HubMsg {
    DevicesSnapshot(Vec<Uuid, MAX_DEVICES>),
    DevicePacket { dev: Uuid, pkt: Packet },
    RequestDevices,
    SendTo { dev: Uuid, pkt: Packet },
}
