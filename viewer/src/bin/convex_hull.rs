use opencv::core::{Point2f, Size};
use viewer::circles::special::{convex_hull, find_circles_grid_special};

fn main() {
    let mut pts = vec![];

    let stdin = std::io::stdin();
    for line in stdin.lines() {
        let line = line.unwrap();
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let x = parts[0].parse::<f32>().unwrap();
        let y = parts[1].parse::<f32>().unwrap();
        pts.push(Point2f::new(x, y));
    }
    let sorted = find_circles_grid_special(&pts, Size::new(9, 9)).unwrap();
    for p in sorted {
        println!("{} {}", p.x, p.y);
    }
    println!("---");
    let hull = convex_hull(pts).unwrap();
    for p in hull {
        println!("{} {}", p.x, p.y);
    }
}
