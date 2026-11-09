//! Camera frames as GPUI images, built on the camera thread so the UI thread
//! only hands a ready texture to the renderer.

use std::sync::Arc;

use gpui::RenderImage;
use image::imageops::FilterType;
use image::{Frame, GrayImage, RgbImage, RgbaImage};

/// Widest picture sent to the UI while previewing; the preview is painted at
/// roughly this size, and smaller frames upload faster at ~15 frames a second.
pub(super) const PREVIEW_MAX_WIDTH: u32 = 800;

/// Downscales `image` to at most `max_width` and converts it for GPUI.
/// Returns the image with the *original* dimensions, the coordinate system of
/// the overlays.
pub(super) fn encode_rgb(image: &RgbImage, max_width: u32) -> (Arc<RenderImage>, u32, u32) {
    let (w, h) = image.dimensions();
    let resized;
    let shown = if w > max_width && max_width > 0 {
        let scaled_h = (u64::from(h) * u64::from(max_width) / u64::from(w)).max(1) as u32;
        resized = image::imageops::resize(image, max_width, scaled_h, FilterType::Triangle);
        &resized
    } else {
        image
    };
    (render_image(shown), w, h)
}

/// Encodes an infrared frame as gray pixels.
pub(super) fn encode_gray(image: &GrayImage) -> (Arc<RenderImage>, u32, u32) {
    let (w, h) = image.dimensions();
    let mut bgra = Vec::with_capacity(image.as_raw().len() * 4);
    for &luma in image.as_raw() {
        bgra.extend_from_slice(&[luma, luma, luma, 255]);
    }
    (bgra_image(w, h, bgra), w, h)
}

/// GPUI textures are BGRA.
fn render_image(image: &RgbImage) -> Arc<RenderImage> {
    let (w, h) = image.dimensions();
    let mut bgra = Vec::with_capacity(image.as_raw().len() / 3 * 4);
    for pixel in image.as_raw().as_chunks::<3>().0 {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    bgra_image(w, h, bgra)
}

fn bgra_image(w: u32, h: u32, bgra: Vec<u8>) -> Arc<RenderImage> {
    let buffer =
        RgbaImage::from_raw(w, h, bgra).unwrap_or_else(|| RgbaImage::new(w.max(1), h.max(1)));
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}
