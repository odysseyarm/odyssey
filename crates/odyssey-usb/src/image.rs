//! Owned USB image sessions with a single latest-frame mailbox.
//!
//! Dropping a stream cancels its session. The device's supervised dispatcher
//! owns and awaits the stop handshake before accepting a replacement. Prefer
//! `stop().await` when the caller needs proof that object capture has resumed.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use odyssey_protocol::Port;
use odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, ImageChunk, ImageFrame, ImageObject, MAX_IMAGE_OBJECTS, MAX_IMAGE_PIXELS,
    ObjectFeature,
};
use tokio::sync::{Notify, oneshot};
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use crate::{Error, Result};

#[derive(Debug, Default)]
struct Latest {
    value: Option<Result<ImageFrame>>,
    closed: bool,
    waker: Option<Waker>,
}

/// A bounded, cancellation-owning stream of complete images.
/// Slow consumers get the newest complete frame, and invalid metadata ends the
/// stream. It does not borrow the `VmDevice` that opened it.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug)]
pub struct ImageStream {
    latest: Arc<parking_lot::Mutex<Latest>>,
    cancel: CancellationToken,
    stopped: Option<oneshot::Receiver<Result<()>>>,
    renew: Arc<Notify>,
}

impl ImageStream {
    pub(crate) fn channel(
        cancel: CancellationToken,
        stopped: oneshot::Receiver<Result<()>>,
    ) -> (Self, FrameSender) {
        let latest = Arc::new(parking_lot::Mutex::new(Latest::default()));
        let renew = Arc::new(Notify::new());
        (
            Self {
                latest: Arc::clone(&latest),
                cancel,
                stopped: Some(stopped),
                renew: Arc::clone(&renew),
            },
            FrameSender { latest, renew },
        )
    }

    /// Extend the device's capture lease (see
    /// [`IMAGE_LEASE_RENEW_INTERVAL_MS`](odyssey_protocol::image::IMAGE_LEASE_RENEW_INTERVAL_MS)).
    /// Call it only from the code that consumes frames: modern firmware
    /// ends capture and restores tracking once renewals stop. Repeated calls before
    /// the dispatcher sends one coalesce. Never blocks.
    ///
    /// # Errors
    /// [`Error::Closed`] once the stream is stopped or dropped.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub fn renew_lease(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(Error::Closed);
        }
        self.renew.notify_one();
        Ok(())
    }

    /// Cancel capture and wait for the generation-specific Stop acknowledgment.
    /// A replacement cannot start while this handshake is still in flight.
    ///
    /// # Errors
    /// Returns stop-handshake or timeout errors.
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn stop(&mut self) -> Result<()> {
        self.cancel.cancel();
        let wake = {
            let mut latest = self.latest.lock();
            latest.value = None;
            latest.closed = true;
            latest.waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
        // Keep the receiver in self while awaiting: cancelling stop() must not
        // lose the ability to await this same supervised handshake again.
        let result = match self.stopped.as_mut() {
            Some(done) => done.await.unwrap_or(Err(Error::Closed)),
            None => Ok(()),
        };
        self.stopped.take();
        result
    }
}

impl Stream for ImageStream {
    type Item = Result<ImageFrame>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut latest = self.latest.lock();
        if let Some(value) = latest.value.take() {
            return Poll::Ready(Some(value));
        }
        if latest.closed {
            return Poll::Ready(None);
        }
        latest.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for ImageStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(crate) struct FrameSender {
    latest: Arc<parking_lot::Mutex<Latest>>,
    renew: Arc<Notify>,
}
impl FrameSender {
    pub(crate) fn send(&self, frame: Result<ImageFrame>) {
        let wake = {
            let mut latest = self.latest.lock();
            if latest.closed {
                return;
            }
            latest.value = Some(frame);
            latest.waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }

    pub(crate) async fn renewal_requested(&self) {
        self.renew.notified().await;
    }
}
impl Drop for FrameSender {
    fn drop(&mut self) {
        let wake = {
            let mut latest = self.latest.lock();
            latest.closed = true;
            latest.waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

/// At most one frame and 16 sensor features can be in progress. Frame ids use
/// wrapping serial-number order, so repeats and backwards ids cannot resurrect a
/// rejected/complete frame. Every failure invalidates the entire partial frame.
pub(crate) struct Reassembler {
    generation: u32,
    camera: Port,
    newest: Option<u32>,
    current: Option<Partial>,
}
struct Partial {
    id: u32,
    objects: Vec<ObjectFeature>,
    frame: Option<ImageFrame>,
    object_count: u8,
}

impl Reassembler {
    pub(crate) fn new(generation: u32, camera: Port) -> Self {
        Self {
            generation,
            camera,
            newest: None,
            current: None,
        }
    }
    fn invalidate(&mut self, why: &'static str) -> Error {
        self.current = None;
        Error::InvalidImage(why)
    }
    fn select_frame(&mut self, id: u32) -> bool {
        if self.current.as_ref().is_some_and(|p| p.id == id) {
            return true;
        }
        if self
            .newest
            .is_some_and(|old| (id.wrapping_sub(old) as i32) <= 0)
        {
            return false;
        }
        self.newest = Some(id);
        self.current = Some(Partial {
            id,
            objects: Vec::new(),
            frame: None,
            object_count: 0,
        });
        true
    }
    pub(crate) fn object(&mut self, object: ImageObject) -> Result<()> {
        if object.generation != self.generation || !self.select_frame(object.frame_id) {
            return Ok(());
        }
        let p = self.current.as_ref().expect("selected frame");
        let feature = object.feature;
        if p.frame.is_some()
            || usize::from(object.index) != p.objects.len()
            || p.objects.len() >= MAX_IMAGE_OBJECTS
            || p.objects.iter().any(|old| old.id == feature.id)
            || !feature.x.is_finite()
            || !feature.y.is_finite()
            || feature.x < 0.0
            || feature.y < 0.0
            || feature.radius.is_some_and(|v| !v.is_finite() || v < 0.0)
            || feature
                .aspect_ratio
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err(self.invalidate("out-of-order or invalid object metadata"));
        }
        let objects = &mut self.current.as_mut().expect("selected frame").objects;
        if objects.is_empty() {
            objects.reserve_exact(MAX_IMAGE_OBJECTS);
        }
        objects.push(feature);
        Ok(())
    }
    pub(crate) fn chunk(&mut self, chunk: &ImageChunk) -> Result<Option<ImageFrame>> {
        if chunk.generation != self.generation || !self.select_frame(chunk.frame_id) {
            return Ok(None);
        }
        let total = usize::from(chunk.width) * usize::from(chunk.height);
        let valid_dimensions = matches!((chunk.width, chunk.height), (320, 240) | (98, 98));
        let len = usize::from(chunk.len);
        if !valid_dimensions
            || total > MAX_IMAGE_PIXELS
            || chunk.camera != self.camera
            || usize::from(chunk.object_count) > MAX_IMAGE_OBJECTS
            || len == 0
            || len > IMAGE_CHUNK_CAPACITY
            || usize::try_from(chunk.offset)
                .ok()
                .and_then(|n| n.checked_add(len))
                .is_none_or(|end| end > total)
        {
            return Err(self.invalidate("invalid dimensions, camera, count or payload bounds"));
        }
        let p = self.current.as_mut().expect("selected frame");
        if p.frame.is_none() {
            if chunk.offset != 0
                || p.objects.len() != usize::from(chunk.object_count)
                || p.objects.iter().any(|f| {
                    f.x >= f32::from(chunk.width)
                        || f.y >= f32::from(chunk.height)
                        || f.bounds.is_some_and(|[l, t, r, b]| {
                            l > r || t > b || r >= chunk.width || b >= chunk.height
                        })
                })
            {
                return Err(self.invalidate(
                    "missing objects, out-of-frame features or nonzero initial offset",
                ));
            }
            p.object_count = chunk.object_count;
            p.frame = Some(ImageFrame {
                frame_id: chunk.frame_id,
                timestamp_us: chunk.timestamp_us,
                camera: chunk.camera,
                width: chunk.width,
                height: chunk.height,
                pixels: Vec::with_capacity(total),
                objects: std::mem::take(&mut p.objects),
                objects_timestamp_us: chunk.objects_timestamp_us,
            });
        }
        let frame = p.frame.as_mut().expect("initialized frame");
        if chunk.offset as usize != frame.pixels.len()
            || frame.width != chunk.width
            || frame.height != chunk.height
            || frame.camera != chunk.camera
            || frame.timestamp_us != chunk.timestamp_us
            || frame.objects_timestamp_us != chunk.objects_timestamp_us
            || p.object_count != chunk.object_count
        {
            return Err(self.invalidate("mixed frame headers or noncontiguous pixel offset"));
        }
        frame.pixels.extend_from_slice(&chunk.data[..len]);
        if frame.pixels.len() == total {
            return Ok(self.current.take().and_then(|p| p.frame));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
