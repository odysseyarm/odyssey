//! Writing the calibration and the diagnostic bundle.

use dioxus::core::Runtime;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::config::{self};

#[cfg(all(test, feature = "sim"))]
mod tests;

use super::{SavedInfo, Stage, Wizard};
use crate::views::calibrate::automatic::worker::Command;

impl Wizard {
    /// Asks where to save the latest attempt's diagnostic bundle, then has the
    /// camera thread write it. Nothing is written unless the user picks a place.
    pub(crate) fn save_attempt(mut self) {
        if *self.attempt_saving.peek() {
            return;
        }
        let session = *self.session.peek();
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
            // The picker can outlive the review it was opened from.
            if *self.session.peek() != session
                || !matches!(*self.stage.peek(), Stage::Review | Stage::Problem)
            {
                return;
            }
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
        if *self.saving.peek() {
            return;
        }
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
        let session = *self.session.peek();
        let save = async move {
            let calibration = finished.outcome(use_measured).calibration.clone();
            let written = tokio::task::spawn_blocking(move || -> eros::Result<_> {
                let dir = config::screens_dir()?;
                Ok(odyssey_autocalib::session::save_calibration(
                    &dir,
                    screen_id,
                    &calibration,
                )?)
            })
            .await
            .map_err(eros::ErrorUnion::from)
            .and_then(|result| result);
            // Leaving or restarting the wizard must not be undone by a late save.
            if *self.session.peek() != session || *self.stage.peek() != Stage::Review {
                return;
            }
            match written {
                Ok(path) => {
                    if let Some(key) = key {
                        let persisted_key = key.clone();
                        let update = self.ui_state_writer.update(move |state| {
                            state.remember_calibration_screen(persisted_key.clone(), screen_id);
                            state.remember_display_kind(persisted_key, kind);
                        });
                        if let Err(e) = update.await {
                            tracing::warn!("could not remember the screen: {e:?}");
                        }
                        if *self.session.peek() != session || *self.stage.peek() != Stage::Review {
                            return;
                        }
                        let mut state = self.ui_state.write();
                        state.remember_calibration_screen(key.clone(), screen_id);
                        state.remember_display_kind(key, kind);
                    }
                    self.refresh_saved_screens().await;
                    if *self.session.peek() != session || *self.stage.peek() != Stage::Review {
                        return;
                    }
                    *self.saving.write() = false;
                    *self.saved_to.write() = Some(SavedInfo {
                        screen_id,
                        path: path.display().to_string(),
                    });
                    self.set_stage(Stage::Saved);
                }
                Err(e) => {
                    *self.saving.write() = false;
                    tracing::error!("could not save the calibration: {e:?}");
                    *self.save_error.write() = Some(crate::error_message(&e));
                }
            }
        };
        // The overwrite dialog unmounts on confirmation; the wizard owns its save.
        Runtime::current().in_scope(self.stage.origin_scope(), || spawn(save));
    }
}
