/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[expect(
    clippy::large_enum_variant,
    reason = "wraps DeviceEvent, whose kind enum is large; boxing would change every constructor and pattern of this public enum in the hub server and the desktop app"
)]
#[repr(C)]
#[derive(Clone, Debug)]
pub enum Event {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    DeviceEvent(DeviceEvent),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    DonglePairingResult {
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        dongle_id: String,
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        success: bool,
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        paired_address: [u8; 6],
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        paired_name: String,
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        error: String,
    },
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Debug)]
pub struct DeviceEvent(pub crate::device::Device, pub DeviceEventKind);

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[expect(
    clippy::large_enum_variant,
    reason = "the packet variant is the large one; boxing it would change every constructor and pattern of this public enum in the hub server and the desktop app"
)]
#[repr(C)]
#[derive(Clone, Debug)]
pub enum DeviceEventKind {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    AccelerometerEvent(AccelerometerEvent),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    TrackingEvent(TrackingEvent),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    ImpactEvent(ImpactEvent),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    ZeroResult(bool),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    SaveZeroResult(bool),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    PacketEvent(odyssey_usb::packets::vm::Packet),
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    CapabilitiesChanged,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    PairingResult {
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        success: bool,
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        paired_address: [u8; 6],
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        paired_name: String,
        /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
        error: String,
    },
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    BatteryEvent(BatteryEvent),
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub rotation: nalgebra::Matrix3<f32>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub translation: nalgebra::Matrix3x1<f32>,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AccelerometerEvent {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub timestamp: u32,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub accel: nalgebra::Vector3<f32>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub gyro: nalgebra::Vector3<f32>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub euler_angles: nalgebra::Vector3<f32>,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrackingEvent {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub timestamp: u32,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub aimpoint: nalgebra::Vector2<f32>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub pose: Pose,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub distance: f32,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub screen_id: u32,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ImpactEvent {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub timestamp: u32,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BatteryEvent {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub percent: u8,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub charging: bool,
}
