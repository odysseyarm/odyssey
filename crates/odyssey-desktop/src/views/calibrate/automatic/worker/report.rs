//! The words of a diagnostic bundle.

use odyssey_autocalib::session::Calibration;

use super::backend::OpenRequest;
use super::engine::Attempt;

/// The settings an attempt was opened with, as words for the bundle.
pub(super) fn describe_request(request: &OpenRequest) -> String {
    use std::fmt::Write as _;
    let camera = &request.camera;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "camera: {} (serial {}, id {}, link {})",
        camera.name, camera.serial, camera.id, camera.link
    );
    let _ = writeln!(text, "capture settings: {:?}", request.settings);
    let _ = writeln!(text, "screen aspect: {:.4}", request.aspect);
    let _ = writeln!(text, "options: {:?}", request.options);
    let _ = writeln!(text, "declared size from: {:?}", request.size_origin);
    text
}

/// What a finished calibration measured, as words for the bundle.
pub(super) fn result_text(calibration: &Calibration) -> String {
    use std::fmt::Write as _;
    let review = &calibration.review;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "screen {:.3} x {:.3} m ({:.1} in diagonal), size from {:?}",
        review.size_m[0], review.size_m[1], review.diagonal_in, review.size_source
    );
    let _ = writeln!(
        text,
        "plane rms {:.2} mm, max {:.2} mm, {} inliers",
        review.plane_rmse_m * 1000.0,
        review.plane_max_error_m * 1000.0,
        review.plane_inliers
    );
    let _ = writeln!(
        text,
        "dots used {} of {}",
        review.dots_used, review.dots_expected
    );
    let _ = writeln!(text, "edges: {:?}", review.edges);
    let _ = writeln!(text, "size check: {:?}", review.size_check);
    let _ = writeln!(text, "gravity aligned: {}", review.gravity_aligned);
    for (i, point) in calibration.calibration.object_points.iter().enumerate() {
        let _ = writeln!(
            text,
            "marker {} (object_points[{i}]): ({:.4}, {:.4}) m",
            i + 1,
            point.x,
            point.y
        );
    }
    if let Some(circle) = review.seed_circle {
        let _ = writeln!(
            text,
            "circle through markers 1, 2 and 3: centre ({:.3}, {:.3}) radius {:.3} m",
            circle.center.x, circle.center.y, circle.radius
        );
    }
    text
}

/// How a successful attempt ended, for the bundle's outcome section. `measured`
/// is the same frames solved with the declared size set aside.
pub(super) fn finished_summary(declared: &Calibration, measured: Option<&Calibration>) -> String {
    let mut summary = format!("finished\n{}", result_text(declared));
    if let Some(measured) = measured {
        summary.push_str(
            "\nthe same frames solved with the declared size set aside (the camera's own size):\n",
        );
        summary.push_str(&result_text(measured));
    }
    summary
}

/// The text at the top of a bundle: what ran, with what, and how it ended.
pub(super) fn attempt_summary(attempt: &Attempt) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Odyssey Desktop {} on {} {}",
        env!("ODYSSEY_BUILD_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(text, "\n== request ==\n{}", attempt.request);
    let _ = writeln!(text, "== outcome ==\n{}", attempt.outcome);
    if let Some(frames) = &attempt.frames {
        let geometry = &frames.geometry;
        let _ = writeln!(text, "== camera geometry ==");
        let _ = writeln!(text, "color: {:?}", geometry.color);
        let _ = writeln!(text, "depth: {:?}", geometry.depth);
        let _ = writeln!(text, "infrared: {:?}", geometry.ir);
        let _ = writeln!(text, "color to depth: {:?}", geometry.color_to_depth);
        let _ = writeln!(text, "infrared to depth: {:?}", geometry.ir_to_depth);
        let _ = writeln!(text, "\n== frames ==");
        let _ = writeln!(text, "screen burst: {} frames", frames.screen_burst.len());
        if let Some(first) = frames.screen_burst.first() {
            let _ = writeln!(
                text,
                "  color {}x{}, depth {}x{} (16-bit, {} m per unit), infrared {}x{}",
                first.color.width(),
                first.color.height(),
                first.depth.width(),
                first.depth.height(),
                first.depth_unit_m,
                first.ir.width(),
                first.ir.height()
            );
        }
        let _ = writeln!(
            text,
            "marker burst: {} infrared frames; preview frame: {}",
            frames.marker_burst_ir.len(),
            frames.marker_preview_ir.is_some()
        );
    }
    text
}
