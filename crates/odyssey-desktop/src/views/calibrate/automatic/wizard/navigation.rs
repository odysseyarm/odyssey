//! Moving between stages: back, retry, failure, exit.

use gpui::Context;

use super::{Stage, Wizard};
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::worker::{Command, Failure, FailureKind};

impl Wizard {
    pub(crate) fn set_stage(&mut self, stage: Stage, cx: &mut Context<Self>) {
        self.stage = stage;
        if stage != Stage::Setup {
            self.cancel_helper(cx);
            self.camera_poll = None;
        } else if self.camera_poll.is_none() {
            self.camera_poll = Some(Self::poll_cameras(cx));
        }
        if stage != Stage::Problem {
            self.step = stage.step();
        }
        cx.notify();
    }

    /// Stops the camera and takes the pattern down.
    fn release_hardware(&mut self, cx: &mut Context<Self>) {
        self.cancel_flow();
        self.send(Command::Close, cx);
        self.close_pattern(cx);
        self.restore_window(cx);
        self.restarting = false;
    }

    pub(crate) fn back_to_setup(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.release_hardware(cx);
        // Ignore anything the stopped session still reports.
        self.session += 1;
        self.preview = None;
        self.markers = None;
        self.finished = None;
        self.use_measured = false;
        self.saved_to = None;
        self.confirm_replace = false;
        self.saving = false;
        self.save_task = None;
        self.attempt_saving = false;
        self.attempt_status = None;
        self.save_error = None;
        self.set_stage(Stage::Setup, cx);
        Self::refresh_saved_screens(cx);
    }

    /// Calibrate another screen: back to Setup with the screen number chosen afresh and
    /// the size cleared, since it described the last screen.
    pub(crate) fn calibrate_another(&mut self, cx: &mut Context<Self>) {
        self.screen_choice = None;
        self.size_text.clear();
        self.manual_size = false;
        self.back_to_setup(cx);
    }

    /// Returns to the workspace after any unfinished measurement is confirmed.
    pub(crate) fn exit(cx: &mut Context<Self>) {
        cx.defer(crate::shell::finish_calibration);
    }

    pub(crate) fn go_home(&mut self, cx: &mut Context<Self>) {
        if self.stage != Stage::Saved || self.saving {
            return;
        }
        let monitor = self.monitors.read(cx).selected_key();
        cx.defer(move |cx| {
            crate::shell::show_home(cx);
            if let Some(monitor) = monitor {
                crate::views::zero::open(monitor, cx);
            }
        });
    }

    pub(super) fn problem(&mut self, failure: Failure, cx: &mut Context<Self>) {
        // A failed preview can arrive during the same-monitor countdown.
        // Stop it before it can move the problem screen back into Measuring.
        self.cancel_flow();
        self.close_pattern(cx);
        self.restore_window(cx);
        self.failure = Some(failure);
        self.set_stage(Stage::Problem, cx);
    }

    /// The button on the problem screen: back to the step that can try again.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        let Some(failure) = self.failure.clone() else {
            return self.back_to_setup(cx);
        };
        match failure.kind {
            FailureKind::Markers if failure.camera_open => {
                if !self.same_monitor {
                    self.open_pattern(PatternMode::Black, cx);
                }
                self.enter_markers(cx);
            }
            // The size lives in Setup. Trying again would meet the same error.
            FailureKind::Internal | FailureKind::SizeInvalid | FailureKind::SizeShape => {
                self.back_to_setup(cx);
            }
            _ => self.begin_position(cx),
        }
    }
}

#[cfg(all(test, feature = "ui-automation"))]
mod tests {
    use gpui::{AppContext as _, TestAppContext, WindowOptions};

    use super::{Stage, Wizard};
    use crate::cli::CalibrateArgs;
    use crate::hub::{Hub, HubHandle};
    use crate::theme::{ActivePalette, Palette};
    use crate::ui_state::UiStateWriter;
    use crate::views::calibrate::automatic::calibrate_automatic::CalibrateAutomaticView;
    use crate::workspace::{Workspace, WorkspaceHandle};

    #[gpui::test]
    fn back_to_setup_preserves_active_save(cx: &mut TestAppContext) {
        crate::runtime::start().expect("Tokio runtime starts");
        cx.update(gpui::init);
        cx.update(|cx| {
            cx.set_global(ActivePalette(Palette::for_appearance(
                gpui::WindowAppearance::Light,
            )));
            cx.set_global(UiStateWriter::start());
            let (_, server_ready) = tokio::sync::watch::channel(false);
            let hub = cx.new(|cx| Hub::new(server_ready, cx));
            cx.set_global(HubHandle(hub));
            let workspace = cx.new(Workspace::new);
            cx.set_global(WorkspaceHandle(workspace));
        });
        let mut wizard = None;
        let (window, _) = cx
            .update(|cx| {
                gpui::open_window(WindowOptions::default(), cx, |window, cx| {
                    let state = cx.new(|cx| {
                        let mut state = Wizard::new(
                            Some(CalibrateArgs {
                                demo: true,
                                ..CalibrateArgs::default()
                            }),
                            window,
                            cx,
                        );
                        // Navigation needs no startup I/O that could wake GPUI from Tokio.
                        state._tasks.clear();
                        state.camera_poll = None;
                        state.finding_task = None;
                        state
                    });
                    wizard = Some(state.clone());
                    cx.new(|cx| CalibrateAutomaticView::new(state, window, cx))
                })
            })
            .expect("test window opens");
        let wizard = wizard.expect("calibration view owns wizard");

        cx.update_window(window, |_, _, cx| {
            wizard.update(cx, |wizard, cx| {
                wizard.stage = Stage::Review;
                wizard.saving = true;
                wizard.save_task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
            });
            wizard.update(cx, Wizard::back_to_setup);
            let wizard = wizard.read(cx);
            assert_eq!(wizard.stage, Stage::Review);
            assert!(wizard.saving);
            assert!(wizard.save_task.is_some());
        })
        .expect("test window remains open");
    }
}
