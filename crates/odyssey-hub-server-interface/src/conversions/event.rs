use odyssey_hub_common as common;

use crate::{
    Device, DeviceEvent, DonglePairingResultEvent, Event, Matrix3x1, Matrix3x3, Pose, Vector2,
    Vector3, device_event, event,
};

impl From<common::events::Event> for Event {
    fn from(value: common::events::Event) -> Self {
        match value {
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
                        device_event::DeviceEventOneof::Tracking(device_event::TrackingEvent {
                            timestamp: e.timestamp,
                            aimpoint: Some(Vector2::from(e.aimpoint)),
                            pose: Some(Pose {
                                rotation: Some(Matrix3x3::from(e.pose.rotation)),
                                translation: Some(Matrix3x1::from(e.pose.translation)),
                            }),
                            distance: e.distance,
                            screen_id: e.screen_id,
                        })
                    }
                    common::events::DeviceEventKind::ImpactEvent(e) => {
                        device_event::DeviceEventOneof::Impact(device_event::ImpactEvent {
                            timestamp: e.timestamp,
                        })
                    }
                    common::events::DeviceEventKind::PacketEvent(pkt) => {
                        device_event::DeviceEventOneof::Packet(device_event::PacketEvent {
                            bytes: postcard::to_allocvec(&pkt).expect("postcard serialize Packet"),
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
        }
    }
}

impl TryFrom<Event> for common::events::Event {
    type Error = ();

    fn try_from(event: Event) -> Result<Self, ()> {
        match event.event_oneof.ok_or(())? {
            event::EventOneof::DonglePairingResult(e) => {
                let mut paired_address = [0u8; 6];
                let len = e.paired_address.len().min(6);
                paired_address[..len].copy_from_slice(&e.paired_address[..len]);
                Ok(Self::DonglePairingResult {
                    dongle_id: e.dongle_id,
                    success: e.success,
                    paired_address,
                    error: e.error,
                    paired_name: e.paired_name,
                })
            }
            event::EventOneof::Device(d) => {
                let device: common::device::Device = match d.device {
                    Some(dev) => dev.into(),
                    None => return Err(()),
                };
                let kind = match d.device_event_oneof.ok_or(())? {
                    device_event::DeviceEventOneof::Accelerometer(e) => {
                        common::events::DeviceEventKind::AccelerometerEvent(
                            common::events::AccelerometerEvent {
                                timestamp: e.timestamp,
                                accel: e.acceleration.expect("acceleration").into(),
                                gyro: e.angular_velocity.expect("angular_velocity").into(),
                                euler_angles: e.euler_angles.expect("euler_angles").into(),
                            },
                        )
                    }
                    device_event::DeviceEventOneof::Tracking(e) => {
                        common::events::DeviceEventKind::TrackingEvent(
                            common::events::TrackingEvent {
                                timestamp: e.timestamp,
                                aimpoint: e.aimpoint.expect("aimpoint").into(),
                                pose: {
                                    let p = e.pose.expect("pose");
                                    common::events::Pose {
                                        rotation: p.rotation.expect("rotation").into(),
                                        translation: p.translation.expect("translation").into(),
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
                            postcard::from_bytes(&bytes).expect("postcard deserialize Packet");
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
                        let mut paired_address = [0u8; 6];
                        let len = e.paired_address.len().min(6);
                        paired_address[..len].copy_from_slice(&e.paired_address[..len]);
                        common::events::DeviceEventKind::PairingResult {
                            success: e.success,
                            paired_address,
                            error: e.error,
                            paired_name: e.paired_name,
                        }
                    }
                    device_event::DeviceEventOneof::Battery(e) => {
                        common::events::DeviceEventKind::BatteryEvent(
                            common::events::BatteryEvent {
                                percent: e.percent as u8,
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
