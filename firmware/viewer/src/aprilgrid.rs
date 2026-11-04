use aprilgrid::TagFamily;
use aprilgrid::detector::{DetectorParams, TagDetector};
use image::{DynamicImage, GrayImage};
use opencv::core::{
    _InputArrayTraitConst, Mat, MatTraitConstManual, Point2f, Point3f, Size, ToInputArray, Vector,
};
use opencv::hub_prelude::MatTraitConst;
use opencv::imgproc::{INTER_CUBIC, resize};

use crate::ViewerError;

#[derive(Clone)]
pub struct AprilGridConfig {
    pub rows: u16,
    pub cols: u16,
    pub tag_size: f64,
    pub tag_spacing: f64,
}

impl AprilGridConfig {
    pub fn new(rows: u16, cols: u16, tag_size: f64, tag_spacing: f64) -> Self {
        Self {
            rows,
            cols,
            tag_size,
            tag_spacing,
        }
    }
}

pub struct AprilGrid {
    detector: TagDetector,
    config: AprilGridConfig,
}

impl AprilGrid {
    pub fn new(config: AprilGridConfig) -> Self {
        let detector_params = DetectorParams {
            tag_spacing_ratio: (config.tag_spacing / config.tag_size) as f32,
            min_saddle_angle: 30.0,
            max_saddle_angle: 60.0,
            max_num_of_boards: 1,
        };
        Self {
            detector: TagDetector::new(&TagFamily::T36H11B1, Some(detector_params)),
            config,
        }
    }

    pub fn get_corner_and_object_points(
        &self,
        image: &[u8],
        resolution: (u16, u16),
        object_resolution: (u32, u32),
    ) -> Result<Option<(Vector<Point2f>, Vector<Point3f>)>, ViewerError> {
        let im = Mat::new_rows_cols_with_data(resolution.1.into(), resolution.0.into(), image)?;
        self.get_corner_and_object_points_cv(&im, object_resolution)
    }

    fn get_corner_and_object_points_cv(
        &self,
        image: &impl ToInputArray,
        object_resolution: (u32, u32),
    ) -> Result<Option<(Vector<Point2f>, Vector<Point3f>)>, ViewerError> {
        let mut im_upscaled = Mat::default();
        let ss;
        if image.input_array()?.size_def()? == (98, 98).into() {
            ss = 4.;
            resize(
                image,
                &mut im_upscaled,
                Size::new(0, 0),
                ss,
                ss,
                INTER_CUBIC,
            )?;
        } else {
            ss = 2.;
            resize(
                image,
                &mut im_upscaled,
                Size::new(0, 0),
                ss,
                ss,
                INTER_CUBIC,
            )?;
        }

        let (width, height) = (im_upscaled.cols(), im_upscaled.rows());
        let buffer = im_upscaled.data_typed::<u8>()?.to_vec();
        let gray_img = GrayImage::from_raw(width as u32, height as u32, buffer)
            .ok_or(ViewerError::InvalidImageBuffer)?;
        let dynamic_img = DynamicImage::ImageLuma8(gray_img);

        let detections = self.detector.detect(&dynamic_img);
        let mut image_points = Vector::<Point2f>::new();
        let mut object_points = Vector::<Point3f>::new();

        for (tag_id, corners) in detections {
            let Some(obj_pts) = self.generate_3d_points(tag_id) else {
                continue;
            };

            let scaled_corners = corners.iter().map(|(x, y)| {
                Point2f::new((x + 0.5) / ss as f32 - 0.5, (y + 0.5) / ss as f32 - 0.5)
            });

            image_points.extend(scaled_corners);
            object_points.extend(obj_pts.into_iter());
        }

        if image_points.is_empty() {
            return Ok(None);
        }

        let mut image_points_object_points =
            std::iter::zip(image_points, object_points).collect::<Vec<_>>();
        image_points_object_points.sort_by(|a, b| {
            a.1.x
                .total_cmp(&b.1.x)
                .then(a.1.y.total_cmp(&b.1.y))
                .then(a.1.z.total_cmp(&b.1.z))
        });
        let (mut image_points, object_points): (Vec<_>, Vec<_>) =
            image_points_object_points.into_iter().unzip();

        let size = image.input_array()?.size(-1)?;
        for p in image_points.as_mut_slice() {
            p.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
            p.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
        }

        Ok(Some((image_points.into(), object_points.into())))
    }

    fn generate_3d_points(&self, tag_id: u32) -> Option<Vec<Point3f>> {
        if tag_id >= u32::from(self.config.rows) * u32::from(self.config.cols) {
            return None;
        }

        let row = (tag_id / self.config.cols as u32) as f32;
        let col = (tag_id % self.config.cols as u32) as f32;
        let size = self.config.tag_size as f32;
        let spacing = self.config.tag_spacing as f32;

        Some(vec![
            Point3f::new(col * (size + spacing), row * (size + spacing), 0.0),
            Point3f::new(col * (size + spacing) + size, row * (size + spacing), 0.0),
            Point3f::new(
                col * (size + spacing) + size,
                row * (size + spacing) + size,
                0.0,
            ),
            Point3f::new(col * (size + spacing), row * (size + spacing) + size, 0.0),
        ])
    }
}
