//! What Setup has been asked, and the answers derived from it.

use gpui::{Context, Task};
use odyssey_autocalib::camera::CameraInfo;
use odyssey_hub_common::config::MonitorKey;

use super::init::read_saved_screens;
use super::{CAMERA_POLL_INTERVAL, CameraList, Wizard};
use crate::runtime;
use crate::views::calibrate::automatic::kind::{self, DisplayKind, SizePlan};
use crate::views::calibrate::automatic::screens::default_screen;
use crate::views::calibrate::automatic::settings::{Entered, SizeError, SizeField, parse_entered};
use crate::views::calibrate::automatic::worker::Command;

impl Wizard {
    pub(crate) fn selected_monitor(&self) -> Option<MonitorKey> {
        self.selected.clone()
    }

    /// The screen id Setup shows as chosen.
    pub(crate) fn screen_id(&self) -> u8 {
        if let Some(chosen) = self.screen_choice {
            return chosen;
        }
        let remembered = self
            .selected
            .as_ref()
            .and_then(|key| self.ui_state.calibration_screen(key));
        default_screen(remembered, self.saved)
    }

    /// The camera Setup shows as chosen: the picked one if it is attached, or
    /// with nothing picked, the only one there is.
    pub(crate) fn camera(&self) -> Option<CameraInfo> {
        let CameraList::Found(list) = &self.cameras else {
            return None;
        };
        let chosen = self.camera_choice.as_deref();
        let by_choice = chosen.and_then(|id| list.iter().find(|c| c.id == id || c.serial == id));
        match (by_choice, list.as_slice()) {
            (Some(camera), _) => Some(camera.clone()),
            (None, [only]) if chosen.is_none() => Some(only.clone()),
            _ => None,
        }
    }

    /// The kind of display Setup shows as chosen: the tech's answer, else the
    /// one used last time on this monitor, else a flat screen.
    pub(crate) fn kind(&self) -> DisplayKind {
        if let Some(chosen) = self.kind_choice {
            return chosen;
        }
        let remembered = self
            .selected
            .as_ref()
            .and_then(|key| self.ui_state.display_kind(key));
        kind::default_kind(remembered)
    }

    /// Answers "What are you calibrating?". Switching kind clears the size typed
    /// for the other kind.
    pub(crate) fn set_kind(&mut self, kind: DisplayKind, cx: &mut Context<Self>) {
        if self.kind() != kind {
            self.size_text.clear();
            self.manual_size = false;
            self.size_field = SizeField::Diagonal;
        }
        self.kind_choice = Some(kind);
        cx.notify();
    }

    /// The field the size box takes: flat screens are sold by diagonal, a
    /// projected picture is also measured by width.
    pub(crate) fn field(&self) -> SizeField {
        match self.kind() {
            DisplayKind::Flat => SizeField::Diagonal,
            DisplayKind::Projector => self.size_field,
        }
    }

    /// The size the tech typed, if any.
    pub(crate) fn entered(&self) -> Result<Option<Entered>, SizeError> {
        parse_entered(&self.size_text, self.field())
    }

    /// Whether flat screens are sized by the camera rather than by what the
    /// display reports. A remembered setting, off until someone turns it on.
    pub(crate) fn prefer_camera_size(&self) -> bool {
        self.ui_state.prefer_camera_size
    }

    /// Turns the camera-size preference on or off and remembers it.
    pub(crate) fn set_prefer_camera_size(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ui_state.prefer_camera_size = on;
        cx.notify();
        // Enqueue before spawning so rapid toggles are saved in the order they were made.
        let update = self
            .ui_state_writer
            .update(move |state| state.prefer_camera_size = on);
        cx.spawn(async move |_, _| {
            if let Err(e) = update.await {
                tracing::warn!("Failed to save UI state: {e:?}");
            }
        })
        .detach();
    }

    /// The size and edge rule the session will get for the current answers.
    pub(crate) fn size_plan(&self) -> Option<SizePlan> {
        let aspect = self.aspect()?;
        let entered = self.entered().ok().flatten();
        Some(kind::plan(
            self.kind(),
            self.finding.as_ref(),
            entered,
            aspect,
            self.prefer_camera_size(),
            self.reference_size(),
        ))
    }

    pub(crate) fn can_continue(&self) -> bool {
        self.setup_ready
            && self.selected.is_some()
            && self.camera().is_some()
            && self.entered().is_ok()
            // The display's size decides the plan, so wait until it is read.
            && self.finding.is_some()
    }

    pub(super) fn refresh_saved_screens(cx: &mut Context<Self>) {
        let read = runtime::spawn(read_saved_screens());
        cx.spawn(async move |this, cx| {
            let found = read.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.saved = found;
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks the camera thread for the attached cameras, unless it is still
    /// answering the last request.
    pub(crate) fn request_camera_list(&mut self, cx: &mut Context<Self>) {
        if !self.list_pending {
            self.list_pending = true;
            self.send(Command::ListCameras, cx);
        }
    }

    /// Looks for cameras while Setup is on screen. [`Wizard::set_stage`] drops
    /// the returned task when Setup is left, which is what stops the polling.
    pub(super) fn poll_cameras(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, Self::request_camera_list).is_err() {
                    break;
                }
                cx.background_executor().timer(CAMERA_POLL_INTERVAL).await;
            }
        })
    }
}
