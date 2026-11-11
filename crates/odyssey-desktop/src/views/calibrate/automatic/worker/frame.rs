//! The pictures and results the camera thread hands to the UI.

use std::sync::Arc;
use std::time::Duration;

use gpui::RenderImage;
use odyssey_autocalib::session::{
    Calibration, EdgeCheck, Hint, MarkerHint, MarkerPreview, Preview, SideSet, SizeCheck,
    SizeSource,
};
use odyssey_common::ScreenCalibration;

use super::encode::{PREVIEW_MAX_WIDTH, encode_gray, encode_rgb};
use crate::views::calibrate::automatic::kind::SizeOrigin;
use crate::views::calibrate::automatic::quality::Measures;

/// One live color frame and what the detector found in it. Coordinates are
/// pixels of the original image (`width` by `height`), which is also the
/// overlay's coordinate system.
#[derive(Debug, Clone)]
pub(crate) struct PreviewFrame {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub dots: Vec<[f32; 2]>,
    pub quad: Option<[[f32; 2]; 4]>,
    pub dots_matched: usize,
    pub dots_expected: usize,
    pub depth_coverage: Option<f32>,
    pub distance_m: Option<f32>,
    pub tilt_deg: Option<(f64, f64)>,
    /// The sides of the picture the display crops. Set only when the wizard
    /// requires the whole picture, since a projector's crop does not matter.
    pub cropped: Option<SideSet>,
    pub hint: Hint,
    /// How long the hint has been `Ready` without interruption.
    pub ready_for: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct MarkerFrame {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub blobs: Vec<[f32; 2]>,
    /// Index into `blobs` for each of the six markers, when all were matched.
    pub matched: Option<[usize; 6]>,
    pub hint: MarkerHint,
}

/// One complete result: the calibration and what the review shows about it.
#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    pub calibration: ScreenCalibration<f32>,
    pub screen_quad: [[f32; 2]; 4],
    pub markers: [[f32; 2]; 6],
    pub size_m: [f64; 2],
    pub diagonal_in: f64,
    pub size_source: SizeSource,
    /// The depth measurement against the declared size. Present when a size
    /// was declared.
    pub size_check: Option<SizeCheck>,
    pub measures: Measures,
    pub gravity_aligned: bool,
    /// Whether the display showed the whole picture. `Cropped` is only
    /// possible when the edges were not required, i.e. the camera sized it.
    pub edges: EdgeCheck,
    /// The circle through markers 1, 2 and 3. See `Review::seed_circle`.
    pub seed_circle: Option<odyssey_autocalib::session::Circle>,
}

/// What the review screen needs once the calibration is done.
#[derive(Debug, Clone)]
pub(crate) struct Finished {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub size_origin: Option<SizeOrigin>,
    /// The result with the declared size, or the camera's own when none was
    /// declared.
    pub primary: Outcome,
    /// The result with the declared size set aside, solved from the same
    /// frames. Present only when a size was declared.
    pub measured: Option<Outcome>,
}

impl Finished {
    pub(crate) fn outcome(&self, use_measured: bool) -> &Outcome {
        match (&self.measured, use_measured) {
            (Some(measured), true) => measured,
            _ => &self.primary,
        }
    }
}

pub(super) fn outcome(done: Calibration) -> Outcome {
    let r = &done.review;
    Outcome {
        screen_quad: r.screen_quad_px.map(|p| [p.x, p.y]),
        markers: r.marker_px.map(|p| [p.x, p.y]),
        size_m: r.size_m,
        diagonal_in: r.diagonal_in,
        size_source: r.size_source,
        size_check: r.size_check,
        measures: Measures::from(r),
        gravity_aligned: r.gravity_aligned,
        edges: r.edges,
        seed_circle: r.seed_circle,
        calibration: done.calibration,
    }
}

fn xy(p: nalgebra::Point2<f32>) -> [f32; 2] {
    [p.x, p.y]
}

pub(super) fn preview_frame(
    preview: &Preview,
    ready_for: Duration,
    require_edges: bool,
) -> PreviewFrame {
    let (image, width, height) = encode_rgb(&preview.color, PREVIEW_MAX_WIDTH);
    PreviewFrame {
        image,
        width,
        height,
        dots: preview.dots.iter().copied().map(xy).collect(),
        quad: preview.screen_quad.map(|q| q.map(xy)),
        dots_matched: preview.dots_matched,
        dots_expected: preview.dots_expected,
        depth_coverage: preview.depth_coverage,
        distance_m: preview.distance_m,
        tilt_deg: preview.tilt.map(|t| (t.pitch_deg, t.roll_deg)),
        cropped: match preview.edges {
            EdgeCheck::Cropped { sides } if require_edges => Some(sides),
            _ => None,
        },
        hint: preview.hint,
        ready_for,
    }
}

pub(super) fn marker_frame(preview: &MarkerPreview) -> MarkerFrame {
    let (image, width, height) = encode_gray(&preview.ir);
    MarkerFrame {
        image,
        width,
        height,
        blobs: preview.blobs.iter().copied().map(xy).collect(),
        matched: preview.matched,
        hint: preview.hint,
    }
}
