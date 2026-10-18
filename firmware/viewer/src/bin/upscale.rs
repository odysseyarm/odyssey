use clap::Parser;
use opencv::core::{Mat, Size};
use opencv::imgcodecs::{IMREAD_COLOR, imread, imwrite};
use opencv::imgproc::{INTER_CUBIC, resize};

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    size: i32,
    #[arg(short, long)]
    output: String,
    image: String,
}

fn main() {
    let cli = Cli::parse();
    let image = imread(&cli.image, IMREAD_COLOR).unwrap();
    let mut upscaled = Mat::default();
    resize(
        &image,
        &mut upscaled,
        Size::new(cli.size, cli.size),
        0.0,
        0.0,
        INTER_CUBIC,
    )
    .unwrap();
    imwrite(&cli.output, &upscaled, &vec![].into()).unwrap();
}
