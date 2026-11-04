//! Opening the pattern window and keeping it tied to the wizard.

use std::sync::Arc;
use std::time::Duration;

use dioxus::desktop::tao::monitor::MonitorHandle;
use dioxus::desktop::tao::window::{Fullscreen, Window};
use dioxus::desktop::{Config, DesktopContext, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;
use odyssey_autocalib::pattern::Pattern;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::mode::{PatternEvent, PatternLoss, PatternMode};
use super::screen::{PatternScreen, PatternScreenProps};
use crate::components::screen_picker::monitor_key;

/// How long a window manager gets to honor the fullscreen request before the
/// window is checked to be on the right monitor.
const PLACEMENT_CHECK_DELAY: Duration = Duration::from_millis(800);

/// Everything the window component shares with the wizard. Compared by
/// identity: the window is built once and never re-rendered from outside.
#[derive(Clone)]
pub(super) struct Link(pub(super) Arc<LinkInner>);

pub(super) struct LinkInner {
    pub(super) pattern: Pattern,
    /// The monitor's resolution, which the ruler and card are drawn in.
    pub(super) pixels: (u32, u32),
    pub(super) mode: watch::Receiver<PatternMode>,
    pub(super) events: UnboundedSender<PatternEvent>,
}

impl PartialEq for Link {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// An open pattern window. Dropping it closes the window.
pub(crate) struct PatternDisplay {
    mode: watch::Sender<PatternMode>,
}

impl PatternDisplay {
    pub(crate) fn show(&self, mode: PatternMode) {
        self.mode.send_replace(mode);
    }
}

/// Opens a borderless fullscreen window on `monitor`. `events` hears about it if
/// the window goes away or lands on the wrong monitor while the wizard still
/// wants it, and about the card frame being resized from the keyboard.
pub(crate) fn open(
    host: &DesktopContext,
    monitor: MonitorHandle,
    pattern: Pattern,
    initial: PatternMode,
    events: UnboundedSender<PatternEvent>,
) -> PatternDisplay {
    let (mode, receiver) = watch::channel(initial);
    let size = monitor.size();
    let link = Link(Arc::new(LinkInner {
        pattern,
        pixels: (size.width, size.height),
        mode: receiver,
        events: events.clone(),
    }));
    let dom = VirtualDom::new_with_props(PatternScreen, PatternScreenProps { link });
    let config = Config::default()
        .with_menu(None)
        .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
        .with_window(
            WindowBuilder::new()
                .with_title("Calibration pattern")
                .with_position(monitor.position())
                .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor.clone())))),
        );
    let pending = host.new_window(dom, config);
    // Some window managers (tiling ones in particular) ignore a fullscreen or
    // position request made before the window is mapped and open it wherever
    // the user is working, so ask again, for the right monitor, once it exists.
    let target = monitor.clone();
    let wizard_side = mode.subscribe();
    spawn(async move {
        let window = pending.await;
        let tao: &Window = &window;
        tao.set_outer_position(target.position());
        tao.set_fullscreen(Some(Fullscreen::Borderless(Some(target.clone()))));

        // Fullscreen is a request. If the window manager put the window on
        // another monitor anyway, the wizard must not go on with the camera
        // looking at the wrong screen.
        tokio::time::sleep(PLACEMENT_CHECK_DELAY).await;
        // The wizard dropping its display means it no longer cares.
        if wizard_side.has_changed().is_err() {
            return;
        }
        let landed = tao.current_monitor().map(|m| monitor_key(&m));
        if landed.is_some_and(|landed| landed != monitor_key(&target)) {
            let _ = events.send(PatternEvent::Lost(PatternLoss::WrongMonitor));
        }
    });
    PatternDisplay { mode }
}
