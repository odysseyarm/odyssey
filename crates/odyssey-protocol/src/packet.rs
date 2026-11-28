use odyssey_protocol_macros::protocol_type;

use crate::legacy::{CombinedMarkersReport, Mode, ObjectReport};
use crate::{
    AccelReport, BatteryReport, ConfigKind, DeviceName, Error, GeneralConfig, ImpactReport,
    MarkersReport, PropKind, Props, ReadRegisterResponse, Register, SetDeviceNameResult,
    StreamUpdate, Version, WriteRegister,
};

/// First id of the vendor range. Ids strictly between this and
/// [`VENDOR_END_ID`] are vendor packets carrying their own tag.
const VENDOR_START_ID: u8 = 0x80;
/// Last id of the vendor range.
const VENDOR_END_ID: u8 = 0xff;

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub struct Packet {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub data: PacketData,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub id: u8,
}

// Pixel frames never live in Packet: embedded delivery keeps the existing
// small-packet stack budget even for full-resolution capture.
const _: () = assert!(size_of::<Packet>() <= 768);

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing a payload would change the public enum and the no_std wire types"
)]
pub enum PacketData {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    WriteRegister(#[cfg_attr(feature = "minicbor", n(0))] WriteRegister),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    ReadRegister(#[cfg_attr(feature = "minicbor", n(0))] Register),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(2))]
    ReadRegisterResponse(#[cfg_attr(feature = "minicbor", n(0))] ReadRegisterResponse),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(3))]
    WriteConfig(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
        GeneralConfig,
    ),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(4))]
    ReadConfig(#[cfg_attr(feature = "minicbor", n(0))] ConfigKind),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::serde_with"))]
        GeneralConfig,
    ),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(6))]
    ReadProp(#[cfg_attr(feature = "minicbor", n(0))] PropKind),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropResponse(#[cfg_attr(feature = "minicbor", n(0))] Props),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(8))]
    LegacyObjectReportRequest(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(9))]
    LegacyObjectReport(#[cfg_attr(feature = "minicbor", n(0))] ObjectReport),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(10))]
    LegacyCombinedMarkersReport(#[cfg_attr(feature = "minicbor", n(0))] CombinedMarkersReport),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(11))]
    MarkersReport(#[cfg_attr(feature = "minicbor", n(0))] MarkersReport),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(12))]
    AccelReport(#[cfg_attr(feature = "minicbor", n(0))] AccelReport),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(13))]
    ImpactReport(#[cfg_attr(feature = "minicbor", n(0))] ImpactReport),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(14))]
    StreamUpdate(#[cfg_attr(feature = "minicbor", n(0))] StreamUpdate),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(15))]
    FlashSettings(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(16))]
    Ack(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(17))]
    LegacyWriteMode(#[cfg_attr(feature = "minicbor", n(0))] Mode),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(18))]
    ReadVersion(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(19))]
    ReadVersionResponse(#[cfg_attr(feature = "minicbor", n(0))] Version),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(20))]
    Vendor(
        #[cfg_attr(feature = "minicbor", n(0))] u8,
        #[cfg_attr(feature = "minicbor", n(1))] VendorData,
    ),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(21))]
    BatteryReport(#[cfg_attr(feature = "minicbor", n(0))] BatteryReport),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(22))]
    SetDeviceName(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::heapless_str32"))]
        DeviceName,
    ),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(23))]
    SetDeviceNameResponse(#[cfg_attr(feature = "minicbor", n(0))] SetDeviceNameResult),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(24))]
    ImageCommand(#[cfg_attr(feature = "minicbor", n(0))] crate::image::ImageCommand),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(25))]
    ImageStatus(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "serde", feature = "device", not(feature = "std")),
            serde(deserialize_with = "crate::serde_reject::image_response")
        )]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageStatus,
    ),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(26))]
    ImageChunk(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "serde", feature = "device", not(feature = "std")),
            serde(deserialize_with = "crate::serde_reject::image_response")
        )]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageChunk,
    ),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(27))]
    ImageObject(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(
            all(feature = "serde", feature = "device", not(feature = "std")),
            serde(deserialize_with = "crate::serde_reject::image_response")
        )]
        #[cfg_attr(
            all(feature = "minicbor", feature = "device", not(feature = "std")),
            cbor(decode_with = "crate::cbor::reject_image_response")
        )]
        crate::image::ImageObject,
    ),
    /// One radio OTA request frame (`crate::ota::Request`), host to device. The device answers
    /// with [`PacketData::OtaStatus`] under the same packet id.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(28))]
    OtaRequest(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::postcard_only"))]
        crate::ota::Frame,
    ),
    /// Encoded `crate::ota::Status` answering an [`PacketData::OtaRequest`].
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(29))]
    OtaStatus(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "serde", serde(with = "serde_bytes"))]
        [u8; crate::ota::STATUS_LEN],
    ),
    /// One-way marker hint from the hub, see [`crate::marker_hint`].
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(30))]
    MarkerHint(#[cfg_attr(feature = "minicbor", n(0))] crate::marker_hint::MarkerHint),
    /// A control message (pairing, bonds, transport mode) on the packet link, for hosts that
    /// reach the unit without its USB control endpoint. Replies come back the same way.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(31))]
    DeviceControl(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "crate::cbor::postcard_only"))]
        crate::control::device::DeviceMsg,
    ),
    /// Marker report without its empty slots.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.4.0.
    #[cfg_attr(feature = "minicbor", n(32))]
    SparseMarkersReport(#[cfg_attr(feature = "minicbor", n(0))] crate::SparseMarkersReport),
    /// IMU reports of one stream that were queued together.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.4.0.
    #[cfg_attr(feature = "minicbor", n(33))]
    AccelBatch(#[cfg_attr(feature = "minicbor", n(0))] crate::AccelBatch),
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VendorData {
    /// Bytes of `data` in use. At most [`VendorData::CAPACITY`].
    ///
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub len: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg(feature = "serde")]
    #[serde(with = "serde_bytes")]
    #[cfg_attr(feature = "minicbor", n(1))]
    pub data: [u8; Self::CAPACITY],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg(not(feature = "serde"))]
    pub data: [u8; Self::CAPACITY],
}

impl VendorData {
    /// Payload bytes a vendor packet can carry.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub const CAPACITY: usize = 98;
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Copy, Clone, Debug)]
pub enum PacketType {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    WriteRegister(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    ReadRegister(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(2))]
    ReadRegisterResponse(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(3))]
    WriteConfig(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(4))]
    ReadConfig(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(6))]
    ReadProp(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropResponse(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(8))]
    LegacyObjectReportRequest(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(9))]
    LegacyObjectReport(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(10))]
    LegacyCombinedMarkersReport(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(11))]
    AccelReport(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(12))]
    ImpactReport(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(13))]
    StreamUpdate(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(14))]
    FlashSettings(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(15))]
    Ack(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(16))]
    MarkersReport(),
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(17))]
    LegacyWriteMode(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(18))]
    ReadVersion(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(19))]
    ReadVersionResponse(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(20))]
    End(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(21))]
    VendorStart(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(22))]
    Vendor(#[cfg_attr(feature = "minicbor", n(0))] u8),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(23))]
    VendorEnd(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(24))]
    BatteryReport(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(25))]
    SetDeviceName(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(26))]
    SetDeviceNameResponse(),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(27))]
    ImageCommand(),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(28))]
    ImageStatus(),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(29))]
    ImageChunk(),
    /// Since: Odyssey Desktop 0.7.0.
    ///
    /// Wire baseline: device protocol 0.1.6.
    #[cfg_attr(feature = "minicbor", n(30))]
    ImageObject(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(31))]
    OtaRequest(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(32))]
    OtaStatus(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(33))]
    MarkerHint(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.3.0.
    #[cfg_attr(feature = "minicbor", n(34))]
    DeviceControl(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.4.0.
    #[cfg_attr(feature = "minicbor", n(35))]
    SparseMarkersReport(),
    /// Since: Unreleased.
    ///
    /// Wire baseline: device protocol 0.4.0.
    #[cfg_attr(feature = "minicbor", n(36))]
    AccelBatch(),
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
            0x08 => Ok(Self::LegacyObjectReportRequest()),
            0x09 => Ok(Self::LegacyObjectReport()),
            0x0a => Ok(Self::LegacyCombinedMarkersReport()),
            0x0b => Ok(Self::AccelReport()),
            0x0c => Ok(Self::ImpactReport()),
            0x0d => Ok(Self::StreamUpdate()),
            0x0e => Ok(Self::FlashSettings()),
            0x0f => Ok(Self::Ack()),
            0x10 => Ok(Self::MarkersReport()),
            0x11 => Ok(Self::LegacyWriteMode()),
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
            0x1c => Ok(Self::OtaRequest()),
            0x1d => Ok(Self::OtaStatus()),
            0x1e => Ok(Self::MarkerHint()),
            0x1f => Ok(Self::DeviceControl()),
            0x20 => Ok(Self::SparseMarkersReport()),
            0x21 => Ok(Self::AccelBatch()),
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
            PacketType::LegacyObjectReportRequest() => 0x08,
            PacketType::LegacyObjectReport() => 0x09,
            PacketType::LegacyCombinedMarkersReport() => 0x0a,
            PacketType::AccelReport() => 0x0b,
            PacketType::ImpactReport() => 0x0c,
            PacketType::StreamUpdate() => 0x0d,
            PacketType::FlashSettings() => 0x0e,
            PacketType::Ack() => 0x0f,
            PacketType::MarkersReport() => 0x10,
            PacketType::LegacyWriteMode() => 0x11,
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
            PacketType::OtaRequest() => 0x1c,
            PacketType::OtaStatus() => 0x1d,
            PacketType::MarkerHint() => 0x1e,
            PacketType::DeviceControl() => 0x1f,
            PacketType::SparseMarkersReport() => 0x20,
            PacketType::AccelBatch() => 0x21,
            PacketType::VendorStart() => VENDOR_START_ID,
            PacketType::VendorEnd() => VENDOR_END_ID,
            PacketType::Vendor(n) => n,
        }
    }
}

impl Packet {
    /// Since: Odyssey Desktop 0.6.0.
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
            PacketData::LegacyObjectReportRequest() => PacketType::LegacyObjectReportRequest(),
            PacketData::LegacyObjectReport(_) => PacketType::LegacyObjectReport(),
            PacketData::LegacyCombinedMarkersReport(_) => PacketType::LegacyCombinedMarkersReport(),
            PacketData::MarkersReport(_) => PacketType::MarkersReport(),
            PacketData::AccelReport(_) => PacketType::AccelReport(),
            PacketData::ImpactReport(_) => PacketType::ImpactReport(),
            PacketData::StreamUpdate(_) => PacketType::StreamUpdate(),
            PacketData::FlashSettings() => PacketType::FlashSettings(),
            PacketData::Ack() => PacketType::Ack(),
            PacketData::LegacyWriteMode(_) => PacketType::LegacyWriteMode(),
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
            PacketData::OtaRequest(_) => PacketType::OtaRequest(),
            PacketData::OtaStatus(_) => PacketType::OtaStatus(),
            PacketData::MarkerHint(_) => PacketType::MarkerHint(),
            PacketData::DeviceControl(_) => PacketType::DeviceControl(),
            PacketData::SparseMarkersReport(_) => PacketType::SparseMarkersReport(),
            PacketData::AccelBatch(_) => PacketType::AccelBatch(),
        }
    }

    /// True for commands and their responses, false for streamed reports.
    ///
    /// BLE links carry the two kinds on separate L2CAP channels (control and
    /// data) so a burst of reports cannot delay a response. Every device and the
    /// dongle route by this one answer.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn is_control(&self) -> bool {
        match self.data {
            PacketData::StreamUpdate(_)
            | PacketData::LegacyWriteMode(_)
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
            | PacketData::LegacyObjectReportRequest()
            | PacketData::Ack()
            | PacketData::FlashSettings()
            | PacketData::Vendor(_, _)
            | PacketData::SetDeviceName(_)
            | PacketData::SetDeviceNameResponse(_)
            | PacketData::OtaRequest(_)
            | PacketData::OtaStatus(_)
            | PacketData::MarkerHint(_)
            | PacketData::DeviceControl(_) => true,
            PacketData::LegacyObjectReport(_)
            | PacketData::LegacyCombinedMarkersReport(_)
            | PacketData::MarkersReport(_)
            | PacketData::AccelReport(_)
            | PacketData::SparseMarkersReport(_)
            | PacketData::AccelBatch(_)
            | PacketData::ImpactReport(_)
            | PacketData::ImageChunk(_)
            | PacketData::ImageObject(_)
            | PacketData::BatteryReport(_) => false,
        }
    }

    /// The IMU reports of `self` followed by those of `next`, as one
    /// [`AccelBatch`](crate::AccelBatch) packet.
    ///
    /// Returns `None` unless both packets carry IMU reports for the same id and
    /// the reports fit one batch. A sender merges reports that are queued back to
    /// back this way; a receiver splits the batch into single reports again.
    ///
    /// Since: Unreleased.
    pub fn join_accel(&self, next: &Self) -> Option<Self> {
        if self.id != next.id {
            return None;
        }
        let mut reports = heapless::Vec::new();
        reports.extend_from_slice(self.data.accel_reports()?).ok()?;
        reports.extend_from_slice(next.data.accel_reports()?).ok()?;
        Some(Self {
            id: self.id,
            data: PacketData::AccelBatch(crate::AccelBatch {
                reports: reports.into(),
            }),
        })
    }
}

impl PacketData {
    pub(crate) fn accel_reports(&self) -> Option<&[AccelReport]> {
        match self {
            Self::AccelReport(report) => Some(core::slice::from_ref(report)),
            Self::AccelBatch(batch) => Some(&batch.reports),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn read_register_response(self) -> Option<ReadRegisterResponse> {
        match self {
            Self::ReadRegisterResponse(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn read_config_response(self) -> Option<GeneralConfig> {
        match self {
            Self::ReadConfigResponse(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn read_prop_response(self) -> Option<Props> {
        match self {
            Self::ReadPropResponse(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn legacy_object_report(self) -> Option<ObjectReport> {
        match self {
            Self::LegacyObjectReport(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn legacy_combined_markers_report(self) -> Option<CombinedMarkersReport> {
        match self {
            Self::LegacyCombinedMarkersReport(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn markers_report(self) -> Option<MarkersReport> {
        match self {
            Self::MarkersReport(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn accel_report(self) -> Option<AccelReport> {
        match self {
            Self::AccelReport(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn impact_report(self) -> Option<ImpactReport> {
        match self {
            Self::ImpactReport(x) => Some(x),
            _ => None,
        }
    }

    /// Since: Odyssey Desktop 0.6.0.
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

    /// The two tables (`u8 -> PacketType` and back) are written out by hand. A
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
            PacketType::try_from(0x22),
            Err(Error::UnrecognizedPacketId(0x22))
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
