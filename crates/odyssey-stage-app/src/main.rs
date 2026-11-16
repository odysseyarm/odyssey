//! Local web app for staging simulated and real Odyssey devices.

#![expect(
    clippy::print_stdout,
    reason = "the program announces the address of its page"
)]

mod api;
mod app;
mod blender;
mod error;
mod evaluate;
mod hub;
mod sim;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::Parser;
use eros::Context as _;
use odyssey_stage::Stage;
use tokio::sync::watch;

#[derive(Parser)]
#[command(about = "Stage simulated and real Odyssey devices in the browser")]
struct Args {
    /// Port of the page on this computer.
    #[arg(long, default_value_t = 7878)]
    port: u16,
    /// Stage file to open, created and saved on every change.
    #[arg(long)]
    stage: Option<PathBuf>,
    /// Do not look for a running hub.
    #[arg(long)]
    no_hub: bool,
    /// Serve the page from this directory instead of the built-in copy (front-end development).
    #[arg(long, hide = true)]
    web_dir: Option<PathBuf>,
    /// Write a tracker's run as a scene file for the Blender and Rerun tools, then exit.
    #[arg(long, value_name = "FILE", requires = "stage")]
    export_scene: Option<PathBuf>,
    /// Write a tracker's run as an `odyssey-sim-control --scenario` file, then exit.
    #[arg(long, value_name = "FILE", requires = "stage")]
    export_scenario: Option<PathBuf>,
    /// Tracker to export.
    #[arg(long, default_value = "tracker-1")]
    device: String,
    /// Frames per second of the exported scene.
    #[arg(long, default_value_t = 30.0)]
    fps: f64,
}

fn load(path: Option<&PathBuf>) -> eros::Result<Stage> {
    let Some(path) = path.filter(|path| path.exists()) else {
        return Ok(Stage::default());
    };
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a stage file", path.display()))?)
}

fn write_json(output: &Path, value: &impl serde::Serialize) -> eros::Result<()> {
    let json = serde_json::to_vec_pretty(value)?;
    std::fs::create_dir_all(output.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(output, json).with_context(|| format!("cannot write {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

#[tokio::main]
async fn main() -> eros::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    if args.export_scene.is_some() || args.export_scenario.is_some() {
        let compiled = load(args.stage.as_ref())?.compile()?;
        if let Some(output) = &args.export_scene {
            write_json(output, &compiled.scene(&args.device, args.fps)?)?;
        }
        if let Some(output) = &args.export_scenario {
            write_json(
                output,
                &odyssey_stage::scenario(&compiled, &args.device, 50)?,
            )?;
        }
        return Ok(());
    }
    let (hub_sender, hub_view) = watch::channel(hub::HubView::default());
    if args.no_hub {
        hub_sender.send_modify(|view| view.error = Some("started with --no-hub".to_owned()));
    } else {
        hub::spawn(hub_sender);
    }
    let app = app::App::new(
        &load(args.stage.as_ref())?,
        hub_view,
        args.stage.clone(),
        args.web_dir.clone(),
    )
    .map_err(|error| eros::error!("{error:?}"))?;

    let ticker = std::sync::Arc::clone(&app);
    tokio::spawn(async move {
        let mut frame = tokio::time::interval(app::FRAME_INTERVAL);
        let mut sim = tokio::time::interval(app::sim_interval());
        let mut count = 0_u64;
        loop {
            tokio::select! {
                _ = frame.tick() => {
                    ticker.publish(count);
                    count += 1;
                }
                _ = sim.tick() => ticker.drive_sim().await,
            }
        }
    });

    let address = SocketAddr::from(([127, 0, 0, 1], args.port));
    println!("Odyssey stage: http://127.0.0.1:{}", args.port);
    axum::Server::try_bind(&address)
        .with_context(|| format!("port {} is in use; pick another with --port", args.port))?
        .serve(api::router(app, args.port).into_make_service())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("the web server stopped")?;
    Ok(())
}
