use dioxus::prelude::*;
use futures::StreamExt;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;

#[derive(Clone)]
pub struct HubContext {
    pub client: Client,
    pub devices: Signal<Vec<Device>>,
    pub events: Signal<Vec<odyssey_hub_common::events::Event>>,
}

impl HubContext {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
            devices: Signal::new(Vec::new()),
            events: Signal::new(Vec::new()),
        }
    }

    pub async fn run(mut self) {
        let mut client = self.client;
        client.connect().await.unwrap();

        let list = client.get_device_list().await.unwrap();
        self.devices.set(list);

        let mut stream = client.poll().await.unwrap();
        while let Some(evt) = stream.next().await {
            self.events.write().push(evt.unwrap().event.unwrap().into());
        }
    }
}
