use odyssey_hub_common as common;

use super::hardware_address::address_from_bytes;
use crate::{
    Device, DeviceEvent, DonglePairingResultEvent, Event, Matrix3x1, Matrix3x3, Pose, Vector2,
    Vector3, device_event, event,
};

impl From<common::events::TrackingEvent> for device_event::TrackingEvent {
    fn from(e: common::events::TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: Some(Vector2::from(e.aimpoint)),
            pose: Some(Pose {
                rotation: Some(Matrix3x3::from(e.pose.rotation)),
                translation: Some(Matrix3x1::from(e.pose.translation)),
            }),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

/// Packet events carry a postcard payload, so encoding can fail and must not
/// unwind the hub's event-forwarding task.
impl TryFrom<common::events::Event> for Event {
    type Error = postcard::Error;

    fn try_from(value: common::events::Event) -> Result<Self, Self::Error> {
        Ok(match value {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                let device_pb: Device = device.into();
                let oneof = match kind {
                    common::events::DeviceEventKind::AccelerometerEvent(e) => {
                        device_event::DeviceEventOneof::Accelerometer(
                            device_event::AccelerometerEvent {
                                timestamp: e.timestamp,
                                acceleration: Some(Vector3::from(e.accel)),
                                angular_velocity: Some(Vector3::from(e.gyro)),
                                euler_angles: Some(Vector3::from(e.euler_angles)),
                            },
                        )
                    }
                    common::events::DeviceEventKind::TrackingEvent(e) => {
                        device_event::DeviceEventOneof::Tracking(e.into())
                    }
                    common::events::DeviceEventKind::ImpactEvent(e) => {
                        device_event::DeviceEventOneof::Impact(device_event::ImpactEvent {
                            timestamp: e.timestamp,
                        })
                    }
                    common::events::DeviceEventKind::PacketEvent(pkt) => {
                        device_event::DeviceEventOneof::Packet(device_event::PacketEvent {
                            bytes: postcard::to_allocvec(&pkt)?,
                        })
                    }
                    common::events::DeviceEventKind::ZeroResult(success) => {
                        device_event::DeviceEventOneof::ZeroResult(device_event::ZeroResultEvent {
                            success,
                        })
                    }
                    common::events::DeviceEventKind::SaveZeroResult(success) => {
                        device_event::DeviceEventOneof::SaveZeroResult(
                            device_event::SaveZeroResultEvent { success },
                        )
                    }
                    common::events::DeviceEventKind::CapabilitiesChanged => {
                        device_event::DeviceEventOneof::CapabilitiesChanged(
                            device_event::CapabilitiesChangedEvent {},
                        )
                    }
                    common::events::DeviceEventKind::PairingResult {
                        success,
                        paired_address,
                        error,
                        paired_name,
                    } => device_event::DeviceEventOneof::PairingResult(
                        device_event::PairingResultEvent {
                            success,
                            paired_address: paired_address.to_vec(),
                            error,
                            paired_name,
                        },
                    ),
                    common::events::DeviceEventKind::BatteryEvent(e) => {
                        device_event::DeviceEventOneof::Battery(device_event::BatteryEvent {
                            percent: u32::from(e.percent),
                            charging: e.charging,
                        })
                    }
                };
                Self {
                    event_oneof: Some(event::EventOneof::Device(DeviceEvent {
                        device: Some(device_pb),
                        device_event_oneof: Some(oneof),
                    })),
                }
            }
            common::events::Event::DonglePairingResult {
                dongle_id,
                success,
                paired_address,
                error,
                paired_name,
            } => Self {
                event_oneof: Some(event::EventOneof::DonglePairingResult(
                    DonglePairingResultEvent {
                        dongle_id,
                        success,
                        paired_address: paired_address.to_vec(),
                        error,
                        paired_name,
                    },
                )),
            },
        })
    }
}

impl TryFrom<Event> for common::events::Event {
    type Error = ();

    fn try_from(event: Event) -> Result<Self, ()> {
        match event.event_oneof.ok_or(())? {
            event::EventOneof::DonglePairingResult(e) => Ok(Self::DonglePairingResult {
                dongle_id: e.dongle_id,
                success: e.success,
                paired_address: address_from_bytes(&e.paired_address)
                    .map_err(|_conversion_error| ())?,
                error: e.error,
                paired_name: e.paired_name,
            }),
            event::EventOneof::Device(d) => {
                let device = d
                    .device
                    .ok_or(())?
                    .try_into()
                    .map_err(|_conversion_error| ())?;
                let kind = match d.device_event_oneof.ok_or(())? {
                    device_event::DeviceEventOneof::Accelerometer(e) => {
                        common::events::DeviceEventKind::AccelerometerEvent(
                            common::events::AccelerometerEvent {
                                timestamp: e.timestamp,
                                accel: e.acceleration.ok_or(())?.into(),
                                gyro: e.angular_velocity.ok_or(())?.into(),
                                euler_angles: e.euler_angles.ok_or(())?.into(),
                            },
                        )
                    }
                    device_event::DeviceEventOneof::Tracking(e) => {
                        common::events::DeviceEventKind::TrackingEvent(
                            common::events::TrackingEvent {
                                timestamp: e.timestamp,
                                aimpoint: e.aimpoint.ok_or(())?.into(),
                                pose: {
                                    let p = e.pose.ok_or(())?;
                                    common::events::Pose {
                                        rotation: p.rotation.ok_or(())?.into(),
                                        translation: p.translation.ok_or(())?.into(),
                                    }
                                },
                                distance: e.distance,
                                screen_id: e.screen_id,
                            },
                        )
                    }
                    device_event::DeviceEventOneof::Impact(e) => {
                        common::events::DeviceEventKind::ImpactEvent(common::events::ImpactEvent {
                            timestamp: e.timestamp,
                        })
                    }
                    device_event::DeviceEventOneof::Packet(device_event::PacketEvent { bytes }) => {
                        let pkt: odyssey_usb::packets::vm::Packet =
                            postcard::from_bytes(&bytes).map_err(|_conversion_error| ())?;
                        common::events::DeviceEventKind::PacketEvent(pkt)
                    }
                    device_event::DeviceEventOneof::ZeroResult(e) => {
                        common::events::DeviceEventKind::ZeroResult(e.success)
                    }
                    device_event::DeviceEventOneof::SaveZeroResult(e) => {
                        common::events::DeviceEventKind::SaveZeroResult(e.success)
                    }
                    device_event::DeviceEventOneof::CapabilitiesChanged(_) => {
                        common::events::DeviceEventKind::CapabilitiesChanged
                    }
                    device_event::DeviceEventOneof::PairingResult(e) => {
                        common::events::DeviceEventKind::PairingResult {
                            success: e.success,
                            paired_address: address_from_bytes(&e.paired_address)
                                .map_err(|_conversion_error| ())?,
                            error: e.error,
                            paired_name: e.paired_name,
                        }
                    }
                    device_event::DeviceEventOneof::Battery(e) => {
                        common::events::DeviceEventKind::BatteryEvent(
                            common::events::BatteryEvent {
                                percent: u8::try_from(e.percent).map_err(|_conversion_error| ())?,
                                charging: e.charging,
                            },
                        )
                    }
                };
                Ok(Self::DeviceEvent(common::events::DeviceEvent(device, kind)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_event(kind: device_event::DeviceEventOneof) -> Event {
        Event {
            event_oneof: Some(event::EventOneof::Device(DeviceEvent {
                device: Some(Device {
                    uuid: vec![0; 6],
                    ..Device::default()
                }),
                device_event_oneof: Some(kind),
            })),
        }
    }

    #[test]
    fn incomplete_tracking_pose_is_an_error() {
        let event = wire_event(device_event::DeviceEventOneof::Tracking(
            device_event::TrackingEvent {
                aimpoint: Some(Vector2::default()),
                pose: Some(Pose {
                    rotation: Some(Matrix3x3::default()),
                    translation: None,
                }),
                ..device_event::TrackingEvent::default()
            },
        ));
        common::events::Event::try_from(event)
            .expect_err("missing pose translation must be rejected");
    }

    #[test]
    fn incomplete_accelerometer_event_is_an_error() {
        let event = wire_event(device_event::DeviceEventOneof::Accelerometer(
            device_event::AccelerometerEvent {
                acceleration: Some(Vector3::default()),
                angular_velocity: None,
                euler_angles: Some(Vector3::default()),
                ..device_event::AccelerometerEvent::default()
            },
        ));
        common::events::Event::try_from(event)
            .expect_err("missing angular velocity must be rejected");
    }

    #[test]
    fn malformed_packet_is_an_error() {
        let event = wire_event(device_event::DeviceEventOneof::Packet(
            device_event::PacketEvent { bytes: vec![] },
        ));
        common::events::Event::try_from(event).expect_err("empty packet must be rejected");
    }
}
