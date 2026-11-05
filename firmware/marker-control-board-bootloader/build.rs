#[path = "../marker-control-board/build_support.rs"]
mod build_support;

fn main() -> std::io::Result<()> {
    build_support::build(true)
}
