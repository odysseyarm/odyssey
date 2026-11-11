use odyssey_hub_common as common;

use crate::funny::{Matrix3f32, Matrix3x1f32, Vector2f32, Vector3f32};
use crate::packet::PacketEvent;

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone)]
pub enum Event {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    DeviceEvent(DeviceEvent),
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone)]
pub struct DeviceEvent {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub device: common::device::Device,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub kind: DeviceEventKind,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy)]
pub struct AccelerometerEvent {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub timestamp: u32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub accel: Vector3f32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub gyro: Vector3f32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub euler_angles: Vector3f32,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy, Default)]
pub struct Pose {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub rotation: Matrix3f32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub translation: Matrix3x1f32,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy)]
pub struct TrackingEvent {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub timestamp: u32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub aimpoint: Vector2f32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub pose: Pose,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub distance: f32,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub screen_id: u32,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy)]
pub struct ImpactEvent {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub timestamp: u32,
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone, Copy)]
pub struct ScreenInfo {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub id: u8,
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    pub bounds: [Vector2f32; 4],
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[repr(C)]
#[derive(Debug)]
#[derive(Clone)]
pub enum DeviceEventKind {
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    AccelerometerEvent(AccelerometerEvent),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    TrackingEvent(TrackingEvent),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    ImpactEvent(ImpactEvent),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    ZeroResult(bool),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    SaveZeroResult(bool),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
    PacketEvent(PacketEvent),
    /// Since: FFI SDK 0.1.0 (earliest tagged release).
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

/// Events not represented by this ABI are skipped, never relabeled as another
/// event kind. In particular, dongle pairing notifications are not device events.
impl TryFrom<common::events::Event> for Event {
    type Error = ();

    fn try_from(event: common::events::Event) -> Result<Self, Self::Error> {
        match event {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                let kind = match kind {
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
                    common::events::DeviceEventKind::PairingResult { .. }
                    | common::events::DeviceEventKind::BatteryEvent(_) => return Err(()),
                };
                Ok(Self::DeviceEvent(DeviceEvent { device, kind }))
            }
            common::events::Event::DonglePairingResult { .. } => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrepresented_dongle_events_are_rejected_without_panicking() {
        let event = common::events::Event::DonglePairingResult {
            dongle_id: "dongle".to_string(),
            success: true,
            paired_address: [1, 2, 3, 4, 5, 6],
            paired_name: "device".to_string(),
            error: String::new(),
        };
        Event::try_from(event).expect_err("dongle pairing results have no native event");
    }
}
