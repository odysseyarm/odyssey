use anyhow::Result;
use odyssey_vision_tool::app::{self, Launch};
use tracing::Level;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_env_var("RUST_LOG")
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    app::run(Launch::from_args(&args)?)
}
