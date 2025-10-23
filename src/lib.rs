// Assume we're running on little-endian
#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "std")]
use std::{error::Error as StdError, fmt::Display, vec::Vec};

#[cfg(feature = "minicbor")]
use minicbor::{Decode, Encode};

use core::mem::MaybeUninit;

use nalgebra::{Isometry3, Point2, Vector3};
use opencv_ros_camera::RosOpenCvIntrinsics;

pub mod hub;
pub mod wire;

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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Debug)]
pub struct Packet {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub data: PacketData,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub id: u8,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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
    ReadConfig(),
    #[cfg_attr(feature = "minicbor", n(5))]
    ReadConfigResponse(
        #[cfg_attr(feature = "minicbor", n(0))]
        #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
        GeneralConfig,
    ),
    #[cfg_attr(feature = "minicbor", n(6))]
    ReadProps(),
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropsResponse(#[cfg_attr(feature = "minicbor", n(0))] Props),
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
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Debug)]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[repr(u8)]
#[derive(Copy, Clone, Debug, enumn::N, PartialEq)]
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

#[repr(u8)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Object,
    #[cfg_attr(feature = "minicbor", n(1))]
    Image,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Version {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub protocol_semver: [u16; 3],
    #[cfg_attr(feature = "minicbor", n(1))]
    pub firmware_semver: [u16; 3],
}

impl Version {
    pub fn new(firmware_semver: [u16; 3]) -> Self {
        const PROTO_MAJOR: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MAJOR"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_MAJOR"),
            };
        const PROTO_MINOR: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MINOR"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_MINOR"),
            };
        const PROTO_PATCH: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_PATCH"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_PATCH"),
            };
        Self {
            protocol_semver: [PROTO_MAJOR, PROTO_MINOR, PROTO_PATCH],
            firmware_semver,
        }
    }
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Register {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub port: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub address: u8,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadRegisterResponse {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub address: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub data: u8,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(from = "wire::AccelConfig", into = "wire::AccelConfig")
)]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Default, Clone, Debug, PartialEq)]
pub struct GyroConfig {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub b_x: f32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub b_y: f32,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_z: f32,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(from = "wire::GeneralConfig", into = "wire::GeneralConfig")
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Debug, PartialEq)]
pub enum GeneralConfig {
    ImpactThreshold(u8),
    SuppressMs(u8),
    AccelConfig(AccelConfig),
    GyroConfig(GyroConfig),
    CameraModelNf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsics<f32>),
    CameraModelWf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsics<f32>),
    StereoIso(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] Isometry3<f32>),
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Props {
    #[cfg_attr(feature = "minicbor", n(0))]
    Uuid(#[cfg_attr(feature = "minicbor", n(0))] [u8; 6]),
    #[cfg_attr(feature = "minicbor", n(1))]
    ProductId(#[cfg_attr(feature = "minicbor", n(0))] u16),
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub mot_data_nf: [MotData; 16],
    #[cfg_attr(feature = "minicbor", n(2))]
    pub mot_data_wf: [MotData; 16],
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombinedMarkersReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub nf_points: [Point2<u16>; 16],
    #[cfg_attr(feature = "minicbor", n(1))]
    #[cfg_attr(feature = "minicbor", cbor(with = "serde_cbor_with"))]
    pub wf_points: [Point2<u16>; 16],
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactReport {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub timestamp: u32,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
#[derive(Clone, Copy, Debug)]
pub struct StreamUpdate {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub packet_id: PacketType,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub action: StreamUpdateAction,
}

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(Encode, Decode))]
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
    ReadProps(),
    #[cfg_attr(feature = "minicbor", n(7))]
    ReadPropsResponse(),
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
            0x06 => Ok(Self::ReadProps()),
            0x07 => Ok(Self::ReadPropsResponse()),
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
            PacketType::ReadProps() => 0x06,
            PacketType::ReadPropsResponse() => 0x07,
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
            PacketData::ReadConfig() => PacketType::ReadConfig(),
            PacketData::ReadConfigResponse(_) => PacketType::ReadConfigResponse(),
            PacketData::ReadProps() => PacketType::ReadProps(),
            PacketData::ReadPropsResponse(_) => PacketType::ReadPropsResponse(),
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

    pub fn read_props_response(self) -> Option<Props> {
        match self {
            PacketData::ReadPropsResponse(x) => Some(x),
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
}
