#[derive(Clone, Debug)]
pub enum Event {
    None,
    DeviceEvent(DeviceEvent),
}

#[derive(Clone, Debug)]
pub struct DeviceEvent {
    pub device: crate::device::Device,
    pub kind: DeviceEventKind,
}

#[derive(Clone, Copy, Debug)]
pub enum DeviceEventKind {
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub rotation: nalgebra::Matrix3<f64>,
    pub translation: nalgebra::Matrix3x1<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: nalgebra::Vector2<f64>,
    pub pose: Option<Pose>,
}

#[derive(Clone, Copy, Debug)]
pub struct ImpactEvent {
    pub timestamp: u32,
}
