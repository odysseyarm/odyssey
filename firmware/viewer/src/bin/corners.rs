use clap::Parser;
use eros::Context as _;
use opencv::imgcodecs::{IMREAD_COLOR, imread};
use viewer::Port;
use viewer::chessboard::get_chessboard_corners_cv;

#[derive(Parser)]
struct Cli {
    #[arg(short = 'n', long)]
    grid_size: u16,
    images: Vec<String>,
}

fn main() -> std::process::ExitCode {
    match run().context("Chessboard corner inspection failed") {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eros::Result<()> {
    let cli = Cli::parse();

    println!("# filename x y");
    for filename in cli.images {
        let im = imread(&filename, IMREAD_COLOR)
            .with_context(|| format!("Unable to read image {filename}"))?;
        let corners = get_chessboard_corners_cv(
            &im,
            Port::Nf,
            (4095, 4095),
            cli.grid_size,
            cli.grid_size,
            false,
        )?;
        if let Some(corners) = corners {
            for corner in corners {
                println!("{filename} {} {}", corner.x, corner.y);
            }
        } else {
            println!("{filename} - -");
        }
    }
    Ok(())
}
