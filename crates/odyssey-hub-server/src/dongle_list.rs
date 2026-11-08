use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::device_tasks::{DongleInfo, DongleTaskMessage};

type Entry = (DongleInfo, mpsc::Sender<DongleTaskMessage>);

/// The connected dongles, each with the channel that reaches its task.
///
/// Cloning shares the list. Dongles are identified by their USB serial number.
/// No method holds the lock across an await.
#[derive(Clone, Default)]
pub(crate) struct DongleList(Arc<Mutex<Vec<Entry>>>);

impl DongleList {
    pub(crate) fn snapshot(&self) -> Vec<DongleInfo> {
        self.0.lock().iter().map(|(info, _)| info.clone()).collect()
    }

    /// The channel to a dongle's task, if the dongle is listed.
    pub(crate) fn task_sender(&self, id: &str) -> Option<mpsc::Sender<DongleTaskMessage>> {
        let entries = self.0.lock();
        let (_, sender) = entries.iter().find(|(info, _)| info.id == id)?;
        Some(sender.clone())
    }

    /// Routes only to the selected live actor; an unavailable radio never falls back.
    pub(crate) async fn accessory_request(
        &self,
        id: &str,
        command: odyssey_protocol::accessory::Command,
    ) -> Result<odyssey_protocol::accessory::Reply, String> {
        let sender = self
            .task_sender(id)
            .ok_or_else(|| format!("Dongle '{id}' unavailable"))?;
        let (reply, response) = tokio::sync::oneshot::channel();
        let operation = async {
            sender
                .send(DongleTaskMessage::Accessory { command, reply })
                .await
                .map_err(|_| format!("Dongle '{id}' disconnected"))?;
            response
                .await
                .map_err(|_| format!("Dongle '{id}' disconnected"))?
        };
        tokio::time::timeout(
            std::time::Duration::from_millis(
                odyssey_protocol::accessory::REQUEST_TIMEOUT_MILLIS + 5_000,
            ),
            operation,
        )
        .await
        .map_err(|_| format!("Dongle '{id}' accessory request timed out"))?
    }

    pub(crate) async fn subscribe_accessory_events(
        &self,
        id: &str,
    ) -> Result<tokio::sync::broadcast::Receiver<odyssey_protocol::accessory::Event>, String> {
        let sender = self
            .task_sender(id)
            .ok_or_else(|| format!("Dongle '{id}' unavailable"))?;
        let (reply, response) = tokio::sync::oneshot::channel();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            sender
                .send(DongleTaskMessage::SubscribeAccessoryEvents { reply })
                .await
                .map_err(|_| format!("Dongle '{id}' disconnected"))?;
            response
                .await
                .map_err(|_| format!("Dongle '{id}' disconnected"))?
        })
        .await
        .map_err(|_| format!("Dongle '{id}' event subscription timed out"))?
    }

    /// Adds a dongle, or replaces the listed one with the same id.
    pub(crate) fn upsert(&self, info: DongleInfo, commands: mpsc::Sender<DongleTaskMessage>) {
        let mut entries = self.0.lock();
        match entries.iter().position(|(listed, _)| listed.id == info.id) {
            Some(index) => entries[index] = (info, commands),
            None => entries.push((info, commands)),
        }
    }

    /// Replaces the listed info of a dongle, keeping its channel. A dongle that
    /// is not listed stays unlisted: only a connect adds one.
    pub(crate) fn update(&self, info: DongleInfo) {
        let mut entries = self.0.lock();
        if let Some((listed, _)) = entries.iter_mut().find(|(listed, _)| listed.id == info.id) {
            *listed = info;
        }
    }

    pub(crate) fn remove(&self, id: &str) {
        self.0.lock().retain(|(info, _)| info.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dongle(id: &str, connected: usize) -> DongleInfo {
        DongleInfo {
            id: id.to_owned(),
            protocol_version: [0; 3],
            control_protocol_version: [0; 3],
            firmware_version: [0; 3],
            connected_devices: vec![[0; 6]; connected],
            bonded_devices: Vec::new(),
        }
    }

    #[test]
    fn update_never_adds_and_upsert_replaces() {
        let list = DongleList::default();
        list.update(dongle("a", 1));
        assert!(list.snapshot().is_empty());

        let (commands, _received) = mpsc::channel(1);
        list.upsert(dongle("a", 1), commands.clone());
        list.upsert(dongle("a", 2), commands);
        list.update(dongle("b", 3));

        let snapshot = list.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].connected_devices.len(), 2);
        assert!(list.task_sender("a").is_some());
        assert!(list.task_sender("b").is_none());
    }

    #[test]
    fn remove_drops_only_the_matching_dongle() {
        let list = DongleList::default();
        let (commands, _received) = mpsc::channel(1);
        list.upsert(dongle("a", 0), commands.clone());
        list.upsert(dongle("b", 0), commands);

        list.remove("a");
        list.remove("missing");

        let ids: Vec<_> = list.snapshot().into_iter().map(|info| info.id).collect();
        assert_eq!(ids, ["b"]);
    }

    #[tokio::test]
    async fn unplugged_selection_never_uses_another_radio() {
        let list = DongleList::default();
        let (commands, mut received) = mpsc::channel(1);
        list.upsert(dongle("other", 0), commands);
        let error = list
            .accessory_request(
                "selected",
                odyssey_protocol::accessory::Command::Capabilities(),
            )
            .await
            .unwrap_err();
        assert!(error.contains("selected"));
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn unplug_wakes_in_flight_accessory_wait() {
        let list = DongleList::default();
        let (commands, mut received) = mpsc::channel(1);
        list.upsert(dongle("selected", 0), commands);
        let request_list = list.clone();
        let request = tokio::spawn(async move {
            request_list
                .accessory_request(
                    "selected",
                    odyssey_protocol::accessory::Command::Capabilities(),
                )
                .await
        });
        let in_flight = received.recv().await.unwrap();
        list.remove("selected");
        drop(in_flight);
        drop(received);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().contains("disconnected"));
    }
}
