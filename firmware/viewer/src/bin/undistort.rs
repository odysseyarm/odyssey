use clap::Parser;
use eros::Context as _;
use opencv::calib3d::{get_optimal_new_camera_matrix_def, undistort};
use opencv::core::{Mat, Size};
use opencv::highgui::{imshow, wait_key_def};
use opencv::imgcodecs::{IMREAD_COLOR, imread, imwrite_def};
use opencv::imgproc::{INTER_CUBIC, resize};

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    camera_params: String,
    image: String,
    #[arg(short, long)]
    out: Option<String>,
}

fn main() -> std::process::ExitCode {
    match run().context("Image undistortion failed") {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eros::Result<()> {
    let cli = Cli::parse();
    let (camera_matrix, dist_coeffs) = viewer::chessboard::read_camera_params(&cli.camera_params)?;
    let new_camera_matrix = get_optimal_new_camera_matrix_def(
        &camera_matrix,
        &dist_coeffs,
        Size::new(4095, 4095),
        1.0,
    )?;
    let tmp = imread(&cli.image, IMREAD_COLOR)?;
    let mut im = Mat::default();
    resize(&tmp, &mut im, Size::new(4095, 4095), 0.0, 0.0, INTER_CUBIC)?;
    let mut unim = Mat::default();
    undistort(
        &im,
        &mut unim,
        &camera_matrix,
        &dist_coeffs,
        &new_camera_matrix,
    )?;
    let mut scaled_unim = Mat::default();
    let mut scaled_im = Mat::default();
    resize(
        &unim,
        &mut scaled_unim,
        Size::new(512, 512),
        0.0,
        0.0,
        INTER_CUBIC,
    )?;
    resize(
        &im,
        &mut scaled_im,
        Size::new(512, 512),
        0.0,
        0.0,
        INTER_CUBIC,
    )?;
    imshow("distorted", &scaled_im)?;
    imshow("undistorted", &scaled_unim)?;
    wait_key_def()?;
    if let Some(out) = cli.out {
        if !imwrite_def(&out, &unim)? {
            eros::bail!("Unable to write undistorted image {out}");
        }
    }
    Ok(())
}
