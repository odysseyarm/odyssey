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
}
