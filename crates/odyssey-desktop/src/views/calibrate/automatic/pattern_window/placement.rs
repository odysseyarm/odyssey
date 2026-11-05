//! Whether the pattern window would cover the wizard's own window.

use gpui::{Bounds, DisplayId, PlatformDisplay, Pixels, Point};

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    fn of_bounds(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f32::from(bounds.origin.x).round() as i64,
            y: f32::from(bounds.origin.y).round() as i64,
            width: f32::from(bounds.size.width).round() as i64,
            height: f32::from(bounds.size.height).round() as i64,
        }
    }

    /// Whether the two share any area; touching edges do not count.
    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// Whether showing the pattern fullscreen on `target` would cover (part of)
/// the wizard window, which has `host` bounds and sits on `host_display`.
/// A window on the target display is covered outright. A window straddling
/// displays is caught by overlap, unless the platform reports no window
/// position (Wayland gives the origin), where only the display counts.
pub(crate) fn covers_window(
    host: Bounds<Pixels>,
    host_display: Option<DisplayId>,
    target: &dyn PlatformDisplay,
) -> bool {
    if host_display == Some(target.id()) {
        return true;
    }
    host.origin != Point::default()
        && Rect::of_bounds(host).overlaps(&Rect::of_bounds(target.bounds()))
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
