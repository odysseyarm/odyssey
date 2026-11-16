//! `VmDevice` operations that go over the USB control endpoint instead of the
//! packet stream. They need a direct control link and work in either transport
//! mode, which is why pairing and mode switching live here.

use std::time::Duration;

use nusb::DeviceInfo;
use odyssey_protocol::control::device::{
    ClearBondResult, DeviceMsg, PairingOutcome, StartPairing, TransportMode,
};
use odyssey_protocol::control::{AddBondResult, BondEntry, BondedDevice};
use odyssey_protocol::{PropKind, Props, SetDeviceNameError, SetDeviceNameResult, Version};
use tracing::info;

use super::VmDevice;
use crate::device::control_transfer::{
    self, ControlLink, FLASH_WRITE_REPLY_TIMEOUT, REPLY_TIMEOUT, UndecodableReply,
};
use crate::device::general_settings::GeneralSettings;
use crate::device::usb_bulk_io::find_vendor_interface;
use crate::packets::vm::{ConfigKind, GeneralConfig};
use crate::{Error, Result};

/// Longest a pairing session can run before the device reports a timeout.
const PAIRING_EVENT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long to wait for the reply to a rename, which writes flash.
const SET_NAME_REPLY_TIMEOUT: Duration = Duration::from_secs(3);

fn unexpected(msg: &DeviceMsg) -> Error {
    Error::unexpected(msg)
}

impl VmDevice {
    fn ctrl_iface(&self) -> Result<&ControlLink<DeviceMsg>> {
        self.ctrl_if.as_ref().ok_or(Error::NoControlInterface)
    }

    async fn send_ctrl_msg(&self, msg: DeviceMsg) -> Result<()> {
        let ctrl_if = self.ctrl_iface()?;
        info!("send_ctrl_msg: control_out");
        let result = ctrl_if.send(&msg).await;
        info!(
            "send_ctrl_msg: control_out returned {:?}",
            result.as_ref().map(|()| "ok").map_err(ToString::to_string)
        );
        result
    }

    /// The next control message `accept` takes. Unrelated pairing results are kept
    /// for the event waiter and everything else is dropped.
    async fn recv_ctrl_msg_filtered(
        &self,
        timeout: Duration,
        pending_pairing: &mut Option<PairingOutcome>,
        mut accept: impl FnMut(&DeviceMsg) -> bool,
    ) -> Result<DeviceMsg> {
        self.ctrl_iface()?
            .recv_matching(timeout, UndecodableReply::Skip, |msg| {
                if accept(msg) {
                    return true;
                }
                if let DeviceMsg::PairingResult(outcome) = msg {
                    *pending_pairing = Some(outcome.clone());
                }
                false
            })
            .await
    }

    /// Sends `msg`, then waits for the first reply `accept` takes.
    async fn ctrl_request(
        &self,
        msg: DeviceMsg,
        timeout: Duration,
        accept: impl FnMut(&DeviceMsg) -> bool,
    ) -> Result<DeviceMsg> {
        let mut pending = self.ctrl.lock().await;
        self.send_ctrl_msg(msg).await?;
        self.recv_ctrl_msg_filtered(timeout, &mut pending, accept)
            .await
    }

    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-property errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn read_version(&self) -> Result<Version> {
        match self
            .ctrl_request(
                DeviceMsg::ReadProp(PropKind::Version),
                REPLY_TIMEOUT,
                |msg| matches!(msg, DeviceMsg::ReadPropResponse(Props::Version(_))),
            )
            .await?
        {
            DeviceMsg::ReadPropResponse(Props::Version(version)) => Ok(version),
            other => Err(unexpected(&other)),
        }
    }

    /// Read the net core's release counter from the application, 0 when the net
    /// core reports none.
    ///
    /// Needs device protocol 0.1.5 or newer. Older applications cannot decode the
    /// request, so callers gate on the version.
    ///
    /// Reads netcore release over control endpoint.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-property errors.
    /// Since: Odyssey Desktop 0.6.4.
    pub async fn read_netcore_release(&self) -> Result<u16> {
        match self
            .ctrl_request(
                DeviceMsg::ReadProp(PropKind::NetCoreRelease),
                REPLY_TIMEOUT,
                |msg| matches!(msg, DeviceMsg::ReadPropResponse(Props::NetCoreRelease(_))),
            )
            .await?
        {
            DeviceMsg::ReadPropResponse(Props::NetCoreRelease(release)) => Ok(release),
            other => Err(unexpected(&other)),
        }
    }

    /// Read the net core's release counter through the packet stream, the path a
    /// unit reached over Bluetooth through a dongle answers on. Same semantics
    /// and protocol gate as [`Self::read_netcore_release`], which needs a USB
    /// control connection.
    ///
    /// Reads netcore release using packet protocol.
    ///
    /// # Errors
    /// Returns packet transport, timeout, or unexpected-property errors.
    /// Since: Odyssey Desktop 0.6.5.
    pub async fn read_netcore_release_over_packets(&self) -> Result<u16> {
        match self.read_prop(PropKind::NetCoreRelease).await? {
            Props::NetCoreRelease(release) => Ok(release),
            other => Err(Error::unexpected(&other)),
        }
    }

    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-property errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn read_uuid(&self) -> Result<[u8; 6]> {
        match self
            .ctrl_request(DeviceMsg::ReadProp(PropKind::Uuid), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::ReadPropResponse(Props::Uuid(_)))
            })
            .await?
        {
            DeviceMsg::ReadPropResponse(Props::Uuid(uuid)) => Ok(uuid),
            other => Err(unexpected(&other)),
        }
    }

    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn get_transport_mode(&self) -> Result<TransportMode> {
        match self
            .ctrl_request(DeviceMsg::GetTransportMode(), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::TransportModeStatus(_))
            })
            .await?
        {
            DeviceMsg::TransportModeStatus(mode) => Ok(mode),
            other => Err(unexpected(&other)),
        }
    }

    /// Asks for the transport mode over the vendor interface of a device that
    /// has no `VmDevice`, opening it for this one request.
    ///
    /// Probes device transport mode over its control endpoint.
    ///
    /// # Errors
    /// Returns USB, interface discovery, claim, control-transfer, or decode errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn probe_transport_mode(info: &DeviceInfo) -> Result<TransportMode> {
        let device = info.open().await?;
        let vendor_iface_num = find_vendor_interface(info)?;
        let ctrl_if = device.claim_interface(vendor_iface_num).await?;

        control_transfer::send(&ctrl_if, &DeviceMsg::GetTransportMode()).await?;

        // Unlike the replies to a connected device, an undecodable one here
        // means the device does not speak this protocol at all.
        let reply = control_transfer::recv_matching(
            &ctrl_if,
            REPLY_TIMEOUT,
            UndecodableReply::Fail,
            |msg| matches!(msg, DeviceMsg::TransportModeStatus(_)),
        )
        .await?;
        match reply {
            DeviceMsg::TransportModeStatus(mode) => Ok(mode),
            other => Err(unexpected(&other)),
        }
    }

    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<()> {
        let mut pending = self.ctrl.lock().await;
        *pending = None;
        self.send_ctrl_msg(DeviceMsg::StartPairing(StartPairing { timeout_ms }))
            .await?;
        match self
            .recv_ctrl_msg_filtered(REPLY_TIMEOUT, &mut pending, |msg| {
                matches!(
                    msg,
                    DeviceMsg::StartPairingResponse() | DeviceMsg::PairingResult(_)
                )
            })
            .await?
        {
            DeviceMsg::StartPairingResponse() => Ok(()),
            // Retain an immediate completion for wait_pairing_event.
            DeviceMsg::PairingResult(outcome @ PairingOutcome::Ok(_)) => {
                *pending = Some(outcome);
                Ok(())
            }
            other => Err(unexpected(&other)),
        }
    }

    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn wait_pairing_event(&self) -> Result<BondedDevice> {
        let mut pending = self.ctrl.lock().await;
        let outcome = match pending.take() {
            Some(outcome) => outcome,
            None => match self
                .recv_ctrl_msg_filtered(PAIRING_EVENT_TIMEOUT, &mut pending, |msg| {
                    matches!(msg, DeviceMsg::PairingResult(_))
                })
                .await?
            {
                DeviceMsg::PairingResult(outcome) => outcome,
                other => return Err(unexpected(&other)),
            },
        };
        match outcome {
            PairingOutcome::Ok(device) => Ok(device),
            PairingOutcome::Err(error) => Err(Error::Pairing(error)),
        }
    }

    /// Cancels device pairing.
    ///
    /// # Errors
    /// Returns control-transfer errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn cancel_pairing(&self) -> Result<()> {
        self.send_ctrl_msg(DeviceMsg::CancelPairing()).await
    }

    /// Clears stored device bond.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn clear_bond(&self) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::ClearBond(), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::ClearBondResponse(_))
            })
            .await?
        {
            DeviceMsg::ClearBondResponse(ClearBondResult::Ok()) => Ok(()),
            DeviceMsg::ClearBondResponse(ClearBondResult::Err(_)) => {
                Err(Error::Refused("clearing the bond"))
            }
            other => Err(unexpected(&other)),
        }
    }

    /// Adds a bonded device.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, flash-write, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn add_bond(&self, entry: BondEntry) -> Result<()> {
        match self
            .ctrl_request(
                DeviceMsg::AddBond(entry),
                FLASH_WRITE_REPLY_TIMEOUT,
                |msg| matches!(msg, DeviceMsg::AddBondResponse(_)),
            )
            .await?
        {
            DeviceMsg::AddBondResponse(AddBondResult::Ok()) => Ok(()),
            DeviceMsg::AddBondResponse(AddBondResult::Err(e)) => Err(Error::BondRejected(e)),
            other => Err(unexpected(&other)),
        }
    }

    /// Reads configuration using control endpoint.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn read_config_ctrl(&self, kind: ConfigKind) -> Result<GeneralConfig> {
        match self
            .ctrl_request(DeviceMsg::ReadConfig(kind), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::ReadConfigResponse(_))
            })
            .await?
        {
            DeviceMsg::ReadConfigResponse(config) => Ok(config),
            other => Err(unexpected(&other)),
        }
    }

    /// Writes configuration using control endpoint.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn write_config_ctrl(&self, config: GeneralConfig) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::WriteConfig(config), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::WriteConfigAck())
            })
            .await?
        {
            DeviceMsg::WriteConfigAck() => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// Persists the current settings to flash.
    ///
    /// Persists current settings to device flash.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn flash_settings_ctrl(&self) -> Result<()> {
        match self
            .ctrl_request(
                DeviceMsg::FlashSettings(),
                FLASH_WRITE_REPLY_TIMEOUT,
                |msg| matches!(msg, DeviceMsg::FlashSettingsAck()),
            )
            .await?
        {
            DeviceMsg::FlashSettingsAck() => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// Returns once the device acknowledges. It resets itself after the ack.
    ///
    /// Reboots device and waits for acknowledgment.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn reboot(&self) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::Reboot(), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::RebootAck())
            })
            .await?
        {
            DeviceMsg::RebootAck() => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// For devices that do not answer config reads on the packet stream.
    ///
    /// Reads all configuration sections through control endpoint.
    ///
    /// # Errors
    /// Returns the first section-read error.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn read_all_config_ctrl(&self) -> Result<GeneralSettings> {
        GeneralSettings::read_with(|kind| self.read_config_ctrl(kind)).await
    }

    /// Sets device name without truncating rejected input.
    ///
    /// # Errors
    /// Returns invalid-name, control-transfer, timeout, or device-rejection errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_name(&self, name: &str) -> Result<()> {
        // Reject instead of truncating. Silently renaming a device to a prefix of
        // what was asked for is worse than failing.
        let hname: heapless::String<32> = name
            .try_into()
            .map_err(|_overlong| Error::InvalidArgument("device name longer than 32 bytes"))?;
        match self
            .ctrl_request(
                DeviceMsg::SetDeviceName(hname.into()),
                SET_NAME_REPLY_TIMEOUT,
                |msg| matches!(msg, DeviceMsg::SetDeviceNameResponse(_)),
            )
            .await?
        {
            DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Ok()) => Ok(()),
            DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Err(
                SetDeviceNameError::TooLong,
            )) => Err(Error::Refused("the name is too long")),
            DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Err(
                SetDeviceNameError::Failed,
            )) => Err(Error::Refused("storing the name")),
            other => Err(unexpected(&other)),
        }
    }

    /// Asks the device to switch to `mode` and returns the mode it reports back.
    ///
    /// Changes active device transport mode.
    ///
    /// # Errors
    /// Returns control-transfer, timeout, or unexpected-reply errors.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_transport_mode(&self, mode: TransportMode) -> Result<TransportMode> {
        info!(
            "set_transport_mode: ctrl_if present={}, sending {:?}",
            self.ctrl_if.is_some(),
            mode
        );
        match self
            .ctrl_request(DeviceMsg::SetTransportMode(mode), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::TransportModeStatus(_))
            })
            .await?
        {
            DeviceMsg::TransportModeStatus(confirmed) => {
                info!("set_transport_mode: got TransportModeStatus({confirmed:?})");
                Ok(confirmed)
            }
            other => Err(unexpected(&other)),
        }
    }
}
