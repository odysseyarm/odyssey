//! One app-scoped queue for UI-state edits. Enqueue at the UI intent, before
//! spawning a waiter: separate event tasks must not reorder the persisted edits.

use eros::Context as _;
use gpui::Global;
use odyssey_hub_common::config::{self, UiState};
use tokio::sync::{mpsc, oneshot};

use crate::runtime;

type UpdateResult = eros::Result<UiState>;

struct UpdateRequest {
    edit: Box<dyn FnOnce(&mut UiState) + Send>,
    completion: oneshot::Sender<UpdateResult>,
}

#[derive(Clone)]
pub(crate) struct UiStateWriter {
    queue: mpsc::UnboundedSender<UpdateRequest>,
}

impl Global for UiStateWriter {}

impl UiStateWriter {
    /// Starts the writer task on the tokio runtime. It ends when the last
    /// writer handle drops.
    pub(crate) fn start() -> Self {
        let (queue, mut updates) = mpsc::unbounded_channel::<UpdateRequest>();
        runtime::spawn(async move {
            while let Some(request) = updates.recv().await {
                let result = config::ui_state_update_async(request.edit)
                    .await
                    .map_err(Into::into);
                // Closing a page only drops its waiter, not the already-queued edit.
                let _ = request.completion.send(result);
            }
        });
        Self { queue }
    }

    /// Reserves this edit's place immediately, even if the returned future is
    /// awaited later or dropped. Disk updates remain FIFO across all app views.
    pub(crate) fn update<F>(&self, edit: F) -> impl Future<Output = UpdateResult> + use<F>
    where
        F: FnOnce(&mut UiState) + Send + 'static,
    {
        let (completion, result) = oneshot::channel();
        let sent = self.queue.send(UpdateRequest {
            edit: Box::new(edit),
            completion,
        });
        async move {
            if sent.is_err() {
                eros::bail!("The UI-state writer stopped before saving the change");
            }
            result
                .await
                .context("The UI-state writer stopped before saving the change")?
        }
    }
}
