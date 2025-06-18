use std::collections::HashMap;

use dioxus::prelude::*;
use futures::StreamExt;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events as oe;
use slab::Slab;

#[derive(Clone)]
pub struct HubContext {
    pub client: SyncSignal<Client>,
    pub devices: SyncSignal<Slab<Device>>,
    pub latest_event: Signal<Option<oe::Event>>,
    device_keys: SyncSignal<HashMap<odyssey_hub_common::device::Device, usize>>,
}

impl HubContext {
    pub fn new() -> Self {
        Self {
            client: SyncSignal::new_maybe_sync(Client::default()),
            devices: SyncSignal::new_maybe_sync(Slab::new()),
            latest_event: Signal::new(None),
            device_keys: SyncSignal::new_maybe_sync(HashMap::new()),
        }
    }

    pub async fn run(&mut self) {
        self.client.write().connect().await.unwrap();

        let list = (self.client)().get_device_list().await.unwrap();
        for d in list {
            self.devices.write().insert(d);
        }

        let mut stream = (self.client)().poll().await.unwrap();
        while let Some(evt) = stream.next().await {
            let evt = evt.unwrap().event.unwrap().into();
            match &evt {
                oe::Event::DeviceEvent(de) => match de {
                    oe::DeviceEvent(device, kind) => match kind {
                        oe::DeviceEventKind::ConnectEvent => {
                            self.device_keys.write().insert(
                                device.clone(),
                                self.devices.write().insert(device.clone()),
                            );
                        }
                        oe::DeviceEventKind::DisconnectEvent => {
                            if let Some(device_key) = self.device_keys.write().remove(device) {
                                self.devices.write().remove(device_key);
                            }
                        }
                        _ => {}
                    },
                },
            }
            self.latest_event.set(Some(evt));
        }
    }

    pub fn device_key(&self, device: &Device) -> Option<usize> {
        self.device_keys.peek().get(device).cloned()
    }
}
