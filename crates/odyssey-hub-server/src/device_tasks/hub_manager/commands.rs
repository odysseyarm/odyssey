use odyssey_usb::device::MuxDevice;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::{CancellationToken, DropGuard};

use super::super::messages::{DongleTaskMessage, Reply};

pub(super) enum ControlCommand {
    StartPairing { timeout_ms: u32, reply: Reply<()> },
    CancelPairing { reply: Reply<()> },
    ClearBonds { reply: Reply<()> },
}

impl ControlCommand {
    fn reject(self) {
        let (Self::StartPairing { reply, .. }
        | Self::CancelPairing { reply }
        | Self::ClearBonds { reply }) = self;
        let _ = reply.send(Err("Dongle control command queue unavailable".into()));
    }
}

/// Tracker setup and control polling cannot delay fresh marker lease commands.
pub(super) fn dispatch(
    hub: MuxDevice,
    mut commands: mpsc::Receiver<DongleTaskMessage>,
) -> (mpsc::Receiver<ControlCommand>, DropGuard) {
    let (control_tx, control_rx) = mpsc::channel(super::DONGLE_COMMAND_CHANNEL_CAPACITY);
    let cancel = CancellationToken::new();
    let lifetime = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let mut requests = JoinSet::new();
        loop {
            let command = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                _ = requests.join_next(), if !requests.is_empty() => continue,
                command = commands.recv() => match command {
                    Some(command) => command,
                    None => break,
                },
            };
            let control = match command {
                DongleTaskMessage::Accessory { command, mut reply } => {
                    if reply.is_closed() {
                        continue;
                    }
                    if requests.len() >= odyssey_protocol::accessory::MAX_PENDING_REQUESTS {
                        let _ =
                            reply.send(Err(odyssey_protocol::accessory::Error::Busy.to_string()));
                        continue;
                    }
                    let hub = hub.clone();
                    requests.spawn(async move {
                        let result = tokio::select! {
                            () = reply.closed() => return,
                            result = hub.accessory_request(command) => result.map_err(|error| error.to_string()),
                        };
                        let _ = reply.send(result);
                    });
                    continue;
                }
                DongleTaskMessage::SubscribeAccessoryEvents { reply } => {
                    let _ = reply.send(Ok(hub.subscribe_accessory_events()));
                    continue;
                }
                DongleTaskMessage::StartPairing { timeout_ms, reply } => {
                    ControlCommand::StartPairing { timeout_ms, reply }
                }
                DongleTaskMessage::CancelPairing { reply } => {
                    ControlCommand::CancelPairing { reply }
                }
                DongleTaskMessage::ClearBonds { reply } => ControlCommand::ClearBonds { reply },
            };
            if let Err(error) = control_tx.try_send(control) {
                error.into_inner().reject();
            }
        }
    });
    (control_rx, lifetime)
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use odyssey_protocol::accessory::{Command, Completion, Error};
    use odyssey_protocol::mux::{MuxMsg, Version};
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn blocked_tracker_control_does_not_block_marker_write_ack() {
        let (outgoing, mut device_rx) = mpsc::channel(8);
        let (device_tx, incoming) = mpsc::channel(8);
        let (control_out, _control_rx) = mpsc::channel(8);
        let (_control_tx, control_in) = mpsc::channel(8);
        let hub = MuxDevice::from_simulated_channels(outgoing, incoming, control_out, control_in);
        let (commands_tx, commands_rx) = mpsc::channel(32);
        let (_blocked_tracker_commands, _lifetime) = dispatch(hub, commands_rx);
        for _ in 0..=super::super::DONGLE_COMMAND_CHANNEL_CAPACITY {
            let (reply, _) = oneshot::channel();
            commands_tx
                .send(DongleTaskMessage::StartPairing {
                    timeout_ms: 1000,
                    reply,
                })
                .await
                .unwrap();
        }
        let (abandoned_reply, abandoned_answer) = oneshot::channel();
        drop(abandoned_answer);
        commands_tx
            .send(DongleTaskMessage::Accessory {
                command: Command::MarkerDemand {
                    address: [9; 6],
                    active: true,
                },
                reply: abandoned_reply,
            })
            .await
            .unwrap();
        let (reply, answer) = oneshot::channel();
        commands_tx
            .send(DongleTaskMessage::Accessory {
                command: Command::MarkerDemand {
                    address: [1; 6],
                    active: true,
                },
                reply,
            })
            .await
            .unwrap();
        let proof = async {
            loop {
                match device_rx.recv().await.unwrap() {
                    MuxMsg::ReadVersion() => device_tx
                        .send(MuxMsg::ReadVersionResponse(Version::new([1, 0, 0])))
                        .await
                        .unwrap(),
                    MuxMsg::AccessoryRequest(request) => {
                        assert_eq!(
                            request.command,
                            Command::MarkerDemand {
                                address: [1; 6],
                                active: true
                            }
                        );
                        device_tx
                            .send(MuxMsg::AccessoryCompletion(Completion {
                                id: request.id,
                                result: Err(Error::NotConnected),
                            }))
                            .await
                            .unwrap();
                        break;
                    }
                    other => panic!("unexpected mux request: {other:?}"),
                }
            }
            let error = answer.await.unwrap().unwrap_err();
            assert!(error.contains("not connected"));
        };
        tokio::time::timeout(std::time::Duration::from_secs(1), proof)
            .await
            .unwrap();
    }
}
