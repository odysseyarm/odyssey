use eros::Context as _;
use opencv::core::{Point2f, Size};
use viewer::circles::special::{convex_hull, find_circles_grid_special};

fn main() -> std::process::ExitCode {
    match run().context("Circle-grid hull inspection failed") {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> eros::Result<()> {
    let mut pts = vec![];

    let stdin = std::io::stdin();
    for line in stdin.lines() {
        let line = line.context("Unable to read input point")?;
        let mut parts = line.split_whitespace();
        let x = parts
            .next()
            .ok_or_else(|| eros::error!("Point is missing its x coordinate"))?
            .parse::<f32>()
            .context("Invalid point x coordinate")?;
        let y = parts
            .next()
            .ok_or_else(|| eros::error!("Point is missing its y coordinate"))?
            .parse::<f32>()
            .context("Invalid point y coordinate")?;
        if !x.is_finite() || !y.is_finite() {
            eros::bail!("Point coordinates must be finite");
        }
        pts.push(Point2f::new(x, y));
    }
    let sorted = find_circles_grid_special(&pts, Size::new(9, 9))
        .ok_or_else(|| eros::error!("Points do not form the requested 9x9 circle grid"))?;
    for p in sorted {
        println!("{} {}", p.x, p.y);
    }
    println!("---");
    let hull = convex_hull(pts)
        .ok_or_else(|| eros::error!("A convex hull requires at least three points"))?;
    for p in hull {
        println!("{} {}", p.x, p.y);
    }
    Ok(())
}
