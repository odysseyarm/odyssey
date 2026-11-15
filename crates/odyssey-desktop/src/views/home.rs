//! Setup workspace and connected device details, sharing display and tracker selection.

mod connected_device_row;
mod connected_devices;
mod firmware_state;
mod offline_notice;
mod screen_actions;

use std::collections::HashMap;

use gpui::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, Render, Subscription, Window,
};

use self::connected_device_row::ConnectedDeviceRow;
use self::firmware_state::FirmwareManifestModel;
use self::offline_notice::offline_notice;
use crate::components::firmware_update::FirmwareUpdateManager;
use crate::components::page::page;
use crate::components::screen_picker::{MonitorSelection, ScreenPicker};
use crate::hub::{Hub, HubHandle};
use crate::theme;
use crate::workspace::{Workspace, WorkspaceHandle};

/// Where the page asks the shell to go.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HomeEvent {
    OpenPairing,
    OpenDevices,
    BackToSetup,
}

impl EventEmitter<HomeEvent> for HomeView {}

pub(crate) struct HomeView {
    devices_only: bool,
    workspace: Entity<Workspace>,
    hub: Entity<Hub>,
    manager: FirmwareUpdateManager,
    manifest: Entity<FirmwareManifestModel>,
    /// One row per connected device, kept across renders so each row's text
    /// field and settings panels keep their state.
    rows: HashMap<[u8; 6], Entity<ConnectedDeviceRow>>,
    selection: Entity<MonitorSelection>,
    picker: Entity<ScreenPicker>,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        let (manager, ticker) = FirmwareUpdateManager::global(cx);
        let manifest = cx.new(FirmwareManifestModel::new);
        let workspace = cx.global::<WorkspaceHandle>().0.clone();
        let selection = workspace.read(cx).monitors.clone();
        let picker = cx.new(|cx| ScreenPicker::new(selection.clone(), window, cx));

        let subscriptions = vec![
            cx.observe_in(&hub, window, |this, _, window, cx| {
                this.sync_rows(window, cx);
            }),
            cx.observe(&ticker, |_, _, cx| cx.notify()),
            cx.observe(&manifest, |_, _, cx| cx.notify()),
            cx.observe(&selection, |_, _, cx| cx.notify()),
            cx.observe(&workspace, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            devices_only: false,
            workspace,
            hub,
            manager,
            manifest,
            rows: HashMap::new(),
            selection,
            picker,
            _subscriptions: subscriptions,
        };
        this.sync_rows(window, cx);
        this
    }

    pub(crate) fn devices(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(window, cx);
        view.devices_only = true;
        view
    }

    /// Creates rows for new devices, drops rows of vanished ones and hands every
    /// row its latest device.
    fn sync_rows(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let devices = self.hub.read(cx).devices.clone();
        self.rows
            .retain(|uuid, _| devices.iter().any(|device| &device.uuid == uuid));
        for device in devices {
            if let Some(row) = self.rows.get(&device.uuid) {
                row.update(cx, |row, cx| row.set_device(device, cx));
                continue;
            }
            let uuid = device.uuid;
            let (hub, manifest, manager) = (
                self.hub.clone(),
                self.manifest.clone(),
                self.manager.clone(),
            );
            let row =
                cx.new(|cx| ConnectedDeviceRow::new(device, hub, manifest, manager, window, cx));
            self.rows.insert(uuid, row);
        }
        cx.notify();
    }
}

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = &theme::palette(cx);
        let error = self.manifest.read(cx).error.clone();
        let mut children = Vec::new();
        children.extend(offline_notice(error.as_deref(), p));
        if self.devices_only {
            children.push(
                crate::components::ui::button(
                    "back-setup",
                    "Back to setup",
                    crate::components::ui::ButtonKind::Ghost,
                    p,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::BackToSetup)))
                .into_any_element(),
            );
            children.extend(self.dongle_updates(p, cx));
            children.push(self.connected_devices(p, cx));
            return page(
                "devices",
                p,
                "Devices",
                Some("Connection, firmware and advanced tools for each device.".into()),
                children,
            )
            .into_any_element();
        }
        children.push(self.screen_actions(p, cx));
        if let Some(row) = self
            .workspace
            .read(cx)
            .selected_device
            .and_then(|uuid| self.rows.get(&uuid))
        {
            children.push(row.clone().into_any_element());
        }
        page(
            "home",
            p,
            "Setup",
            Some(
                "Choose a display and tracker, then calibrate the screen and check your aim."
                    .into(),
            ),
            children,
        )
        .into_any_element()
    }
}
