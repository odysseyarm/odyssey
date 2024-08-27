use opencv::{calib3d::{get_optimal_new_camera_matrix, get_optimal_new_camera_matrix_def, undistort}, core::{no_array, FileStorage, FileStorage_READ, Mat, Size}, highgui::{imshow, wait_key_def}, hub_prelude::{FileNodeTraitConst, FileStorageTraitConst}, imgcodecs::{imread, IMREAD_COLOR}, imgproc::{resize, INTER_CUBIC}};
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    camera_params: String,
    image: String,
}

fn main() {
    let cli = Cli::parse();
    let (camera_matrix, dist_coeffs) = read_camara_params(&cli.camera_params);
    let new_camera_matrix = get_optimal_new_camera_matrix_def(&camera_matrix, &dist_coeffs, Size::new(4096, 4096), 1.0).unwrap();
    let tmp = imread(&cli.image, IMREAD_COLOR).unwrap();
    let mut im = Mat::default();
    resize(&tmp, &mut im, Size::new(4096, 4096), 0.0, 0.0, INTER_CUBIC).unwrap();
    let mut unim = Mat::default();
    undistort(&im, &mut unim, &camera_matrix, &dist_coeffs, &new_camera_matrix).unwrap();
    let mut scaled_unim = Mat::default();
    let mut scaled_im = Mat::default();
    resize(&unim, &mut scaled_unim, Size::new(512, 512), 0.0, 0.0, INTER_CUBIC).unwrap();
    resize(&im, &mut scaled_im, Size::new(512, 512), 0.0, 0.0, INTER_CUBIC).unwrap();
    imshow("distorted", &scaled_im).unwrap();
    imshow("undistorted", &scaled_unim).unwrap();
    wait_key_def().unwrap();
}

fn read_camara_params(path: &str) -> (Mat, Mat) {
    let fs = FileStorage::new_def(path, FileStorage_READ).unwrap();
    let camera_matrix = fs.get("camera_matrix").unwrap().mat().unwrap();
    let dist_coeffs = fs.get("dist_coeffs").unwrap().mat().unwrap();
    (camera_matrix, dist_coeffs)
}
