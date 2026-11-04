use tokio::sync::{mpsc, oneshot};

use super::rpc_error::RpcError;

/// Where a task reports the outcome of a command: the value, or why it failed.
pub(super) type TaskReply<T> = oneshot::Sender<Result<T, String>>;

/// The command channel of one device task or dongle task.
pub(super) struct TaskSender<Message> {
    /// "device" or "dongle", for error messages.
    task: &'static str,
    sender: mpsc::Sender<Message>,
}

impl<Message> TaskSender<Message> {
    pub(super) fn new(task: &'static str, sender: mpsc::Sender<Message>) -> Self {
        Self { task, sender }
    }

    /// Sends the command that `command` builds around a fresh reply channel and
    /// waits for the task's answer. Waits for room in the channel if it is full.
    pub(super) async fn request<T>(
        &self,
        command: impl FnOnce(TaskReply<T>) -> Message,
    ) -> Result<T, RpcError> {
        let (reply, answer) = oneshot::channel();
        let task = self.task;
        self.sender.send(command(reply)).await.map_err(|cause| {
            tracing::debug!("{task} task closed its command channel: {cause}");
            RpcError::TaskUnavailable { task }
        })?;
        match answer.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(RpcError::TaskFailed(message)),
            Err(cause) => {
                tracing::debug!("{task} task dropped its reply: {cause}");
                Err(RpcError::ReplyDropped { task })
            }
        }
    }

    /// Sends a command that has no reply, failing at once if the channel is full.
    pub(super) fn send_now(&self, command: Message) -> Result<(), RpcError> {
        let task = self.task;
        self.sender.try_send(command).map_err(|cause| {
            tracing::debug!("{task} task rejected a command: {cause}");
            RpcError::TaskUnavailable { task }
        })
    }
}
