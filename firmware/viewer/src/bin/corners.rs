use clap::Parser;
use opencv::imgcodecs::{imread, IMREAD_COLOR};
use viewer::{chessboard::get_chessboard_corners_cv, Port};

#[derive(Parser)]
struct Cli {
    #[arg(short = 'n', long)]
    grid_size: u16,
    images: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    println!("# filename x y");
    for filename in cli.images {
        let im = imread(&filename, IMREAD_COLOR).unwrap();
        let corners = get_chessboard_corners_cv(&im, Port::Nf, (4095, 4095), cli.grid_size, cli.grid_size, false);
        if let Some(corners) = corners {
            for corner in corners {
                println!("{filename} {} {}", corner.x, corner.y);
            }
        } else {
            println!("{filename} - -");
        }
    }
}
