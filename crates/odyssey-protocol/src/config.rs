use nalgebra::Isometry3;
use odyssey_protocol_macros::protocol_type;
use opencv_ros_camera::RosOpenCvIntrinsicsPod;

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(from = "crate::wire::AccelConfig", into = "crate::wire::AccelConfig")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccelConfig {
    /// How often the unit sends an `AccelReport`, in Hz. The sensor may sample faster.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub accel_odr: u16,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub b_x: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_y: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub b_z: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(4))]
    pub s_x: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(5))]
    pub s_y: f32,
    /// Since: Odyssey Desktop 0.6.0.
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

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct GyroConfig {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub b_x: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub b_y: f32,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub b_z: f32,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfigKind {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    ImpactThreshold = 0,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    SuppressMs = 1,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(2))]
    AccelConfig = 2,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(3))]
    GyroConfig = 3,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(4))]
    CameraModelNf = 4,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(5))]
    CameraModelWf = 5,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(6))]
    StereoIso = 6,
}

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[repr(C)]
// Not a `#[pyclass]`: foreign payloads, converted through `wire::GeneralConfig`.
#[cfg_attr(
    feature = "serde",
    serde(
        from = "crate::wire::GeneralConfig",
        into = "crate::wire::GeneralConfig"
    )
)]
#[derive(Clone, Debug, PartialEq)]
pub enum GeneralConfig {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ImpactThreshold(u8),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    SuppressMs(u8),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    AccelConfig(AccelConfig),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    GyroConfig(GyroConfig),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    CameraModelNf(RosOpenCvIntrinsicsPod<f32>),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    CameraModelWf(RosOpenCvIntrinsicsPod<f32>),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    StereoIso(Isometry3<f32>),
}

// Native defmt avoids linking core's float-to-text formatter into firmware.
#[cfg(feature = "defmt")]
impl defmt::Format for GeneralConfig {
    fn format(&self, fmt: defmt::Formatter<'_>) {
        match self {
            Self::ImpactThreshold(x) => defmt::write!(fmt, "ImpactThreshold({:?})", x),
            Self::SuppressMs(x) => defmt::write!(fmt, "SuppressMs({:?})", x),
            Self::AccelConfig(x) => defmt::write!(fmt, "AccelConfig({:?})", x),
            Self::GyroConfig(x) => defmt::write!(fmt, "GyroConfig({:?})", x),
            Self::CameraModelNf(x) => defmt::write!(
                fmt,
                "CameraModelNf {{ p: {:?}, k: {:?}, distortion: {:?}, rect: {:?} }}",
                x.p,
                x.k,
                x.distortion,
                x.rect
            ),
            Self::CameraModelWf(x) => defmt::write!(
                fmt,
                "CameraModelWf {{ p: {:?}, k: {:?}, distortion: {:?}, rect: {:?} }}",
                x.p,
                x.k,
                x.distortion,
                x.rect
            ),
            Self::StereoIso(x) => defmt::write!(
                fmt,
                "StereoIso {{ translation: {:?}, rotation: {:?} }}",
                x.translation.vector.as_slice(),
                x.rotation.coords.as_slice()
            ),
        }
    }
}
