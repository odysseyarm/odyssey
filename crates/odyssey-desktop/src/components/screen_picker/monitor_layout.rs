use odyssey_hub_common::config::MonitorKey;

/// What the picker knows about one monitor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MonitorInfo {
    pub(crate) key: MonitorKey,
    pub(crate) primary: bool,
}

impl MonitorInfo {
    /// GPUI does not report display names, so a key's name is usually empty.
    pub(crate) fn display_name(&self) -> &str {
        self.key.name.as_deref().unwrap_or("Display")
    }

    pub(crate) fn resolution(&self) -> String {
        format!("{} × {}", self.key.width, self.key.height)
    }
}

/// Index of the monitor to show as selected: the remembered one if it is still
/// attached, otherwise the primary monitor, otherwise the first.
pub(super) fn resolve_selected(
    monitors: &[MonitorInfo],
    remembered: Option<&MonitorKey>,
) -> Option<usize> {
    remembered
        .and_then(|key| monitors.iter().position(|m| &m.key == key))
        .or_else(|| monitors.iter().position(|m| m.primary))
        .or(if monitors.is_empty() { None } else { Some(0) })
}

/// A monitor's rectangle in the diagram, in diagram pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tile {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) w: f64,
    pub(super) h: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layout {
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) tiles: Vec<Tile>,
}

/// Scales the monitors' physical-pixel rectangles to fit `max_w` × `max_h`,
/// preserving the arrangement. Positions may be negative (a monitor left of or
/// above the primary), so the layout is relative to the bounding box's corner.
/// The scale never exceeds 1 so a tiny virtual desktop is not blown up.
pub(super) fn layout(monitors: &[MonitorInfo], max_w: f64, max_h: f64) -> Option<Layout> {
    if monitors.is_empty() {
        return None;
    }

    // A zero-sized monitor would collapse its tile and could zero the bounding
    // box, so every extent is at least one pixel.
    let rects: Vec<(i64, i64, i64, i64)> = monitors
        .iter()
        .map(|m| {
            (
                i64::from(m.key.x),
                i64::from(m.key.y),
                i64::from(m.key.width.max(1)),
                i64::from(m.key.height.max(1)),
            )
        })
        .collect();

    let min_x = rects.iter().map(|r| r.0).min()?;
    let min_y = rects.iter().map(|r| r.1).min()?;
    let max_x = rects.iter().map(|r| r.0 + r.2).max()?;
    let max_y = rects.iter().map(|r| r.1 + r.3).max()?;
    let span_w = (max_x - min_x) as f64;
    let span_h = (max_y - min_y) as f64;

    let scale = (max_w / span_w).min(max_h / span_h).min(1.0);
    Some(Layout {
        width: span_w * scale,
        height: span_h * scale,
        tiles: rects
            .iter()
            .map(|&(x, y, w, h)| Tile {
                x: (x - min_x) as f64 * scale,
                y: (y - min_y) as f64 * scale,
                w: w as f64 * scale,
                h: h as f64 * scale,
            })
            .collect(),
    })
}

/// How much a tile can say about its monitor before the text overflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Detail {
    /// Number only.
    Minimal,
    /// Number and resolution.
    Compact,
    /// Number, name and resolution.
    Named,
    /// Number, name, resolution and the primary tag.
    Full,
}

/// The height thresholds add up the stacked text rows (badge 24 px, name
/// 16 px, resolution 14 px, tag 16 px) plus the tile's border and padding.
pub(super) fn detail_for(tile: &Tile) -> Detail {
    if tile.w >= 120.0 && tile.h >= 92.0 {
        Detail::Full
    } else if tile.w >= 110.0 && tile.h >= 72.0 {
        Detail::Named
    } else if tile.w >= 72.0 && tile.h >= 52.0 {
        Detail::Compact
    } else {
        Detail::Minimal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        name: Option<&str>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            key: MonitorKey {
                name: name.map(str::to_string),
                x,
                y,
                width,
                height,
            },
            primary,
        }
    }

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn identical_monitors_get_distinct_keys_by_position() {
        let left = monitor(Some("DELL U2723QE"), 0, 0, 2560, 1440, true);
        let right = monitor(Some("DELL U2723QE"), 2560, 0, 2560, 1440, false);
        assert_ne!(left.key, right.key);

        let monitors = [left, right.clone()];
        assert_eq!(resolve_selected(&monitors, Some(&right.key)), Some(1));
    }

    #[test]
    fn remembered_monitor_wins_over_primary() {
        let monitors = [
            monitor(Some("A"), 0, 0, 1920, 1080, true),
            monitor(Some("B"), 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(resolve_selected(&monitors, Some(&monitors[1].key)), Some(1));
    }

    #[test]
    fn missing_remembered_monitor_falls_back_to_primary() {
        let monitors = [
            monitor(Some("A"), -1920, 0, 1920, 1080, false),
            monitor(Some("B"), 0, 0, 1920, 1080, true),
        ];
        let gone = monitor(Some("C"), 1920, 0, 1920, 1080, false).key;
        assert_eq!(resolve_selected(&monitors, Some(&gone)), Some(1));
        assert_eq!(resolve_selected(&monitors, None), Some(1));
    }

    #[test]
    fn a_moved_monitor_is_not_the_remembered_one() {
        let monitors = [
            monitor(Some("A"), 0, 0, 1920, 1080, true),
            monitor(Some("B"), 0, -1080, 1920, 1080, false),
        ];
        let before_rearranging = monitor(Some("B"), 1920, 0, 1920, 1080, false).key;
        assert_eq!(
            resolve_selected(&monitors, Some(&before_rearranging)),
            Some(0)
        );
    }

    #[test]
    fn without_a_primary_the_first_monitor_is_selected() {
        let monitors = [
            monitor(None, 0, 0, 1920, 1080, false),
            monitor(None, 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(resolve_selected(&monitors, None), Some(0));
        assert_eq!(resolve_selected(&[], None), None);
    }

    #[test]
    fn layout_fits_width_and_preserves_arrangement() {
        // Two 1920×1080 side by side: 3840×1080, width-limited.
        let monitors = [
            monitor(None, 0, 0, 1920, 1080, true),
            monitor(None, 1920, 0, 1920, 1080, false),
        ];
        let l = layout(&monitors, 384.0, 200.0).unwrap();
        assert_close(l.width, 384.0);
        assert_close(l.height, 108.0);
        assert_eq!(
            l.tiles[0],
            Tile {
                x: 0.0,
                y: 0.0,
                w: 192.0,
                h: 108.0
            }
        );
        assert_eq!(
            l.tiles[1],
            Tile {
                x: 192.0,
                y: 0.0,
                w: 192.0,
                h: 108.0
            }
        );
    }

    #[test]
    fn layout_fits_height_for_stacked_monitors() {
        // 1080×1920 portrait on top of another: 1080×3840, height-limited.
        let monitors = [
            monitor(None, 0, 0, 1080, 1920, true),
            monitor(None, 0, 1920, 1080, 1920, false),
        ];
        let l = layout(&monitors, 400.0, 192.0).unwrap();
        assert_close(l.height, 192.0);
        assert_close(l.width, 54.0);
        assert_close(l.tiles[1].y, 96.0);
    }

    #[test]
    fn layout_handles_negative_and_offset_positions() {
        // A monitor left of and above the primary, and a taller one offset down.
        let monitors = [
            monitor(None, -1000, -500, 1000, 500, false),
            monitor(None, 0, 0, 1000, 1000, true),
            monitor(None, 1000, 250, 1000, 1000, false),
        ];
        let l = layout(&monitors, 1.0e6, 1.0e6).unwrap();
        // The scale is capped at 1, so tiles are in physical pixels relative
        // to the bounding box's top-left corner at (-1000, -500).
        assert_close(l.width, 3000.0);
        assert_close(l.height, 1750.0);
        assert_eq!(
            l.tiles[0],
            Tile {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 500.0
            }
        );
        assert_eq!(
            l.tiles[1],
            Tile {
                x: 1000.0,
                y: 500.0,
                w: 1000.0,
                h: 1000.0
            }
        );
        assert_eq!(
            l.tiles[2],
            Tile {
                x: 2000.0,
                y: 750.0,
                w: 1000.0,
                h: 1000.0
            }
        );
    }

    #[test]
    fn layout_never_upscales_and_tolerates_empty_or_degenerate_monitors() {
        assert_eq!(layout(&[], 400.0, 200.0), None);

        let tiny = [monitor(None, 0, 0, 100, 50, true)];
        let l = layout(&tiny, 400.0, 200.0).unwrap();
        assert_close(l.width, 100.0);

        let zero = [monitor(None, 0, 0, 0, 0, true)];
        let l = layout(&zero, 400.0, 200.0).unwrap();
        assert!(l.width > 0.0 && l.height > 0.0);
    }

    #[test]
    fn detail_shrinks_with_tile_size() {
        let tile = |w, h| Tile {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        assert_eq!(detail_for(&tile(356.0, 200.0)), Detail::Full);
        assert_eq!(detail_for(&tile(130.0, 80.0)), Detail::Named);
        assert_eq!(detail_for(&tile(100.0, 56.0)), Detail::Compact);
        assert_eq!(detail_for(&tile(60.0, 34.0)), Detail::Minimal);
        // A wide strip too short for any text beside the number.
        assert_eq!(detail_for(&tile(300.0, 30.0)), Detail::Minimal);
    }
}
