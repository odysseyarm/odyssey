use std::collections::HashMap;
use std::time::Duration;

use btleplug::api::{Central as _, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use odyssey_hub_common::accessory::{AccessoryRadio, DiscoveredAccessory};
use odyssey_protocol::marker_control;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::transport::{PairingWindows, cache_discovery, profile, unix_time_ms};
use super::{RadioBackend, RadioDescriptor, RadioEvent, detected_type, io};

pub(super) struct NativeManager {
    manager: Manager,
    #[cfg(target_os = "linux")]
    bluez: bluez_async::BluetoothSession,
}

impl NativeManager {
    pub(super) async fn new() -> Result<Self, String> {
        let manager = io(Manager::new()).await?;
        #[cfg(target_os = "linux")]
        let bluez = bluez_async::BluetoothSession::new()
            .await
            .map_err(|e| e.to_string())?
            .1;
        Ok(Self {
            manager,
            #[cfg(target_os = "linux")]
            bluez,
        })
    }

    pub(super) async fn enumerate(&self) -> Result<Vec<RadioDescriptor>, String> {
        let adapters = io(self.manager.adapters()).await?;
        #[cfg(target_os = "linux")]
        let bluez = io(self.bluez.get_adapters()).await?;
        let mut radios = Vec::new();
        for (index, adapter) in adapters.into_iter().enumerate() {
            let description = io(adapter.adapter_info()).await?;
            #[cfg(target_os = "linux")]
            let (radio, name, available) = {
                let info = bluez
                    .iter()
                    .find(|info| {
                        description.split_whitespace().next() == Some(info.id.to_string().as_str())
                    })
                    .ok_or_else(|| format!("No stable BlueZ identity for {description}"))?;
                (
                    AccessoryRadio::Native {
                        id: info.mac_address.to_string(),
                    },
                    format!("{} ({})", info.alias, info.mac_address),
                    info.powered,
                )
            };
            #[cfg(not(target_os = "linux"))]
            let (radio, name, available) = (AccessoryRadio::System, description, true);
            radios.push(RadioDescriptor {
                radio,
                name,
                is_default: index == 0,
                available,
                backend: RadioBackend::Native(adapter),
            });
            #[cfg(not(target_os = "linux"))]
            break;
        }
        Ok(radios)
    }
}

pub(super) async fn discover(
    adapter: Adapter,
    radio: AccessoryRadio,
    nearby: watch::Sender<HashMap<[u8; 6], Peripheral>>,
    events: tokio::sync::mpsc::Sender<RadioEvent>,
    cancel: CancellationToken,
) {
    let run = async {
        io(adapter.start_scan(ScanFilter::default())).await?;
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        let mut discoveries = HashMap::new();
        let mut pairing_windows = PairingWindows::default();
        loop {
            tick.tick().await;
            let peripherals = io(adapter.peripherals()).await?;
            let mut by_address = HashMap::new();
            let mut found = Vec::new();
            for peripheral in peripherals {
                let Some(properties) = io(peripheral.properties()).await? else {
                    continue;
                };
                let address = properties.address.into_inner();
                let name = properties.local_name.unwrap_or_default();
                let Some(ty) = detected_type(&name, &properties.services, &properties.service_data)
                    .or_else(|| {
                        discoveries
                            .get(&address)
                            .map(|seen: &DiscoveredAccessory| seen.ty)
                    })
                else {
                    continue;
                };
                let accessory =
                    cache_discovery(&mut discoveries, address, profile(ty), name.as_bytes());
                if ty == odyssey_hub_common::accessory::AccessoryType::MarkerControlBoard
                    && let Some(seconds) = advertised_pairing_window(&properties.service_data)
                {
                    accessory.pairing_window_expires_at_ms =
                        pairing_windows.update(address, seconds, unix_time_ms());
                }
                found.push(accessory.clone());
                by_address.insert(address, peripheral);
            }
            nearby.send_replace(by_address);
            events
                .send(RadioEvent::Discovery {
                    radio: radio.clone(),
                    nearby: found,
                })
                .await
                .map_err(|e| e.to_string())?;
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
    let _ = io(adapter.stop_scan()).await;
}

fn advertised_pairing_window(service_data: &HashMap<Uuid, Vec<u8>>) -> Option<u8> {
    service_data
        .get(&Uuid::from_bytes(marker_control::SERVICE_UUID))
        .and_then(|data| marker_control::parse_pairing_state(data))
}

/// Linux needs an explicit BlueZ Pair request; btleplug exposes no bonding API.
#[cfg(target_os = "linux")]
pub(super) async fn pair(peripheral: &Peripheral) -> Result<(), String> {
    let (_, session) = bluez_async::BluetoothSession::new()
        .await
        .map_err(|e| e.to_string())?;
    let id = peripheral.id().to_string();
    let device = io(session.get_devices())
        .await?
        .into_iter()
        .find(|device| device.id.to_string() == id)
        .ok_or_else(|| format!("BlueZ device {id} unavailable"))?;
    if !device.paired {
        session
            .pair_with_timeout(&device.id, Duration::from_secs(25))
            .await
            .map_err(|e| e.to_string())?;
    }
    if !io(session.get_device_info(&device.id)).await?.paired {
        return Err("BlueZ did not establish a bond".into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) async fn pair(peripheral: &Peripheral) -> Result<(), String> {
    use windows::Devices::Bluetooth::BluetoothLEDevice;
    use windows::Devices::Enumeration::{DevicePairingProtectionLevel, DevicePairingResultStatus};
    let mut address = 0u64;
    for byte in peripheral.address().into_inner() {
        address = (address << 8) | u64::from(byte);
    }
    let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let pairing = device
        .DeviceInformation()
        .and_then(|info| info.Pairing())
        .map_err(|e| e.to_string())?;
    if !pairing.IsPaired().map_err(|e| e.to_string())? {
        let result = pairing
            .PairWithProtectionLevelAsync(DevicePairingProtectionLevel::Encryption)
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        let status = result.Status().map_err(|e| e.to_string())?;
        if status != DevicePairingResultStatus::Paired
            && status != DevicePairingResultStatus::AlreadyPaired
        {
            return Err(format!("Windows Bluetooth pairing failed: {status:?}"));
        }
    }
    if !pairing.IsPaired().map_err(|e| e.to_string())? {
        return Err("Windows did not establish a bond".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::accessory::AccessoryType;

    use super::*;

    #[test]
    fn service_data_detects_nameless_marker_without_assuming_pairing_mode() {
        let marker = Uuid::from_bytes(marker_control::SERVICE_UUID);
        let mut service_data =
            HashMap::from([(marker, vec![marker_control::PAIRING_STATE_VERSION, 30])]);
        assert_eq!(
            detected_type("", &[], &service_data),
            Some(AccessoryType::MarkerControlBoard)
        );
        assert_eq!(advertised_pairing_window(&service_data), Some(30));
        for data in [vec![], vec![1], vec![1, 31], vec![2, 30], vec![1, 30, 0]] {
            service_data.insert(marker, data);
            assert_eq!(advertised_pairing_window(&service_data), None);
        }
        service_data.clear();
        assert_eq!(advertised_pairing_window(&service_data), None);
        assert_eq!(detected_type("", &[], &service_data), None);
        assert_eq!(
            detected_type("Odyssey Marker", &[], &service_data),
            Some(AccessoryType::MarkerControlBoard)
        );
        assert_eq!(
            detected_type("", &[marker], &service_data),
            Some(AccessoryType::MarkerControlBoard)
        );
        service_data.insert(
            Uuid::from_bytes(marker_control::DEMAND_UUID),
            vec![marker_control::PAIRING_STATE_VERSION, 30],
        );
        assert_eq!(advertised_pairing_window(&service_data), None);
        assert_eq!(detected_type("", &[], &service_data), None);
        service_data.insert(marker, vec![marker_control::PAIRING_STATE_VERSION, 0]);
        assert_eq!(advertised_pairing_window(&service_data), Some(0));
    }
}
