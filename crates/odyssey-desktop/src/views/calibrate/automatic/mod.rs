//! Automatic calibration: a depth camera measures the screen and the tracker
//! markers while this app shows a dot pattern, and the result is saved as a
//! screen calibration.
//!
//! [`open`] shows the wizard in its own window. [`CalibrateAutomaticView`] owns
//! the frame and draws the step that matches the [`wizard::Wizard`]'s stage;
//! see `wizard.rs` for the flow.

mod attempt_bundle;
mod attempt_button;
mod calibrate_automatic;
mod camera_support;
pub(crate) mod hints;
mod illustrations;
pub(crate) mod kind;
mod markers;
mod measuring;
pub(crate) mod pattern_window;
mod position;
mod problem;
pub(crate) mod quality;
pub(crate) mod reference;
mod reference_helper;
mod review;
mod scale_board_info;
pub(crate) mod screens;
mod sdk_download;
pub(crate) mod settings;
mod setup;
mod ui;
pub(crate) mod wizard;
pub(crate) mod worker;

pub(crate) use calibrate_automatic::CalibrateAutomaticView;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use wizard::Wizard;

use crate::cli::CalibrateArgs;

const WINDOW_SIZE: (f32, f32) = (1100.0, 760.0);

/// The wizard window while one is open: two wizards would fight over the camera
/// and the pattern window, so a second request raises the first.
struct OpenWizard(AnyWindowHandle);

impl Global for OpenWizard {}

/// Opens the wizard in its own window. `args` are the command-line answers
/// that skip or prefill Setup; they go to the wizard this call opens.
pub(crate) fn open(args: Option<CalibrateArgs>, cx: &mut App) {
    let open = cx.try_global::<OpenWizard>().map(|open| open.0);
    if let Some(window) = open
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Odyssey calibration".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
            cx,
        ))),
        ..Default::default()
    };
    match gpui::open_window(options, cx, |window, cx| {
        let wizard = cx.new(|cx| Wizard::new(args, window, cx));
        cx.new(|cx| CalibrateAutomaticView::new(wizard, window, cx))
    }) {
        Ok((window, _)) => cx.set_global(OpenWizard(window)),
        Err(error) => tracing::error!("could not open the calibration window: {error:#}"),
    }
}
