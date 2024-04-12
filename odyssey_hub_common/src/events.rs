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
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub rotation: nalgebra::Matrix3<f64>,
    pub translation: nalgebra::Matrix3x1<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackingEvent {
    pub aimpoint: nalgebra::Vector2<f64>,
    pub pose: Option<Pose>,
}
