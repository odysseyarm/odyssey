//! One app-scoped queue for UI-state edits. Enqueue at the UI intent, before
//! spawning a waiter: separate event tasks must not reorder the persisted edits.

use dioxus::prelude::{Coroutine, use_coroutine};
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use odyssey_hub_common::config::{self, UiState};
use tokio::sync::oneshot;

type UpdateResult = Result<UiState, String>;

struct UpdateRequest {
    edit: Box<dyn FnOnce(&mut UiState) + Send>,
    completion: oneshot::Sender<UpdateResult>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct UiStateWriter {
    queue: Coroutine<UpdateRequest>,
}

impl UiStateWriter {
    /// Reserves this edit's place immediately, even if the returned future is
    /// awaited later or dropped. Disk updates remain FIFO across all app views.
    pub(crate) fn update(
        self,
        edit: impl FnOnce(&mut UiState) + Send + 'static,
    ) -> impl Future<Output = UpdateResult> {
        let (completion, result) = oneshot::channel();
        self.queue.send(UpdateRequest {
            edit: Box::new(edit),
            completion,
        });
        async move {
            result.await.map_err(|error| {
                format!("The UI-state writer stopped before saving the change: {error}")
            })?
        }
    }
}

pub(crate) fn use_ui_state_writer() -> UiStateWriter {
    let queue = use_coroutine(|mut updates: UnboundedReceiver<UpdateRequest>| async move {
        while let Some(request) = updates.next().await {
            let result = config::ui_state_update_async(request.edit)
                .await
                .map_err(|error| error.to_string());
            // Closing a page only drops its waiter, not the already-queued edit.
            let _ = request.completion.send(result);
        }
    });
    UiStateWriter { queue }
}
