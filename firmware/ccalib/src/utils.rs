use nalgebra::{Matrix3, Vector3};
use opencv::{
    calib3d::undistort_points,
    core::{
        BORDER_CONSTANT, CV_32FC1, Mat, MatTraitManual as _, Point2f, Size, ToInputArray,
        ToInputOutputArray, ToOutputArray, Vector, no_array,
    },
    highgui::{
        WND_PROP_VISIBLE, destroy_window, get_window_property, imshow, set_window_title, wait_key,
    },
    imgproc::{INTER_LINEAR, LINE_AA, ellipse, remap},
};

pub fn distort_image(
    undistorted: &impl ToInputArray,
    distorted: &mut impl ToOutputArray,
    cam_mtx: &Matrix3<f64>,
    dist_coeffs: &[f64; 5],
    distorted_size: Size,
    cam_mtx_ud: &Matrix3<f64>,
) {
    let mut dis = Vector::<Point2f>::new();
    for y in 0..distorted_size.height {
        for x in 0..distorted_size.width {
            let p = Point2f::new(x as f32, y as f32);
            dis.push(p);
        }
    }
    let mut und = Vector::<Point2f>::new();
    let _cam_mtx = cam_mtx.transpose();
    let cam_mtx = Mat::new_rows_cols_with_data(3, 3, _cam_mtx.as_slice()).unwrap();
    undistort_points(
        &dis,
        &mut und,
        &cam_mtx,
        &Vector::from_slice(dist_coeffs),
        &no_array(),
        &no_array(),
    )
    .unwrap();

    let mut map_x = Mat::new_size_with_default(distorted_size, CV_32FC1, [0.0; 4].into()).unwrap();
    let mut map_y = Mat::new_size_with_default(distorted_size, CV_32FC1, [0.0; 4].into()).unwrap();
    let map_x_data = map_x.data_typed_mut::<f32>().unwrap();
    let map_y_data = map_y.data_typed_mut::<f32>().unwrap();
    for y in 0..distorted_size.height {
        for x in 0..distorted_size.width {
            let i = y * distorted_size.width + x;
            let p = und.get(i as usize).unwrap();
            let p = Vector3::new(p.x as f64, p.y as f64, 1.0);
            let p = cam_mtx_ud * p;
            let p = p.xy() / p.z;
            map_x_data[i as usize] = p.x as f32;
            map_y_data[i as usize] = p.y as f32;
        }
    }
    remap(
        undistorted,
        distorted,
        &map_x,
        &map_y,
        INTER_LINEAR,
        BORDER_CONSTANT,
        [0.; 4].into(),
    )
    .unwrap();
}

pub fn imshow_multi<T: ToInputArray>(imgs: &[T], title: &str) {
    let mut i = 0;
    let mut current_title = format!("{title} (Image {} of {})", i + 1, imgs.len());
    imshow("imshow_multi", &imgs[0]).unwrap();
    set_window_title("imshow_multi", &current_title).unwrap();
    loop {
        let c = wait_key(100).unwrap();
        // println!("{} {:?}", c, char::from_u32(c as u32));
        if c == i32::from(b'q')
            || c == 0x1b // esc
            || get_window_property(&"imshow_multi", WND_PROP_VISIBLE).unwrap() == 0.
        {
            break;
        }

        'change_image: {
            // left arrow
            if c == 81 || c == i32::from(b'h') || c == i32::from(b',') {
                i = (i + imgs.len() - 1) % imgs.len();
            } else if c == 83 || c == i32::from(b'l') || c == i32::from(b'.') {
                i = (i + 1) % imgs.len();
            } else {
                break 'change_image;
            }
            imshow(&"imshow_multi", &imgs[i].input_array().unwrap()).unwrap();
            current_title = format!("{title} (Image {} of {})", i + 1, imgs.len());
            set_window_title("imshow_multi", &current_title).unwrap();
        }
    }
    destroy_window("imshow_multi").unwrap();
}

pub fn fill_circle(
    im: &mut impl ToInputOutputArray,
    center: (f64, f64),
    radius: f64,
    color: [f64; 4],
) -> opencv::Result<()> {
    let cx = (center.0 * 1024.0).round() as i32;
    let cy = (center.1 * 1024.0).round() as i32;
    let r = (radius * 1024.0).round() as i32;
    ellipse(
        im,
        (cx, cy).into(),
        (r, r).into(),
        0.0,
        0.0,
        360.0,
        color.into(),
        -1,
        LINE_AA,
        10,
    )
}

pub fn stroke_circle(
    im: &mut impl ToInputOutputArray,
    center: (f64, f64),
    radius: f64,
    color: [f64; 4],
    thickness: i32,
) -> opencv::Result<()> {
    let cx = (center.0 * 1024.0).round() as i32;
    let cy = (center.1 * 1024.0).round() as i32;
    let r = (radius * 1024.0).round() as i32;
    ellipse(
        im,
        (cx, cy).into(),
        (r, r).into(),
        0.0,
        0.0,
        360.0,
        color.into(),
        thickness,
        LINE_AA,
        10,
    )
}
