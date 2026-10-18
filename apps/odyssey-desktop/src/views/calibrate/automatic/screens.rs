//! Screens: which `screen_<id>.json` files exist, which screen a new
//! calibration should default to, and how a screen id is shown.
//!
//! The id is 0-based because it names the file the hub reads; people see it
//! 1-based ("Screen 1"). [`screen_label`] is the one place that maps between
//! the two.

use std::path::Path;

use odyssey_common::MAX_SCREEN_ID;
use odyssey_hub_common::config::screen_file_name;

pub const SCREEN_COUNT: usize = MAX_SCREEN_ID as usize + 1;

/// The name people see for screen `id`, counted from 1.
pub fn screen_label(id: u8) -> String {
    format!("Screen {}", u32::from(id) + 1)
}

/// For each screen id, whether a calibration file is saved in `screens_dir`.
pub fn saved_screens(screens_dir: &Path) -> [bool; SCREEN_COUNT] {
    std::array::from_fn(|id| screens_dir.join(screen_file_name(id as u8)).is_file())
}

/// The screen id to preselect: the one last calibrated for this monitor, else
/// the first without a calibration, else the first (everything is taken, so the
/// tech is replacing something either way and Review asks before it does).
pub fn default_screen(remembered: Option<u8>, saved: &[bool; SCREEN_COUNT]) -> u8 {
    remembered
        .filter(|&id| usize::from(id) < SCREEN_COUNT)
        .or_else(|| saved.iter().position(|&taken| !taken).map(|id| id as u8))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(ids: &[usize]) -> [bool; SCREEN_COUNT] {
        std::array::from_fn(|i| ids.contains(&i))
    }

    #[test]
    fn people_count_screens_from_one() {
        assert_eq!(screen_label(0), "Screen 1");
        assert_eq!(screen_label(5), "Screen 6");
    }

    #[test]
    fn remembered_screen_wins_even_when_it_is_already_calibrated() {
        assert_eq!(default_screen(Some(2), &saved(&[0, 1, 2])), 2);
    }

    #[test]
    fn without_a_memory_the_first_free_screen_is_used() {
        assert_eq!(default_screen(None, &saved(&[])), 0);
        assert_eq!(default_screen(None, &saved(&[0, 1, 3])), 2);
    }

    #[test]
    fn full_screens_fall_back_to_the_first() {
        assert_eq!(default_screen(None, &saved(&[0, 1, 2, 3, 4, 5])), 0);
    }

    #[test]
    fn a_remembered_screen_that_no_longer_exists_is_ignored() {
        assert_eq!(default_screen(Some(200), &saved(&[0])), 1);
    }

    #[test]
    fn saved_screens_follow_the_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("odyssey-screens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(screen_file_name(1)), "{}").unwrap();
        std::fs::write(dir.join(screen_file_name(4)), "{}").unwrap();
        std::fs::write(dir.join("screen_9.json"), "{}").unwrap();
        assert_eq!(saved_screens(&dir), saved(&[1, 4]));
        let _ = std::fs::remove_dir_all(dir);
    }
}
