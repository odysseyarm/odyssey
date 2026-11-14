use std::collections::HashMap;

use gpui::{AppContext as _, Context, Entity, Global, Subscription, Task};
use odyssey_hub_common::config::{self, MonitorKey};

use crate::components::screen_picker::MonitorSelection;
use crate::hub::HubHandle;
use crate::runtime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Alignment {
    #[default]
    NotVerified,
    Applied,
    Saved,
}

pub(crate) struct Workspace {
    pub(crate) monitors: Entity<MonitorSelection>,
    pub(crate) selected_device: Option<[u8; 6]>,
    pub(crate) calibrations: HashMap<MonitorKey, u8>,
    pub(crate) saved_calibration_ids: Vec<u8>,
    pub(crate) calibration_loading: bool,
    pub(crate) calibration_error: Option<String>,
    calibration_task: Option<Task<()>>,
    alignments: HashMap<[u8; 6], Alignment>,
    _hub: Subscription,
}

#[derive(Clone)]
pub(crate) struct WorkspaceHandle(pub(crate) Entity<Workspace>);

impl Global for WorkspaceHandle {}

impl Workspace {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        let subscription = cx.observe(&hub, |this, hub, cx| {
            let only_device = match hub.read(cx).devices.as_slice() {
                [device] => Some(device.uuid),
                _ => None,
            };
            if this.selected_device.is_none() && only_device.is_some() {
                this.selected_device = only_device;
                cx.notify();
            }
        });
        let selected_device = match hub.read(cx).devices.as_slice() {
            [device] => Some(device.uuid),
            _ => None,
        };
        let mut workspace = Self {
            monitors: cx.new(MonitorSelection::new),
            selected_device,
            calibrations: HashMap::new(),
            saved_calibration_ids: Vec::new(),
            calibration_loading: true,
            calibration_error: None,
            calibration_task: None,
            alignments: HashMap::new(),
            _hub: subscription,
        };
        workspace.refresh_calibrations(cx);
        workspace
    }

    pub(crate) fn select_device(&mut self, uuid: [u8; 6], cx: &mut Context<Self>) {
        self.selected_device = Some(uuid);
        cx.notify();
    }

    pub(crate) fn alignment(&self, uuid: [u8; 6]) -> Alignment {
        self.alignments.get(&uuid).copied().unwrap_or_default()
    }

    pub(crate) fn set_alignment(
        &mut self,
        uuid: [u8; 6],
        state: Alignment,
        cx: &mut Context<Self>,
    ) {
        self.alignments.insert(uuid, state);
        cx.notify();
    }

    pub(crate) fn refresh_calibrations(&mut self, cx: &mut Context<Self>) {
        self.calibration_loading = true;
        self.calibration_error = None;
        let load = runtime::spawn(async {
            let state = config::ui_state_async().await;
            let error = state.corruption_info().map(|(path, error)| {
                format!(
                    "Couldn't read display assignments at {}: {error}",
                    path.display()
                )
            });
            let state = state.into_value();
            let saved = config::screen_calibrations_async().await;
            let corrupt = !saved.corrupted_screens.is_empty();
            let saved_calibration_ids: Vec<_> =
                saved.calibrations.iter().map(|(id, _)| *id).collect();
            let calibrations = calibration_assignments(state, &saved_calibration_ids);
            (calibrations, saved_calibration_ids, error.or_else(|| corrupt.then(|| "A saved screen calibration couldn't be read. Calibrate it again or repair it in Settings.".to_owned())))
        });
        self.calibration_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((calibrations, saved_calibration_ids, error)) => {
                        this.calibrations = calibrations;
                        this.saved_calibration_ids = saved_calibration_ids;
                        this.calibration_error = error;
                    }
                    Err(error) => {
                        this.calibration_error =
                            Some(format!("Couldn't read screen calibrations: {error}"));
                    }
                }
                this.calibration_loading = false;
                cx.notify();
            });
        }));
    }
}

fn calibration_assignments(
    state: config::UiState,
    saved_calibration_ids: &[u8],
) -> HashMap<MonitorKey, u8> {
    let mut candidates = HashMap::new();
    for entry in state.calibration_screens {
        let mut monitor = entry.monitor;
        let exact = monitor.name.take().is_none();
        let screen = Some(entry.screen_id);
        candidates
            .entry(monitor)
            .and_modify(|(existing, has_exact): &mut (Option<u8>, bool)| {
                if exact && !*has_exact {
                    *existing = screen;
                    *has_exact = true;
                } else if exact == *has_exact && *existing != screen {
                    *existing = None;
                }
            })
            .or_insert((screen, exact));
    }
    candidates
        .into_iter()
        .filter_map(|(monitor, (screen, _))| {
            screen
                .filter(|screen| saved_calibration_ids.contains(screen))
                .map(|screen| (monitor, screen))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> MonitorKey {
        MonitorKey {
            name: None,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn saved_calibration_does_not_require_or_infer_monitor_assignment() {
        let assignments = calibration_assignments(config::UiState::default(), &[0]);
        assert_eq!(assignments.get(&monitor()), None);
    }

    #[test]
    fn explicit_assignment_requires_valid_saved_screen() {
        let mut state = config::UiState::default();
        state.remember_calibration_screen(monitor(), 2);
        assert_eq!(
            calibration_assignments(state.clone(), &[2]).get(&monitor()),
            Some(&2)
        );
        assert_eq!(calibration_assignments(state, &[0]).get(&monitor()), None);
    }

    #[test]
    fn legacy_named_assignment_matches_current_gpui_identity() {
        let mut state = config::UiState::default();
        let mut legacy = monitor();
        legacy.name = Some("Display".into());
        state.remember_calibration_screen(legacy, 2);
        assert_eq!(
            calibration_assignments(state, &[2]).get(&monitor()),
            Some(&2)
        );
    }

    #[test]
    fn conflicting_legacy_assignments_remain_unknown() {
        let mut state = config::UiState::default();
        let mut first = monitor();
        first.name = Some("First".into());
        let mut second = monitor();
        second.name = Some("Second".into());
        state.remember_calibration_screen(first, 1);
        state.remember_calibration_screen(second, 2);
        assert_eq!(
            calibration_assignments(state, &[1, 2]).get(&monitor()),
            None
        );
    }

    #[test]
    fn current_assignment_takes_precedence_over_legacy_in_either_order() {
        let mut state = config::UiState::default();
        let mut legacy = monitor();
        legacy.name = Some("Legacy".into());
        state.remember_calibration_screen(legacy, 1);
        state.remember_calibration_screen(monitor(), 2);
        assert_eq!(
            calibration_assignments(state.clone(), &[1, 2]).get(&monitor()),
            Some(&2)
        );
        state.calibration_screens.reverse();
        assert_eq!(
            calibration_assignments(state, &[1, 2]).get(&monitor()),
            Some(&2)
        );
    }

    #[test]
    fn missing_current_assignment_does_not_revive_legacy_assignment() {
        let mut state = config::UiState::default();
        let mut legacy = monitor();
        legacy.name = Some("Legacy".into());
        state.remember_calibration_screen(legacy, 1);
        state.remember_calibration_screen(monitor(), 2);
        assert_eq!(calibration_assignments(state, &[1]).get(&monitor()), None);
    }

    #[test]
    fn legacy_assignment_does_not_follow_changed_bounds() {
        let mut state = config::UiState::default();
        let mut legacy = monitor();
        legacy.name = Some("Legacy".into());
        legacy.x += 1920;
        state.remember_calibration_screen(legacy, 2);
        assert_eq!(calibration_assignments(state, &[2]).get(&monitor()), None);
    }
}
