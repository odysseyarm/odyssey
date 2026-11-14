//! Writing the calibration and the diagnostic bundle.

use gpui::Context;
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::config::{self, MonitorKey};

#[cfg(all(test, feature = "sim"))]
mod tests;

use super::init::read_saved_screens;
use super::{SavedInfo, Stage, Wizard};
use crate::runtime;
use crate::ui_state::UiStateWriter;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::screens::SCREEN_COUNT;
use crate::views::calibrate::automatic::worker::Command;

/// What a finished save leaves behind for the wizard to show.
#[derive(Debug)]
struct Written {
    path: String,
    screens: [bool; SCREEN_COUNT],
}

/// Writes `calibration` as screen `screen_id`, remembers the screen and kind
/// for `remember`'s monitor, and rereads which screens now hold a calibration.
/// A failed write changes nothing on disk.
async fn write_calibration(
    calibration: ScreenCalibration<f32>,
    screen_id: u8,
    remember: Option<(MonitorKey, DisplayKind)>,
    writer: UiStateWriter,
) -> eros::Result<Written> {
    let path = tokio::task::spawn_blocking(move || -> eros::Result<_> {
        let dir = config::screens_dir()?;
        Ok(odyssey_autocalib::session::save_calibration(
            &dir,
            screen_id,
            &calibration,
        )?)
    })
    .await
    .map_err(eros::ErrorUnion::from)
    .and_then(|result| result)?;
    if let Some((key, kind)) = remember {
        let update = writer.update(move |state| {
            state.remember_calibration_screen(key.clone(), screen_id);
            state.remember_display_kind(key, kind);
        });
        if let Err(e) = update.await {
            tracing::warn!("could not remember the screen: {e:?}");
        }
    }
    Ok(Written {
        path: path.display().to_string(),
        screens: read_saved_screens().await.unwrap_or([false; SCREEN_COUNT]),
    })
}

impl Wizard {
    /// Asks where to save the latest attempt's diagnostic bundle, then has the
    /// camera thread write it. Nothing is written unless the user picks a place.
    pub(crate) fn save_attempt(&mut self, cx: &mut Context<Self>) {
        if self.attempt_saving {
            return;
        }
        let session = self.session;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let dialog = runtime::spawn(async move {
            rfd::AsyncFileDialog::new()
                .set_file_name(format!("odyssey-calibration-{stamp}.zip"))
                .add_filter("Zip archive", &["zip"])
                .save_file()
                .await
                .map(|file| file.path().to_path_buf())
        });
        cx.spawn(async move |this, cx| {
            let Ok(Some(path)) = dialog.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // The picker can outlive the review it was opened from.
                if this.session != session || !matches!(this.stage, Stage::Review | Stage::Problem)
                {
                    return;
                }
                this.attempt_status = None;
                this.attempt_saving = true;
                this.send(Command::SaveAttempt { path }, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Saves unless that screen already holds a calibration and the tech has not
    /// confirmed replacing it.
    pub(crate) fn save(&mut self, replace_confirmed: bool, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(finished) = self.finished.clone() else {
            return;
        };
        let screen_id = self.screen_id();
        if self.saved[usize::from(screen_id)] && !replace_confirmed {
            self.confirm_replace = true;
            cx.notify();
            return;
        }
        self.confirm_replace = false;
        self.saving = true;
        self.save_error = None;
        cx.notify();
        let key = self.selected.clone();
        let kind = self.kind();
        let remember = key.clone().map(|key| (key, kind));
        let calibration = finished.outcome(self.use_measured).calibration.clone();
        let session = self.session;
        let write = runtime::spawn(write_calibration(
            calibration,
            screen_id,
            remember,
            self.ui_state_writer.clone(),
        ));
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let written = write
                .await
                .map_err(eros::ErrorUnion::from)
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                // Leaving or restarting the wizard must not be undone by a late save.
                if this.session != session || this.stage != Stage::Review {
                    return;
                }
                match written {
                    Ok(written) => {
                        if let Some(key) = key {
                            this.ui_state
                                .remember_calibration_screen(key.clone(), screen_id);
                            this.ui_state.remember_display_kind(key, kind);
                        }
                        this.saved = written.screens;
                        this.saving = false;
                        this.saved_to = Some(SavedInfo {
                            screen_id,
                            path: written.path,
                        });
                        this.set_stage(Stage::Saved, cx);
                    }
                    Err(e) => {
                        this.saving = false;
                        tracing::error!("could not save the calibration: {e:?}");
                        this.save_error = Some(crate::error_message(&e));
                        cx.notify();
                    }
                }
            });
        }));
    }
}
