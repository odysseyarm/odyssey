//! Hot reload of `screens/screen_<id>.json`.
//!
//! The calibration wizard, hand edits and manual calibration all write these
//! files while the hub is running. A cheap mtime poll picks the changes up;
//! `notify` is not in the dependency tree and six `stat` calls a second cost
//! nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::config::{
    ScreenCalibrationsResult, screen_calibrations_from_dir_async, screen_file_name,
};
use tokio_util::sync::CancellationToken;

const SLOTS: usize = odyssey_common::MAX_SCREEN_ID as usize + 1;

/// Time between scans. A change has to look identical on two consecutive scans
/// before it loads, so a write takes between one and two intervals to apply.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub type Calibrations = ArrayVec<(u8, ScreenCalibration<f32>), SLOTS>;

/// What a scan can tell about a file without reading it. `None` means the file
/// is absent. The inode is part of it because an atomic replace can land inside
/// the filesystem's mtime granularity with an unchanged length.
type Fingerprint = Option<(SystemTime, u64, u64)>;
type Fingerprints = [Fingerprint; SLOTS];

/// Outcome of a reload that was applied.
#[derive(Debug, PartialEq, Eq)]
pub struct Reload {
    /// Screens whose file failed to load and whose previous calibration stays active.
    pub kept_previous: Vec<u8>,
}

pub struct ScreenCalibrationWatcher {
    dir: PathBuf,
    /// Fingerprints of the files as of the last load.
    applied: Fingerprints,
    /// A differing scan seen once; it loads when the next scan matches it.
    pending: Option<Fingerprints>,
}

impl ScreenCalibrationWatcher {
    /// Snapshots the directory. Call it before the initial load so a write that
    /// lands in between still counts as a change.
    pub async fn new(dir: PathBuf) -> Self {
        let applied = scan(&dir).await;
        Self {
            dir,
            applied,
            pending: None,
        }
    }

    /// Polls until `cancel` fires, publishing changes into `target`.
    pub async fn run(mut self, target: Arc<ArcSwap<Calibrations>>, cancel: CancellationToken) {
        self.run_every(POLL_INTERVAL, target, cancel).await;
    }

    async fn run_every(
        &mut self,
        period: Duration,
        target: Arc<ArcSwap<Calibrations>>,
        cancel: CancellationToken,
    ) {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    if let Some(reload) = self.poll_once(&target).await {
                        log_reload(&target.load(), &reload);
                    }
                }
            }
        }
    }

    /// Scans once and applies a change that has held still since the previous
    /// scan. Returns `None` when nothing was applied.
    ///
    /// The wait for a stable scan covers writers that truncate and then fill a
    /// file. A file that is still unreadable after that never replaces a good
    /// calibration: the previous one stays active until the file parses.
    async fn poll_once(&mut self, target: &ArcSwap<Calibrations>) -> Option<Reload> {
        let current = scan(&self.dir).await;
        if current == self.applied {
            self.pending = None;
            return None;
        }
        if self.pending.as_ref() != Some(&current) {
            self.pending = Some(current);
            return None;
        }
        self.pending = None;
        self.applied = current;

        let loaded = screen_calibrations_from_dir_async(&self.dir).await;
        let (merged, kept_previous) = merge(&target.load(), loaded);
        target.store(Arc::new(merged));
        Some(Reload { kept_previous })
    }
}

fn log_reload(active: &Calibrations, reload: &Reload) {
    for id in &reload.kept_previous {
        tracing::warn!(
            "Screen {} calibration file is unreadable; keeping the previous calibration",
            id
        );
    }
    tracing::info!(
        "Screen calibrations reloaded; active screens: {:?}",
        active.iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
}

/// Combines a fresh load with the active calibrations: a file that loaded
/// replaces its screen, a missing file removes it, an unreadable file keeps it.
fn merge(previous: &Calibrations, loaded: ScreenCalibrationsResult) -> (Calibrations, Vec<u8>) {
    let mut merged = Calibrations::new();
    let mut kept_previous = Vec::new();
    for id in 0..=odyssey_common::MAX_SCREEN_ID {
        let fresh = loaded.calibrations.iter().find(|(i, _)| *i == id);
        let entry = if let Some(entry) = fresh {
            Some(*entry)
        } else if loaded.corrupted_screens.iter().any(|(i, _, _)| *i == id) {
            let old = previous.iter().find(|(i, _)| *i == id).copied();
            if old.is_some() {
                kept_previous.push(id);
            }
            old
        } else {
            None
        };
        if let Some(entry) = entry {
            merged.push(entry);
        }
    }
    (merged, kept_previous)
}

async fn scan(dir: &Path) -> Fingerprints {
    let mut out = [None; SLOTS];
    for (id, slot) in out.iter_mut().enumerate() {
        // Any metadata failure reads as absent; the loader decides what an
        // unreadable file means.
        if let Ok(meta) = tokio::fs::metadata(dir.join(screen_file_name(id as u8))).await {
            *slot = meta.modified().ok().map(|m| (m, meta.len(), inode(&meta)));
        }
    }
    out
}

#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(meta)
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nalgebra::{Matrix3, Point3, UnitQuaternion};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "odyssey-screen-watch-{}-{}",
                std::process::id(),
                tag
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Calibration whose homography diagonal is `scale`, so tests tell
    /// versions apart.
    fn cal(scale: f32) -> ScreenCalibration<f32> {
        ScreenCalibration {
            rotation: UnitQuaternion::identity(),
            homography: Matrix3::new(scale, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 1.0),
            object_points: [Point3::new(0.0, 0.0, 0.0); 6],
        }
    }

    /// Gives every write a distinct mtime so tests do not depend on the
    /// filesystem's timestamp granularity.
    fn bump_mtime(path: &Path) {
        static CLOCK: AtomicU64 = AtomicU64::new(1);
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(CLOCK.fetch_add(1, Ordering::Relaxed));
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn save(dir: &Path, id: u8, scale: f32) {
        let path = odyssey_autocalib::session::save_calibration(dir, id, &cal(scale)).unwrap();
        bump_mtime(&path);
    }

    fn write_raw(dir: &Path, id: u8, contents: &str) {
        let path = dir.join(screen_file_name(id));
        std::fs::write(&path, contents).unwrap();
        bump_mtime(&path);
    }

    fn scale_of(active: &Calibrations, id: u8) -> Option<f32> {
        active
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c.homography[(0, 0)])
    }

    /// Starts a watcher over `dir` and an empty published set, the way
    /// `run_server` does with its initial load.
    async fn start(dir: &TempDir) -> (ScreenCalibrationWatcher, Arc<ArcSwap<Calibrations>>) {
        let watcher = ScreenCalibrationWatcher::new(dir.0.clone()).await;
        let loaded = screen_calibrations_from_dir_async(&dir.0).await;
        (
            watcher,
            Arc::new(ArcSwap::from_pointee(loaded.calibrations)),
        )
    }

    /// Polls until a change applies. The first differing scan only arms the
    /// debounce, so a change needs exactly two polls.
    async fn settle(
        watcher: &mut ScreenCalibrationWatcher,
        target: &ArcSwap<Calibrations>,
    ) -> Reload {
        assert_eq!(
            watcher.poll_once(target).await,
            None,
            "applied without debounce"
        );
        watcher
            .poll_once(target)
            .await
            .expect("change did not apply")
    }

    #[tokio::test]
    async fn creating_a_file_adds_the_screen() {
        let dir = TempDir::new("create");
        let (mut watcher, target) = start(&dir).await;
        assert_eq!(watcher.poll_once(&target).await, None);

        save(&dir.0, 2, 1.5);
        let reload = settle(&mut watcher, &target).await;

        assert_eq!(
            reload,
            Reload {
                kept_previous: vec![]
            }
        );
        assert_eq!(scale_of(&target.load(), 2), Some(1.5));
        assert_eq!(watcher.poll_once(&target).await, None, "idle scan reloaded");
    }

    #[tokio::test]
    async fn replacing_a_file_swaps_the_value_and_keeps_other_screens() {
        let dir = TempDir::new("replace");
        save(&dir.0, 0, 1.0);
        save(&dir.0, 1, 7.0);
        let (mut watcher, target) = start(&dir).await;
        assert_eq!(scale_of(&target.load(), 0), Some(1.0));

        save(&dir.0, 0, 2.0);
        settle(&mut watcher, &target).await;

        assert_eq!(scale_of(&target.load(), 0), Some(2.0));
        assert_eq!(scale_of(&target.load(), 1), Some(7.0));
    }

    #[tokio::test]
    async fn deleting_a_file_removes_the_screen() {
        let dir = TempDir::new("delete");
        save(&dir.0, 0, 1.0);
        save(&dir.0, 3, 3.0);
        let (mut watcher, target) = start(&dir).await;

        std::fs::remove_file(dir.0.join(screen_file_name(0))).unwrap();
        settle(&mut watcher, &target).await;

        assert_eq!(scale_of(&target.load(), 0), None);
        assert_eq!(scale_of(&target.load(), 3), Some(3.0));
    }

    #[tokio::test]
    async fn missing_directory_is_empty_and_appears_later() {
        let dir = TempDir::new("late-dir");
        let screens = dir.0.join("screens");
        let mut watcher = ScreenCalibrationWatcher::new(screens.clone()).await;
        let target = Arc::new(ArcSwap::from_pointee(Calibrations::new()));
        assert_eq!(watcher.poll_once(&target).await, None);

        save(&screens, 1, 4.0);
        settle(&mut watcher, &target).await;

        assert_eq!(scale_of(&target.load(), 1), Some(4.0));
    }

    #[tokio::test]
    async fn corrupt_file_keeps_the_previous_calibration_until_it_parses() {
        let dir = TempDir::new("corrupt");
        save(&dir.0, 0, 1.0);
        save(&dir.0, 1, 5.0);
        let (mut watcher, target) = start(&dir).await;

        // A writer that truncated the file and has not filled it yet.
        write_raw(&dir.0, 0, "");
        let reload = settle(&mut watcher, &target).await;
        assert_eq!(reload.kept_previous, vec![0]);
        assert_eq!(scale_of(&target.load(), 0), Some(1.0));
        assert_eq!(scale_of(&target.load(), 1), Some(5.0));

        write_raw(&dir.0, 0, "{ \"rotation\": [0, 0,");
        let reload = settle(&mut watcher, &target).await;
        assert_eq!(reload.kept_previous, vec![0]);
        assert_eq!(scale_of(&target.load(), 0), Some(1.0));

        // The writer finishes.
        save(&dir.0, 0, 9.0);
        let reload = settle(&mut watcher, &target).await;
        assert!(reload.kept_previous.is_empty());
        assert_eq!(scale_of(&target.load(), 0), Some(9.0));
    }

    #[tokio::test]
    async fn corrupt_file_with_no_previous_calibration_stays_absent() {
        let dir = TempDir::new("corrupt-new");
        let (mut watcher, target) = start(&dir).await;

        write_raw(&dir.0, 4, "not json");
        let reload = settle(&mut watcher, &target).await;

        assert!(reload.kept_previous.is_empty());
        assert_eq!(scale_of(&target.load(), 4), None);
    }

    #[tokio::test]
    async fn a_write_that_finishes_between_scans_never_exposes_the_partial_state() {
        let dir = TempDir::new("debounce");
        save(&dir.0, 0, 1.0);
        let (mut watcher, target) = start(&dir).await;

        write_raw(&dir.0, 0, "{ \"rotation\"");
        assert_eq!(watcher.poll_once(&target).await, None);
        save(&dir.0, 0, 2.0);
        assert_eq!(watcher.poll_once(&target).await, None);
        let reload = watcher
            .poll_once(&target)
            .await
            .expect("change did not apply");

        assert!(reload.kept_previous.is_empty());
        assert_eq!(scale_of(&target.load(), 0), Some(2.0));
    }

    #[tokio::test]
    async fn a_change_that_reverts_before_it_settles_does_not_reload() {
        let dir = TempDir::new("revert");
        save(&dir.0, 0, 1.0);
        let (mut watcher, target) = start(&dir).await;
        let before = target.load_full();

        let path = dir.0.join(screen_file_name(0));
        let original = std::fs::read(&path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        std::fs::write(&path, b"{").unwrap();
        bump_mtime(&path);
        assert_eq!(watcher.poll_once(&target).await, None);
        std::fs::write(&path, original).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(meta.modified().unwrap())
            .unwrap();
        // Same mtime, length and inode as the applied state again.
        assert_eq!(watcher.poll_once(&target).await, None);
        assert_eq!(watcher.poll_once(&target).await, None);

        assert!(Arc::ptr_eq(&before, &target.load_full()));
    }

    #[tokio::test]
    async fn run_applies_changes_and_stops_on_cancel() {
        let dir = TempDir::new("run");
        let (mut watcher, target) = start(&dir).await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (target, cancel) = (target.clone(), cancel.clone());
            async move {
                watcher
                    .run_every(Duration::from_millis(10), target, cancel)
                    .await;
            }
        });

        save(&dir.0, 5, 6.0);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while scale_of(&target.load(), 5).is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "reload never happened"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(scale_of(&target.load(), 5), Some(6.0));

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("watcher ignored cancellation")
            .unwrap();
    }
}
