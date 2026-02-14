use odyssey_hub_common as common;

use crate::funny::*;
use crate::packet::PacketEvent;

#[repr(C)]
#[derive(Clone)]
pub enum Event {
    DeviceEvent(DeviceEvent),
}

#[repr(C)]
#[derive(Clone)]
pub struct DeviceEvent {
    pub device: common::device::Device,
    pub kind: DeviceEventKind,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: Vector3f32,
    pub gyro: Vector3f32,
    pub euler_angles: Vector3f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Pose {
    pub rotation: Matrix3f32,
    pub translation: Matrix3x1f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: Vector2f32,
    pub pose: Pose,
    pub distance: f32,
    pub screen_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImpactEvent {
    pub timestamp: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScreenInfo {
    pub id: u8,
    pub bounds: [Vector2f32; 4],
}

#[repr(C)]
#[derive(Clone)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ZeroResult(bool),
    SaveZeroResult(bool),
    PacketEvent(PacketEvent),
    CapabilitiesChanged,
}

impl From<common::events::AccelerometerEvent> for AccelerometerEvent {
    fn from(e: common::events::AccelerometerEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            accel: e.accel.into(),
            gyro: e.gyro.into(),
            euler_angles: e.euler_angles.into(),
        }
    }
}

impl From<common::events::Pose> for Pose {
    fn from(p: common::events::Pose) -> Self {
        Self {
            rotation: p.rotation.into(),
            translation: p.translation.into(),
        }
    }
}

impl From<common::events::TrackingEvent> for TrackingEvent {
    fn from(e: common::events::TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: e.pose.into(),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<TrackingEvent> for common::events::TrackingEvent {
    fn from(e: TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: common::events::Pose {
                rotation: e.pose.rotation.into(),
                translation: e.pose.translation.into(),
            },
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<common::events::ImpactEvent> for ImpactEvent {
    fn from(e: common::events::ImpactEvent) -> Self {
        Self {
            timestamp: e.timestamp,
        }
    }
}

impl From<common::ScreenInfo> for ScreenInfo {
    fn from(s: common::ScreenInfo) -> Self {
        Self {
            id: s.id,
            bounds: [
                s.bounds[0].into(),
                s.bounds[1].into(),
                s.bounds[2].into(),
                s.bounds[3].into(),
            ],
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                Event::DeviceEvent(DeviceEvent {
                    device,
                    kind: match kind {
                        common::events::DeviceEventKind::AccelerometerEvent(e) => {
                            DeviceEventKind::AccelerometerEvent(e.into())
                        }
                        common::events::DeviceEventKind::TrackingEvent(e) => {
                            DeviceEventKind::TrackingEvent(e.into())
                        }
                        common::events::DeviceEventKind::ImpactEvent(e) => {
                            DeviceEventKind::ImpactEvent(e.into())
                        }
                        common::events::DeviceEventKind::ZeroResult(b) => {
                            DeviceEventKind::ZeroResult(b)
                        }
                        common::events::DeviceEventKind::SaveZeroResult(b) => {
                            DeviceEventKind::SaveZeroResult(b)
                        }
                        common::events::DeviceEventKind::PacketEvent(p) => {
                            DeviceEventKind::PacketEvent(p.into())
                        }
                        common::events::DeviceEventKind::CapabilitiesChanged => {
                            DeviceEventKind::CapabilitiesChanged
                        }
                    },
                })
            }
        }
    }
}
