//! Measures the printed scale board's distance from its apparent size and
//! compares it with the distance the depth sensor reports for the same board.
//! A prototype probe for validation on real hardware: it tests whether the
//! board's apparent size can expose a depth scale error, and claims nothing
//! about accuracy by itself.
//!
//! Usage: `board_probe [--pitch-mm <millimeters>]`. The pitch defaults to the
//! nominal value in `board.json`. If the print was scaled by the printer,
//! measure the printed scale bar with a ruler and pass the corrected pitch:
//! the board distance is proportional to it.
//!
//! Hold the board still in the camera's view, flat and facing the camera more
//! or less, with the whole dot grid visible. The program opens the first
//! Orbbec camera (the SDK is found as described in
//! `odyssey_autocalib::orbbec::runtime`; `ODYSSEY_ORBBEC_SDK_DIR` points it at
//! a particular install), grabs about 30 frames over a couple of seconds and
//! prints what it measured. The exit code is 0 whenever the program runs to
//! the end, including when the SDK, the camera or the board is missing; the
//! printed text says what happened. A bad command line exits with 2.

use std::time::{Duration, Instant};

use odyssey_autocalib::board::{
    BoardError, BoardScale, BoardSpec, GOOD_DOT_DIAMETER_PX, MIN_DOT_DIAMETER_PX, TargetSize,
    board_scale,
};
use odyssey_autocalib::camera::{CameraBackend, CameraError, CaptureSettings, Frames};
use odyssey_autocalib::orbbec::{OrbbecBackend, runtime_status};

const WANTED_FRAMES: usize = 30;
const CAPTURE_WINDOW: Duration = Duration::from_secs(4);
/// A depth scale error worth measuring is around 1%, so the frame-to-frame
/// scatter of the board distance has to sit well under that: a quarter of a
/// percent of the distance leaves the 1% effect four times the scatter.
const STABLE_SPREAD_FRACTION: f64 = 0.0025;
/// Above this the scatter is too large for even a 2% effect to be told apart.
const UNSTABLE_SPREAD_FRACTION: f64 = 0.005;

fn main() {
    let pitch_mm = match parse_pitch(std::env::args().skip(1)) {
        Ok(pitch) => pitch,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("usage: board_probe [--pitch-mm <millimeters>]");
            std::process::exit(2);
        }
    };
    let status = runtime_status(None);
    println!("runtime: {status}");
    if pitch_mm == BoardSpec::nominal_pitch_mm() {
        println!("pitch: {pitch_mm} mm (nominal, from board.json; not measured)");
    } else {
        println!(
            "pitch: {pitch_mm} mm (given; nominal is {} mm)",
            BoardSpec::nominal_pitch_mm()
        );
    }
    if let Err(e) = probe(pitch_mm) {
        println!("probe stopped: {e}");
    }
}

fn parse_pitch(args: impl Iterator<Item = String>) -> Result<f64, String> {
    let mut pitch = BoardSpec::nominal_pitch_mm();
    let mut args = args;
    while let Some(arg) = args.next() {
        let value = if arg == "--pitch-mm" {
            args.next()
                .ok_or("--pitch-mm needs a value in millimeters")?
        } else if let Some(value) = arg.strip_prefix("--pitch-mm=") {
            value.to_string()
        } else {
            return Err(format!("unknown argument {arg:?}"));
        };
        pitch = value
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && *p > 0.0)
            .ok_or_else(|| format!("{value:?} is not a positive pitch in millimeters"))?;
    }
    Ok(pitch)
}

fn probe(pitch_mm: f64) -> Result<(), CameraError> {
    let backend = OrbbecBackend::new();
    let cameras = match backend.list() {
        Ok(cameras) => cameras,
        Err(e @ CameraError::RuntimeUnavailable(_)) => {
            println!("cameras: not listed ({e})");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if cameras.is_empty() {
        println!("no camera found");
        return Ok(());
    }
    for camera in &cameras {
        println!(
            "camera: {} (serial {}, id {}, link {})",
            camera.name, camera.serial, camera.id, camera.link
        );
        if let Some(warning) = camera.link.warning() {
            println!("WARNING: {warning}");
        }
    }

    let mut stream = backend.open(&cameras[0], &CaptureSettings::default())?;
    let geometry = *stream.geometry();
    println!(
        "color {}x{}, fx {:.1} fy {:.1}",
        geometry.color.width, geometry.color.height, geometry.color.fx, geometry.color.fy
    );

    let start = Instant::now();
    let mut frames: Vec<Frames> = Vec::with_capacity(WANTED_FRAMES);
    while frames.len() < WANTED_FRAMES && start.elapsed() < CAPTURE_WINDOW {
        match stream.next_frames(Duration::from_millis(500)) {
            Ok(f) => frames.push(f),
            Err(CameraError::Timeout) => println!("timeout waiting for frames"),
            Err(e) => return Err(e),
        }
        stream.drain_imu();
    }
    println!(
        "{} frames in {:.1} s",
        frames.len(),
        start.elapsed().as_secs_f64()
    );
    if frames.is_empty() {
        println!("no frames, nothing to measure");
        return Ok(());
    }

    match board_scale(&geometry, &frames, pitch_mm) {
        Ok(scale) => report(&scale),
        Err(e) => explain(&e),
    }
    Ok(())
}

fn explain(error: &BoardError) {
    println!("board: not measured: {error}");
    let hint = match error {
        BoardError::NoBoard { .. } => {
            "make sure the whole board is in the color image, lit evenly and not too far away"
        }
        BoardError::TooFewDots { .. } => {
            "more of the dot grid has to be visible; check for glare, shadows or something covering it"
        }
        BoardError::AmbiguousPlacement { .. } => {
            "show the corner with the missing dot so the board's orientation is certain"
        }
        BoardError::BadFit(_) => "is the board flat, in focus and not warped or curled?",
        BoardError::NoDepth { .. } => {
            "the depth sensor returned too little on the board; check range, angle and the projector"
        }
        BoardError::DegeneratePose(_) => "face the board more toward the camera",
    };
    println!("hint: {hint}");
}

fn report(scale: &BoardScale) {
    let d = &scale.detection;
    println!(
        "board dots: {} found of {} expected",
        d.dots_matched, d.dots_expected
    );
    println!("dot diameter: {:.1} px (median)", d.median_dot_diameter_px);
    println!(
        "board footprint: {:.0} x {:.0} px (printed board outline, color image)",
        d.footprint_extent_px[0], d.footprint_extent_px[1]
    );
    println!("fit error: {:.2} px RMS", d.rms_px);
    println!(
        "viewing angle: {:.0} deg off the board's normal",
        scale.view_angle_deg
    );
    println!(
        "board distance (from size, pitch {} mm): {:.4} m",
        scale.pitch_mm, scale.board_distance_m
    );
    println!(
        "depth distance (plane of {} points, {} on the plane, {:.1} mm RMS): {:.4} m",
        scale.depth_points,
        scale.depth_inliers,
        scale.depth_plane_rms_m * 1000.0,
        scale.depth_distance_m
    );
    println!(
        "ratio depth / board: {:.4} ({:+.2}% difference)",
        scale.ratio,
        (scale.ratio - 1.0) * 100.0
    );
    println!(
        "depth and board planes differ by {:.1} deg in orientation",
        scale.plane_disagreement_deg
    );
    let spread = scale.distance_spread_m();
    let spread_fraction = spread / scale.board_distance_m;
    println!(
        "frames used: {} of {}; board distance spread {:.1} mm ({:.2}%)",
        scale.frames_used,
        scale.frames_total,
        spread * 1000.0,
        spread_fraction * 100.0
    );

    let size = match d.target_size {
        TargetSize::Adequate => format!(
            "target size ok ({:.0} px dots, at least {GOOD_DOT_DIAMETER_PX} px wanted)",
            d.median_dot_diameter_px
        ),
        TargetSize::Marginal => format!(
            "target size marginal ({:.0} px dots; {GOOD_DOT_DIAMETER_PX} px or more is better, move the board closer or print it larger)",
            d.median_dot_diameter_px
        ),
        TargetSize::TooSmall => format!(
            "target too small ({:.0} px dots; under {MIN_DOT_DIAMETER_PX} px the result cannot be trusted, move the board closer or print it larger)",
            d.median_dot_diameter_px
        ),
    };
    let coverage = if scale.frames_used * 5 < scale.frames_total * 4 {
        format!(
            "detection patchy (board found in {} of {} frames)",
            scale.frames_used, scale.frames_total
        )
    } else if spread_fraction <= STABLE_SPREAD_FRACTION {
        "detection stable".to_string()
    } else if spread_fraction <= UNSTABLE_SPREAD_FRACTION {
        "detection fairly stable (scatter limits what a small scale error can show)".to_string()
    } else {
        "detection unstable (hold the board still and check the lighting)".to_string()
    };
    println!("verdict: {size}; {coverage}");
    println!(
        "note: the board distance assumes the printed pitch is exact and the color intrinsics are right; this probe only compares the two numbers"
    );
}
