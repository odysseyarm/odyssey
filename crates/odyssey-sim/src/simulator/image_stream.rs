use nalgebra::{Matrix3, Rotation3, Vector3};
use odyssey_protocol::image::{IMAGE_CHUNK_CAPACITY, ImageChunk, ImageStreamConfig};
use odyssey_protocol::{Packet, PacketData};
use tokio::sync::mpsc;

use super::SensorModel;

// Leave room for command replies and IMU reports on the shared USB endpoint.
const PACKETS_PER_TICK: usize = 32;
const MODERN_BOARD_DISTANCE_M: f32 = 0.5;
const LEGACY_BOARD_DISTANCE_M: f32 = 0.3;

/// One bounded frame, transmitted incrementally through the real packet client.
/// This is an explicit synthetic optical scene, not emulated sensor firmware.
pub(super) struct ImageSession {
    pub(super) generation: u32,
    pub(super) request_id: u8,
    /// Modern only: the model ends the session at this simulated time.
    pub(super) lease_until_ms: u64,
    config: ImageStreamConfig,
    width: u16,
    height: u16,
    board_distance_m: f32,
    frame_id: u32,
    timestamp_us: u32,
    next_frame_ms: u64,
    pixels: Vec<u8>,
    offset: usize,
}

impl ImageSession {
    pub(super) fn new(
        generation: u32,
        request_id: u8,
        config: ImageStreamConfig,
        sensor_model: SensorModel,
        lease_until_ms: u64,
    ) -> Self {
        let (width, height) = sensor_model.image_dimensions();
        Self {
            generation,
            request_id,
            lease_until_ms,
            config,
            width,
            height,
            // The lower-resolution camera needs a nearer board to resolve its corners.
            board_distance_m: match sensor_model {
                SensorModel::Pag7665qn => MODERN_BOARD_DISTANCE_M,
                SensorModel::Paj7025 => LEGACY_BOARD_DISTANCE_M,
            },
            frame_id: 0,
            timestamp_us: 0,
            next_frame_ms: 0,
            pixels: vec![0; usize::from(width) * usize::from(height)],
            offset: usize::from(width) * usize::from(height),
        }
    }

    /// Returns false when the endpoint has closed. A full endpoint retains the
    /// current cursor, so a partial frame never becomes a misleading full image.
    pub(super) fn tick(&mut self, time_ms: u64, sender: &mpsc::Sender<Packet>) -> bool {
        if self.offset == self.pixels.len() {
            if time_ms < self.next_frame_ms {
                return !sender.is_closed();
            }
            self.frame_id = self.frame_id.wrapping_add(1);
            self.timestamp_us = time_ms.wrapping_mul(1000) as u32;
            self.next_frame_ms = time_ms.saturating_add(1000 / u64::from(self.config.fps));
            render_checkerboard(
                &mut self.pixels,
                self.frame_id,
                self.width,
                self.height,
                self.board_distance_m,
            );
            self.offset = 0;
        }

        for _ in 0..PACKETS_PER_TICK {
            if self.offset == self.pixels.len() {
                break;
            }
            let len = IMAGE_CHUNK_CAPACITY.min(self.pixels.len() - self.offset);
            let mut data = [0; IMAGE_CHUNK_CAPACITY];
            data[..len].copy_from_slice(&self.pixels[self.offset..self.offset + len]);
            let data = PacketData::ImageChunk(ImageChunk {
                generation: self.generation,
                frame_id: self.frame_id,
                timestamp_us: self.timestamp_us,
                objects_timestamp_us: 0,
                camera: self.config.camera,
                width: self.width,
                height: self.height,
                object_count: 0,
                offset: self.offset as u32,
                len: len as u16,
                data,
            });
            match sender.try_send(Packet {
                id: self.request_id,
                data,
            }) {
                Ok(()) => self.offset += len,
                Err(mpsc::error::TrySendError::Full(_)) => break,
                Err(mpsc::error::TrySendError::Closed(_)) => return false,
            }
        }
        true
    }
}

/// Nine by seven squares give eight by six inner corners. Vary the physical
/// board pose so actual mono calibration can observe more than one homography.
fn render_checkerboard(
    pixels: &mut [u8],
    frame_id: u32,
    width: u16,
    height: u16,
    board_distance_m: f32,
) {
    const PITCH: f32 = 0.025;
    let phase = frame_id as f32 * 0.27;
    let rotation = Rotation3::from_euler_angles(
        0.22 * phase.sin(),
        0.3 * (phase * 0.7).cos(),
        0.12 * (phase * 0.6).sin(),
    );
    let translation = rotation * Vector3::new(-4.5 * PITCH, -3.5 * PITCH, 0.0)
        + Vector3::new(
            0.035 * phase.sin(),
            0.025 * phase.cos(),
            board_distance_m + 0.04 * phase.sin(),
        );
    let intrinsic = Matrix3::new(
        280.0 * f32::from(width) / 320.0,
        0.0,
        f32::from(width) * 0.5,
        0.0,
        280.0 * f32::from(height) / 240.0,
        f32::from(height) * 0.5,
        0.0,
        0.0,
        1.0,
    );
    let homography = intrinsic
        * Matrix3::from_columns(&[
            rotation.matrix().column(0).into_owned(),
            rotation.matrix().column(1).into_owned(),
            translation,
        ]);
    let inverse = homography
        .try_inverse()
        .expect("the synthetic board is a visible plane at positive depth");
    for (row, output) in pixels.chunks_exact_mut(usize::from(width)).enumerate() {
        for (column, pixel) in output.iter_mut().enumerate() {
            let local = inverse * Vector3::new(column as f32 + 0.5, row as f32 + 0.5, 1.0);
            let x = local.x / local.z / PITCH;
            let y = local.y / local.z / PITCH;
            *pixel = if (0.0..9.0).contains(&x) && (0.0..7.0).contains(&y) {
                if (x as u32 + y as u32).is_multiple_of(2) {
                    20
                } else {
                    240
                }
            } else {
                110
            };
        }
    }
}
