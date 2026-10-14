use aprilgrid::{detector::{TagDetector, DetectorParams}, TagFamily};
use opencv::{
    calib3d::{calibrate_camera, stereo_calibrate, CALIB_FIX_INTRINSIC},
    core::{
        no_array, FileStorage, FileStorage_READ, Mat, MatTraitConstManual, Point2f, Point3f, Size,
        TermCriteria, TermCriteria_COUNT, TermCriteria_EPS, ToInputArray, Vector,
        _InputArrayTraitConst,
    },
    hub_prelude::{FileNodeTraitConst, FileStorageTraitConst, MatTraitConst},
    imgproc::{resize, INTER_CUBIC},
};
use image::{DynamicImage, GrayImage};

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
            tag_spacing_ratio: 0.25,
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
    ) -> Option<(Vector<Point2f>, Vector<Point3f>)> {
        let im = Mat::new_rows_cols_with_data(
            resolution.1.into(),
            resolution.0.into(),
            image,
        )
        .unwrap();
        self.get_corner_and_object_points_cv(&im, object_resolution)
    }

    fn get_corner_and_object_points_cv(
        &self,
        image: &impl ToInputArray,
        object_resolution: (u32, u32),
    ) -> Option<(Vector<Point2f>, Vector<Point3f>)> {
        let mut im_upscaled = Mat::default();
        let ss;
        if image.input_array().unwrap().size_def().unwrap() == (98, 98).into() {
            ss = 4.;
            resize(image, &mut im_upscaled, Size::new(0, 0), ss, ss, INTER_CUBIC).unwrap();
        } else {
            ss = 2.;
            resize(image, &mut im_upscaled, Size::new(0, 0), ss, ss, INTER_CUBIC).unwrap();
            // im_upscaled = image.input_array().unwrap().get_mat_def().unwrap();
        }

        let (width, height) = (im_upscaled.cols(), im_upscaled.rows());
        let buffer = im_upscaled.data_typed::<u8>().unwrap().to_vec();
        let gray_img = GrayImage::from_raw(width as u32, height as u32, buffer).unwrap();
        let dynamic_img = DynamicImage::ImageLuma8(gray_img);

        let detections = self.detector.detect(&dynamic_img);
        let mut image_points = Vector::<Point2f>::new();
        let mut object_points = Vector::<Point3f>::new();

        for (tag_id, corners) in detections {
            let Some(obj_pts) = self.generate_3d_points(tag_id) else {
                continue;
            };

            let scaled_corners = corners.iter().map(|(x, y)| {
                Point2f::new(
                    (x + 0.5) / ss as f32 - 0.5,
                    (y + 0.5) / ss as f32 - 0.5,
                )
            });

            image_points.extend(scaled_corners);
            object_points.extend(obj_pts.into_iter());
        }

        if image_points.is_empty() {
            return None;
        }

        let mut image_points_object_points = std::iter::zip(image_points, object_points).collect::<Vec<_>>();
        image_points_object_points.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let (mut image_points, object_points): (Vec<_>, Vec<_>) = image_points_object_points.into_iter().unzip();

        let size = image.input_array().unwrap().size(-1).unwrap();
        for p in image_points.as_mut_slice() {
            p.x *= (object_resolution.0 - 1) as f32 / (size.width - 1) as f32;
            p.y *= (object_resolution.1 - 1) as f32 / (size.height - 1) as f32;
        }

        Some((image_points.into(), object_points.into()))
    }

    fn generate_3d_points(&self, tag_id: u32) -> Option<Vec<Point3f>> {
        if tag_id >= (self.config.rows * self.config.cols) as u32 {
            return None;
        }

        let row = (tag_id / self.config.cols as u32) as f32;
        let col = (tag_id % self.config.cols as u32) as f32;
        let size = self.config.tag_size as f32;
        let spacing = self.config.tag_spacing as f32;

        Some(vec![
            Point3f::new(col * (size + spacing), row * (size + spacing), 0.0),
            Point3f::new(col * (size + spacing) + size, row * (size + spacing), 0.0),
            Point3f::new(col * (size + spacing) + size, row * (size + spacing) + size, 0.0),
            Point3f::new(col * (size + spacing), row * (size + spacing) + size, 0.0),
        ])
    }
}

pub fn calibrate_single(
    images: &[Vec<u8>],
    image_files: &Vector<String>,
    port: crate::Port,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    device_uuid: crate::DeviceUuid,
    config: &AprilGridConfig,
) {
    let grid = AprilGrid::new(config.clone());
    let mut corners_arr = Vector::<Vector<Point2f>>::new();
    let mut object_points_arr = Vector::<Vector<Point3f>>::new();

    for image in images {
        if let Some((corners, obj_points)) = grid.get_corner_and_object_points(image, resolution, object_resolution) {
            corners_arr.push(corners);
            object_points_arr.push(obj_points);
        }
    }

    let mut camera_matrix = Mat::default();
    let mut dist_coeffs = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: f64::EPSILON,
    };

    let reproj_err = calibrate_camera(
        &object_points_arr,
        &corners_arr,
        (object_resolution.0 as i32, object_resolution.1 as i32).into(),
        &mut camera_matrix,
        &mut dist_coeffs,
        &mut no_array(),
        &mut no_array(),
        0,
        criteria,
    )
    .unwrap();
    println!("RMS error: {}", reproj_err);

    super::save_single_calibration(
        port,
        "aprilgrid",
        &camera_matrix,
        &dist_coeffs,
        reproj_err,
        image_files,
        device_uuid,
    );
}

pub fn my_stereo_calibrate(
    wf: &[Vec<u8>],
    nf: &[Vec<u8>],
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    wf_image_files: &Vector<String>,
    nf_image_files: &Vector<String>,
    device_uuid: crate::DeviceUuid,
    config: &AprilGridConfig,
) {
    let grid = AprilGrid::new(config.clone());

    let Some((nf_camera_matrix, nf_dist_coeffs)) = &mut read_camera_params(&format!("calibrations/{device_uuid}/nearfield.json")) else {
        println!("Couldn't get nearfield intrinsics");
        return;
    };
    let Some((wf_camera_matrix, wf_dist_coeffs)) = &mut read_camera_params(&format!("calibrations/{device_uuid}/widefield.json")) else {
        println!("Couldn't get widefield intrinsics");
        return;
    };

    let mut object_points_arr = Vector::<Vector<Point3f>>::new();
    let mut wf_corners_arr = Vector::<Vector<Point2f>>::new();
    let mut nf_corners_arr = Vector::<Vector<Point2f>>::new();

    for (wf_image, nf_image) in wf.iter().zip(nf) {
        let Some((wf_corners, obj_pts)) = grid.get_corner_and_object_points(wf_image, resolution, object_resolution) else {
            continue;
        };
        let Some((nf_corners, _)) = grid.get_corner_and_object_points(nf_image, resolution, object_resolution) else {
            continue;
        };

        wf_corners_arr.push(wf_corners);
        nf_corners_arr.push(nf_corners);
        object_points_arr.push(obj_pts);
    }

    let mut r = Mat::default();
    let mut t = Mat::default();
    let criteria = TermCriteria {
        typ: TermCriteria_EPS + TermCriteria_COUNT,
        max_count: 30,
        epsilon: f64::EPSILON,
    };

    let reproj_err = stereo_calibrate(
        &object_points_arr,
        &wf_corners_arr,
        &nf_corners_arr,
        wf_camera_matrix,
        wf_dist_coeffs,
        nf_camera_matrix,
        nf_dist_coeffs,
        (object_resolution.0 as i32, object_resolution.1 as i32).into(),
        &mut r,
        &mut t,
        &mut no_array(),
        &mut no_array(),
        CALIB_FIX_INTRINSIC,
        criteria,
    )
    .unwrap();

    super::save_stereo_calibration(
        "aprilgrid",
        r,
        t,
        reproj_err,
        wf_image_files,
        nf_image_files,
        device_uuid,
    );
}

pub fn read_camera_params(path: &str) -> Option<(Mat, Mat)> {
    let fs = FileStorage::new_def(path, FileStorage_READ).unwrap();
    if !fs.is_opened().unwrap() {
        None
    } else {
        let camera_matrix = fs.get("camera_matrix").unwrap().mat().unwrap();
        let dist_coeffs = fs.get("dist_coeffs").unwrap().mat().unwrap();
        Some((camera_matrix, dist_coeffs))
    }
}
