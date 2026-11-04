use odyssey_protocol::Port;
use odyssey_protocol::image::{ImageFrame, ObjectFeature};

use crate::{ImageBounds, ImageObjectFeature};

fn camera(value: u32) -> Result<Port, &'static str> {
    match value {
        0 => Ok(Port::Nf),
        1 => Ok(Port::Wf),
        _ => Err("invalid image camera"),
    }
}

impl From<ImageFrame> for crate::ImageFrame {
    fn from(frame: ImageFrame) -> Self {
        Self {
            frame_id: frame.frame_id,
            timestamp_us: frame.timestamp_us,
            camera: match frame.camera {
                Port::Nf => 0,
                Port::Wf => 1,
            },
            width: frame.width.into(),
            height: frame.height.into(),
            pixels: frame.pixels,
            objects: frame.objects.into_iter().map(Into::into).collect(),
            objects_timestamp_us: frame.objects_timestamp_us,
        }
    }
}

impl From<ObjectFeature> for ImageObjectFeature {
    fn from(feature: ObjectFeature) -> Self {
        Self {
            id: feature.id.into(),
            x: feature.x,
            y: feature.y,
            area: feature.area,
            mean_brightness: feature.mean_brightness.into(),
            max_brightness: feature.max_brightness.map(Into::into),
            bounds: feature
                .bounds
                .map(|[left, top, right, bottom]| ImageBounds {
                    left: left.into(),
                    top: top.into(),
                    right: right.into(),
                    bottom: bottom.into(),
                }),
            radius: feature.radius,
            aspect_ratio: feature.aspect_ratio,
            is_circle: feature.is_circle,
        }
    }
}

impl TryFrom<crate::ImageFrame> for ImageFrame {
    type Error = &'static str;

    fn try_from(frame: crate::ImageFrame) -> Result<Self, Self::Error> {
        let width = u16::try_from(frame.width).map_err(|_range| "invalid image width")?;
        let height = u16::try_from(frame.height).map_err(|_range| "invalid image height")?;
        if !matches!((width, height), (320, 240) | (98, 98)) {
            return Err("unsupported image dimensions");
        }
        if frame.pixels.len() != usize::from(width) * usize::from(height) {
            return Err("image pixel length does not match dimensions");
        }
        if frame.objects.len() > odyssey_protocol::image::MAX_IMAGE_OBJECTS {
            return Err("too many image objects");
        }
        let objects: Vec<ObjectFeature> = frame
            .objects
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
        let mut seen = [false; 256];
        for object in &objects {
            if seen[usize::from(object.id)] {
                return Err("duplicate image object id");
            }
            seen[usize::from(object.id)] = true;
            if object.x < 0.0
                || object.y < 0.0
                || object.x >= f32::from(width)
                || object.y >= f32::from(height)
                || object.bounds.is_some_and(|[left, top, right, bottom]| {
                    left > right || top > bottom || right >= width || bottom >= height
                })
            {
                return Err("image object lies outside image");
            }
        }
        Ok(Self {
            frame_id: frame.frame_id,
            timestamp_us: frame.timestamp_us,
            camera: camera(frame.camera)?,
            width,
            height,
            pixels: frame.pixels,
            objects,
            objects_timestamp_us: frame.objects_timestamp_us,
        })
    }
}

impl TryFrom<ImageObjectFeature> for ObjectFeature {
    type Error = &'static str;

    fn try_from(feature: ImageObjectFeature) -> Result<Self, Self::Error> {
        if !feature.x.is_finite()
            || !feature.y.is_finite()
            || feature.radius.is_some_and(|v| !v.is_finite() || v < 0.0)
            || feature
                .aspect_ratio
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err("invalid image object geometry");
        }
        let bounds = feature
            .bounds
            .map(|bounds| -> Result<[u16; 4], &'static str> {
                Ok([
                    u16::try_from(bounds.left).map_err(|_range| "invalid image bounds")?,
                    u16::try_from(bounds.top).map_err(|_range| "invalid image bounds")?,
                    u16::try_from(bounds.right).map_err(|_range| "invalid image bounds")?,
                    u16::try_from(bounds.bottom).map_err(|_range| "invalid image bounds")?,
                ])
            })
            .transpose()?;
        Ok(Self {
            id: u8::try_from(feature.id).map_err(|_range| "invalid image object id")?,
            x: feature.x,
            y: feature.y,
            area: feature.area,
            mean_brightness: u8::try_from(feature.mean_brightness)
                .map_err(|_range| "invalid image brightness")?,
            max_brightness: feature
                .max_brightness
                .map(u8::try_from)
                .transpose()
                .map_err(|_range| "invalid image brightness")?,
            bounds,
            radius: feature.radius,
            aspect_ratio: feature.aspect_ratio,
            is_circle: feature.is_circle,
        })
    }
}
