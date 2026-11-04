//! The optional Bevy 3D viewer, behind the `bevy` feature. It runs on its own
//! thread with its own event loop; the tool only asks it to show its window.

use std::sync::Arc;

use bevy::DefaultPlugins;
use bevy::app::{App, Startup, Update};
use bevy::ecs::system::{Commands, Query};
use bevy::prelude::*;
use bevy::window::{PresentMode, WindowCloseRequested, WindowPlugin, WindowTheme};
use bevy::winit::WinitPlugin;
use bevy_atmosphere::plugin::{AtmosphereCamera, AtmospherePlugin};
use bevy_infinite_grid::{
    GridShadowCamera, InfiniteGridBundle, InfiniteGridPlugin, InfiniteGridSettings,
};
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;

/// Handle to the viewer thread.
pub struct BevyViewer {
    sender: crossbeam::channel::Sender<Event>,
}

impl BevyViewer {
    /// Starts the viewer with its window hidden.
    pub fn spawn(runner: Arc<Mutex<MotRunner>>) -> Self {
        let (sender, receiver) = crossbeam::channel::bounded(10);
        std::thread::spawn(move || {
            App::new()
                .add_plugins((
                    DefaultPlugins
                        .set(WindowPlugin {
                            primary_window: Some(create_bevy_window(false)),
                            exit_condition: bevy::window::ExitCondition::DontExit,
                            close_when_requested: false,
                            ..default()
                        })
                        .set(WinitPlugin {
                            run_on_any_thread: true,
                            ..default()
                        }),
                    InfiniteGridPlugin,
                    AtmospherePlugin,
                ))
                .add_systems(Startup, setup)
                .add_systems(
                    Update,
                    (read_stream, show_window, rotate_cube, hide_instead_of_close),
                )
                .add_event::<StreamEvent>()
                .insert_resource(StreamReceiver(receiver))
                .insert_resource(MotRunnerResource(runner))
                .run();
        });
        Self { sender }
    }

    /// Shows the viewer's window. A full queue means the viewer is already busy
    /// opening it, so the request can be dropped.
    pub fn open(&self) {
        if self.sender.try_send(Event::OpenWindow).is_err() {
            tracing::debug!("bevy viewer is busy; ignoring the open request");
        }
    }
}

#[derive(Resource)]
struct MotRunnerResource(Arc<Mutex<MotRunner>>);

enum Event {
    OpenWindow,
}

#[derive(Resource, Deref)]
struct StreamReceiver(crossbeam::channel::Receiver<Event>);

#[derive(Event)]
struct StreamEvent(Event);

fn read_stream(receiver: Res<StreamReceiver>, mut events: EventWriter<StreamEvent>) {
    for from_stream in receiver.try_iter() {
        events.send(StreamEvent(from_stream));
    }
}

fn show_window(
    mut commands: Commands,
    mut reader: EventReader<StreamEvent>,
    mut window: Query<&mut bevy::window::Window>,
) {
    for (_, event) in reader.read().enumerate() {
        match event.0 {
            Event::OpenWindow => {
                if window.iter().len() == 0 {
                    commands.spawn(create_bevy_window(true));
                } else {
                    window.single_mut().visible = true;
                }
            }
        }
    }
}

pub fn hide_instead_of_close(
    mut closed: EventReader<WindowCloseRequested>,
    mut window: Query<&mut bevy::window::Window>,
) {
    for event in closed.read() {
        if let Ok(mut window) = window.get_mut(event.window) {
            window.visible = false;
        }
    }
}

fn create_bevy_window(visible: bool) -> bevy::window::Window {
    bevy::window::Window {
        title: "I am a window!".into(),
        name: Some("bevy.app".into()),
        resolution: (500., 300.).into(),
        present_mode: PresentMode::AutoVsync,
        // Tells wasm not to override default event handling, like F5, Ctrl+R etc.
        prevent_default_event_handling: false,
        window_theme: Some(WindowTheme::Dark),
        enabled_buttons: bevy::window::EnabledButtons {
            maximize: false,
            ..Default::default()
        },
        // This will spawn an invisible window
        // This is useful when you want to avoid the white window that shows up before the GPU is ready to render the app.
        visible,
        ..default()
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Spawn a cube to rotate.
    commands.spawn((PbrBundle {
        mesh: meshes.add(Cuboid::default()),
        material: materials.add(Color::WHITE),
        transform: Transform::from_translation(Vec3::ZERO),
        ..default()
    },));

    commands.spawn(InfiniteGridBundle {
        settings: InfiniteGridSettings {
            // shadow_color: None,
            ..default()
        },
        ..default()
    });

    // Spawn a camera looking at the entities to show what's happening in this example.
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_xyz(0.0, 10.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
        GridShadowCamera,
        AtmosphereCamera::default(),
    ));

    // Add a light source so we can see clearly.
    commands.spawn(DirectionalLightBundle {
        transform: Transform::from_xyz(3.0, 3.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
        ..default()
    });
}

// This system will rotate any entity in the scene with a Rotatable component around its y-axis.
fn rotate_cube(mut camera_query: Query<(&Camera, &mut Transform)>, runner: Res<MotRunnerResource>) {
    let runner = runner.0.lock();
    let rotation = runner.state.rotation_mat;
    let translation = runner.state.translation_mat;

    let offset = Vec3::new(0.0, 1.6, 10.);

    let (_camera, mut camera_transform) = camera_query.single_mut();
    camera_transform.translation = Vec3::new(
        translation.x as f32,
        translation.y as f32,
        translation.z as f32,
    ) + offset;
    let rotation = Mat3::from_cols_slice(rotation.cast().as_slice());
    camera_transform.rotation = Quat::from_mat3(&rotation);
}
