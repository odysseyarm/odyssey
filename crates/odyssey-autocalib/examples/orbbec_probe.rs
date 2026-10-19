//! Reports whether the Orbbec SDK can be loaded, lists Orbbec cameras and, if
//! one is attached, streams for about three seconds while printing its geometry
//! and frame statistics.
//!
//! The SDK is found as described in `odyssey_autocalib::orbbec::runtime`;
//! `ODYSSEY_ORBBEC_SDK_DIR` points the probe at a particular install. The exit
//! code is 0 whenever the program runs to the end, including when the SDK or
//! the camera is missing; the printed text says what happened.

use std::time::{Duration, Instant};

use odyssey_autocalib::camera::{CameraBackend, CameraError, CaptureSettings, Intrinsics};
use odyssey_autocalib::orbbec::{OrbbecBackend, runtime_status};

fn describe(name: &str, i: &Intrinsics) {
    println!(
        "  {name}: {}x{} f=({:.2}, {:.2}) c=({:.2}, {:.2}) k={:?} p={:?}",
        i.width, i.height, i.fx, i.fy, i.cx, i.cy, i.k, i.p
    );
}

fn main() {
    let status = runtime_status(None);
    println!("runtime: {status}");
    println!("runtime: {status:?}");
    if let Err(e) = probe() {
        println!("probe stopped: {e}");
    }
}

fn probe() -> Result<(), CameraError> {
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
            "camera: {} (serial {}, id {})",
            camera.name, camera.serial, camera.id
        );
    }

    println!("offered profiles (* = what streaming uses):");
    for p in backend.offered_profiles(&cameras[0])? {
        println!(
            "  {} {:<8} {:>4}x{:<4} @ {:>2} fps  {}",
            if p.chosen { '*' } else { ' ' },
            p.stream,
            p.width,
            p.height,
            p.fps,
            p.format
        );
    }

    let mut stream = backend.open(&cameras[0], &CaptureSettings::default())?;
    let g = *stream.geometry();
    println!("geometry:");
    describe("color", &g.color);
    describe("depth", &g.depth);
    describe("ir", &g.ir);
    println!(
        "  color->depth: R={:?} t={:?}",
        g.color_to_depth.rotation, g.color_to_depth.translation
    );
    println!(
        "  ir->depth:    R={:?} t={:?}",
        g.ir_to_depth.rotation, g.ir_to_depth.translation
    );

    let start = Instant::now();
    let (mut frames, mut imu) = (0u32, 0usize);
    while start.elapsed() < Duration::from_secs(3) {
        let f = match stream.next_frames(Duration::from_millis(500)) {
            Ok(f) => f,
            Err(CameraError::Timeout) => {
                println!("timeout waiting for frames");
                continue;
            }
            Err(e) => return Err(e),
        };
        frames += 1;
        imu += stream.drain_imu().len();
        if frames == 1 || frames % 30 == 0 {
            let valid = f.depth.pixels().filter(|p| p.0[0] != 0).count();
            let (cx, cy) = (f.depth.width() / 2, f.depth.height() / 2);
            let ir_mean = f.ir.pixels().map(|p| u64::from(p.0[0])).sum::<u64>() as f64
                / f.ir.pixels().len() as f64;
            println!(
                "frame {frames}: color {}x{} depth {}x{} ({:.0}% valid, centre {:?} m, unit {} m) ir {}x{} mean {ir_mean:.1}",
                f.color.width(),
                f.color.height(),
                f.depth.width(),
                f.depth.height(),
                100.0 * valid as f64 / f.depth.pixels().len() as f64,
                f.depth_m(cx, cy),
                f.depth_unit_m,
                f.ir.width(),
                f.ir.height(),
            );
        }
    }
    println!(
        "{frames} frames and {imu} IMU samples in {:.1} s",
        start.elapsed().as_secs_f64()
    );
    println!(
        "projector switchable: {}",
        stream.set_projector_enabled(true)?
    );
    Ok(())
}
