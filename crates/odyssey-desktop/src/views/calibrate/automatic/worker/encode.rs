//! JPEG data URLs for the webview.

use base64::Engine as _;
use dioxus::logger::tracing;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, GrayImage, RgbImage};

/// Widest picture sent to the UI while previewing. A webview paints this at
/// roughly its natural size, and the JPEG stays small enough for ~15 frames a
/// second as a data URL.
pub(super) const PREVIEW_MAX_WIDTH: u32 = 800;
pub(super) const PREVIEW_JPEG_QUALITY: u8 = 72;
pub(super) const REVIEW_JPEG_QUALITY: u8 = 85;

/// Downscales `image` to at most `max_width` and encodes it as a JPEG data
/// URL. Returns the URL with the *original* dimensions, the coordinate system
/// of the overlays.
pub(super) fn encode_rgb(image: &RgbImage, max_width: u32, quality: u8) -> (String, u32, u32) {
    let (w, h) = image.dimensions();
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
    let result = if w > max_width && max_width > 0 {
        let scaled_h = (u64::from(h) * u64::from(max_width) / u64::from(w)).max(1) as u32;
        let small = image::imageops::resize(image, max_width, scaled_h, FilterType::Triangle);
        encoder.encode(
            small.as_raw(),
            small.width(),
            small.height(),
            ExtendedColorType::Rgb8,
        )
    } else {
        encoder.encode(image.as_raw(), w, h, ExtendedColorType::Rgb8)
    };
    (data_url(result, bytes), w, h)
}

/// Encodes an infrared frame. It goes out as a three-channel JPEG: the embedded
/// web view did not paint single-channel JPEGs.
pub(super) fn encode_gray(image: &GrayImage, quality: u8) -> (String, u32, u32) {
    let rgb = image::DynamicImage::ImageLuma8(image.clone()).into_rgb8();
    encode_rgb(&rgb, u32::MAX, quality)
}

fn data_url(result: image::ImageResult<()>, bytes: Vec<u8>) -> String {
    if let Err(e) = result {
        tracing::warn!("could not encode a preview frame: {e}");
        return String::new();
    }
    let mut url = String::from("data:image/jpeg;base64,");
    base64::engine::general_purpose::STANDARD.encode_string(bytes, &mut url);
    url
}
