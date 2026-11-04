//! The words of a diagnostic bundle.

use odyssey_autocalib::session::Calibration;

use super::backend::OpenRequest;
use super::engine::Attempt;

/// The settings an attempt was opened with, as words for the bundle.
pub(super) fn describe_request(request: &OpenRequest) -> String {
    use std::fmt::Write as _;
    let camera = &request.camera;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "camera: {} (serial {}, id {}, link {})",
        camera.name, camera.serial, camera.id, camera.link
    );
    let _ = writeln!(s, "capture settings: {:?}", request.settings);
    let _ = writeln!(s, "screen aspect: {:.4}", request.aspect);
    let _ = writeln!(s, "options: {:?}", request.options);
    let _ = writeln!(s, "declared size from: {:?}", request.size_origin);
    s
}

/// What a finished calibration measured, as words for the bundle.
pub(super) fn result_text(c: &Calibration) -> String {
    use std::fmt::Write as _;
    let r = &c.review;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "screen {:.3} x {:.3} m ({:.1} in diagonal), size from {:?}",
        r.size_m[0], r.size_m[1], r.diagonal_in, r.size_source
    );
    let _ = writeln!(
        s,
        "plane rms {:.2} mm, max {:.2} mm, {} inliers",
        r.plane_rmse_m * 1000.0,
        r.plane_max_error_m * 1000.0,
        r.plane_inliers
    );
    let _ = writeln!(s, "dots used {} of {}", r.dots_used, r.dots_expected);
    let _ = writeln!(s, "edges: {:?}", r.edges);
    let _ = writeln!(s, "size check: {:?}", r.size_check);
    let _ = writeln!(s, "gravity aligned: {}", r.gravity_aligned);
    for (i, p) in c.calibration.object_points.iter().enumerate() {
        let _ = writeln!(
            s,
            "marker {} (object_points[{i}]): ({:.4}, {:.4}) m",
            i + 1,
            p.x,
            p.y
        );
    }
    if let Some(circle) = r.seed_circle {
        let _ = writeln!(
            s,
            "circle through markers 1, 2 and 3: centre ({:.3}, {:.3}) radius {:.3} m",
            circle.center.x, circle.center.y, circle.radius
        );
    }
    s
}

/// The text at the top of a bundle: what ran, with what, and how it ended.
pub(super) fn attempt_summary(attempt: &Attempt) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Odyssey Desktop {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(s, "\n== request ==\n{}", attempt.request);
    let _ = writeln!(s, "== outcome ==\n{}", attempt.outcome);
    if let Some(frames) = &attempt.frames {
        let g = &frames.geometry;
        let _ = writeln!(s, "== camera geometry ==");
        let _ = writeln!(s, "color: {:?}", g.color);
        let _ = writeln!(s, "depth: {:?}", g.depth);
        let _ = writeln!(s, "infrared: {:?}", g.ir);
        let _ = writeln!(s, "color to depth: {:?}", g.color_to_depth);
        let _ = writeln!(s, "infrared to depth: {:?}", g.ir_to_depth);
        let _ = writeln!(s, "\n== frames ==");
        let _ = writeln!(s, "screen burst: {} frames", frames.screen_burst.len());
        if let Some(first) = frames.screen_burst.first() {
            let _ = writeln!(
                s,
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
            s,
            "marker burst: {} infrared frames; preview frame: {}",
            frames.marker_burst_ir.len(),
            frames.marker_preview_ir.is_some()
        );
    }
    s
}
