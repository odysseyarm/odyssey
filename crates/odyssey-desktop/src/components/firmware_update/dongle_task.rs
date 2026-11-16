use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use eros::Context as _;
use futures::FutureExt as _;
use odyssey_hub_server::firmware::FirmwareEntry;
use odyssey_hub_server::firmware::dongle::{self, DongleTarget, DongleUpdateProgress};
use tokio::sync::watch;

use super::dongle_update::{DongleJob, VersionSnapshot};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;
use crate::runtime;

struct Claim {
    manager: FirmwareUpdateManager,
    id: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(job) = self.manager.dongles.lock().jobs.get_mut(&self.id) {
            job.active = false;
            if !matches!(
                job.state,
                FirmwareUpdateState::Complete | FirmwareUpdateState::Failed(_)
            ) {
                job.state = FirmwareUpdateState::Failed(
                    "Dongle update stopped. Keep it in the same USB port and retry.".into(),
                );
            }
        }
        self.manager.bump();
    }
}

pub(super) fn start_update(
    manager: FirmwareUpdateManager,
    target: DongleTarget,
    entry: FirmwareEntry,
) {
    let id = target.id.clone();
    let mut updates = manager.dongles.lock();
    if updates
        .jobs
        .iter()
        .any(|(key, job)| job.active && (key == &id || job.target.same_port(&target)))
    {
        return;
    }
    updates.jobs.insert(
        id.clone(),
        DongleJob {
            target: target.clone(),
            entry: entry.clone(),
            state: FirmwareUpdateState::Downloading,
            active: true,
            transferred_at: None,
        },
    );
    let versions = updates.versions.subscribe();
    drop(updates);
    manager.bump();
    let claim = Claim {
        manager: manager.clone(),
        id: id.clone(),
    };
    runtime::spawn(async move {
        let _claim = claim;
        let result = AssertUnwindSafe(run_update(&manager, &id, target, entry, versions))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(eros::error!("Dongle update task stopped unexpectedly")));
        match result {
            Ok(()) => manager.set_dongle_state(&id, FirmwareUpdateState::Complete),
            Err(error) => {
                tracing::error!("Dongle {id} update failed: {error:?}");
                manager.set_dongle_state(
                    &id,
                    FirmwareUpdateState::Failed(crate::error_message(&error)),
                );
            }
        }
    });
}

async fn run_update(
    manager: &FirmwareUpdateManager,
    id: &str,
    target: DongleTarget,
    entry: FirmwareEntry,
    versions: watch::Receiver<VersionSnapshot>,
) -> eros::Result<()> {
    let expected = entry
        .version_parts()
        .ok_or_else(|| eros::error!("Unreadable dongle firmware version"))?;
    let progress_manager = manager.clone();
    let key = id.to_owned();
    let returned_id = dongle::update_dongle(target, entry, move |progress| {
        let state = match progress {
            DongleUpdateProgress::Downloading => FirmwareUpdateState::Downloading,
            DongleUpdateProgress::WaitingForBootloader => FirmwareUpdateState::WaitingForDfu,
            DongleUpdateProgress::Flashing { written, total } => FirmwareUpdateState::Flashing {
                progress: written,
                total,
            },
            DongleUpdateProgress::Reconnecting => FirmwareUpdateState::Verifying,
        };
        progress_manager.set_dongle_state(&key, state);
    })
    .await
    .context("Dongle firmware update failed")?;
    let transferred_at = manager
        .dongles
        .lock()
        .jobs
        .get(id)
        .and_then(|job| job.transferred_at)
        .ok_or_else(|| eros::error!("Dongle transfer did not complete"))?;
    wait_for_version(versions, &returned_id, expected, transferred_at).await
}

async fn wait_for_version(
    mut versions: watch::Receiver<VersionSnapshot>,
    id: &str,
    expected: [u32; 3],
    transferred_at: Instant,
) -> eros::Result<()> {
    let wait = async {
        loop {
            {
                let snapshot = versions.borrow_and_update();
                if snapshot.observed_at >= transferred_at
                    && snapshot.dongles.iter().any(|info| {
                        info.id.eq_ignore_ascii_case(id) && info.firmware_version == expected
                    })
                {
                    return Ok(());
                }
            }
            versions
                .changed()
                .await
                .context("Hub stopped reporting dongle versions")?;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait).await
        .map_err(|_elapsed| eros::error!("Dongle reconnected, but Desktop could not confirm firmware {}.{}.{}. Reconnect it and check its version before retrying.", expected[0], expected[1], expected[2]))?
}

#[cfg(test)]
mod tests {
    use odyssey_hub_client::client::DongleInfo;

    use super::*;

    fn info(id: &str, firmware_version: [u32; 3]) -> DongleInfo {
        DongleInfo {
            id: id.into(),
            firmware_version,
            protocol_version: [0; 3],
            control_protocol_version: [0; 3],
            connected_devices: Vec::new(),
            bonded_devices: Vec::new(),
        }
    }

    #[tokio::test]
    async fn completion_requires_fresh_full_identity_and_expected_version() {
        let transferred = Instant::now();
        let (sender, receiver) = watch::channel(VersionSnapshot {
            observed_at: transferred.checked_sub(Duration::from_secs(1)).unwrap(),
            dongles: vec![info("0011223344556677", [0, 1, 13])],
        });
        let wait = wait_for_version(receiver, "0011223344556677", [0, 1, 13], transferred);
        tokio::pin!(wait);
        assert!(futures::poll!(&mut wait).is_pending());
        for dongle in [
            info("8899223344556677", [0, 1, 13]),
            info("0011223344556677", [0, 1, 12]),
        ] {
            sender.send_replace(VersionSnapshot {
                observed_at: Instant::now(),
                dongles: vec![dongle],
            });
            assert!(futures::poll!(&mut wait).is_pending());
        }
        sender.send_replace(VersionSnapshot {
            observed_at: Instant::now(),
            dongles: vec![info("0011223344556677", [0, 1, 13])],
        });
        wait.await.unwrap();
    }
}
