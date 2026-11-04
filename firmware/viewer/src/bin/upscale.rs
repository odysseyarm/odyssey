use clap::Parser;
use eros::Context as _;
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

fn main() -> std::process::ExitCode {
    match run().context("Image upscaling failed") {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eros::Result<()> {
    let cli = Cli::parse();
    let image = imread(&cli.image, IMREAD_COLOR)
        .with_context(|| format!("Unable to read image {}", cli.image))?;
    let mut upscaled = Mat::default();
    resize(
        &image,
        &mut upscaled,
        Size::new(cli.size, cli.size),
        0.0,
        0.0,
        INTER_CUBIC,
    )?;
    if !imwrite(&cli.output, &upscaled, &opencv::core::Vector::new())? {
        eros::bail!("Unable to write image {}", cli.output);
    }
    Ok(())
}
