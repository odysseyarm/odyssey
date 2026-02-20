use std::collections::HashMap;

use dioxus::{logger::tracing, prelude::*};
use futures::StreamExt;
use odyssey_hub_client::client::{Client, DongleInfo};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events as oe;
use slab::Slab;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct HubContext {
    pub client: SyncSignal<Client>,
    pub devices: SyncSignal<Slab<Device>>,
    pub dongles: SyncSignal<Vec<DongleInfo>>,
    pub latest_event: Signal<Option<oe::Event>>,
    pub tracking_events: broadcast::Sender<(Device, oe::TrackingEvent)>,
    device_keys: SyncSignal<HashMap<odyssey_hub_common::device::Device, usize>>,
}

impl HubContext {
    pub fn new() -> Self {
        let (tracking_events, _) = broadcast::channel(128);
        Self {
            client: SyncSignal::new_maybe_sync(Client::default()),
            devices: SyncSignal::new_maybe_sync(Slab::new()),
            dongles: SyncSignal::new_maybe_sync(Vec::new()),
            latest_event: Signal::new(None),
            tracking_events,
            device_keys: SyncSignal::new_maybe_sync(HashMap::new()),
        }
    }

    fn replace_devices(&mut self, list: Vec<Device>) {
        let mut devices = self.devices.write();
        let mut keys = self.device_keys.write();
        devices.clear();
        keys.clear();

        for device in list {
            let idx = devices.insert(device.clone());
            keys.insert(device, idx);
        }
    }

    pub async fn run(&mut self) {
        loop {
            tracing::info!("Hub connecting to server...");
            let (device_list_stream, dongle_list_stream, event_stream) = {
                let mut client = self.client.write();
                match client.connect().await {
                    Ok(_) => {
                        tracing::info!("Hub connected successfully");
                    }
                    Err(e) => {
                        tracing::error!("Failed to connect to hub server: {}", e);
                        drop(client);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                }

                let list = match client.get_device_list().await {
                    Ok(list) => list,
                    Err(e) => {
                        tracing::error!("Failed to get device list: {}", e);
                        drop(client);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                let device_list_stream = match client.subscribe_device_list().await {
                    Ok(stream) => stream,
                    Err(e) => {
                        tracing::error!("Failed to subscribe to device list: {}", e);
                        drop(client);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                let dongle_list_stream = match client.subscribe_dongle_list().await {
                    Ok(stream) => stream,
                    Err(e) => {
                        tracing::error!("Failed to subscribe to dongle list: {}", e);
                        drop(client);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                let event_stream = match client.subscribe_events().await {
                    Ok(stream) => stream,
                    Err(e) => {
                        tracing::error!("Failed to subscribe to events: {}", e);
                        drop(client);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };

                drop(client);
                self.replace_devices(list);

                (device_list_stream, dongle_list_stream, event_stream)
            };

            let mut devices_signal = self.devices.clone();
            let mut device_keys = self.device_keys.clone();
            tokio::spawn(async move {
                let mut device_list_stream = device_list_stream;
                while let Some(update) = device_list_stream.next().await {
                    match update {
                        Ok(list) => {
                            tracing::debug!("Device list update: {} devices", list.len());
                            let mut devices = devices_signal.write();
                            let mut keys = device_keys.write();
                            devices.clear();
                            keys.clear();
                            for device in list {
                                let idx = devices.insert(device.clone());
                                keys.insert(device, idx);
                            }
                        }
                        Err(e) => {
                            tracing::error!("Device list stream error: {}", e);
                            break;
                        }
                    }
                }
            });

            let mut dongles_signal = self.dongles.clone();
            tokio::spawn(async move {
                let mut dongle_list_stream = dongle_list_stream;
                while let Some(update) = dongle_list_stream.next().await {
                    match update {
                        Ok(list) => {
                            tracing::debug!("Dongle list update: {} dongles", list.len());
                            dongles_signal.set(list);
                        }
                        Err(e) => {
                            tracing::error!("Dongle list stream error: {}", e);
                            break;
                        }
                    }
                }
            });

            let mut event_stream = event_stream;
            while let Some(evt) = event_stream.next().await {
                let evt = evt.unwrap().into();
                tracing::trace!("Hub event: {:?}", evt);
                if let oe::Event::DeviceEvent(oe::DeviceEvent(
                    device,
                    oe::DeviceEventKind::TrackingEvent(tracking),
                )) = &evt
                {
                    tracing::trace!(
                        "Hub received TrackingEvent for device {:?}: aimpoint=({}, {}), screen_id={}",
                        device.uuid,
                        tracking.aimpoint.x,
                        tracking.aimpoint.y,
                        tracking.screen_id
                    );
                    let result = self.tracking_events.send((device.clone(), *tracking));
                    if let Err(e) = result {
                        tracing::trace!(
                            "Failed to broadcast tracking event: {} (no subscribers?)",
                            e
                        );
                    } else {
                        tracing::debug!(
                            "Successfully broadcast tracking event to {} subscribers",
                            self.tracking_events.receiver_count()
                        );
                    }
                }
                self.latest_event.set(Some(evt));
            }

            // Stream ended, reconnect after a delay
            tracing::warn!("Event stream ended, reconnecting in 2 seconds...");
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    }

    pub fn device_key(&self, device: &Device) -> Option<usize> {
        self.device_keys.peek().get(device).cloned()
    }

    pub fn subscribe_tracking(&self) -> broadcast::Receiver<(Device, oe::TrackingEvent)> {
        self.tracking_events.subscribe()
    }
}
