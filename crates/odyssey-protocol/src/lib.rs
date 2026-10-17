// Assume we're running on little-endian
#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "std")]
use std::{error::Error as StdError, fmt::Display, vec::Vec};

#[cfg(feature = "minicbor")]
use minicbor::{CborLen, Decode, Encode};

use core::mem::MaybeUninit;

use nalgebra::{Isometry3, Point2, Vector3};
use odyssey_protocol_macros::protocol_type;
use opencv_ros_camera::RosOpenCvIntrinsicsPod;

pub mod control;
pub mod mux;
#[cfg(feature = "pyo3")]
mod py;
pub mod wire;

/// A device name, at most 32 bytes.
///
/// A newtype because pyo3 cannot convert `heapless::String`. Serializes
/// transparently.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "serde", serde(transparent))]
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceName(pub heapless::String<32>);

impl DeviceName {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Truncates on overflow, matching how names arrive off the wire.
    pub fn from_str_lossy(s: &str) -> Self {
        let mut out = heapless::String::new();
        for c in s.chars() {
            if out.push(c).is_err() {
                break;
            }
        }
        Self(out)
    }
}

impl core::ops::Deref for DeviceName {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl From<heapless::String<32>> for DeviceName {
    fn from(value: heapless::String<32>) -> Self {
        Self(value)
    }
}

impl From<DeviceName> for heapless::String<32> {
    fn from(value: DeviceName) -> Self {
        value.0
    }
}

/// A bounded list. A newtype because pyo3 cannot convert `heapless::Vec`.
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[cfg_attr(feature = "serde", serde(bound(
    serialize = "T: serde::Serialize",
    deserialize = "T: serde::Deserialize<'de>"
)))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List<T, const N: usize>(pub heapless::Vec<T, N>);

impl<T, const N: usize> core::ops::Deref for List<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> From<heapless::Vec<T, N>> for List<T, N> {
    fn from(value: heapless::Vec<T, N>) -> Self {
        Self(value)
    }
}

impl<T, const N: usize> From<List<T, N>> for heapless::Vec<T, N> {
    fn from(value: List<T, N>) -> Self {
        value.0
    }
}

/// Why a rename was refused.
///
/// `TooLong` is reachable only if the caller stops truncating: `DeviceName`
/// holds 32 bytes, so a longer name has to be rejected before it reaches the
/// device. `Failed` covers the device-side causes — a flash write that did not
/// stick, or a state that forbids renaming.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(eq, eq_int))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SetDeviceNameError {
    #[cfg_attr(feature = "minicbor", n(0))]
    TooLong,
    #[cfg_attr(feature = "minicbor", n(1))]
    Failed,
}

/// The result of a rename request.
///
/// A dedicated enum rather than `Result` because pyo3 converts `Result` only
/// as a function return, where it raises; a status reported by the device is
/// data.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<(), SetDeviceNameError>",
        into = "Result<(), SetDeviceNameError>"
    )
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetDeviceNameResult {
    Ok(),
    Err(SetDeviceNameError),
}

impl From<Result<(), SetDeviceNameError>> for SetDeviceNameResult {
    fn from(value: Result<(), SetDeviceNameError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<SetDeviceNameResult> for Result<(), SetDeviceNameError> {
    fn from(value: SetDeviceNameResult) -> Self {
        match value {
            SetDeviceNameResult::Ok() => Ok(()),
            SetDeviceNameResult::Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "minicbor")]
impl<C> Encode<C> for SetDeviceNameResult {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        Result::<(), SetDeviceNameError>::from(*self).encode(e, ctx)
    }
}

#[cfg(feature = "minicbor")]
impl<'b, C> Decode<'b, C> for SetDeviceNameResult {
    fn decode(
        d: &mut minicbor::Decoder<'b>,
        ctx: &mut C,
    ) -> Result<Self, minicbor::decode::Error> {
        Result::<(), SetDeviceNameError>::decode(d, ctx).map(Self::from)
    }
}

#[cfg(feature = "minicbor")]
impl<C> CborLen<C> for SetDeviceNameResult {
    fn cbor_len(&self, ctx: &mut C) -> usize {
        Result::<(), SetDeviceNameError>::from(*self).cbor_len(ctx)
    }
}

pub trait Parse: Sized {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error>;
}

pub trait Serialize {
    const SIZE: usize;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]);

    #[cfg(feature = "std")]
    fn serialize_to_vec(&self, buf: &mut Vec<u8>) {
        buf.reserve(Self::SIZE);
        let _ = self.serialize(&mut buf.spare_capacity_mut());
        unsafe {
            buf.set_len(buf.len() + Self::SIZE);
        }
    }
}

#[inline]
fn push(buf: &mut &mut [MaybeUninit<u8>], data: &[u8]) {
    assert!(buf.len() >= data.len());
    let ptr = <[_]>::as_mut_ptr(buf) as *mut u8;
    unsafe {
        ptr.copy_from_nonoverlapping(data.as_ptr(), data.len());
    }
    *buf = &mut core::mem::take(buf)[data.len()..];
}

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
        #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
        GeneralConfig,
    ),
    #[cfg_attr(feature = "minicbor", n(4))]
    ReadConfig(#[cfg_attr(feature = "minicbor", n(0))] ConfigKind),
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
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
        #[cfg_attr(feature = "minicbor", cbor(with = "heapless_str32_cbor"))]
        DeviceName,
    ),
    #[cfg_attr(feature = "minicbor", n(23))]
    SetDeviceNameResponse(
        #[cfg_attr(feature = "minicbor", n(0))]
        SetDeviceNameResult,
    ),
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
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[repr(u8)]
#[derive(Clone, Copy, Debug, enumn::N, PartialEq)]
pub enum StreamUpdateAction {
    #[cfg_attr(feature = "minicbor", n(0))]
    Enable,
    #[cfg_attr(feature = "minicbor", n(1))]
    Disable,
    #[cfg_attr(feature = "minicbor", n(2))]
    DisableAll,
}

impl TryFrom<u8> for StreamUpdateAction {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        Self::n(n).ok_or(Error::UnrecognizedStreamUpdateAction(n))
    }
}

#[protocol_type]
#[repr(u8)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Object,
    #[cfg_attr(feature = "minicbor", n(1))]
    Image,
}

/// Version of the device wire format, reported by a device in
/// [`Props::Version`] and over the control endpoint. Bump it when the packets
/// in this module change shape. It is deliberately not the crate version: a
/// release that only refactors must not tell every device in the field that
/// the protocol moved, and `semver_at_least` gates on the host read this.
pub const DEVICE_PROTOCOL: [u16; 3] = [0, 1, 4];

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Version {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub protocol_semver: [u16; 3],
    #[cfg_attr(feature = "minicbor", n(1))]
    pub firmware_semver: [u16; 3],
}

impl Version {
    pub fn new(firmware_semver: [u16; 3]) -> Self {
        Self {
            protocol_semver: DEVICE_PROTOCOL,
            firmware_semver,
        }
    }
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Register {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub port: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub address: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WriteRegister {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub port: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub address: u8,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub data: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadRegisterResponse {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub address: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub data: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(from = "wire::AccelConfig", into = "wire::AccelConfig")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccelConfig {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub accel_odr: u16,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub b_x: f32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_y: f32,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub b_z: f32,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub s_x: f32,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub s_y: f32,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub s_z: f32,
}

impl Default for AccelConfig {
    fn default() -> Self {
        Self {
            accel_odr: 100,
            b_x: 0.0,
            b_y: 0.0,
            b_z: 0.0,
            s_x: 1.0,
            s_y: 1.0,
            s_z: 1.0,
        }
    }
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct GyroConfig {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub b_x: f32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub b_y: f32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_z: f32,
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfigKind {
    #[cfg_attr(feature = "minicbor", n(0))]
    ImpactThreshold = 0,
    #[cfg_attr(feature = "minicbor", n(1))]
    SuppressMs = 1,
    #[cfg_attr(feature = "minicbor", n(2))]
    AccelConfig = 2,
    #[cfg_attr(feature = "minicbor", n(3))]
    GyroConfig = 3,
    #[cfg_attr(feature = "minicbor", n(4))]
    CameraModelNf = 4,
    #[cfg_attr(feature = "minicbor", n(5))]
    CameraModelWf = 5,
    #[cfg_attr(feature = "minicbor", n(6))]
    StereoIso = 6,
}

#[protocol_type(no_cbor)]
#[repr(C)]
// Not a `#[pyclass]`: foreign payloads, converted through `wire::GeneralConfig`.
#[cfg_attr(
    feature = "serde",
    serde(from = "wire::GeneralConfig", into = "wire::GeneralConfig")
)]
#[derive(Clone, Debug, PartialEq)]
pub enum GeneralConfig {
    ImpactThreshold(u8),
    SuppressMs(u8),
    AccelConfig(AccelConfig),
    GyroConfig(GyroConfig),
    CameraModelNf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsicsPod<f32>),
    CameraModelWf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsicsPod<f32>),
    StereoIso(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] Isometry3<f32>),
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropKind {
    #[cfg_attr(feature = "minicbor", n(0))]
    Uuid = 0,
    #[cfg_attr(feature = "minicbor", n(1))]
    ProductId = 1,
    #[cfg_attr(feature = "minicbor", n(2))]
    Name = 2,
    #[cfg_attr(feature = "minicbor", n(3))]
    Version = 3,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Debug, PartialEq)]
pub enum Props {
    #[cfg_attr(feature = "minicbor", n(0))]
    Uuid(#[cfg_attr(feature = "minicbor", n(0))] [u8; 6]),
    #[cfg_attr(feature = "minicbor", n(1))]
    ProductId(#[cfg_attr(feature = "minicbor", n(0))] u16),
    #[cfg_attr(feature = "minicbor", n(2))]
    Name(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "heapless_str32_cbor"))]
        DeviceName,
    ),
    #[cfg_attr(feature = "minicbor", n(3))]
    Version(#[cfg_attr(feature = "minicbor", n(0))] Version),
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotData {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub area: u16,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub cx: u16,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub cy: u16,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub avg_brightness: u8,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub max_brightness: u8,
    #[cfg_attr(feature = "minicbor", n(5))]
    pub range: u8,
    #[cfg_attr(feature = "minicbor", n(6))]
    pub radius: u8,
    #[cfg_attr(feature = "minicbor", n(7))]
    pub boundary_left: u8,
    #[cfg_attr(feature = "minicbor", n(8))]
    pub boundary_right: u8,
    #[cfg_attr(feature = "minicbor", n(9))]
    pub boundary_up: u8,
    #[cfg_attr(feature = "minicbor", n(10))]
    pub boundary_down: u8,
    #[cfg_attr(feature = "minicbor", n(11))]
    pub aspect_ratio: u8,
    #[cfg_attr(feature = "minicbor", n(12))]
    pub vx: u8,
    #[cfg_attr(feature = "minicbor", n(13))]
    pub vy: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub mot_data_nf: [MotData; 16],
    #[cfg_attr(feature = "minicbor", n(2))]
    pub mot_data_wf: [MotData; 16],
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombinedMarkersReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub nf_points: [Point2<u16>; 16],
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub wf_points: [Point2<u16>; 16],
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PocMarkersReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub points: [Point2<u16>; 16],
}

impl From<PocMarkersReport> for CombinedMarkersReport {
    fn from(t: PocMarkersReport) -> Self {
        CombinedMarkersReport {
            nf_points: t.points,
            wf_points: Default::default(),
        }
    }
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AccelReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub accel: Vector3<f32>,
    #[cfg_attr(feature = "minicbor", n(2))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub gyro: Vector3<f32>,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub percent: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub charging: bool,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct StreamUpdate {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub packet_id: PacketType,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub action: StreamUpdateAction,
}

#[protocol_type]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum Error {
    #[cfg_attr(feature = "minicbor", n(0))]
    UnexpectedEof {
        #[cfg_attr(feature = "minicbor", n(0))]
        packet_type: Option<PacketType>,
    },
    #[cfg_attr(feature = "minicbor", n(1))]
    UnrecognizedPacketId(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(2))]
    UnrecognizedPort,
    #[cfg_attr(feature = "minicbor", n(3))]
    UnrecognizedStreamUpdateAction(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(4))]
    InvalidBitPattern,
}

#[cfg(feature = "std")]
impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Error as S;
        match self {
            S::UnexpectedEof { packet_type: None } => write!(f, "unexpected eof"),
            S::UnexpectedEof {
                packet_type: Some(p),
            } => write!(f, "unexpected eof, packet id {p:?}"),
            S::UnrecognizedPacketId(id) => write!(f, "unrecognized packet id {id}"),
            S::UnrecognizedPort => write!(f, "unrecognized port"),
            S::UnrecognizedStreamUpdateAction(n) => {
                write!(f, "unrecognized stream update action {n}")
            }
            S::InvalidBitPattern => write!(f, "invalid bit pattern"),
        }
    }
}

#[cfg(feature = "std")]
impl StdError for Error {}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Port {
    #[cfg_attr(feature = "minicbor", n(0))]
    Nf,
    #[cfg_attr(feature = "minicbor", n(1))]
    Wf,
}
impl TryFrom<u8> for Port {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        match n {
            0 => Ok(Self::Nf),
            1 => Ok(Self::Wf),
            _ => Err(Error::UnrecognizedPort),
        }
    }
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
            n if (PacketType::VendorStart().into()..PacketType::VendorEnd().into())
                .contains(&n) =>
            {
                Ok(Self::Vendor(n))
            }
            _ => Err(Error::UnrecognizedPacketId(n)),
        }
    }
}

impl From<PacketType> for u8 {
    fn from(ty: PacketType) -> u8 {
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
            PacketData::ReadRegisterResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn read_config_response(self) -> Option<GeneralConfig> {
        match self {
            PacketData::ReadConfigResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn read_prop_response(self) -> Option<Props> {
        match self {
            PacketData::ReadPropResponse(x) => Some(x),
            _ => None,
        }
    }

    pub fn object_report(self) -> Option<ObjectReport> {
        match self {
            PacketData::ObjectReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn combined_markers_report(self) -> Option<CombinedMarkersReport> {
        match self {
            PacketData::CombinedMarkersReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn poc_markers_report(self) -> Option<PocMarkersReport> {
        match self {
            PacketData::PocMarkersReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn accel_report(self) -> Option<AccelReport> {
        match self {
            PacketData::AccelReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn impact_report(self) -> Option<ImpactReport> {
        match self {
            PacketData::ImpactReport(x) => Some(x),
            _ => None,
        }
    }

    pub fn battery_report(self) -> Option<BatteryReport> {
        match self {
            PacketData::BatteryReport(x) => Some(x),
            _ => None,
        }
    }
}

impl Parse for MotData {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        let mot_data = MotData {
            area: bytes[0] as u16 | ((bytes[1] as u16) << 8),
            cx: bytes[2] as u16 | ((bytes[3] & 0x0f) as u16) << 8,
            cy: bytes[4] as u16 | ((bytes[5] & 0x0f) as u16) << 8,
            avg_brightness: bytes[6],
            max_brightness: bytes[7],
            radius: bytes[8] & 0x0f,
            range: bytes[8] >> 4,
            boundary_left: bytes[9] & 0x7f,
            boundary_right: bytes[10] & 0x7f,
            boundary_up: bytes[11] & 0x7f,
            boundary_down: bytes[12] & 0x7f,
            aspect_ratio: bytes[13],
            vx: bytes[14],
            vy: bytes[15],
        };
        *bytes = &bytes[16..];
        Ok(mot_data)
    }
}

impl Serialize for MotData {
    const SIZE: usize = 16;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        let data = [
            self.area as u8,
            (self.area >> 8) as u8,
            self.cx as u8,
            (self.cx >> 8) as u8,
            self.cy as u8,
            (self.cy >> 8) as u8,
            self.avg_brightness,
            self.max_brightness,
            self.radius | self.range << 4,
            self.boundary_left,
            self.boundary_right,
            self.boundary_up,
            self.boundary_down,
            self.aspect_ratio,
            self.vx,
            self.vy,
        ];
        let ptr = <[_]>::as_mut_ptr(buf) as *mut u8;
        unsafe {
            ptr.copy_from_nonoverlapping(data.as_ptr(), data.len());
        }
    }
}

impl Parse for ObjectReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        use Error as E;
        let timestamp = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        *bytes = &bytes[4..];
        let data = &mut &bytes[..512];
        *bytes = &bytes[512..];
        let [_format, _, ..] = **bytes else {
            return Err(E::UnexpectedEof {
                packet_type: Some(PacketType::ObjectReport()),
            });
        };
        *bytes = &bytes[2..];
        Ok(Self {
            timestamp,
            mot_data_nf: [(); 16].map(|_| MotData::parse(data).expect("MotData parse error")),
            mot_data_wf: [(); 16].map(|_| MotData::parse(data).expect("MotData parse error")),
        })
    }
}

impl Serialize for ObjectReport {
    const SIZE: usize = 518;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        push(buf, &self.timestamp.to_le_bytes());
        for i in 0..16 {
            self.mot_data_nf[i].serialize(buf);
        }
        for i in 0..16 {
            self.mot_data_wf[i].serialize(buf);
        }
        push(buf, &[1, 0]);
    }
}

impl Parse for CombinedMarkersReport {
    fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        use Error as E;
        let size = Self::SIZE as usize;
        if bytes.len() < size {
            return Err(E::UnexpectedEof {
                packet_type: Some(PacketType::CombinedMarkersReport()),
            });
        }

        let data = &mut &bytes[..size];
        *bytes = &bytes[size..];

        let mut positions = [Point2::new(0, 0); 16 * 2];
        for i in 0..positions.len() {
            // x, y is 12 bits each
            let x = u16::from_le_bytes([data[0], data[1] & 0x0f]);
            let y = (data[1] >> 4) as u16 | ((data[2] as u16) << 4);
            positions[i] = Point2::new(x, y);
            *data = &data[3..];
        }
        let nf_positions = positions[..16].try_into().unwrap();
        let wf_positions = positions[16..].try_into().unwrap();

        Ok(Self {
            nf_points: nf_positions,
            wf_points: wf_positions,
        })
    }
}

impl Serialize for CombinedMarkersReport {
    const SIZE: usize = 96;
    fn serialize(&self, buf: &mut &mut [MaybeUninit<u8>]) {
        for p in self.nf_points.iter().chain(&self.wf_points) {
            let (x, y) = (p.x, p.y);
            let byte0 = x & 0xff;
            let byte1 = ((x >> 8) & 0x0f) | ((y & 0x0f) << 4);
            let byte2 = y >> 4;
            push(buf, &[byte0 as u8, byte1 as u8, byte2 as u8]);
        }
    }
}

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl CombinedMarkersReport {
    #[getter]
    fn nf_points(&self) -> [[u16; 2]; 16] {
        self.nf_points.map(|p| [p.x, p.y])
    }
    #[getter]
    fn wf_points(&self) -> [[u16; 2]; 16] {
        self.wf_points.map(|p| [p.x, p.y])
    }
}

impl AccelReport {
    pub fn corrected_accel(&self, accel_config: &AccelConfig) -> Vector3<f32> {
        Vector3::new(
            (self.accel.x - accel_config.b_x) / accel_config.s_x,
            (self.accel.y - accel_config.b_y) / accel_config.s_y,
            (self.accel.z - accel_config.b_z) / accel_config.s_z,
        )
    }

    pub fn corrected_gyro(&self, gyro_config: &GyroConfig) -> Vector3<f32> {
        Vector3::new(
            self.gyro.x - gyro_config.b_x,
            self.gyro.y - gyro_config.b_y,
            self.gyro.z - gyro_config.b_z,
        )
    }
}

#[cfg(feature = "pyo3")]
#[pyo3::pymethods]
impl AccelReport {
    #[getter]
    fn timestamp(&self) -> u32 {
        self.timestamp
    }
}

impl StreamUpdate {
    pub fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        let stream_update = StreamUpdate {
            packet_id: bytes[0].try_into()?,
            action: bytes[1].try_into()?,
        };
        *bytes = &bytes[2..];
        Ok(stream_update)
    }
}

#[cfg(feature = "minicbor")]
mod heapless_str32_cbor {
    use minicbor::{decode::Error, encode::Error as EncodeError, encode::Write, Decoder, Encoder};

    use crate::DeviceName;

    pub fn encode<Ctx, W: Write>(
        s: &DeviceName,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), EncodeError<W::Error>> {
        e.str(s.as_str())?;
        Ok(())
    }

    pub fn decode<'b, Ctx>(d: &mut Decoder<'b>, _ctx: &mut Ctx) -> Result<DeviceName, Error> {
        Ok(DeviceName::from_str_lossy(d.str()?))
    }

    pub fn cbor_len<Ctx>(s: &DeviceName, _ctx: &mut Ctx) -> usize {
        let n = s.as_str().len();
        let header = if n <= 23 { 1 } else { 2 };
        header + n
    }
}

#[cfg(feature = "minicbor")]
mod serde_cbor_with {
    use minicbor::{Decoder, Encoder};
    use minicbor_serde::{Deserializer, Serializer};
    use serde::Serialize;

    pub fn encode<W, C, T>(
        v: &T,
        e: &mut Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>>
    where
        W: minicbor::encode::Write,
        T: Serialize,
    {
        let mut ser = Serializer::new(e.writer_mut());
        v.serialize(&mut ser)
            .map_err(|_| minicbor::encode::Error::message("serde encode error"))
    }

    pub fn decode<'b, C, T>(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<T, minicbor::decode::Error>
    where
        T: serde::de::DeserializeOwned, // owns its data; no borrows from input
    {
        let start = d.position();
        let rest = &d.input()[start..];

        let mut de = Deserializer::new(rest);
        let v: T = T::deserialize(&mut de)
            .map_err(|_| minicbor::decode::Error::message("serde decode error"))?;

        let consumed = de.decoder().position();
        d.set_position(start + consumed);
        Ok(v)
    }

    pub fn cbor_len<C, T>(v: &T, _ctx: &mut C) -> usize
    where
        T: Serialize,
    {
        struct CountingWriter(usize);

        impl minicbor::encode::Write for CountingWriter {
            type Error = core::convert::Infallible;

            fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
                self.0 += buf.len();
                Ok(())
            }
        }

        let mut writer = CountingWriter(0);
        {
            let mut encoder = Encoder::new(&mut writer);
            let mut ser = Serializer::new(encoder.writer_mut());
            v.serialize(&mut ser).unwrap_or_else(|_| ());
        }
        writer.0
    }
}

/// A version as published in the USB `bcdDevice` field.
///
/// Firmware and bootloaders put their version here so a host learns it from
/// enumeration alone, without opening the device or speaking the control
/// protocol.
///
/// The USB spec defines this field as binary-coded decimal with a single
/// sub-minor digit, which cannot represent a patch above 9 — ours are already
/// past that. The fields below are a private layout instead, so `lsusb` will
/// print something that is not the version. The widths are types, so a
/// component that cannot fit fails to compile at the call site.
#[bitbybit::bitfield(u16)]
#[derive(Debug, PartialEq, Eq)]
pub struct UsbDeviceRelease {
    #[bits(12..=15, rw)]
    pub major: arbitrary_int::u4,
    #[bits(6..=11, rw)]
    pub minor: arbitrary_int::u6,
    #[bits(0..=5, rw)]
    pub patch: arbitrary_int::u6,
}

impl UsbDeviceRelease {
    /// `major` 15 is reserved: it does not mean version 15, it means the
    /// remaining twelve bits use a layout this build does not know.
    ///
    /// Nothing is free to add later once devices ship, because a host cannot
    /// ask a device how to read a field it already published. Holding one major
    /// value back leaves room to switch to, say, a monotonic build counter
    /// without the two encodings being indistinguishable. A host that sees it
    /// must treat the version as unknown rather than guessing.
    pub const RESERVED_MAJOR: u16 = 15;

    /// Pack a version. Out-of-range components fail to build, at compile time
    /// when called from a `const`.
    pub const fn from_version(version: [u16; 3]) -> Self {
        let [major, minor, patch] = version;
        assert!(
            major < Self::RESERVED_MAJOR,
            "major 15 is reserved for a future layout"
        );
        Self::builder()
            .with_major(arbitrary_int::u4::new(major as u8))
            .with_minor(arbitrary_int::u6::new(minor as u8))
            .with_patch(arbitrary_int::u6::new(patch as u8))
            .build()
    }

    /// What the field says about the version.
    pub const fn report(self) -> UsbVersionReport {
        if self.raw_value() == 0 {
            return UsbVersionReport::Unset;
        }
        if self.major().value() as u16 == Self::RESERVED_MAJOR {
            return UsbVersionReport::Unreadable;
        }
        UsbVersionReport::Known([
            self.major().value() as u16,
            self.minor().value() as u16,
            self.patch().value() as u16,
        ])
    }
}

/// The three things `bcdDevice` can tell a host, which must not be conflated.
///
/// `Unset` and `Unreadable` both mean "no version", but they call for opposite
/// behaviour: the first is a device older than version reporting, which an
/// update should replace; the second is a device newer than this build, which
/// an update must leave alone or it would install something older.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbVersionReport {
    /// The field was never set. The device predates version reporting.
    Unset,
    /// The field uses a layout this build does not know, so it was written by
    /// something newer.
    Unreadable,
    Known([u16; 3]),
}

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ProductId {
    AtsVm = 0x520f,
    AtsLite = 0x5210,
    AtsPro = 0x5211,
    Mux = 0x5212,
}

impl ProductId {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x520f => Some(Self::AtsVm),
            0x5210 => Some(Self::AtsLite),
            0x5211 => Some(Self::AtsPro),
            0x5212 => Some(Self::Mux),
            _ => None,
        }
    }

    pub fn supports_control_endpoint(&self) -> bool {
        matches!(self, Self::AtsLite | Self::Mux)
    }
}
