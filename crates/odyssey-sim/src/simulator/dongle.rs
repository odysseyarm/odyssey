use std::collections::BTreeMap;

use odyssey_protocol::control::usb_mux::{self, UsbMuxCtrlMsg};
use odyssey_protocol::control::{AddBondError, AddBondResult, BondEntry, BondedDevice};
use odyssey_protocol::mux::{self, MuxMsg};
use odyssey_protocol::{DeviceName, Version};
use tokio::sync::mpsc;

use super::model::{Errors, send};
use super::{DongleState, Uuid};

pub(crate) struct Dongle {
    pub(crate) uuid: Uuid,
    pub(crate) plugged: bool,
    pub(crate) usb_generation: u64,
    pub(crate) mux: Option<mpsc::Sender<MuxMsg>>,
    pub(crate) control: Option<mpsc::Sender<UsbMuxCtrlMsg>>,
    pub(crate) bonds: BTreeMap<Uuid, BondEntry>,
    pub(crate) bond_names: BTreeMap<Uuid, DeviceName>,
    pub(crate) pairing_deadline: Option<u64>,
    /// Units connected over BLE, by address.
    pub(crate) connected: Vec<Uuid>,
    list_subscribed: bool,
}

impl Dongle {
    pub(crate) fn new(uuid: Uuid) -> Self {
        Self {
            uuid,
            plugged: false,
            usb_generation: 0,
            mux: None,
            control: None,
            bonds: BTreeMap::new(),
            bond_names: BTreeMap::new(),
            pairing_deadline: None,
            connected: Vec::new(),
            list_subscribed: false,
        }
    }

    pub(crate) fn write(&self, state: &mut DongleState) {
        state.uuid = self.uuid;
        state.usb = self.plugged;
        state.usb_generation = self.usb_generation;
        state.pairing = self.pairing_deadline.is_some();
        state.bonds = self.bonds.len();
    }

    pub(crate) fn disconnect_mux(&mut self) {
        self.mux = None;
        self.control = None;
        self.list_subscribed = false;
    }

    pub(crate) fn set_plugged(&mut self, plugged: bool) {
        if self.plugged != plugged {
            self.usb_generation = self.usb_generation.wrapping_add(1);
        }
        self.plugged = plugged;
        if !plugged {
            self.disconnect_mux();
            self.pairing_deadline = None;
        }
    }

    pub(crate) fn event(&mut self, event: UsbMuxCtrlMsg, errors: &mut Errors) {
        if let Err(error) = send(self.control.as_ref(), event) {
            errors.report(error);
        }
    }

    pub(crate) fn cancel_pairing(&mut self, error: usb_mux::PairingError, errors: &mut Errors) {
        if self.pairing_deadline.take().is_some() {
            self.event(
                UsbMuxCtrlMsg::PairingResult(usb_mux::PairingOutcome::Err(error)),
                errors,
            );
        }
    }

    pub(crate) fn has_room_for(&self, unit: &Uuid) -> bool {
        self.bonds.len() < mux::MAX_DEVICES || self.bonds.contains_key(unit)
    }

    fn list(&self, errors: &mut Errors) {
        let mut devices = odyssey_protocol::List::default();
        for unit in &self.connected {
            let _ = devices.0.push(*unit);
        }
        if let Err(error) = send(self.mux.as_ref(), MuxMsg::DevicesSnapshot(devices)) {
            errors.report(error);
        }
    }

    /// Record which units are connected, telling a subscribed host of a change.
    pub(crate) fn set_connected<'a>(
        &mut self,
        units: impl Iterator<Item = &'a Uuid> + Clone,
        errors: &mut Errors,
    ) {
        if self.connected.iter().eq(units.clone()) {
            return;
        }
        self.connected.clear();
        self.connected.extend(units);
        if self.list_subscribed {
            self.list(errors);
        }
    }

    /// Every mux request except `SendTo`, which the model routes to the unit.
    pub(crate) fn mux_message(&mut self, message: MuxMsg, errors: &mut Errors) {
        match message {
            MuxMsg::RequestDevices() => self.list(errors),
            MuxMsg::SubscribeDeviceList() => {
                self.list_subscribed = true;
                self.list(errors);
            }
            MuxMsg::UnsubscribeDeviceList() => self.list_subscribed = false,
            MuxMsg::ReadVersion() => {
                if let Err(error) = send(
                    self.mux.as_ref(),
                    MuxMsg::ReadVersionResponse(mux::Version::new([0, 0, 0])),
                ) {
                    errors.report(error);
                }
            }
            MuxMsg::AccessoryRequest(request) => {
                let completion = odyssey_protocol::accessory::Completion {
                    id: request.id,
                    result: Err(odyssey_protocol::accessory::Error::Unsupported),
                };
                if let Err(error) = send(self.mux.as_ref(), MuxMsg::AccessoryCompletion(completion))
                {
                    errors.report(error);
                }
            }
            other => errors.unsupported(other),
        }
    }

    pub(crate) fn control_message(
        &mut self,
        message: UsbMuxCtrlMsg,
        now_ms: u64,
        errors: &mut Errors,
    ) {
        match message {
            UsbMuxCtrlMsg::ReadVersion() => {
                self.event(
                    UsbMuxCtrlMsg::ReadVersionResponse(Version::new([0, 0, 0])),
                    errors,
                );
            }
            UsbMuxCtrlMsg::ListBonds() => {
                let mut bonds = odyssey_protocol::List(Default::default());
                for uuid in self.bonds.keys() {
                    let _ = bonds.0.push(BondedDevice {
                        uuid: *uuid,
                        name: self.bond_names.get(uuid).cloned().unwrap_or_default(),
                    });
                }
                self.event(UsbMuxCtrlMsg::ListBondsResponse(bonds), errors);
            }
            UsbMuxCtrlMsg::ClearBonds() => {
                self.pairing_deadline = None;
                self.bonds.clear();
                self.bond_names.clear();
                self.event(
                    UsbMuxCtrlMsg::ClearBondsResponse(usb_mux::ClearBondsResult::Ok()),
                    errors,
                );
            }
            UsbMuxCtrlMsg::StartPairing(parameters) => {
                self.pairing_deadline =
                    Some(now_ms.saturating_add(u64::from(parameters.timeout_ms)));
                self.event(UsbMuxCtrlMsg::StartPairingResponse(), errors);
            }
            UsbMuxCtrlMsg::CancelPairing() => {
                self.cancel_pairing(usb_mux::PairingError::Cancelled, errors);
            }
            UsbMuxCtrlMsg::AddBond(entry) => {
                let result = if self.has_room_for(&entry.bd_addr) {
                    self.bonds.insert(entry.bd_addr, entry);
                    AddBondResult::Ok()
                } else {
                    AddBondResult::Err(AddBondError::Full)
                };
                self.event(UsbMuxCtrlMsg::AddBondResponse(result), errors);
            }
            UsbMuxCtrlMsg::UpdateBondName { uuid, name } => {
                if self.bonds.contains_key(&uuid) {
                    self.bond_names.insert(uuid, name);
                }
            }
            other => errors.unsupported(other),
        }
    }
}
