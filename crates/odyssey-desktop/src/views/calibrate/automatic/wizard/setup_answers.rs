//! What Setup has been asked, and the answers derived from it.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_autocalib::camera::CameraInfo;
use odyssey_hub_common::config::{self, MonitorKey};

use super::{CAMERA_POLL_INTERVAL, CameraList, Wizard};
use crate::views::calibrate::automatic::kind::{self, DisplayKind, SizePlan};
use crate::views::calibrate::automatic::screens::{SCREEN_COUNT, default_screen, saved_screens};
use crate::views::calibrate::automatic::settings::{Entered, SizeError, SizeField, parse_entered};
use crate::views::calibrate::automatic::worker::Command;

impl Wizard {
    pub(crate) fn selected_monitor(self) -> Option<MonitorKey> {
        let monitors = self.monitors.monitors();
        monitors
            .get(self.monitors.selected()?)
            .map(|m| m.key.clone())
    }

    /// The screen id Setup shows as chosen.
    pub(crate) fn screen_id(self) -> u8 {
        if let Some(chosen) = *self.screen_choice.read() {
            return chosen;
        }
        let remembered = self
            .selected_monitor()
            .and_then(|key| self.ui_state.read().calibration_screen(&key));
        default_screen(remembered, &self.saved.read())
    }

    /// The camera Setup shows as chosen: the picked one if it is attached, or
    /// with nothing picked, the only one there is.
    pub(crate) fn camera(self) -> Option<CameraInfo> {
        let CameraList::Found(list) = &*self.cameras.read() else {
            return None;
        };
        let chosen = self.camera_choice.read();
        let by_choice = chosen
            .as_deref()
            .and_then(|id| list.iter().find(|c| c.id == id || c.serial == id));
        match (by_choice, list.as_slice()) {
            (Some(camera), _) => Some(camera.clone()),
            (None, [only]) if chosen.is_none() => Some(only.clone()),
            _ => None,
        }
    }

    /// The kind of display Setup shows as chosen: the tech's answer, else the
    /// one used last time on this monitor, else a flat screen.
    pub(crate) fn kind(self) -> DisplayKind {
        if let Some(chosen) = *self.kind_choice.read() {
            return chosen;
        }
        let remembered = self
            .selected_monitor()
            .and_then(|key| self.ui_state.read().display_kind(&key));
        kind::default_kind(remembered)
    }

    /// Answers "What are you calibrating?". Switching kind clears the size typed
    /// for the other kind.
    pub(crate) fn set_kind(mut self, kind: DisplayKind) {
        if self.kind() != kind {
            self.size_text.write().clear();
            *self.manual_size.write() = false;
            *self.size_field.write() = SizeField::Diagonal;
        }
        *self.kind_choice.write() = Some(kind);
    }

    /// The field the size box takes: flat screens are sold by diagonal, a
    /// projected picture is also measured by width.
    pub(crate) fn field(self) -> SizeField {
        match self.kind() {
            DisplayKind::Flat => SizeField::Diagonal,
            DisplayKind::Projector => *self.size_field.read(),
        }
    }

    /// The size the tech typed, if any.
    pub(crate) fn entered(self) -> Result<Option<Entered>, SizeError> {
        parse_entered(&self.size_text.read(), self.field())
    }

    /// Whether flat screens are sized by the camera rather than by what the
    /// display reports. A remembered setting, off until someone turns it on.
    pub(crate) fn prefer_camera_size(self) -> bool {
        self.ui_state.read().prefer_camera_size
    }

    /// Turns the camera-size preference on or off and remembers it.
    pub(crate) fn set_prefer_camera_size(mut self, on: bool) {
        self.ui_state.write().prefer_camera_size = on;
        // Enqueue before spawning so rapid toggles are saved in the order they were made.
        let update = self
            .ui_state_writer
            .update(move |state| state.prefer_camera_size = on);
        spawn(async move {
            if let Err(e) = update.await {
                tracing::warn!("Failed to save UI state: {e:?}");
            }
        });
    }

    /// The size and edge rule the session will get for the current answers.
    pub(crate) fn size_plan(self) -> Option<SizePlan> {
        let aspect = self.aspect()?;
        let entered = self.entered().ok().flatten();
        Some(kind::plan(
            self.kind(),
            self.finding.read().as_ref(),
            entered,
            aspect,
            self.prefer_camera_size(),
            self.reference_size(),
        ))
    }

    pub(crate) fn can_continue(self) -> bool {
        *self.setup_ready.read()
            && self.selected_monitor().is_some()
            && self.camera().is_some()
            && self.entered().is_ok()
            // The display's size decides the plan, so wait until it is read.
            && self.finding.read().is_some()
    }

    pub(crate) async fn refresh_saved_screens(mut self) {
        let found = tokio::task::spawn_blocking(|| {
            config::screens_dir()
                .map(|dir| saved_screens(&dir))
                .unwrap_or([false; SCREEN_COUNT])
        })
        .await
        .unwrap_or([false; SCREEN_COUNT]);
        *self.saved.write() = found;
    }

    /// Asks the camera thread for the attached cameras, unless it is still
    /// answering the last request.
    pub(crate) fn request_camera_list(mut self) {
        if !*self.list_pending.peek() {
            *self.list_pending.write() = true;
            self.send(Command::ListCameras);
        }
    }

    /// Looks for cameras while Setup is on screen. The returned future ends
    /// with the component that runs it, which is what stops the polling.
    pub(crate) async fn poll_cameras(self) {
        loop {
            self.request_camera_list();
            tokio::time::sleep(CAMERA_POLL_INTERVAL).await;
        }
    }
}
