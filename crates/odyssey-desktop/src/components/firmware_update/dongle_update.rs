use std::collections::HashMap;
use std::time::Instant;

use futures::StreamExt as _;
use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div};
use odyssey_hub_client::client::DongleInfo;
use odyssey_hub_server::firmware::dongle::DongleTarget;
use odyssey_hub_server::firmware::{self, FirmwareEntry, FirmwareManifest};
use tokio::sync::watch;

use super::device_firmware_update::flash_progress;
use super::dongle_task::start_update;
use super::update_check::{desktop_version, format_version};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;
use crate::components::ui::{ButtonKind, button, card, card_title};
use crate::runtime;
use crate::theme::Palette;

#[derive(Clone)]
pub(super) struct VersionSnapshot {
    pub(super) observed_at: Instant,
    pub(super) dongles: Vec<DongleInfo>,
}

#[derive(Clone)]
pub(super) struct DongleJob {
    pub(super) target: DongleTarget,
    pub(super) entry: FirmwareEntry,
    pub(super) state: FirmwareUpdateState,
    pub(super) active: bool,
    pub(super) transferred_at: Option<Instant>,
}

pub(super) struct DongleUpdates {
    pub(super) targets: Vec<DongleTarget>,
    pub(super) jobs: HashMap<String, DongleJob>,
    pub(super) versions: watch::Sender<VersionSnapshot>,
    discovery_error: Option<String>,
}

impl Default for DongleUpdates {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            jobs: HashMap::new(),
            versions: watch::channel(VersionSnapshot {
                observed_at: Instant::now(),
                dongles: Vec::new(),
            })
            .0,
            discovery_error: None,
        }
    }
}

impl FirmwareUpdateManager {
    pub(super) fn set_dongle_versions(&self, dongles: Vec<DongleInfo>) {
        let updates = self.dongles.lock();
        if updates.versions.borrow().dongles != dongles {
            updates.versions.send_replace(VersionSnapshot {
                observed_at: Instant::now(),
                dongles,
            });
        }
    }

    pub(super) fn set_dongle_state(&self, id: &str, state: FirmwareUpdateState) {
        if let Some(job) = self.dongles.lock().jobs.get_mut(id) {
            if state == FirmwareUpdateState::Verifying {
                job.transferred_at = Some(Instant::now());
            }
            job.state = state;
        }
        self.bump();
    }
}

pub(super) fn watch_dongles(manager: FirmwareUpdateManager) {
    runtime::spawn(async move {
        let discovery = async {
            let mut watch = nusb::watch_devices()?;
            loop {
                let targets = firmware::dongle::connected_targets().await?;
                manager.dongles.lock().targets = targets;
                manager.bump();
                if watch.next().await.is_none() {
                    return Err::<(), firmware::dongle::DongleUpdateError>(
                        std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "USB discovery stopped",
                        )
                        .into(),
                    );
                }
            }
        }
        .await;
        if let Err(error) = discovery {
            tracing::warn!("Dongle updater USB discovery failed: {error}");
            manager.dongles.lock().discovery_error = Some(error.to_string());
            manager.bump();
        }
    });
}

fn offered(manifest: &FirmwareManifest, installed: [u16; 3]) -> Option<&FirmwareEntry> {
    if installed == [0; 3] {
        return None;
    }
    let entry = firmware::select_firmware(manifest, "dongle", desktop_version(), 0).offered?;
    firmware::version_newer(installed, entry.version_parts()?).then_some(entry)
}

pub(crate) fn dongle_update_cards(
    manager: &FirmwareUpdateManager,
    manifest: Option<&FirmwareManifest>,
    p: Palette,
) -> Vec<AnyElement> {
    let updates = manager.dongles.lock();
    let mut rows: Vec<_> = updates
        .jobs
        .values()
        .map(|job| (job.target.clone(), Some(job.clone())))
        .collect();
    for target in &updates.targets {
        if !rows.iter().any(|(listed, _)| listed.same_port(target)) {
            rows.push((target.clone(), None));
        }
    }
    rows.sort_by(|(left, _), (right, _)| left.id.cmp(&right.id));
    let versions = updates.versions.borrow().dongles.clone();
    let discovery_error = updates.discovery_error.clone();
    drop(updates);
    let mut cards = Vec::new();
    if let Some(error) = discovery_error {
        cards.push(
            card(p)
                .child(card_title(p, "Dongle updater unavailable"))
                .child(error)
                .into_any_element(),
        );
    }
    for (target, job) in rows {
        let entry = job.as_ref().map(|job| &job.entry).or_else(|| {
            let manifest = manifest?;
            if target.in_bootloader {
                firmware::select_firmware(manifest, "dongle", desktop_version(), 0).offered
            } else {
                let installed = versions
                    .iter()
                    .find(|info| info.id.eq_ignore_ascii_case(&target.id))?
                    .firmware_version;
                offered(manifest, installed)
            }
        });
        let Some(entry) = entry else {
            continue;
        };
        let state = job.as_ref().map(|job| &job.state);
        let mut content = card(p)
            .gap_2()
            .child(card_title(p, "Dongle firmware"))
            .child(
                div()
                    .text_sm()
                    .child(format!("Dongle {} · v{}", target.id, entry.version)),
            );
        if let Some(installed) = versions
            .iter()
            .find(|info| info.id.eq_ignore_ascii_case(&target.id))
        {
            content = content.child(div().text_xs().text_color(p.fg_muted).child(format!(
                "Installed v{}",
                format_version(installed.firmware_version)
            )));
        }
        match state {
            Some(FirmwareUpdateState::Flashing { progress, total }) => {
                content = content.child(flash_progress("Updating dongle…", *progress, *total, p));
            }
            Some(FirmwareUpdateState::WaitingForDfu) => {
                content = content.child(div().text_sm().child("Press the dongle’s reset/DFU button to enter bootloader mode. Keep it in the same USB port. Desktop will continue automatically."));
            }
            Some(FirmwareUpdateState::Failed(error)) => {
                content = content.child(div().text_sm().text_color(p.danger_ink).child(error.clone()))
                    .child(div().text_xs().text_color(p.fg_muted).child("Keep the dongle in the same USB port and retry. If needed, press its reset/DFU button again. No command-line tools are required."));
            }
            Some(state) => {
                content = content.child(
                    div()
                        .text_sm()
                        .text_color(p.accent_ink)
                        .child(state.status_text()),
                );
            }
            None => {
                content = content.child(div().text_sm().text_color(p.fg_muted).child("Connected trackers and accessories will disconnect during the update. Leave the dongle plugged in until Desktop confirms completion."));
            }
        }
        if job.as_ref().is_none_or(|job| !job.active) {
            let done = matches!(state, Some(FirmwareUpdateState::Complete));
            let manager = manager.clone();
            let entry = entry.clone();
            let label = if done {
                "Done"
            } else if job.is_some() {
                "Retry update"
            } else if target.in_bootloader {
                "Recover dongle"
            } else {
                "Update dongle"
            };
            let key = target.id.clone();
            content = content.child(
                button(
                    format!("update-dongle-{key}"),
                    label,
                    ButtonKind::Secondary,
                    p,
                )
                .on_click(move |_, _, _| {
                    if done {
                        manager.dongles.lock().jobs.remove(&key);
                        manager.bump();
                    } else {
                        start_update(manager.clone(), target.clone(), entry.clone());
                    }
                }),
            );
        }
        cards.push(content.into_any_element());
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(entries: &[(&str, &str)]) -> FirmwareManifest {
        let firmwares: Vec<_> = entries.iter().map(|(version, min_desktop)| {
            serde_json::json!({ "version": version, "requires_bootloader": 0, "requires_netcore": 0, "min_desktop": min_desktop, "files": {} })
        }).collect();
        serde_json::from_value(serde_json::json!({ "version": 1, "devices": { "dongle": { "firmwares": firmwares } } })).unwrap()
    }

    #[test]
    fn newer_release_is_offered_and_current_or_unread_is_not() {
        let manifest = manifest(&[("0.2.0", "0.0.0")]);
        for (installed, want) in [
            ([0, 1, 11], Some("0.2.0")),
            ([0, 2, 0], None),
            ([0, 3, 0], None),
            ([0, 0, 0], None),
        ] {
            let got = offered(&manifest, installed).map(|entry| entry.version.as_str());
            assert_eq!(got, want, "{installed:?}");
        }
    }

    #[test]
    fn release_needing_a_newer_desktop_is_not_offered() {
        let manifest = manifest(&[("0.2.0", "99.0.0")]);
        assert!(offered(&manifest, [0, 1, 0]).is_none());
    }

    #[test]
    fn an_older_installable_entry_is_offered_when_the_newest_needs_a_newer_desktop() {
        let manifest = manifest(&[("0.3.0", "99.0.0"), ("0.2.0", "0.0.0")]);
        let got = offered(&manifest, [0, 1, 0]).map(|entry| entry.version.as_str());
        assert_eq!(got, Some("0.2.0"));
    }
}
