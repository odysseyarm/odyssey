//! The "save details for support" bundle: one zip with what a calibration attempt
//! looked at and what the app logged about it, so someone else can reproduce the
//! trouble without the room.
//!
//! It holds every frame the screen solve used (color, depth and infrared), the
//! infrared frames of the marker measurement, the calibration lines of the app's log
//! and a summary of the settings and the outcome. The images are camera frames of
//! the room, so the bundle is only ever written when the user asks for it, to the
//! place they choose, readable only by them on Unix.

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use image::ImageFormat;
use odyssey_autocalib::session::AttemptFrames;

/// What goes into a bundle.
pub struct Bundle<'a> {
    pub summary: &'a str,
    /// `None` when no camera was open, e.g. when it would not start.
    pub frames: Option<&'a AttemptFrames>,
    /// The app's log, as text; only the calibration lines are kept.
    pub log: &'a str,
}

/// Writes the bundle to `path` and returns its size in bytes.
pub fn write(path: &Path, bundle: &Bundle) -> Result<u64, String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let text = zip::write::SimpleFileOptions::default();
    // The images are PNG already, so storing them costs nothing in size.
    let image = text.compression_method(zip::CompressionMethod::Stored);

    let mut put = |name: &str, options, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())
    };
    put("summary.txt", text, bundle.summary.as_bytes())?;
    put(
        "calibration.log",
        text,
        calibration_log(bundle.log).as_bytes(),
    )?;

    let Some(frames) = bundle.frames else {
        let file = zip.finish().map_err(|e| e.to_string())?;
        return file.metadata().map(|m| m.len()).map_err(|e| e.to_string());
    };
    for (i, frame) in frames.screen_burst.iter().enumerate() {
        let n = i + 1;
        put(
            &format!("screen/{n:02}-color.png"),
            image,
            &png(&frame.color)?,
        )?;
        // 16-bit raw depth; the unit is in summary.txt.
        put(
            &format!("screen/{n:02}-depth.png"),
            image,
            &png(&frame.depth)?,
        )?;
        put(
            &format!("screen/{n:02}-infrared.png"),
            image,
            &png(&frame.ir)?,
        )?;
    }
    for (i, ir) in frames.marker_burst_ir.iter().enumerate() {
        put(
            &format!("markers/{:02}-infrared.png", i + 1),
            image,
            &png(ir)?,
        )?;
    }
    if let Some(ir) = &frames.marker_preview_ir {
        put("markers/preview-infrared.png", image, &png(ir)?)?;
    }
    let file = zip.finish().map_err(|e| e.to_string())?;
    file.metadata().map(|m| m.len()).map_err(|e| e.to_string())
}

fn png<P>(image: &image::ImageBuffer<P, Vec<P::Subpixel>>) -> Result<Vec<u8>, String>
where
    P: image::PixelWithColorType,
    [P::Subpixel]: image::EncodableLayout,
{
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Whether a log line's source is part of calibrating.
fn is_calibration(line: &str) -> bool {
    line.contains("odyssey_autocalib") || line.contains("views::calibrate")
}

/// The calibration events of an app log. An event can run over several lines (the
/// marker report does), and only its first line carries the timestamp and source, so
/// the lines after it follow it.
pub fn calibration_log(log: &str) -> String {
    let starts_event = |line: &str| {
        let b = line.as_bytes();
        b.len() > 10 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-'
    };
    let mut kept = String::new();
    let mut keep = false;
    for line in log.lines() {
        if starts_event(line) {
            keep = is_calibration(line);
        }
        if keep {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Read;

    use image::{GrayImage, Luma, RgbImage};
    use odyssey_autocalib::camera::{CameraGeometry, DepthImage, Extrinsics, Frames, Intrinsics};

    use super::*;

    fn intrinsics() -> Intrinsics {
        Intrinsics {
            width: 8,
            height: 6,
            fx: 10.0,
            fy: 10.0,
            cx: 4.0,
            cy: 3.0,
            k: [0.0; 6],
            p: [0.0; 2],
        }
    }

    fn geometry() -> CameraGeometry {
        let identity = Extrinsics::identity();
        CameraGeometry {
            color: intrinsics(),
            depth: intrinsics(),
            ir: intrinsics(),
            color_to_depth: identity,
            ir_to_depth: identity,
        }
    }

    fn frames(n: usize) -> Vec<Frames> {
        (0..n)
            .map(|i| Frames {
                color: RgbImage::from_pixel(8, 6, image::Rgb([i as u8, 2, 3])),
                depth: DepthImage::from_pixel(8, 6, Luma([1000 + i as u16])),
                depth_unit_m: 0.001,
                ir: GrayImage::from_pixel(8, 6, Luma([i as u8])),
            })
            .collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("odyssey-bundle-{name}-{}.zip", std::process::id()))
    }

    #[test]
    fn the_bundle_holds_every_frame_and_the_words() {
        let attempt = AttemptFrames {
            geometry: geometry(),
            screen_burst: frames(3),
            marker_burst_ir: vec![GrayImage::new(8, 6); 2],
            marker_preview_ir: Some(GrayImage::new(8, 6)),
        };
        let path = scratch("all");
        let size = write(
            &path,
            &Bundle {
                summary: "outcome: failed",
                frames: Some(&attempt),
                log: "2026-10-19T06:04:59Z  INFO odyssey_autocalib::session: marker mismatch\nmore\n",
            },
        )
        .unwrap();
        assert_eq!(size, fs::metadata(&path).unwrap().len());

        let mut zip = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut names: Vec<_> = zip.file_names().map(str::to_owned).collect();
        names.sort();
        assert_eq!(names.len(), 2 + 3 * 3 + 2 + 1, "{names:?}");
        for expected in [
            "summary.txt",
            "calibration.log",
            "screen/01-color.png",
            "screen/03-depth.png",
            "screen/02-infrared.png",
            "markers/02-infrared.png",
            "markers/preview-infrared.png",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} in {names:?}"
            );
        }
        let mut summary = String::new();
        zip.by_name("summary.txt")
            .unwrap()
            .read_to_string(&mut summary)
            .unwrap();
        assert_eq!(summary, "outcome: failed");

        // The depth survives as 16-bit values, not squeezed to 8.
        let mut depth = Vec::new();
        zip.by_name("screen/02-depth.png")
            .unwrap()
            .read_to_end(&mut depth)
            .unwrap();
        let decoded = image::load_from_memory_with_format(&depth, ImageFormat::Png)
            .unwrap()
            .into_luma16();
        assert_eq!(decoded.get_pixel(0, 0).0[0], 1001);
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_the_user_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let attempt = AttemptFrames {
            geometry: geometry(),
            screen_burst: Vec::new(),
            marker_burst_ir: Vec::new(),
            marker_preview_ir: None,
        };
        let path = scratch("private");
        write(
            &path,
            &Bundle {
                summary: "",
                frames: Some(&attempt),
                log: "",
            },
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn only_the_calibration_part_of_the_log_is_kept() {
        let log = "\
2026-10-19T06:04:30.7Z  INFO odyssey_desktop::hub: BatteryEvent received for [0f, 69]: 73%
2026-10-19T06:04:42.3Z  INFO odyssey_desktop::views::calibrate::automatic::worker: camera opened
2026-10-19T06:04:59.3Z  INFO odyssey_autocalib::session: marker layout mismatch:
infrared camera 1280x800 fx 628.43
  1: (0.18, 0.29) -> (0.106, 0.097)
2026-10-19T06:05:00.1Z  INFO odyssey_desktop::hub: BatteryEvent received for [0f, 69]: 73%
";
        let kept = calibration_log(log);
        assert!(kept.contains("camera opened"));
        assert!(kept.contains("marker layout mismatch"));
        // The lines that belong to the multi-line report come along.
        assert!(kept.contains("infrared camera 1280x800"));
        assert!(kept.contains("0.18, 0.29"));
        assert!(!kept.contains("BatteryEvent"), "{kept}");
    }
}
