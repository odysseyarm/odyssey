use eros::Result;
use odyssey_vision_tool::app;
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

    let mut args = std::env::args().skip(1);
    match (args.next(), args.next()) {
        (None, None) => app::run(false, false),
        (Some(argument), None) if argument == "--sim" => app::run(true, false),
        (Some(argument), None) if argument == "--sim-legacy" => app::run(true, true),
        #[cfg(feature = "bevy")]
        (Some(argument), None) if argument == odyssey_vision_tool::bevy_viewer::VIEWER_ARGUMENT => {
            odyssey_vision_tool::bevy_viewer::run_viewer()
        }
        _ => {
            eros::bail!("unrecognized arguments; usage: odyssey-vision-tool [--sim | --sim-legacy]")
        }
    }
}
