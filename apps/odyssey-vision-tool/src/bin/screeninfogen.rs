#![expect(
    clippy::print_stdout,
    reason = "command-line tool: stdout is its interface"
)]

use anyhow::{Context as _, Result};
use app_dirs2::{AppDataType, get_app_root};
use nalgebra::{ComplexField, Point3, RealField, Scalar};
use odyssey_common::MARKER_PATTERN_LEN;
use odyssey_vision_tool::ScreenInfo;
use odyssey_vision_tool::consts::APP_INFO;
use serde::Serialize;

pub const MARKER_DEPTH_METERS: f64 = 0.0117;

/// Marker positions ("serious wall" layout) as fractions of the screen width
/// and height.
const MARKER_RATIOS: [(f64, f64); MARKER_PATTERN_LEN] = [
    (0.18, 0.29),
    (0.15, 0.82),
    (0.77, 0.8),
    (0.51, 0.35),
    (0.79, 0.25),
    (0.49, 0.76),
];

/// Physical size of the "serious wall" screen: 2.032 m high, 16:9.
const SCREEN_DIMENSIONS_METERS: [f32; 2] = [2.032 * (16. / 9.), 2.032];

const METERS_PER_INCH: f32 = 0.0254;

/// Screen is centered at (0, 0, 0)
/// ```text
/// +--x
/// |
/// y    0    3    4
///
///      1    5    2
/// ```
pub fn marker_pattern<F>(screen_dimensions_meters: [F; 2]) -> [Point3<F>; MARKER_PATTERN_LEN]
where
    F: Scalar + std::ops::SubAssign + ComplexField + RealField + Copy,
{
    let depth = F::from_f64(MARKER_DEPTH_METERS).expect("marker depth is representable");
    let [width, height] = screen_dimensions_meters;

    MARKER_RATIOS.map(|(x_ratio, y_ratio)| {
        Point3::from([
            F::from_f64(x_ratio).expect("marker ratio is representable") * width,
            F::from_f64(y_ratio).expect("marker ratio is representable") * height,
            depth,
        ])
    })
}

/// Writes `value` as pretty JSON to `<user config dir>/<directory>/<file_name>`.
fn save_to_config_dir(directory: &str, file_name: &str, value: &impl Serialize) -> Result<()> {
    let config_dir =
        get_app_root(AppDataType::UserConfig, &APP_INFO).context("no user config directory")?;
    let directory = config_dir.join(directory);
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let path = directory.join(file_name);
    let file = std::fs::File::create(&path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .with_context(|| format!("failed to write {}", path.display()))
}

pub fn main() {
    // Other screens that have been used with this tool:
    //   3840x2160 (16:9) SVT:   [3.64631, 2.05105]
    //   1920x1080 abe's wall:   [2.28231, 1.2838]
    let screen_dimensions_meters = SCREEN_DIMENSIONS_METERS;

    let pattern = marker_pattern(screen_dimensions_meters);

    let screen_info = ScreenInfo {
        screen_dimensions_meters,
        marker_points: pattern,
    };

    for (i, p) in pattern.iter().enumerate() {
        println!(
            "Marker {} = ({:.4}, {:.4})",
            i,
            p.x / METERS_PER_INCH,
            p.y / METERS_PER_INCH
        );
    }

    match save_to_config_dir("ats-vision-tool", "new-screen-info.json", &screen_info) {
        Ok(()) => println!("Screen info saved to {screen_info:?}"),
        Err(e) => println!("Failed to save screen info: {e:#}"),
    }

    let screen_calibration: odyssey_common::ScreenCalibration<f32> = screen_info.into();
    match save_to_config_dir(
        "screens",
        "new-screen-calibration.json",
        &screen_calibration,
    ) {
        Ok(()) => println!("Screen calibration saved to {screen_calibration:?}"),
        Err(e) => println!("Failed to save screen calibration: {e:#}"),
    }
}
