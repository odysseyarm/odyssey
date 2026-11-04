//! Writing the calibration and the diagnostic bundle.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::config::{self};

use super::{SavedInfo, Stage, Wizard};
use crate::views::calibrate::automatic::worker::Command;

impl Wizard {
    /// Asks where to save the latest attempt's diagnostic bundle, then has the
    /// camera thread write it. Nothing is written unless the user picks a place.
    pub(crate) fn save_attempt(mut self) {
        spawn(async move {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let Some(target) = rfd::AsyncFileDialog::new()
                .set_file_name(format!("odyssey-calibration-{stamp}.zip"))
                .add_filter("Zip archive", &["zip"])
                .save_file()
                .await
            else {
                return;
            };
            *self.attempt_status.write() = None;
            *self.attempt_saving.write() = true;
            self.send(Command::SaveAttempt {
                path: target.path().to_path_buf(),
            });
        });
    }

    /// Saves unless that screen already holds a calibration and the tech has not
    /// confirmed replacing it.
    pub(crate) fn save(mut self, replace_confirmed: bool) {
        let Some(finished) = self.finished.peek().clone() else {
            return;
        };
        let screen_id = self.screen_id();
        if self.saved.peek()[usize::from(screen_id)] && !replace_confirmed {
            *self.confirm_replace.write() = true;
            return;
        }
        *self.confirm_replace.write() = false;
        *self.saving.write() = true;
        *self.save_error.write() = None;
        let key = self.selected_monitor();
        let kind = self.kind();
        let use_measured = *self.use_measured.peek();
        spawn(async move {
            let calibration = finished.outcome(use_measured).calibration;
            let written = tokio::task::spawn_blocking(move || {
                let dir = config::screens_dir().map_err(|e| e.to_string())?;
                odyssey_autocalib::session::save_calibration(&dir, screen_id, &calibration)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            *self.saving.write() = false;
            match written {
                Ok(path) => {
                    if let Some(key) = key {
                        // Reload before saving so a field changed elsewhere
                        // meanwhile is not overwritten.
                        let mut state = config::ui_state_async().await.into_value();
                        state.remember_calibration_screen(key.clone(), screen_id);
                        state.remember_display_kind(key, kind);
                        if let Err(e) = config::ui_state_save_async(&state).await {
                            tracing::warn!("could not remember the screen: {e}");
                        }
                        *self.ui_state.write() = state;
                    }
                    self.refresh_saved_screens().await;
                    *self.saved_to.write() = Some(SavedInfo {
                        screen_id,
                        path: path.display().to_string(),
                    });
                    self.set_stage(Stage::Saved);
                }
                Err(e) => {
                    tracing::error!("could not save the calibration: {e}");
                    *self.save_error.write() = Some(e);
                }
            }
        });
    }
}
