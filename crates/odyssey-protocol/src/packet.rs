use core::mem::MaybeUninit;

use odyssey_protocol_macros::protocol_type;

use crate::codec::{Serialize, push};
use crate::{
    AccelReport, BatteryReport, CombinedMarkersReport, ConfigKind, DeviceName, Error,
    GeneralConfig, ImpactReport, Mode, ObjectReport, PocMarkersReport, PropKind, Props,
    ReadRegisterResponse, Register, SetDeviceNameResult, StreamUpdate, Version, WriteRegister,
};

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

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
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
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct VendorData {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub len: u8,
    #[cfg(feature = "serde")]
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "minicbor", n(1))]
    pub data: [u8; 98],
    #[cfg(not(feature = "serde"))]
    pub data: [u8; 98],
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
            0x80 => Ok(Self::VendorStart()),
            0xff => Ok(Self::VendorEnd()),
            n if (Self::VendorStart().into()..Self::VendorEnd().into()).contains(&n) => {
                Ok(Self::Vendor(n))
            }
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
            PacketType::VendorStart() => 0x80,
            PacketType::VendorEnd() => 0xff,
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
        }
    }

    pub fn serialize_parts<D: Serialize>(
        id: u8,
        ty: PacketType,
        data: &D,
        mut buf: &mut [MaybeUninit<u8>],
    ) -> usize {
        let orig_buf_len = buf.len();
        let words = u16::to_le_bytes((D::SIZE as u16 + 4) / 2);
        push(&mut buf, &[words[0], words[1], ty.into(), id]);
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
