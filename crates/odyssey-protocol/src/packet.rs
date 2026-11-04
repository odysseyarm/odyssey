use core::mem::MaybeUninit;

use odyssey_protocol_macros::protocol_type;

use crate::codec::{Serialize, push};
use crate::{
    AccelReport, BatteryReport, CombinedMarkersReport, ConfigKind, DeviceName, Error,
    GeneralConfig, ImpactReport, Mode, ObjectReport, PocMarkersReport, PropKind, Props,
    ReadRegisterResponse, Register, SetDeviceNameResult, StreamUpdate, Version, WriteRegister,
};

/// First id of the vendor range. Ids strictly between this and
/// [`VENDOR_END_ID`] are vendor packets carrying their own tag.
const VENDOR_START_ID: u8 = 0x80;
/// Last id of the vendor range.
const VENDOR_END_ID: u8 = 0xff;

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub struct Packet {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub data: PacketData,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub id: u8,
}

// Pixel frames never live in Packet: embedded delivery keeps the existing
// small-packet stack budget even for full-resolution capture.
const _: () = assert!(size_of::<Packet>() <= 768);

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing a payload would change the public enum and the no_std wire types"
)]
pub enum PacketData {
    #[cfg_attr(feature = "minicbor", n(0))]
    WriteRegister(#[cfg_attr(feature = "minicbor", n(0))] WriteRegister),
    #[cfg_attr(feature = "minicbor", n(1))]
    ReadRegister(#[cfg_attr(feature = "minicbor", n(0))] Register),
    #[cfg_attr(feature = "minicbor", n(2))]
    ReadRegisterResponse(#[cfg_attr(feature = "minicbor", n(0))] ReadRegisterResponse),
    #[cfg_attr(feature = "minicbor", n(3))]
    WriteConfig(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
        GeneralConfig,
    ),
    #[cfg_attr(feature = "minicbor", n(4))]
    ReadConfig(#[cfg_attr(feature = "minicbor", n(0))] ConfigKind),
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
        GeneralConfig,
    ),
    #[cfg_attr(feature = "minicbor", n(6))]
    ReadProp(#[cfg_attr(feature = "minicbor", n(0))] PropKind),
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropResponse(#[cfg_attr(feature = "minicbor", n(0))] Props),
    #[cfg_attr(feature = "minicbor", n(8))]
    ObjectReportRequest(),
    #[cfg_attr(feature = "minicbor", n(9))]
    ObjectReport(#[cfg_attr(feature = "minicbor", n(0))] ObjectReport),
    #[cfg_attr(feature = "minicbor", n(10))]
    CombinedMarkersReport(#[cfg_attr(feature = "minicbor", n(0))] CombinedMarkersReport),
    #[cfg_attr(feature = "minicbor", n(11))]
    PocMarkersReport(#[cfg_attr(feature = "minicbor", n(0))] PocMarkersReport),
    #[cfg_attr(feature = "minicbor", n(12))]
    AccelReport(#[cfg_attr(feature = "minicbor", n(0))] AccelReport),
    #[cfg_attr(feature = "minicbor", n(13))]
    ImpactReport(#[cfg_attr(feature = "minicbor", n(0))] ImpactReport),
    #[cfg_attr(feature = "minicbor", n(14))]
    StreamUpdate(#[cfg_attr(feature = "minicbor", n(0))] StreamUpdate),
    #[cfg_attr(feature = "minicbor", n(15))]
    FlashSettings(),
    #[cfg_attr(feature = "minicbor", n(16))]
    Ack(),
    #[cfg_attr(feature = "minicbor", n(17))]
    WriteMode(#[cfg_attr(feature = "minicbor", n(0))] Mode),
    #[cfg_attr(feature = "minicbor", n(18))]
    ReadVersion(),
    #[cfg_attr(feature = "minicbor", n(19))]
    ReadVersionResponse(#[cfg_attr(feature = "minicbor", n(0))] Version),
    #[cfg_attr(feature = "minicbor", n(20))]
    Vendor(
        #[cfg_attr(feature = "minicbor", n(0))] u8,
        #[cfg_attr(feature = "minicbor", n(1))] VendorData,
    ),
    #[cfg_attr(feature = "minicbor", n(21))]
    BatteryReport(#[cfg_attr(feature = "minicbor", n(0))] BatteryReport),
    #[cfg_attr(feature = "minicbor", n(22))]
    SetDeviceName(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::heapless_str32"))]
        DeviceName,
    ),
    #[cfg_attr(feature = "minicbor", n(23))]
    SetDeviceNameResponse(#[cfg_attr(feature = "minicbor", n(0))] SetDeviceNameResult),
    #[cfg_attr(feature = "minicbor", n(24))]
    ImageCommand(#[cfg_attr(feature = "minicbor", n(0))] crate::image::ImageCommand),
    #[cfg_attr(
        all(feature = "serde", feature = "device", not(feature = "std")),
        serde(skip_deserializing)
    )]
    #[cfg_attr(feature = "minicbor", n(25))]
    ImageStatus(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageStatus,
    ),
    #[cfg_attr(
        all(feature = "serde", feature = "device", not(feature = "std")),
        serde(skip_deserializing)
    )]
    #[cfg_attr(feature = "minicbor", n(26))]
    ImageChunk(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageChunk,
    ),
    #[cfg_attr(
        all(feature = "serde", feature = "device", not(feature = "std")),
        serde(skip_deserializing)
    )]
    #[cfg_attr(feature = "minicbor", n(27))]
    ImageObject(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageObject,
    ),
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct VendorData {
    /// Bytes of `data` in use; at most [`VendorData::CAPACITY`].
    #[cfg_attr(feature = "minicbor", n(0))]
    pub len: u8,
    #[cfg(feature = "serde")]
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "minicbor", n(1))]
    pub data: [u8; Self::CAPACITY],
    #[cfg(not(feature = "serde"))]
    pub data: [u8; Self::CAPACITY],
}

impl VendorData {
    /// Payload bytes a vendor packet can carry.
    pub const CAPACITY: usize = 98;
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Copy, Clone, Debug)]
pub enum PacketType {
    #[cfg_attr(feature = "minicbor", n(0))]
    WriteRegister(),
    #[cfg_attr(feature = "minicbor", n(1))]
    ReadRegister(),
    #[cfg_attr(feature = "minicbor", n(2))]
    ReadRegisterResponse(),
    #[cfg_attr(feature = "minicbor", n(3))]
    WriteConfig(),
    #[cfg_attr(feature = "minicbor", n(4))]
    ReadConfig(),
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(),
    #[cfg_attr(feature = "minicbor", n(6))]
    ReadProp(),
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropResponse(),
    #[cfg_attr(feature = "minicbor", n(8))]
    ObjectReportRequest(),
    #[cfg_attr(feature = "minicbor", n(9))]
    ObjectReport(),
    #[cfg_attr(feature = "minicbor", n(10))]
    CombinedMarkersReport(),
    #[cfg_attr(feature = "minicbor", n(11))]
    AccelReport(),
    #[cfg_attr(feature = "minicbor", n(12))]
    ImpactReport(),
    #[cfg_attr(feature = "minicbor", n(13))]
    StreamUpdate(),
    #[cfg_attr(feature = "minicbor", n(14))]
    FlashSettings(),
    #[cfg_attr(feature = "minicbor", n(15))]
    Ack(),
    #[cfg_attr(feature = "minicbor", n(16))]
    PocMarkersReport(),
    #[cfg_attr(feature = "minicbor", n(17))]
    WriteMode(),
    #[cfg_attr(feature = "minicbor", n(18))]
    ReadVersion(),
    #[cfg_attr(feature = "minicbor", n(19))]
    ReadVersionResponse(),
    #[cfg_attr(feature = "minicbor", n(20))]
    End(),
    #[cfg_attr(feature = "minicbor", n(21))]
    VendorStart(),
    #[cfg_attr(feature = "minicbor", n(22))]
    Vendor(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(23))]
    VendorEnd(),
    #[cfg_attr(feature = "minicbor", n(24))]
    BatteryReport(),
    #[cfg_attr(feature = "minicbor", n(25))]
    SetDeviceName(),
    #[cfg_attr(feature = "minicbor", n(26))]
    SetDeviceNameResponse(),
    #[cfg_attr(feature = "minicbor", n(27))]
    ImageCommand(),
    #[cfg_attr(feature = "minicbor", n(28))]
    ImageStatus(),
    #[cfg_attr(feature = "minicbor", n(29))]
    ImageChunk(),
    #[cfg_attr(feature = "minicbor", n(30))]
    ImageObject(),
}

impl TryFrom<u8> for PacketType {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        match n {
            0x00 => Ok(Self::WriteRegister()),
            0x01 => Ok(Self::ReadRegister()),
            0x02 => Ok(Self::ReadRegisterResponse()),
            0x03 => Ok(Self::WriteConfig()),
            0x04 => Ok(Self::ReadConfig()),
            0x05 => Ok(Self::ReadConfigResponse()),
            0x06 => Ok(Self::ReadProp()),
            0x07 => Ok(Self::ReadPropResponse()),
            0x08 => Ok(Self::ObjectReportRequest()),
            0x09 => Ok(Self::ObjectReport()),
            0x0a => Ok(Self::CombinedMarkersReport()),
            0x0b => Ok(Self::AccelReport()),
            0x0c => Ok(Self::ImpactReport()),
            0x0d => Ok(Self::StreamUpdate()),
            0x0e => Ok(Self::FlashSettings()),
            0x0f => Ok(Self::Ack()),
            0x10 => Ok(Self::PocMarkersReport()),
            0x11 => Ok(Self::WriteMode()),
            0x12 => Ok(Self::ReadVersion()),
            0x13 => Ok(Self::ReadVersionResponse()),
            0x14 => Ok(Self::End()),
            0x15 => Ok(Self::BatteryReport()),
            0x16 => Ok(Self::SetDeviceName()),
            0x17 => Ok(Self::SetDeviceNameResponse()),
            0x18 => Ok(Self::ImageCommand()),
            0x19 => Ok(Self::ImageStatus()),
            0x1a => Ok(Self::ImageChunk()),
            0x1b => Ok(Self::ImageObject()),
            VENDOR_START_ID => Ok(Self::VendorStart()),
            VENDOR_END_ID => Ok(Self::VendorEnd()),
            n if (VENDOR_START_ID..VENDOR_END_ID).contains(&n) => Ok(Self::Vendor(n)),
            _ => Err(Error::UnrecognizedPacketId(n)),
        }
    }
}

impl From<PacketType> for u8 {
    fn from(ty: PacketType) -> Self {
        match ty {
            PacketType::WriteRegister() => 0x00,
            PacketType::ReadRegister() => 0x01,
            PacketType::ReadRegisterResponse() => 0x02,
            PacketType::WriteConfig() => 0x03,
            PacketType::ReadConfig() => 0x04,
            PacketType::ReadConfigResponse() => 0x05,
            PacketType::ReadProp() => 0x06,
            PacketType::ReadPropResponse() => 0x07,
            PacketType::ObjectReportRequest() => 0x08,
            PacketType::ObjectReport() => 0x09,
            PacketType::CombinedMarkersReport() => 0x0a,
            PacketType::AccelReport() => 0x0b,
            PacketType::ImpactReport() => 0x0c,
            PacketType::StreamUpdate() => 0x0d,
            PacketType::FlashSettings() => 0x0e,
            PacketType::Ack() => 0x0f,
            PacketType::PocMarkersReport() => 0x10,
            PacketType::WriteMode() => 0x11,
            PacketType::ReadVersion() => 0x12,
            PacketType::ReadVersionResponse() => 0x13,
            PacketType::End() => 0x14,
            PacketType::BatteryReport() => 0x15,
            PacketType::SetDeviceName() => 0x16,
            PacketType::SetDeviceNameResponse() => 0x17,
            PacketType::ImageCommand() => 0x18,
            PacketType::ImageStatus() => 0x19,
            PacketType::ImageChunk() => 0x1a,
            PacketType::ImageObject() => 0x1b,
            PacketType::VendorStart() => VENDOR_START_ID,
            PacketType::VendorEnd() => VENDOR_END_ID,
            PacketType::Vendor(n) => n,
        }
    }
}

impl Packet {
    pub fn ty(&self) -> PacketType {
        match self.data {
            PacketData::WriteRegister(_) => PacketType::WriteRegister(),
            PacketData::ReadRegister(_) => PacketType::ReadRegister(),
            PacketData::ReadRegisterResponse(_) => PacketType::ReadRegisterResponse(),
            PacketData::WriteConfig(_) => PacketType::WriteConfig(),
            PacketData::ReadConfig(_) => PacketType::ReadConfig(),
            PacketData::ReadConfigResponse(_) => PacketType::ReadConfigResponse(),
            PacketData::ReadProp(_) => PacketType::ReadProp(),
            PacketData::ReadPropResponse(_) => PacketType::ReadPropResponse(),
            PacketData::ObjectReportRequest() => PacketType::ObjectReportRequest(),
            PacketData::ObjectReport(_) => PacketType::ObjectReport(),
            PacketData::CombinedMarkersReport(_) => PacketType::CombinedMarkersReport(),
            PacketData::PocMarkersReport(_) => PacketType::PocMarkersReport(),
            PacketData::AccelReport(_) => PacketType::AccelReport(),
            PacketData::ImpactReport(_) => PacketType::ImpactReport(),
            PacketData::StreamUpdate(_) => PacketType::StreamUpdate(),
            PacketData::FlashSettings() => PacketType::FlashSettings(),
            PacketData::Ack() => PacketType::Ack(),
            PacketData::WriteMode(_) => PacketType::WriteMode(),
            PacketData::ReadVersion() => PacketType::ReadVersion(),
            PacketData::ReadVersionResponse(_) => PacketType::ReadVersionResponse(),
            PacketData::Vendor(n, _) => PacketType::Vendor(n),
            PacketData::BatteryReport(_) => PacketType::BatteryReport(),
            PacketData::SetDeviceName(_) => PacketType::SetDeviceName(),
            PacketData::SetDeviceNameResponse(_) => PacketType::SetDeviceNameResponse(),
            PacketData::ImageCommand(_) => PacketType::ImageCommand(),
            PacketData::ImageStatus(_) => PacketType::ImageStatus(),
            PacketData::ImageChunk(_) => PacketType::ImageChunk(),
            PacketData::ImageObject(_) => PacketType::ImageObject(),
        }
    }

    /// True for commands and their responses, false for streamed reports.
    ///
    /// BLE links carry the two kinds on separate L2CAP channels (control and
    /// data) so a burst of reports cannot delay a response; every device and the
    /// dongle route by this one answer.
    pub fn is_control(&self) -> bool {
        match self.data {
            PacketData::StreamUpdate(_)
            | PacketData::WriteMode(_)
            | PacketData::ImageCommand(_)
            | PacketData::ImageStatus(_)
            | PacketData::ReadVersion()
            | PacketData::ReadVersionResponse(_)
            | PacketData::ReadConfig(_)
            | PacketData::ReadConfigResponse(_)
            | PacketData::WriteConfig(_)
            | PacketData::ReadProp(_)
            | PacketData::ReadPropResponse(_)
            | PacketData::WriteRegister(_)
            | PacketData::ReadRegister(_)
            | PacketData::ReadRegisterResponse(_)
            | PacketData::ObjectReportRequest()
            | PacketData::Ack()
            | PacketData::FlashSettings()
            | PacketData::Vendor(_, _)
            | PacketData::SetDeviceName(_)
            | PacketData::SetDeviceNameResponse(_) => true,
            PacketData::ObjectReport(_)
            | PacketData::CombinedMarkersReport(_)
            | PacketData::PocMarkersReport(_)
            | PacketData::AccelReport(_)
            | PacketData::ImpactReport(_)
            | PacketData::ImageChunk(_)
            | PacketData::ImageObject(_)
            | PacketData::BatteryReport(_) => false,
        }
    }

    /// Writes the packet header followed by `data` and returns the bytes
    /// written.
    ///
    /// The header is a little-endian length in 16-bit words (header
    /// included), the packet type and the request id.
    pub fn serialize_parts<D: Serialize>(
        id: u8,
        ty: PacketType,
        data: &D,
        mut buf: &mut [MaybeUninit<u8>],
    ) -> usize {
        const HEADER_SIZE: usize = 4;
        let orig_buf_len = buf.len();
        let [words_low, words_high] = u16::to_le_bytes(((D::SIZE + HEADER_SIZE) / 2) as u16);
        push(&mut buf, &[words_low, words_high, ty.into(), id]);
        data.serialize(&mut buf);
        orig_buf_len - buf.len()
    }
}

impl PacketData {
    pub fn read_register_response(self) -> Option<ReadRegisterResponse> {
        match self {
            Self::ReadRegisterResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn read_config_response(self) -> Option<GeneralConfig> {
        match self {
            Self::ReadConfigResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn read_prop_response(self) -> Option<Props> {
        match self {
            Self::ReadPropResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn object_report(self) -> Option<ObjectReport> {
        match self {
            Self::ObjectReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn combined_markers_report(self) -> Option<CombinedMarkersReport> {
        match self {
            Self::CombinedMarkersReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn poc_markers_report(self) -> Option<PocMarkersReport> {
        match self {
            Self::PocMarkersReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn accel_report(self) -> Option<AccelReport> {
        match self {
            Self::AccelReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn impact_report(self) -> Option<ImpactReport> {
        match self {
            Self::ImpactReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn battery_report(self) -> Option<BatteryReport> {
        match self {
            Self::BatteryReport(x) => Some(x),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two tables (`u8 -> PacketType` and back) are written out by hand; a
    /// row that differs between them would corrupt every packet of that type.
    #[test]
    fn every_known_packet_id_round_trips() {
        for id in 0..=u8::MAX {
            if let Ok(ty) = PacketType::try_from(id) {
                assert_eq!(u8::from(ty), id);
            }
        }
    }

    #[test]
    fn ids_outside_the_tables_are_rejected() {
        assert!(matches!(
            PacketType::try_from(0x1c),
            Err(Error::UnrecognizedPacketId(0x1c))
        ));
        assert!(matches!(
            PacketType::try_from(0x7f),
            Err(Error::UnrecognizedPacketId(0x7f))
        ));
    }

    #[test]
    fn vendor_ids_carry_their_tag() {
        assert!(matches!(
            PacketType::try_from(0x90),
            Ok(PacketType::Vendor(0x90))
        ));
        assert!(matches!(
            PacketType::try_from(VENDOR_START_ID),
            Ok(PacketType::VendorStart())
        ));
        assert!(matches!(
            PacketType::try_from(VENDOR_END_ID),
            Ok(PacketType::VendorEnd())
        ));
    }
}
