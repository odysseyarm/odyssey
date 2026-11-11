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
#[protocol_type(no_cbor)]
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
    CameraModelNf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsicsPod<f32>),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    CameraModelWf(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] RosOpenCvIntrinsicsPod<f32>),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    StereoIso(#[cfg_attr(feature = "defmt", defmt(Debug2Format))] Isometry3<f32>),
}
