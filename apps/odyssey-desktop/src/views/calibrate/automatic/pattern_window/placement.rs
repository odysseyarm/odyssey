//! Whether the pattern window would cover the wizard's own window.

use dioxus::desktop::DesktopContext;
use odyssey_hub_common::config::MonitorKey;

/// A rectangle in desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Rect {
    pub(crate) fn of_monitor(key: &MonitorKey) -> Self {
        Self {
            x: key.x.into(),
            y: key.y.into(),
            width: key.width.into(),
            height: key.height.into(),
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
/// the window `host`. Falls back to "same monitor" where the platform does not
/// report window geometry.
pub(crate) fn covers_window(
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
