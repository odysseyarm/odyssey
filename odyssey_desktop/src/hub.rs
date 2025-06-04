use std::collections::HashMap;

use dioxus::prelude::*;
use futures::StreamExt;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events as oe;
use slab::Slab;

#[derive(Clone)]
pub struct HubContext {
    pub client: Client,
    pub devices: Signal<Slab<Device>>,
    device_keys: HashMap<odyssey_hub_common::device::Device, usize>,
    pub latest_event: Signal<oe::Event>,
}

impl HubContext {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
            devices: Signal::new(Slab::new()),
            latest_event: Signal::new(oe::Event::None),
            device_keys: HashMap::new(),
        }
    }

    pub async fn run(mut self) {
        let mut client = self.client;
        client.connect().await.unwrap();

        let list = client.get_device_list().await.unwrap();
        for d in list {
            self.devices.write().insert(d);
        }

        let mut stream = client.poll().await.unwrap();
        while let Some(evt) = stream.next().await {
            let evt = evt.unwrap().event.unwrap().into();
            match &evt {
                oe::Event::DeviceEvent(de) => {
                    match de {
                        oe::DeviceEvent { device, kind } => {
                            match kind {
                                oe::DeviceEventKind::ConnectEvent => {
                                    self.device_keys.insert(device.clone(), self.devices.write().insert(device.clone()));
                                }
                                oe::DeviceEventKind::DisconnectEvent => {
                                    if let Some(device_key) = self.device_keys.remove(device) {
                                        self.devices.write().remove(device_key);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                oe::Event::None => {}
            }
            self.latest_event.set(evt);
        }
    }

    pub fn device_key(&self, device: &Device) -> Option<usize> {
        self.device_keys.get(device).cloned()
    }
}
