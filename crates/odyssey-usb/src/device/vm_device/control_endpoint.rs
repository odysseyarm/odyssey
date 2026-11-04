//! `VmDevice` operations that go over the USB control endpoint instead of the
//! packet stream. They need a direct USB connection (`ctrl_if`) and work in
//! either transport mode, which is why pairing and mode switching live here.

use std::time::Duration;

use anyhow::{Result, anyhow};
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::device::{
    ClearBondResult, DeviceMsg, PairingError, PairingOutcome, StartPairing, TransportMode,
};
use odyssey_protocol::control::{AddBondResult, BondEntry, BondedDevice};
use odyssey_protocol::{PropKind, Props, SetDeviceNameError, SetDeviceNameResult, Version};
use tracing::info;

use super::VmDevice;
use crate::device::control_transfer::{
    self, FLASH_WRITE_REPLY_TIMEOUT, REPLY_TIMEOUT, UndecodableReply,
};
use crate::device::general_settings::GeneralSettings;
use crate::device::usb_bulk_io::find_vendor_interface;
use crate::packets::vm::{ConfigKind, GeneralConfig};

/// Longest a pairing session can run before the device reports a timeout.
const PAIRING_EVENT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long to wait for the reply to a rename, which writes flash.
const SET_NAME_REPLY_TIMEOUT: Duration = Duration::from_secs(3);

fn unexpected(msg: DeviceMsg) -> anyhow::Error {
    anyhow!("unexpected response: {msg:?}")
}

impl VmDevice {
    fn ctrl_iface(&self) -> Result<&Interface> {
        self.ctrl_if
            .as_ref()
            .ok_or_else(|| anyhow!("No control interface available"))
    }

    async fn send_ctrl_msg(&self, msg: DeviceMsg) -> Result<()> {
        let ctrl_if = self.ctrl_iface()?;
        info!(
            "send_ctrl_msg: control_out iface={}",
            ctrl_if.interface_number()
        );
        let result = control_transfer::send(ctrl_if, &msg).await;
        info!(
            "send_ctrl_msg: control_out returned {:?}",
            result.as_ref().map(|()| "ok").map_err(ToString::to_string)
        );
        result
    }

    /// The next control message, whatever it is.
    async fn recv_ctrl_msg(&self, timeout: Duration) -> Result<DeviceMsg> {
        control_transfer::recv_matching(self.ctrl_iface()?, timeout, UndecodableReply::Fail, |_| {
            true
        })
        .await
    }

    /// The next control message `accept` takes; others are discarded.
    async fn recv_ctrl_msg_filtered(
        &self,
        timeout: Duration,
        accept: impl FnMut(&DeviceMsg) -> bool,
    ) -> Result<DeviceMsg> {
        control_transfer::recv_matching(self.ctrl_iface()?, timeout, UndecodableReply::Skip, accept)
            .await
    }

    /// Sends `msg`, then waits for the first reply `accept` takes.
    async fn ctrl_request(
        &self,
        msg: DeviceMsg,
        timeout: Duration,
        accept: impl FnMut(&DeviceMsg) -> bool,
    ) -> Result<DeviceMsg> {
        self.send_ctrl_msg(msg).await?;
        self.recv_ctrl_msg_filtered(timeout, accept).await
    }

    pub async fn read_version(&self) -> Result<Version> {
        self.send_ctrl_msg(DeviceMsg::ReadProp(PropKind::Version))
            .await?;
        match self.recv_ctrl_msg(REPLY_TIMEOUT).await? {
            DeviceMsg::ReadPropResponse(Props::Version(version)) => Ok(version),
            other => Err(unexpected(other)),
        }
    }

    /// Read the net core's release counter from the application, 0 when the net
    /// core reports none.
    ///
    /// Only applications with device protocol 0.1.5 or newer understand the
    /// request; older ones cannot decode it, so callers gate on the version.
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
            other => Err(unexpected(other)),
        }
    }

    /// Read the net core's release counter through the packet stream, the path a
    /// unit reached over Bluetooth through a dongle answers on. Same semantics
    /// and protocol gate as [`Self::read_netcore_release`], which needs a USB
    /// control connection.
    pub async fn read_netcore_release_over_packets(&self) -> Result<u16> {
        match self.read_prop(PropKind::NetCoreRelease).await? {
            Props::NetCoreRelease(release) => Ok(release),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read the UUID from the device via control endpoint.
    pub async fn read_uuid(&self) -> Result<[u8; 6]> {
        self.send_ctrl_msg(DeviceMsg::ReadProp(PropKind::Uuid))
            .await?;
        match self.recv_ctrl_msg(REPLY_TIMEOUT).await? {
            DeviceMsg::ReadPropResponse(Props::Uuid(uuid)) => Ok(uuid),
            other => Err(unexpected(other)),
        }
    }

    /// Get the current transport mode of the device (USB vs BLE).
    pub async fn get_transport_mode(&self) -> Result<TransportMode> {
        match self
            .ctrl_request(DeviceMsg::GetTransportMode(), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::TransportModeStatus(_))
            })
            .await?
        {
            DeviceMsg::TransportModeStatus(mode) => Ok(mode),
            other => Err(unexpected(other)),
        }
    }

    /// Probe the transport mode of a device by issuing a control request on the vendor interface.
    /// This opens a temporary connection to query the device, then closes it.
    pub async fn probe_transport_mode(info: &DeviceInfo) -> Result<TransportMode> {
        let device = info.open().await?;
        let vendor_iface_num = find_vendor_interface(info)?;
        let ctrl_if = device
            .claim_interface(vendor_iface_num)
            .await
            .map_err(|e| anyhow!("Failed to claim control interface: {e}"))?;

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
            other => Err(unexpected(other)),
        }
    }

    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<()> {
        match self
            .ctrl_request(
                DeviceMsg::StartPairing(StartPairing { timeout_ms }),
                REPLY_TIMEOUT,
                |msg| {
                    matches!(
                        msg,
                        DeviceMsg::StartPairingResponse() | DeviceMsg::PairingResult(_)
                    )
                },
            )
            .await?
        {
            DeviceMsg::StartPairingResponse() => Ok(()),
            // Pairing finished immediately; treat as success so the caller can proceed
            DeviceMsg::PairingResult(PairingOutcome::Ok(_addr)) => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    pub async fn wait_pairing_event(&self) -> Result<BondedDevice> {
        loop {
            match self.recv_ctrl_msg(PAIRING_EVENT_TIMEOUT).await? {
                DeviceMsg::PairingResult(PairingOutcome::Ok(device)) => return Ok(device),
                DeviceMsg::PairingResult(PairingOutcome::Err(PairingError::Timeout)) => {
                    return Err(anyhow!("Pairing timeout"));
                }
                DeviceMsg::PairingResult(PairingOutcome::Err(PairingError::Cancelled)) => {
                    return Err(anyhow!("Pairing cancelled"));
                }
                _ => { /* ignore unrelated ctrl msgs */ }
            }
        }
    }

    pub async fn cancel_pairing(&self) -> Result<()> {
        self.send_ctrl_msg(DeviceMsg::CancelPairing()).await
    }

    pub async fn clear_bond(&self) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::ClearBond(), REPLY_TIMEOUT, |msg| {
                matches!(
                    msg,
                    DeviceMsg::ClearBondResponse(_) | DeviceMsg::PairingResult(_)
                )
            })
            .await?
        {
            DeviceMsg::ClearBondResponse(ClearBondResult::Ok()) => Ok(()),
            DeviceMsg::ClearBondResponse(ClearBondResult::Err(_)) => {
                Err(anyhow!("Failed to clear bond"))
            }
            other => Err(unexpected(other)),
        }
    }

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
            DeviceMsg::AddBondResponse(AddBondResult::Err(e)) => {
                Err(anyhow!("AddBond failed: {e:?}"))
            }
            other => Err(unexpected(other)),
        }
    }

    /// Read a single config value from the device via control endpoint.
    pub async fn read_config_ctrl(&self, kind: ConfigKind) -> Result<GeneralConfig> {
        match self
            .ctrl_request(DeviceMsg::ReadConfig(kind), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::ReadConfigResponse(_))
            })
            .await?
        {
            DeviceMsg::ReadConfigResponse(config) => Ok(config),
            other => Err(unexpected(other)),
        }
    }

    /// Write a single config value to the device via control endpoint.
    pub async fn write_config_ctrl(&self, config: GeneralConfig) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::WriteConfig(config), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::WriteConfigAck())
            })
            .await?
        {
            DeviceMsg::WriteConfigAck() => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Flush settings to flash on the device via control endpoint.
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
            other => Err(unexpected(other)),
        }
    }

    /// Request the device to reboot via control endpoint.
    /// Sends Reboot, waits for RebootAck, then the device resets itself.
    pub async fn reboot(&self) -> Result<()> {
        match self
            .ctrl_request(DeviceMsg::Reboot(), REPLY_TIMEOUT, |msg| {
                matches!(msg, DeviceMsg::RebootAck())
            })
            .await?
        {
            DeviceMsg::RebootAck() => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Read all config values via control endpoint (for devices that don't support bulk config).
    pub async fn read_all_config_ctrl(&self) -> Result<GeneralSettings> {
        GeneralSettings::read_with(async |kind| self.read_config_ctrl(kind).await).await
    }

    pub async fn set_name(&self, name: &str) -> Result<()> {
        // Rejected here rather than truncated: silently renaming a device to a
        // prefix of what was asked for is worse than failing.
        let hname: heapless::String<32> = name
            .try_into()
            .map_err(|_overlong| anyhow!("device name is longer than 32 bytes"))?;
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
            )) => Err(anyhow!("device rejected the name as too long")),
            DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Err(
                SetDeviceNameError::Failed,
            )) => Err(anyhow!("device failed to store the name")),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }

    pub async fn set_transport_mode(&self, usb_mode: bool) -> Result<TransportMode> {
        let mode = if usb_mode {
            TransportMode::Usb
        } else {
            TransportMode::Ble
        };
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
            other => Err(unexpected(other)),
        }
    }
}
