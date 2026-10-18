//! The fullscreen window that shows the calibration dot pattern on the target
//! monitor.
//!
//! The pattern is an SVG in the pattern's own unit space (`Pattern::dots`), so
//! it fills whatever resolution the monitor has. The window lives exactly as
//! long as its [`PatternDisplay`]: dropping the display closes the window.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use dioxus::desktop::tao::monitor::MonitorHandle;
use dioxus::desktop::tao::window::{Fullscreen, Window};
use dioxus::desktop::{Config, DesktopContext, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;
use odyssey_autocalib::pattern::Pattern;
use odyssey_hub_common::config::MonitorKey;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use super::reference;
use crate::components::screen_picker::monitor_key;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

/// How long a window manager gets to honor the fullscreen request before the
/// window is checked to be on the right monitor.
const PLACEMENT_CHECK_DELAY: std::time::Duration = std::time::Duration::from_millis(800);

/// What the pattern window shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternMode {
    /// The dot pattern, white on black.
    Pattern,
    /// A plain black screen.
    Black,
    /// Seconds left before the pattern appears, over black.
    Countdown(u32),
    /// A line `line_px` pixels long with a tick at each end, for a tape
    /// measure. Drawn in screen pixels, not in the pattern's unit space.
    Ruler { line_px: u32 },
    /// A frame whose inside is a credit card `width_px` pixels wide, for
    /// holding a real card against. Drawn in screen pixels.
    Card { width_px: f64 },
}

/// Why the pattern window can no longer do its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternLoss {
    /// Someone closed the window (Escape or Alt+F4).
    Closed,
    /// The window manager put the window on a different monitor than the one
    /// being calibrated, so the camera would measure the wrong screen.
    WrongMonitor,
}

/// Everything the window component shares with the wizard. Compared by
/// identity: the window is built once and never re-rendered from outside.
#[derive(Clone)]
struct Link(Arc<LinkInner>);

struct LinkInner {
    pattern: Pattern,
    /// The monitor's resolution, which the ruler and card are drawn in.
    pixels: (u32, u32),
    mode: watch::Receiver<PatternMode>,
    lost: UnboundedSender<PatternLoss>,
}

impl PartialEq for Link {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// An open pattern window. Dropping it closes the window.
pub struct PatternDisplay {
    mode: watch::Sender<PatternMode>,
}

impl PatternDisplay {
    pub fn show(&self, mode: PatternMode) {
        self.mode.send_replace(mode);
    }
}

/// Opens a borderless fullscreen window on `monitor`. `lost` hears about it if
/// the window goes away or lands on the wrong monitor while the wizard still
/// wants it.
pub fn open(
    host: &DesktopContext,
    monitor: MonitorHandle,
    pattern: Pattern,
    initial: PatternMode,
    lost: UnboundedSender<PatternLoss>,
) -> PatternDisplay {
    let (mode, receiver) = watch::channel(initial);
    let size = monitor.size();
    let link = Link(Arc::new(LinkInner {
        pattern,
        pixels: (size.width, size.height),
        mode: receiver,
        lost: lost.clone(),
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
            let _ = lost.send(PatternLoss::WrongMonitor);
        }
    });
    PatternDisplay { mode }
}

#[component]
fn PatternScreen(link: Link) -> Element {
    let mut mode = use_signal(|| *link.0.mode.borrow());
    // A plain cell rather than a signal: cleanup may run after the scope's
    // signals are gone.
    let wizard_closed_it = use_hook(|| Rc::new(Cell::new(false)));

    use_future({
        let link = link.clone();
        let wizard_closed_it = wizard_closed_it.clone();
        move || {
            let mut receiver = link.0.mode.clone();
            let wizard_closed_it = wizard_closed_it.clone();
            async move {
                // `changed` fails once the wizard drops its display, which is
                // the wizard's way of closing this window.
                while receiver.changed().await.is_ok() {
                    mode.set(*receiver.borrow_and_update());
                }
                wizard_closed_it.set(true);
                dioxus::desktop::window().close();
            }
        }
    });

    use_drop({
        let link = link.clone();
        move || {
            if !wizard_closed_it.get() {
                let _ = link.0.lost.send(PatternLoss::Closed);
            }
        }
    });

    let pattern = &link.0.pattern;
    let aspect = pattern.aspect();
    let (screen_w, screen_h) = (f64::from(link.0.pixels.0), f64::from(link.0.pixels.1));

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            class: "fixed inset-0 bg-black select-none overflow-hidden outline-none",
            style: "cursor: none;",
            tabindex: 0,
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    dioxus::desktop::window().close();
                }
            },
            match mode() {
                PatternMode::Pattern => rsx! {
                    // The viewBox is the pattern's unit space, so stretching it
                    // over a window with the monitor's aspect keeps dots round.
                    svg {
                        class: "size-full",
                        view_box: "0 0 {aspect} 1",
                        preserve_aspect_ratio: "none",
                        shape_rendering: "geometricPrecision",
                        // The border ring and corner brackets, whose visibility
                        // tells the camera whether the display crops the picture.
                        for (i, rect) in pattern.frame_rects().iter().enumerate() {
                            rect {
                                key: "frame-{i}",
                                x: "{rect.x}",
                                y: "{rect.y}",
                                width: "{rect.width}",
                                height: "{rect.height}",
                                fill: "white",
                            }
                        }
                        for (i, dot) in pattern.dots().iter().enumerate() {
                            circle {
                                key: "{i}",
                                cx: "{dot.u * aspect}",
                                cy: "{dot.v}",
                                r: "{dot.radius_h}",
                                fill: "white",
                            }
                        }
                    }
                },
                PatternMode::Black => rsx! {},
                PatternMode::Ruler { line_px } => {
                    let line = f64::from(line_px);
                    let x0 = ((screen_w - line) / 2.0).floor();
                    let y = (screen_h / 2.0).floor();
                    rsx! {
                        // The viewBox is the monitor's own pixels, so a unit here is one
                        // pixel and the line is as long as the tech is told it is.
                        svg {
                            class: "size-full",
                            view_box: "0 0 {screen_w} {screen_h}",
                            preserve_aspect_ratio: "none",
                            shape_rendering: "crispEdges",
                            rect { x: "{x0}", y: "{y - 1.0}", width: "{line}", height: "2", fill: "white" }
                            rect { x: "{x0 - 1.0}", y: "{y - 40.0}", width: "2", height: "80", fill: "white" }
                            rect { x: "{x0 + line - 1.0}", y: "{y - 40.0}", width: "2", height: "80", fill: "white" }
                            text {
                                x: "{screen_w / 2.0}",
                                y: "{y + 96.0}",
                                text_anchor: "middle",
                                fill: "#9ca3af",
                                font_size: "28",
                                font_family: "sans-serif",
                                "Measure from the middle of one tick to the middle of the other"
                            }
                        }
                    }
                }
                PatternMode::Card { width_px } => {
                    let height_px = width_px * reference::CARD_HEIGHT_MM / reference::CARD_WIDTH_MM;
                    let radius = width_px * reference::CARD_CORNER_OF_WIDTH;
                    let (left, top) = ((screen_w - width_px) / 2.0, (screen_h - height_px) / 2.0);
                    let frame = 4.0;
                    rsx! {
                        svg {
                            class: "size-full",
                            view_box: "0 0 {screen_w} {screen_h}",
                            preserve_aspect_ratio: "none",
                            shape_rendering: "geometricPrecision",
                            // A white frame whose inside edge is exactly the card: a card of
                            // the right size fills it with no dark gap and covers none of it.
                            rect {
                                x: "{left - frame}",
                                y: "{top - frame}",
                                width: "{width_px + 2.0 * frame}",
                                height: "{height_px + 2.0 * frame}",
                                rx: "{radius + frame}",
                                fill: "white",
                            }
                            rect {
                                x: "{left}",
                                y: "{top}",
                                width: "{width_px}",
                                height: "{height_px}",
                                rx: "{radius}",
                                fill: "black",
                            }
                            text {
                                x: "{screen_w / 2.0}",
                                y: "{top + height_px + frame + 64.0}",
                                text_anchor: "middle",
                                fill: "#9ca3af",
                                font_size: "28",
                                font_family: "sans-serif",
                                "Hold a card flat against the glass, inside the frame"
                            }
                        }
                    }
                }
                PatternMode::Countdown(seconds) => rsx! {
                    div {
                        class: "size-full flex flex-col items-center justify-center gap-6 text-white",
                        div { class: "text-3xl opacity-80", "Measuring in" }
                        div { class: "text-[14rem] leading-none font-bold tabular-nums", "{seconds}" }
                        div { class: "text-2xl opacity-80", "Hold the camera steady" }
                    }
                },
            }
        }
    }
}

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    pub fn of_monitor(key: &MonitorKey) -> Self {
        Self {
            x: key.x.into(),
            y: key.y.into(),
            width: key.width.into(),
            height: key.height.into(),
        }
    }

    /// Whether the two share any area; touching edges do not count.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// Whether showing the pattern fullscreen on `target` would cover (part of)
/// the window `host`. Falls back to "same monitor" where the platform does not
/// report window geometry.
pub fn covers_window(
    host: &DesktopContext,
    target: &MonitorKey,
    current_monitor: Option<MonitorKey>,
) -> bool {
    match host.outer_position() {
        Ok(position) => {
            let size = host.outer_size();
            let window = Rect {
                x: position.x.into(),
                y: position.y.into(),
                width: size.width.into(),
                height: size.height.into(),
            };
            window.overlaps(&Rect::of_monitor(target))
        }
        Err(_) => current_monitor.as_ref() == Some(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i64, y: i64, width: i64, height: i64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_inside_the_monitor_is_covered() {
        assert!(rect(100, 100, 900, 700).overlaps(&rect(0, 0, 2560, 1440)));
    }

    #[test]
    fn a_window_straddling_two_monitors_is_covered_by_either() {
        let window = rect(2400, 200, 800, 600);
        assert!(window.overlaps(&rect(0, 0, 2560, 1440)));
        assert!(window.overlaps(&rect(2560, 0, 1920, 1080)));
    }

    #[test]
    fn a_window_on_another_monitor_is_not_covered() {
        assert!(!rect(2700, 100, 900, 700).overlaps(&rect(0, 0, 2560, 1440)));
        // Monitors to the left or above sit at negative coordinates.
        assert!(!rect(100, 100, 900, 700).overlaps(&rect(-1920, 0, 1920, 1080)));
        // Touching edges share no area.
        assert!(!rect(2560, 0, 900, 700).overlaps(&rect(0, 0, 2560, 1440)));
    }
}
