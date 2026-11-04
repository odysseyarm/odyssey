//! The optional 3D viewer owns a separate process: egui and Bevy both use winit,
//! which permits only one event loop per process. A bounded command channel and
//! a fixed-size stdin pipe carry the latest pose without blocking acquisition.

use std::io::{self, Read as _, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

use bevy::app::AppExit;
use bevy::camera::Exposure;
use bevy::light::AtmosphereEnvironmentMapLight;
use bevy::light::light_consts::lux;
use bevy::pbr::Atmosphere;
use bevy::prelude::{
    App, Assets, Camera3d, Color, Commands, Cuboid, DefaultPlugins, DirectionalLight,
    IntoScheduleConfigs as _, Mat3, Mesh, Mesh3d, MeshMaterial3d, MessageReader, MessageWriter,
    PluginGroup as _, Quat, Query, Res, ResMut, Resource, Single, StandardMaterial, Startup,
    Transform, Update, Vec3, Window, With, default,
};
use bevy::window::{PresentMode, PrimaryWindow, WindowCloseRequested, WindowPlugin, WindowTheme};
use bevy::winit::WinitSettings;
use bevy_infinite_grid::{InfiniteGridBundle, InfiniteGridPlugin};
use eros::{Context as _, Result, bail};
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;

/// Internal entry point of this same executable; it never starts egui.
pub const VIEWER_ARGUMENT: &str = "--bevy-viewer";
/// Sample fast enough for high-refresh displays without a busy polling loop.
const POSE_INTERVAL: Duration = Duration::from_millis(4);
/// A show-window flag, nine column-major rotation floats, and three translation floats.
const POSE_FRAME_LEN: usize = 1 + 12 * size_of::<f32>();

/// Lazily starts the viewer. Closing its window hides it; dropping this handle
/// terminates and reaps the owned process and joins its pipe writer.
#[derive(Debug)]
pub struct BevyViewer {
    runner: Arc<Mutex<MotRunner>>,
    process: Option<ViewerProcess>,
}

impl BevyViewer {
    pub fn new(runner: Arc<Mutex<MotRunner>>) -> Self {
        Self {
            runner,
            process: None,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.process.is_some()
    }

    /// Reports an unexpected process exit, pipe failure or writer panic.
    pub fn poll(&mut self) -> Result<()> {
        let Some(process) = &mut self.process else {
            return Ok(());
        };
        if let Some(status) = process.child.try_wait().context("checking the 3D viewer")? {
            self.process = None;
            bail!("3D viewer exited: {status}");
        }
        if process.writer.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(writer) = process.writer.take()
        {
            self.process = None;
            join_pipe_thread(writer)?;
            bail!("3D viewer pipe writer stopped unexpectedly");
        }
        Ok(())
    }

    /// Shows the viewer, starting it if necessary. Pending open requests coalesce.
    pub fn open(&mut self) -> Result<()> {
        self.poll()?;
        if self.process.is_none() {
            let child = Command::new(std::env::current_exe().context("locating the vision tool")?)
                .arg(VIEWER_ARGUMENT)
                .stdin(Stdio::piped())
                .spawn()
                .context("starting the 3D viewer")?;
            let (sender, receiver) = mpsc::sync_channel(1);
            let mut process = ViewerProcess {
                child,
                sender: Some(sender),
                writer: None,
            };
            let stdin = process
                .child
                .stdin
                .take()
                .context("opening the 3D viewer pipe")?;
            let runner = Arc::clone(&self.runner);
            process.writer = Some(
                std::thread::Builder::new()
                    .name("odyssey-viewer-pipe".into())
                    .spawn(move || write_poses(stdin, &runner, &receiver))
                    .context("starting the 3D viewer pipe writer")?,
            );
            self.process = Some(process);
        }
        let sender = self
            .process
            .as_ref()
            .and_then(|process| process.sender.as_ref())
            .context("3D viewer is not running")?;
        match sender.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => bail!("3D viewer pipe writer stopped"),
        }
    }
}

#[derive(Debug)]
struct ViewerProcess {
    child: Child,
    sender: Option<SyncSender<()>>,
    writer: Option<JoinHandle<io::Result<()>>>,
}

impl Drop for ViewerProcess {
    fn drop(&mut self) {
        drop(self.sender.take());
        // Kill before joining: a stalled renderer must not leave the writer
        // blocked on a full OS pipe when the main app closes.
        if !matches!(self.child.try_wait(), Ok(Some(_)))
            && let Err(error) = self.child.kill()
        {
            tracing::error!("Could not stop the 3D viewer: {error}");
        }
        if let Err(error) = self.child.wait() {
            tracing::error!("Could not reap the 3D viewer: {error}");
        }
        if let Some(writer) = self.writer.take()
            && let Err(error) = join_pipe_thread(writer)
        {
            tracing::debug!("3D viewer pipe closed during shutdown: {error:#?}");
        }
    }
}

fn join_pipe_thread(worker: JoinHandle<io::Result<()>>) -> Result<()> {
    worker
        .join()
        .map_err(|_panic| eros::error!("3D viewer pipe thread panicked"))?
        .context("communicating with the 3D viewer")?;
    Ok(())
}

fn write_poses(
    mut stdin: ChildStdin,
    runner: &Mutex<MotRunner>,
    requests: &Receiver<()>,
) -> io::Result<()> {
    loop {
        let show = match requests.recv_timeout(POSE_INTERVAL) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let mut frame = [0; POSE_FRAME_LEN];
        frame[0] = u8::from(show);
        {
            let runner = runner.lock();
            for (index, value) in runner
                .state
                .rotation_mat
                .iter()
                .chain(runner.state.translation_mat.iter())
                .enumerate()
            {
                frame[1 + index * 4..1 + (index + 1) * 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        stdin.write_all(&frame)?;
    }
}

#[derive(Clone, Copy)]
struct Pose {
    rotation: [f32; 9],
    translation: [f32; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            rotation: Mat3::IDENTITY.to_cols_array(),
            translation: [0.0; 3],
        }
    }
}

#[derive(Default)]
struct ViewerInput {
    pose: Pose,
    show: bool,
    closed: bool,
}

#[derive(Resource)]
struct Input(Arc<Mutex<ViewerInput>>);

fn read_poses(input: &Mutex<ViewerInput>) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut frame = [0; POSE_FRAME_LEN];
    loop {
        match stdin.read_exact(&mut frame[..1]) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        stdin.read_exact(&mut frame[1..])?;
        let values: [f32; 12] = std::array::from_fn(|index| {
            let offset = 1 + index * 4;
            f32::from_le_bytes([
                frame[offset],
                frame[offset + 1],
                frame[offset + 2],
                frame[offset + 3],
            ])
        });
        if frame[0] > 1 || !values.iter().all(|value| value.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid 3D viewer pose",
            ));
        }
        let mut input = input.lock();
        input.pose = Pose {
            rotation: std::array::from_fn(|index| values[index]),
            translation: std::array::from_fn(|index| values[index + 9]),
        };
        input.show |= frame[0] != 0;
    }
}

/// Runs the internal viewer process, whose stdin belongs to the main tool.
///
/// # Errors
/// Returns reader startup, pose-pipe or renderer exit failures.
pub fn run_viewer() -> Result<()> {
    let input = Arc::new(Mutex::new(ViewerInput::default()));
    let reader_input = Arc::clone(&input);
    let reader = std::thread::Builder::new()
        .name("odyssey-viewer-input".into())
        .spawn(move || {
            let result = read_poses(&reader_input);
            reader_input.lock().closed = true;
            result
        })
        .context("starting the 3D viewer input reader")?;
    let exit = App::new()
        .add_plugins((
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(create_bevy_window()),
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .disable::<bevy::log::LogPlugin>(),
            InfiniteGridPlugin,
        ))
        .insert_resource(WinitSettings::game())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (read_requests, follow_tracking_pose, hide_instead_of_close).chain(),
        )
        .insert_resource(Input(Arc::clone(&input)))
        .run();
    // A renderer failure must not join a reader blocked on a still-live parent.
    // This internal entry point exits the process on return, closing that reader.
    if !input.lock().closed {
        bail!("3D viewer renderer stopped before its parent disconnected: {exit:?}");
    }
    join_pipe_thread(reader)?;
    if exit != AppExit::Success {
        bail!("3D viewer renderer exited: {exit:?}");
    }
    Ok(())
}

fn read_requests(
    input: Res<'_, Input>,
    mut windows: Query<'_, '_, &mut Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    let mut input = input.0.lock();
    if input.closed {
        exit.write(AppExit::Success);
    }
    if std::mem::take(&mut input.show)
        && let Ok(mut window) = windows.single_mut()
    {
        window.visible = true;
    }
}

fn hide_instead_of_close(
    mut closed: MessageReader<'_, '_, WindowCloseRequested>,
    mut windows: Query<'_, '_, &mut Window>,
) {
    for event in closed.read() {
        if let Ok(mut window) = windows.get_mut(event.window) {
            window.visible = false;
        }
    }
}

fn create_bevy_window() -> Window {
    Window {
        title: "Odyssey 3D Viewer".into(),
        name: Some("odyssey.vision-viewer".into()),
        resolution: (500, 300).into(),
        present_mode: PresentMode::AutoVsync,
        prevent_default_event_handling: false,
        window_theme: Some(WindowTheme::Dark),
        enabled_buttons: bevy::window::EnabledButtons {
            maximize: false,
            ..default()
        },
        visible: false,
        ..default()
    }
}

fn setup(
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::default())),
        MeshMaterial3d(materials.add(Color::WHITE)),
        Transform::from_translation(Vec3::ZERO),
    ));
    commands.spawn(InfiniteGridBundle::default());
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 1.6, 10.0),
        Atmosphere::EARTH,
        Exposure::SUNLIGHT,
        AtmosphereEnvironmentMapLight::default(),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT,
            ..default()
        },
        Transform::from_xyz(3.0, 3.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn follow_tracking_pose(
    mut camera: Single<'_, '_, &mut Transform, With<Camera3d>>,
    input: Res<'_, Input>,
) {
    let pose = input.0.lock().pose;
    camera.translation = Vec3::from_array(pose.translation) + Vec3::new(0.0, 1.6, 10.0);
    camera.rotation = Quat::from_mat3(&Mat3::from_cols_array(&pose.rotation));
}
