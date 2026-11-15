//! The cameras a worker can offer and how a session opens on one.

use odyssey_autocalib::camera::{CameraBackend as _, CameraError, CameraInfo, CaptureSettings};
use odyssey_autocalib::orbbec::{self, OrbbecBackend, RuntimeStatus};
use odyssey_autocalib::session::{Session, SessionOptions};
use odyssey_autocalib::synthetic::{SyntheticBackend, SyntheticScene};

use crate::sdk_install;
use crate::views::calibrate::automatic::kind::SizeOrigin;

/// Diagonal of the simulated screen, whatever size the tech declares: the
/// demo needs a truth that a wrong declaration can disagree with.
pub(crate) const DEMO_DIAGONAL_IN: f64 = 55.0;

/// Which cameras the worker offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Backend {
    /// The simulated camera, and nothing else. `overscan_fraction` is how much
    /// the simulated TV crops its picture.
    Demo { overscan_fraction: f64 },
    /// Attached Orbbec cameras. Needs the Orbbec SDK installed on this machine.
    /// Without it listing fails with [`ListError::Runtime`].
    Hardware,
}

/// Why a camera listing has no list to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListError {
    /// The camera library is missing, broken or of an unsupported version.
    Runtime(RuntimeStatus),
    /// The library is there but enumerating cameras failed.
    Camera(String),
}

fn list_hardware() -> Result<Vec<CameraInfo>, ListError> {
    // The installer's copy of the SDK, or one the user downloaded, is found here.
    // A copy elsewhere on the machine is still found by the loader on its own.
    let extra_dir = sdk_install::search_dir();
    match orbbec::runtime_status(extra_dir.as_deref()) {
        RuntimeStatus::Available { .. } => OrbbecBackend::with_extra_dir(extra_dir)
            .list()
            .map_err(|e| ListError::Camera(e.to_string())),
        unavailable => Err(ListError::Runtime(unavailable)),
    }
}

impl Backend {
    pub(super) fn list(self) -> Result<Vec<CameraInfo>, ListError> {
        match self {
            Self::Demo { .. } => SyntheticBackend::default()
                .list()
                .map_err(|e| ListError::Camera(e.to_string())),
            Self::Hardware => list_hardware(),
        }
    }

    pub(super) fn open(self, request: &OpenRequest) -> Result<Session, CameraError> {
        let stream = match self {
            Self::Demo { overscan_fraction } => {
                let scene = demo_scene(
                    request.aspect,
                    DEMO_DIAGONAL_IN,
                    request.pattern_visible,
                    overscan_fraction,
                );
                SyntheticBackend::new(scene).open(&request.camera, &request.settings)?
            }
            Self::Hardware => OrbbecBackend::with_extra_dir(sdk_install::search_dir())
                .open(&request.camera, &request.settings)?,
        };
        Ok(Session::start(
            stream,
            request.aspect,
            request.options.clone(),
        ))
    }
}

/// The simulated scene for a screen of the given shape. The synthetic camera
/// renders the same dot grid the wizard draws because both derive it from this
/// aspect ratio, and it sits farther from a larger screen so the screen fills
/// a similar part of the picture.
pub(crate) fn demo_scene(
    aspect: f64,
    diagonal_in: f64,
    pattern_visible: bool,
    overscan_fraction: f64,
) -> SyntheticScene {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let diagonal_m = diagonal_in * 0.0254;
    let height = diagonal_m / (aspect * aspect + 1.0).sqrt();
    let mut scene = SyntheticScene {
        screen_size_m: [aspect * height, height],
        show_pattern: pattern_visible,
        overscan_fraction,
        // Deliver frames at the camera's rate instead of as fast as they render.
        realtime: true,
        ..SyntheticScene::default()
    };
    scene.screen_center_m.z = (2.0 * diagonal_in / DEMO_DIAGONAL_IN).clamp(0.9, 3.5);
    scene
}

/// What the UI asks the camera thread to open.
#[derive(Debug, Clone)]
pub(crate) struct OpenRequest {
    pub camera: CameraInfo,
    pub settings: CaptureSettings,
    /// Width over height of the screen showing the pattern.
    pub aspect: f64,
    pub options: SessionOptions,
    /// Whether the screen shows the pattern right now. Only the simulated
    /// camera cares: it draws what a real camera would see.
    pub pattern_visible: bool,
    /// Where the declared size in `options` came from, for the words of a
    /// failure and the review.
    pub size_origin: Option<SizeOrigin>,
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::pattern::Pattern;

    use super::*;

    #[test]
    fn demo_scene_follows_the_monitor_shape() {
        for aspect in [16.0 / 9.0, 21.0 / 9.0, 4.0 / 3.0, 9.0 / 16.0] {
            let scene = demo_scene(aspect, 40.0, true, 0.0);
            assert!((scene.aspect() - aspect).abs() < 1e-9);
            let [w, h] = scene.screen_size_m;
            assert!((w.hypot(h) / 0.0254 - 40.0).abs() < 1e-6);
            // What the camera is shown is what the wizard draws.
            assert_eq!(scene.pattern(), Pattern::new(aspect));
        }
    }

    #[test]
    fn demo_scene_survives_a_bad_aspect() {
        assert!((demo_scene(f64::NAN, 55.0, true, 0.0).aspect() - 16.0 / 9.0).abs() < 1e-9);
        assert!((demo_scene(0.0, 55.0, true, 0.0).aspect() - 16.0 / 9.0).abs() < 1e-9);
    }
}
