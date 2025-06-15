#[derive(Clone, Debug)]
pub enum Event {
    DeviceEvent(DeviceEvent),
}

#[derive(Clone, Debug)]
pub struct DeviceEvent(pub crate::device::Device, pub DeviceEventKind);

#[derive(Clone, Debug)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ConnectEvent,
    DisconnectEvent,
    ZeroResult(bool),
    SaveZeroResult(bool),
    PacketEvent(ats_usb::packets::vm::Packet),
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub rotation: nalgebra::Matrix3<f32>,
    pub translation: nalgebra::Matrix3x1<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: nalgebra::Vector3<f32>,
    pub gyro: nalgebra::Vector3<f32>,
    pub euler_angles: nalgebra::Vector3<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: nalgebra::Vector2<f32>,
    pub pose: Option<Pose>,
    pub distance: f32,
    pub screen_id: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ImpactEvent {
    pub timestamp: u32,
}
