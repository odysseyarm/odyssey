//! Opening the pattern window and keeping it tied to the wizard.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, DisplayId, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions,
};
use odyssey_autocalib::pattern::Pattern;
use tokio::sync::mpsc::UnboundedSender;

use super::mode::{PatternEvent, PatternLoss, PatternMode};
use super::screen::PatternView;

/// How long a window manager gets to honor the fullscreen request before the
/// window is checked to be on the right monitor.
const PLACEMENT_CHECK_DELAY: Duration = Duration::from_millis(800);

/// An open pattern window. [`PatternDisplay::close`] closes it; a window the
/// wizard did not close reports itself lost through the wizard's event channel.
pub(crate) struct PatternDisplay {
    window: WindowHandle<PatternView>,
    wizard_closed: Rc<Cell<bool>>,
}

impl PatternDisplay {
    pub(crate) fn show(&self, mode: PatternMode, cx: &mut App) {
        let _ = self.window.update(cx, |view, _, cx| {
            view.set_mode(mode);
            cx.notify();
        });
    }

    /// Closes the window without reporting it lost.
    pub(crate) fn close(self, cx: &mut App) {
        self.wizard_closed.set(true);
        let _ = self
            .window
            .update(cx, |_, window, _| window.remove_window());
    }
}

/// Opens a borderless fullscreen window on `display`, which is `pixels` big in
/// the units the pictures are laid out in. `events` hears about it if the
/// window goes away or lands on the wrong display while the wizard still wants
/// it, and about the card frame being resized from the keyboard.
pub(crate) fn open(
    display: DisplayId,
    pixels: (u32, u32),
    pattern: Pattern,
    initial: PatternMode,
    events: UnboundedSender<PatternEvent>,
    cx: &mut App,
) -> Option<PatternDisplay> {
    let bounds = cx.find_display(display)?.bounds();
    let wizard_closed = Rc::new(Cell::new(false));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Calibration pattern".into()),
            ..Default::default()
        }),
        display_id: Some(display),
        ..Default::default()
    };
    let window = cx.open_window(options, {
        let events = events.clone();
        let wizard_closed = Rc::clone(&wizard_closed);
        move |window, cx| {
            cx.new(|cx| {
                PatternView::new(pattern, pixels, initial, events, wizard_closed, window, cx)
            })
        }
    });
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            tracing::error!("could not open the pattern window: {error:#}");
            return None;
        }
    };

    // Fullscreen is a request. If the window manager put the window on
    // another display anyway, the wizard must not go on with the camera
    // looking at the wrong screen.
    let closed = Rc::clone(&wizard_closed);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(PLACEMENT_CHECK_DELAY).await;
        if closed.get() {
            return;
        }
        let landed = cx.update(|cx| {
            window
                .update(cx, |_, window, cx| window.display(cx).map(|d| d.id()))
                .ok()
                .flatten()
        });
        if landed.is_some_and(|landed| landed != display) {
            let _ = events.send(PatternEvent::Lost(PatternLoss::WrongMonitor));
        }
    })
    .detach();

    Some(PatternDisplay {
        window,
        wizard_closed,
    })
}
