//! The "Paired hosts" section of a tracker row: the unit's stored BLE bonds,
//! removing one, and pairing this computer's Bluetooth adapter.

use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div,
};
use odyssey_hub_client::client::{Bond, ClientError, SystemPairingResult};
use odyssey_hub_common::device::Device;

use super::MONO_FONT;
use super::pairing_state::{NOT_CONNECTED, PAIRING_TIMEOUT_MS, result_status};
use crate::components::ui::{AlertKind, ButtonKind, alert, button};
use crate::hub::Hub;
use crate::runtime;
use crate::theme::palette;
use crate::views::device_label::format_address;

/// Bond slots of an ATS Lite.
const MAX_BONDS: usize = 4;

/// Runs one hub call on the tokio runtime.
async fn on_runtime<T: Send + 'static>(
    call: impl Future<Output = Result<T, ClientError>> + Send + 'static,
) -> Result<T, String> {
    runtime::spawn(async move { call.await.map_err(|error| error.to_string()) })
        .await
        .map_err(|error| error.to_string())
        .and_then(std::convert::identity)
}

/// The outcome of the last pairing or removal, shown under the list.
enum Notice {
    None,
    /// Every slot is in use, so nothing can be paired before a removal.
    Full,
    Text(String),
}

fn pairing_notice(outcome: Result<SystemPairingResult, String>) -> Notice {
    match outcome {
        Ok(SystemPairingResult::Paired) => Notice::Text("Paired this computer.".into()),
        Ok(SystemPairingResult::BondsFull) => Notice::Full,
        Ok(SystemPairingResult::Timeout) => {
            Notice::Text(result_status(false, [0; 6], "", "timed out"))
        }
        Ok(SystemPairingResult::Cancelled) => {
            Notice::Text(result_status(false, [0; 6], "", "cancelled"))
        }
        Ok(SystemPairingResult::NotBleMode) => {
            Notice::Text("Error: the tracker is not in Bluetooth mode".into())
        }
        Err(error) => Notice::Text(format!("Error: {error}")),
    }
}

/// Bond addresses are little-endian as on air; people read them most
/// significant byte first.
fn bond_label(bond: Bond) -> String {
    if bond.this_computer {
        return "This computer".into();
    }
    let mut address = bond.bd_addr;
    address.reverse();
    format_address(address)
}

pub(super) struct BondsPanel {
    hub: Entity<Hub>,
    device: Device,
    /// `None` until the first answer; an error is the hub's message.
    bonds: Option<Result<Vec<Bond>, String>>,
    notice: Notice,
    // Removing the connected host drops the tracker's link, so removal asks
    // first.
    confirming_remove: Option<[u8; 6]>,
    busy: bool,
    /// Drops answers of a refresh that a newer one overtook.
    refresh_seq: u64,
}

impl BondsPanel {
    pub(super) fn new(hub: Entity<Hub>, device: Device, cx: &mut Context<'_, Self>) -> Self {
        let mut panel = Self {
            hub,
            device,
            bonds: None,
            notice: Notice::None,
            confirming_remove: None,
            busy: false,
            refresh_seq: 0,
        };
        panel.refresh(cx);
        panel
    }

    /// Follows the hub's latest view of the device and reloads its bonds,
    /// since its connection may have changed.
    pub(super) fn set_device(&mut self, device: Device, cx: &mut Context<'_, Self>) {
        self.device = device;
        self.confirming_remove = None;
        self.refresh(cx);
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.refresh_seq += 1;
        let seq = self.refresh_seq;
        let Some(client) = self.hub.read(cx).client() else {
            self.bonds = Some(Err(NOT_CONNECTED.into()));
            cx.notify();
            return;
        };
        let device = self.device.clone();
        cx.spawn(async move |this, cx| {
            let bonds = on_runtime(async move { client.device(device).list_bonds().await }).await;
            let _ = this.update(cx, |this, cx| {
                if this.refresh_seq == seq {
                    this.bonds = Some(bonds);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn pair(&mut self, cx: &mut Context<'_, Self>) {
        if self.busy {
            return;
        }
        if matches!(&self.bonds, Some(Ok(bonds)) if bonds.len() >= MAX_BONDS) {
            self.notice = Notice::Full;
            cx.notify();
            return;
        }
        let Some(client) = self.hub.read(cx).client() else {
            self.notice = Notice::Text(format!("Error: {NOT_CONNECTED}"));
            cx.notify();
            return;
        };
        self.busy = true;
        self.notice = Notice::Text(
            "Pairing this computer… keep the tracker nearby (up to two minutes).".into(),
        );
        cx.notify();
        let device = self.device.clone();
        cx.spawn(async move |this, cx| {
            let outcome = on_runtime(async move {
                client
                    .device(device)
                    .pair_system_bluetooth(PAIRING_TIMEOUT_MS)
                    .await
            })
            .await;
            if let Err(error) = &outcome {
                tracing::error!("Failed to pair this computer: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.notice = pairing_notice(outcome);
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn remove(&mut self, bd_addr: [u8; 6], cx: &mut Context<'_, Self>) {
        if self.busy {
            return;
        }
        let Some(client) = self.hub.read(cx).client() else {
            self.notice = Notice::Text(format!("Error: {NOT_CONNECTED}"));
            cx.notify();
            return;
        };
        self.busy = true;
        self.notice = Notice::Text("Removing host…".into());
        cx.notify();
        let device = self.device.clone();
        cx.spawn(async move |this, cx| {
            let outcome =
                on_runtime(async move { client.device(device).remove_bond(bd_addr).await }).await;
            if let Err(error) = &outcome {
                tracing::error!("Failed to remove bond: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.notice = match outcome {
                    Ok(()) => Notice::Text("Host removed".into()),
                    Err(error) => Notice::Text(format!("Error: {error}")),
                };
                this.refresh(cx);
            });
        })
        .detach();
    }
}

impl Render for BondsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = &palette(cx);
        let id = cx.entity_id();
        let section = div().id(("paired-hosts", id)).flex().flex_col().gap_2();
        let muted = |text: String| div().text_xs().italic().text_color(p.fg_muted).child(text);

        let bonds = match &self.bonds {
            None => return section.child(muted("Loading paired hosts…".into())),
            Some(Err(message)) => return section.child(muted(message.clone())),
            Some(Ok(bonds)) => bonds,
        };

        let mut section = section.child(
            div()
                .text_sm()
                .text_color(p.fg)
                .child(format!("Paired hosts · {} of {MAX_BONDS}", bonds.len())),
        );
        for (slot, bond) in bonds.iter().enumerate() {
            let bd_addr = bond.bd_addr;
            let mut name = div().flex().items_center().gap_2().min_w_0().child(
                div()
                    .text_sm()
                    .font_family(MONO_FONT)
                    .text_color(p.fg)
                    .child(bond_label(*bond)),
            );
            if bond.connected {
                name = name.child(div().text_xs().text_color(p.success_ink).child("Connected"));
            }
            let mut actions = div().flex().flex_shrink_0().items_center().gap_1();
            if self.confirming_remove == Some(bd_addr) {
                actions = actions
                    .child(div().text_xs().text_color(p.fg).child(if bond.connected {
                        "Remove this host? The tracker drops its link to it."
                    } else {
                        "Remove this host's bond?"
                    }))
                    .child(
                        button(
                            ("confirm-remove-host", slot),
                            "Remove",
                            ButtonKind::Danger,
                            p,
                        )
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirming_remove = None;
                            this.remove(bd_addr, cx);
                        })),
                    )
                    .child(
                        button(("keep-host", slot), "Keep", ButtonKind::Secondary, p)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming_remove = None;
                                cx.notify();
                            })),
                    );
            } else {
                actions = actions.child(
                    button(("remove-host", slot), "Remove", ButtonKind::Secondary, p)
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirming_remove = Some(bd_addr);
                            cx.notify();
                        })),
                );
            }
            section = section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(name)
                    .child(actions),
            );
        }

        section = section.child(
            div().child(
                button(
                    "pair-this-computer",
                    "Pair this computer",
                    ButtonKind::Secondary,
                    p,
                )
                .small()
                .disabled(self.busy)
                .on_click(cx.listener(|this, _, _, cx| this.pair(cx))),
            ),
        );
        match &self.notice {
            Notice::None => section,
            Notice::Full => section.child(alert(
                AlertKind::Warning,
                p,
                "All bond slots are in use. Remove one first.",
            )),
            Notice::Text(text) => section.child(muted(text.clone())),
        }
    }
}
