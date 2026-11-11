use std::collections::HashMap;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use btleplug::api::{Characteristic, Peripheral as _, ValueNotification, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use futures::{Stream, StreamExt as _};
use odyssey_hub_common::accessory::{
    AccessoryInfo, AccessoryRadio, AccessoryType, DiscoveredAccessory,
};
use odyssey_protocol::accessory::{Capabilities, Command, Event, Profile, Reply};
use odyssey_protocol::marker_control::{self, Mode};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{RadioEvent, io, native};
use crate::dongle_list::DongleList;

const CONNECT_ATTEMPTS: usize = 4;

#[derive(Clone)]
pub(super) enum RadioBackend {
    Native(Adapter),
    Dongle(DongleList, String),
}

pub(super) enum Link {
    Native {
        peripheral: Peripheral,
        characteristic: Option<Characteristic>,
        // Mutex only makes the Send-only btleplug stream Sync so `&Link` can cross an await.
        notifications:
            Option<std::sync::Mutex<Pin<Box<dyn Stream<Item = ValueNotification> + Send>>>>,
    },
    Dongle {
        list: DongleList,
        id: String,
        address: [u8; 6],
        events: broadcast::Receiver<Event>,
    },
}

pub(super) const fn profile(ty: AccessoryType) -> Profile {
    match ty {
        AccessoryType::MarkerControlBoard => Profile::MarkerControlBoard,
        AccessoryType::DryFireMag => Profile::DryFireMag,
        AccessoryType::MantisX => Profile::MantisX,
    }
}

pub(super) const fn accessory_type(profile: Profile) -> AccessoryType {
    match profile {
        Profile::MarkerControlBoard => AccessoryType::MarkerControlBoard,
        Profile::DryFireMag => AccessoryType::DryFireMag,
        Profile::MantisX => AccessoryType::MantisX,
    }
}

impl RadioBackend {
    pub(super) async fn link(
        &self,
        address: [u8; 6],
        nearby: &mut watch::Receiver<HashMap<[u8; 6], Peripheral>>,
    ) -> Result<Link, String> {
        match self {
            Self::Native(_) => {
                let find = async {
                    loop {
                        if let Some(peripheral) = nearby.borrow_and_update().get(&address).cloned()
                        {
                            return Ok(Link::Native {
                                peripheral,
                                characteristic: None,
                                notifications: None,
                            });
                        }
                        nearby.changed().await.map_err(|e| e.to_string())?;
                    }
                };
                tokio::time::timeout(Duration::from_secs(10), find)
                    .await
                    .map_err(|_elapsed| "Accessory not discovered on selected radio".to_string())?
            }
            Self::Dongle(list, id) => Ok(Link::Dongle {
                list: list.clone(),
                id: id.clone(),
                address,
                events: list.subscribe_accessory_events(id).await?,
            }),
        }
    }
}

impl Link {
    pub(super) async fn connect(
        &mut self,
        info: &AccessoryInfo,
        enroll: bool,
    ) -> Result<(), String> {
        match self {
            Self::Native {
                peripheral,
                characteristic,
                notifications,
            } => {
                // Weak links miss a connection's first events (HCI 0x3E); BlueZ gives up without retrying.
                let mut connected = io(peripheral.connect()).await;
                for _ in 1..CONNECT_ATTEMPTS {
                    let Err(error) = &connected else { break };
                    tracing::info!(%error, "Retrying Bluetooth connect");
                    connected = io(peripheral.connect()).await;
                }
                connected?;
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                if enroll && info.ty == AccessoryType::MarkerControlBoard {
                    tokio::time::timeout(Duration::from_secs(28), native::pair(peripheral))
                        .await
                        .map_err(|_elapsed| "Bluetooth pairing timed out".to_string())??;
                }
                io(peripheral.discover_services()).await?;
                let uuid = Uuid::from_bytes(profile(info.ty).characteristic_uuid());
                let found = peripheral
                    .characteristics()
                    .into_iter()
                    .find(|c| c.uuid == uuid)
                    .ok_or_else(|| format!("Accessory characteristic {uuid} unavailable"))?;
                if info.ty == AccessoryType::MarkerControlBoard {
                    if found.service_uuid != Uuid::from_bytes(marker_control::SERVICE_UUID)
                        || !found
                            .properties
                            .contains(btleplug::api::CharPropFlags::WRITE)
                    {
                        return Err(
                            "Marker demand characteristic does not support acknowledged writes"
                                .into(),
                        );
                    }
                    let timeout = Duration::from_secs(if enroll { 28 } else { 1 });
                    let accepted = tokio::time::timeout(
                        timeout,
                        peripheral.write(&found, &[0], WriteType::WithResponse),
                    )
                    .await
                    .map_err(|_elapsed| "Marker secure access timed out".to_string())?
                    .map_err(|e| e.to_string());
                    *characteristic = Some(found);
                    accepted?;
                } else {
                    io(peripheral.subscribe(&found)).await?;
                    *notifications =
                        Some(std::sync::Mutex::new(io(peripheral.notifications()).await?));
                    *characteristic = Some(found);
                }
                Ok(())
            }
            Self::Dongle {
                list, id, address, ..
            } => {
                done(
                    list,
                    id,
                    Command::Connect {
                        address: *address,
                        profile: profile(info.ty),
                        enroll,
                    },
                )
                .await?;
                if info.ty == AccessoryType::MarkerControlBoard {
                    done(
                        list,
                        id,
                        Command::MarkerDemand {
                            address: *address,
                            active: false,
                        },
                    )
                    .await?;
                }
                Ok(())
            }
        }
    }

    pub(super) async fn marker_demand(&self, active: bool) -> Result<(), String> {
        match self {
            Self::Native {
                peripheral,
                characteristic,
                ..
            } => {
                let characteristic = characteristic
                    .as_ref()
                    .ok_or("Marker characteristic unavailable")?;
                tokio::time::timeout(
                    Duration::from_secs(1),
                    peripheral.write(characteristic, &[u8::from(active)], WriteType::WithResponse),
                )
                .await
                .map_err(|_elapsed| "Marker demand write timed out".to_string())?
                .map_err(|e| e.to_string())
            }
            Self::Dongle {
                list, id, address, ..
            } => {
                done(
                    list,
                    id,
                    Command::MarkerDemand {
                        address: *address,
                        active,
                    },
                )
                .await
            }
        }
    }

    pub(super) async fn marker_mode(&self, mode: Mode) -> Result<(), String> {
        match self {
            Self::Native { peripheral, .. } => {
                let uuid = Uuid::from_bytes(marker_control::MODE_UUID);
                let characteristic = peripheral
                    .characteristics()
                    .into_iter()
                    .find(|c| {
                        c.uuid == uuid
                            && c.service_uuid == Uuid::from_bytes(marker_control::SERVICE_UUID)
                    })
                    .ok_or("Marker mode characteristic unavailable")?;
                tokio::time::timeout(
                    Duration::from_secs(3),
                    peripheral.write(&characteristic, &[mode as u8], WriteType::WithResponse),
                )
                .await
                .map_err(|_elapsed| "Marker mode write timed out".to_string())?
                .map_err(|e| e.to_string())
            }
            Self::Dongle {
                list, id, address, ..
            } => {
                done(
                    list,
                    id,
                    Command::MarkerMode {
                        address: *address,
                        mode,
                    },
                )
                .await
            }
        }
    }

    pub(super) async fn notification(&mut self, ty: AccessoryType) -> Result<Vec<u8>, String> {
        match self {
            Self::Native {
                notifications,
                characteristic,
                ..
            } => {
                let stream = notifications
                    .as_mut()
                    .ok_or("Accessory notifications unavailable")?
                    .get_mut()
                    .map_err(|e| e.to_string())?;
                loop {
                    let notification =
                        stream.next().await.ok_or("Accessory notifications ended")?;
                    if characteristic
                        .as_ref()
                        .is_some_and(|c| c.uuid == notification.uuid)
                    {
                        return Ok(notification.value);
                    }
                }
            }
            Self::Dongle {
                address, events, ..
            } => loop {
                match events.recv().await.map_err(|e| e.to_string())? {
                    Event::Notification {
                        address: peer,
                        profile: p,
                        data,
                    } if peer == *address && p == profile(ty) => {
                        return Ok(data.0.to_vec());
                    }
                    Event::Disconnected { address: peer } if peer == *address => {
                        return Err("Accessory disconnected".into());
                    }
                    Event::Error {
                        address: peer,
                        error,
                    } if peer == *address => return Err(error.to_string()),
                    _ => {}
                }
            },
        }
    }

    pub(super) async fn disconnect(&self, marker: bool) -> Result<(), String> {
        let off = if marker {
            self.marker_demand(false).await
        } else {
            Ok(())
        };
        let disconnected = match self {
            Self::Native { peripheral, .. } => io(peripheral.disconnect()).await,
            Self::Dongle {
                list, id, address, ..
            } => done(list, id, Command::Disconnect { address: *address }).await,
        };
        off.and(disconnected)
    }
}

async fn done(list: &DongleList, id: &str, command: Command) -> Result<(), String> {
    match list.accessory_request(id, command).await? {
        Reply::Done() => Ok(()),
        Reply::Capabilities(_) => Err("Unexpected dongle accessory reply".into()),
    }
}

pub(super) async fn discover_dongle(
    list: DongleList,
    id: String,
    events: mpsc::Sender<RadioEvent>,
    cancel: CancellationToken,
) {
    let radio = AccessoryRadio::Dongle { id: id.clone() };
    let run = async {
        let mut notifications = list.subscribe_accessory_events(&id).await?;
        publish_capabilities(&list, &id, &radio, &events).await?;
        done(&list, &id, Command::Scan { enabled: true }).await?;
        let mut nearby = HashMap::new();
        let mut pairing_windows = PairingWindows::default();
        let mut poll = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = poll.tick() => { publish_capabilities(&list, &id, &radio, &events).await?; }
                event = notifications.recv() => match event.map_err(|e| e.to_string())? {
                    Event::Discovered { address, profile, name } => {
                        cache_discovery(&mut nearby, address, profile, &name.0);
                        events.send(RadioEvent::Discovery { radio: radio.clone(), nearby: nearby.values().cloned().collect() }).await.map_err(|e| e.to_string())?;
                    }
                    Event::PairingWindow { address, seconds_remaining } => {
                        if let Some(expires_at_ms) = pairing_windows.update(address, seconds_remaining, unix_time_ms()) {
                            let accessory = cache_discovery(&mut nearby, address, Profile::MarkerControlBoard, b"");
                            accessory.pairing_window_expires_at_ms = Some(expires_at_ms);
                            events.send(RadioEvent::Discovery { radio: radio.clone(), nearby: nearby.values().cloned().collect() }).await.map_err(|e| e.to_string())?;
                        }
                    }
                    _ => {}
                }
            }
        }
        #[expect(
            unreachable_code,
            reason = "the loop never breaks; the Ok fixes the block's error type"
        )]
        Ok::<(), String>(())
    };
    tokio::select! {
        () = cancel.cancelled() => {}
        result = run => if let Err(error) = result {
            let _ = events.send(RadioEvent::RadioError { radio, error }).await;
        }
    }
    if list.task_sender(&id).is_some() {
        let _ = done(&list, &id, Command::Scan { enabled: false }).await;
    }
}

pub(super) fn cache_discovery<'a>(
    nearby: &'a mut HashMap<[u8; 6], DiscoveredAccessory>,
    address: [u8; 6],
    profile: Profile,
    name: &[u8],
) -> &'a mut DiscoveredAccessory {
    let accessory = nearby
        .entry(address)
        .or_insert_with(|| DiscoveredAccessory {
            address,
            name: String::new(),
            ty: accessory_type(profile),
            pairing_window_expires_at_ms: None,
        });
    if !name.is_empty() {
        accessory.name = String::from_utf8_lossy(name).into_owned();
    }
    accessory.ty = accessory_type(profile);
    accessory
}

#[derive(Default)]
pub(super) struct PairingWindows(HashMap<[u8; 6], (u8, u64)>);

impl PairingWindows {
    pub(super) fn update(&mut self, address: [u8; 6], seconds: u8, now_ms: u64) -> Option<u64> {
        if seconds > marker_control::ENROLLMENT_SECONDS {
            return None;
        }
        let state = self.0.entry(address).or_insert((u8::MAX, 0));
        // Native BLE properties can replay stale data; identical seconds never renew.
        if state.0 != seconds {
            *state = (
                seconds,
                if seconds == 0 {
                    0
                } else {
                    now_ms.saturating_add(u64::from(seconds) * 1000)
                },
            );
        }
        Some(state.1)
    }
}

pub(super) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn publish_capabilities(
    list: &DongleList,
    id: &str,
    radio: &AccessoryRadio,
    events: &mpsc::Sender<RadioEvent>,
) -> Result<(), String> {
    let Reply::Capabilities(capabilities) =
        list.accessory_request(id, Command::Capabilities()).await?
    else {
        return Err("Dongle did not report accessory capabilities".into());
    };
    let Capabilities {
        total_capacity,
        tracker_capacity,
        accessory_capacity,
        connected_trackers,
        connected_accessories,
        connecting_accessories,
    } = capabilities;
    if u32::from(connected_trackers)
        + u32::from(connected_accessories)
        + u32::from(connecting_accessories)
        > u32::from(total_capacity)
        || u32::from(connected_accessories) + u32::from(connecting_accessories)
            > u32::from(accessory_capacity)
        || connected_trackers > tracker_capacity
        || accessory_capacity > total_capacity
    {
        return Err("Dongle reported invalid accessory resource usage".into());
    }
    events
        .send(RadioEvent::Capabilities {
            radio: radio.clone(),
            capabilities,
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_names_merge_per_address_and_radio() {
        let mut nearby = HashMap::new();
        let address = [1; 6];
        let other_address = [2; 6];

        cache_discovery(&mut nearby, address, Profile::DryFireMag, b"");
        assert_eq!(
            nearby[&address],
            DiscoveredAccessory {
                address,
                name: String::new(),
                ty: AccessoryType::DryFireMag,
                pairing_window_expires_at_ms: None,
            }
        );
        cache_discovery(&mut nearby, address, Profile::DryFireMag, b"mag");
        assert_eq!(nearby[&address].name, "mag");
        cache_discovery(&mut nearby, other_address, Profile::MarkerControlBoard, b"");
        cache_discovery(&mut nearby, address, Profile::DryFireMag, b"");
        assert_eq!(
            nearby[&address],
            DiscoveredAccessory {
                address,
                name: "mag".into(),
                ty: AccessoryType::DryFireMag,
                pairing_window_expires_at_ms: None,
            }
        );
        cache_discovery(&mut nearby, address, Profile::MantisX, b"renamed");
        assert_eq!(
            nearby[&address],
            DiscoveredAccessory {
                address,
                name: "renamed".into(),
                ty: AccessoryType::MantisX,
                pairing_window_expires_at_ms: None,
            }
        );
        assert_eq!(
            nearby[&other_address],
            DiscoveredAccessory {
                address: other_address,
                name: String::new(),
                ty: AccessoryType::MarkerControlBoard,
                pairing_window_expires_at_ms: None,
            }
        );

        let mut other_radio = HashMap::new();
        cache_discovery(&mut other_radio, address, Profile::DryFireMag, b"");
        assert_eq!(other_radio[&address].name, "");
        assert_eq!(nearby[&address].name, "renamed");
    }

    #[test]
    fn pairing_deadlines_reject_invalid_values_and_never_renew_identical_seconds() {
        let mut windows = PairingWindows::default();
        let address = [1; 6];
        assert_eq!(windows.update(address, 31, 1000), None);
        assert_eq!(windows.update(address, 30, 1000), Some(31_000));
        for now in [2000, 31_000, u64::MAX] {
            assert_eq!(windows.update(address, 30, now), Some(31_000));
        }
        assert_eq!(windows.update(address, 29, 3000), Some(32_000));
        assert_eq!(windows.update(address, 30, 4000), Some(34_000));
        assert_eq!(windows.update(address, 0, 5000), Some(0));
        assert_eq!(windows.update(address, 0, 6000), Some(0));
        assert_eq!(windows.update(address, 30, 7000), Some(37_000));
        assert_eq!(windows.update(address, u8::MAX, 8000), None);
        assert_eq!(windows.update(address, 30, 9000), Some(37_000));
        assert_eq!(windows.update(address, 1, u64::MAX - 500), Some(u64::MAX));
        assert_eq!(windows.update(address, 0, u64::MAX), Some(0));
    }

    #[test]
    fn pairing_windows_are_isolated_by_address_and_radio() {
        let mut first_radio = PairingWindows::default();
        let mut second_radio = PairingWindows::default();
        assert_eq!(first_radio.update([1; 6], 30, 1000), Some(31_000));
        assert_eq!(first_radio.update([2; 6], 30, 2000), Some(32_000));
        assert_eq!(second_radio.update([1; 6], 30, 3000), Some(33_000));
        assert_eq!(first_radio.update([1; 6], 0, 4000), Some(0));
        assert_eq!(first_radio.update([2; 6], 30, 5000), Some(32_000));
        assert_eq!(second_radio.update([1; 6], 30, 6000), Some(33_000));
    }

    #[test]
    fn pairing_status_and_scan_response_names_preserve_each_other() {
        let mut nearby = HashMap::new();
        let address = [1; 6];
        let marker = Profile::MarkerControlBoard;
        let accessory = cache_discovery(&mut nearby, address, marker, b"Odyssey Marker");
        assert_eq!(accessory.pairing_window_expires_at_ms, None);
        accessory.pairing_window_expires_at_ms = Some(31_000);
        let accessory = cache_discovery(&mut nearby, address, marker, b"");
        assert_eq!(accessory.name, "Odyssey Marker");
        assert_eq!(accessory.pairing_window_expires_at_ms, Some(31_000));
        let accessory = cache_discovery(&mut nearby, address, marker, b"Odyssey Marker renamed");
        assert_eq!(accessory.pairing_window_expires_at_ms, Some(31_000));
        accessory.pairing_window_expires_at_ms = Some(0);
        let accessory = cache_discovery(&mut nearby, address, marker, b"");
        assert_eq!(accessory.name, "Odyssey Marker renamed");
        assert_eq!(accessory.pairing_window_expires_at_ms, Some(0));
        let other = cache_discovery(&mut nearby, [2; 6], marker, b"");
        assert_eq!(other.pairing_window_expires_at_ms, None);
    }
}
