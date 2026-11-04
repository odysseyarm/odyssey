//! The camera thread.
//!
//! A `Session` blocks on the camera for every frame, so one dedicated thread
//! owns it and talks to the UI through channels: [`Command`]s in, [`Event`]s
//! out. The thread also owns camera enumeration, which keeps every SDK call on
//! one thread and lets a listing finish before a stream opens.
//!
//! The camera streams only while a session is open, and a session closes on
//! [`Command::Close`], after [`Command::Finish`], on any camera error and when
//! the [`Worker`] is dropped. Nothing else keeps a stream alive.
//! A missed preview frame resets readiness, even when the camera recovers before
//! the consecutive-timeout limit. Invalid camera data closes the session rather
//! than leaving a supposedly reusable stream behind.

mod backend;
mod command;
mod encode;
mod engine;
mod event;
mod failure;
mod frame;
mod report;
mod stability;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};

pub(crate) use backend::{Backend, DEMO_DIAGONAL_IN, ListError, OpenRequest, demo_scene};
pub(crate) use command::{Command, Mode};
use dioxus::logger::tracing;
use engine::Engine;
pub(crate) use event::{Event, SavedAttempt, SessionEvent};
pub(crate) use failure::{Failure, FailureKind};
pub(crate) use frame::{Finished, MarkerFrame, PreviewFrame};
use tokio::sync::mpsc::UnboundedSender;

/// Handle to the camera thread. Dropping it stops the camera and the thread.
pub(crate) struct Worker {
    commands: Sender<Command>,
    in_flight: Arc<AtomicBool>,
}

impl Worker {
    /// Starts an idle thread: no camera is touched until a command asks.
    ///
    /// If the OS cannot start the thread, the worker still exists but nothing
    /// listens to it; the UI is told through a failed camera listing, which is
    /// where Setup shows what stopped it.
    pub(crate) fn spawn(backend: Backend, events: UnboundedSender<Event>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let in_flight = Arc::new(AtomicBool::new(false));
        let failure_events = events.clone();
        let engine = Engine::new(backend, events, Arc::clone(&in_flight));
        // The thread is detached: it ends when `commands` drops, and closing an
        // SDK pipeline can take a moment the UI thread must not wait for.
        let started = std::thread::Builder::new()
            .name("calibration-camera".into())
            .spawn(move || engine.run(receiver));
        if let Err(e) = started {
            tracing::error!("could not start the camera thread: {e}");
            let _ = failure_events.send(Event::Cameras(Err(ListError::Camera(format!(
                "the camera thread could not start ({e})"
            )))));
        }
        Self {
            commands,
            in_flight,
        }
    }

    /// Queues a command, or returns it if the camera thread has stopped.
    /// Callers must finish the corresponding UI operation on failure; no reply
    /// can arrive from a disconnected thread.
    #[expect(
        clippy::result_large_err,
        reason = "The UI recovers the rejected command; boxing would allocate on a channel close."
    )]
    pub(crate) fn send(&self, command: Command) -> Result<(), mpsc::SendError<Command>> {
        self.commands.send(command)
    }

    /// Tells the thread the UI has taken the last frame and wants the next.
    pub(crate) fn frame_consumed(&self) {
        self.in_flight.store(false, Ordering::Release);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Stop the stream first; the thread then sees the channel close and exits.
        let _ = self.commands.send(Command::Close);
    }
}
