use std::time::Duration;

use anyhow::{Result, anyhow};
use nusb::DeviceInfo;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};
use odyssey_protocol::control::device::TransportMode;
use opencv_ros_camera::RosOpenCvIntrinsics;

use super::VmDevice;
use crate::device::general_settings::GeneralSettings;
use crate::packets::vm::{ConfigKind, GeneralConfig};

impl VmDevice {
    // USB Device control over EP0 (Interface recipient)
    const USB_DEVICE_CTRL_REQ_SEND: u8 = 0x30;
    const USB_DEVICE_CTRL_REQ_RECV: u8 = 0x31;

    async fn send_ctrl_msg(&self, msg: odyssey_protocol::control::device::DeviceMsg) -> Result<()> {
        let ctrl_if = self
            .ctrl_if
            .as_ref()
            .ok_or_else(|| anyhow!("No control interface available"))?;
        let data = postcard::to_allocvec(&msg).map_err(|e| anyhow!("ctrl encode failed: {e}"))?;
        tracing::info!(
            "send_ctrl_msg: control_out iface={} len={}",
            ctrl_if.interface_number(),
            data.len()
        );
        let result = ctrl_if
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: Self::USB_DEVICE_CTRL_REQ_SEND,
                    value: 0,
                    index: u16::from(ctrl_if.interface_number()),
                    data: &data,
                },
                Duration::from_millis(1000),
            )
            .await
            .map_err(|e| anyhow!("ctrl send failed: {e}"));
        tracing::info!(
            "send_ctrl_msg: control_out returned {:?}",
            result.as_ref().map(|()| "ok").map_err(ToString::to_string)
        );
        result
    }

    async fn recv_ctrl_msg(
        &self,
        timeout: Duration,
    ) -> Result<odyssey_protocol::control::device::DeviceMsg> {
        let ctrl_if = self
            .ctrl_if
            .as_ref()
            .ok_or_else(|| anyhow!("No control interface available"))?;
        let idx = u16::from(ctrl_if.interface_number());
        let start = std::time::Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!("timeout waiting for ctrl event"));
            }
            let slice = (timeout - elapsed).min(Duration::from_millis(500));
            match ctrl_if
                .control_in(
                    ControlIn {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Interface,
                        request: Self::USB_DEVICE_CTRL_REQ_RECV,
                        value: 0,
                        index: idx,
                        length: 256,
                    },
                    slice,
                )
                .await
            {
                Ok(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    break postcard::from_bytes::<odyssey_protocol::control::device::DeviceMsg>(
                        &data,
                    )
                    .map_err(|e| anyhow!("ctrl decode failed: {e}"));
                }
                Err(TransferError::Cancelled) => continue, // poll-slice timeout, retry
                Err(e) => return Err(anyhow!("recv_ctrl_msg control_in failed: {e:?}")),
            }
        }
    }

    async fn recv_ctrl_msg_filtered<F>(
        &self,
        timeout: Duration,
        mut accept: F,
    ) -> Result<odyssey_protocol::control::device::DeviceMsg>
    where
        F: FnMut(&odyssey_protocol::control::device::DeviceMsg) -> bool,
    {
        let ctrl_if = self
            .ctrl_if
            .as_ref()
            .ok_or_else(|| anyhow!("No control interface available"))?;
        let idx = u16::from(ctrl_if.interface_number());
        let start = std::time::Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!("timeout waiting for ctrl event"));
            }
            let slice = (timeout - elapsed).min(Duration::from_millis(500));
            let reply = match ctrl_if
                .control_in(
                    ControlIn {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Interface,
                        request: Self::USB_DEVICE_CTRL_REQ_RECV,
                        value: 0,
                        index: idx,
                        length: 256,
                    },
                    slice,
                )
                .await
            {
                Ok(data) => data,
                Err(TransferError::Cancelled) => continue, // poll-slice timeout, retry
                Err(e) => return Err(anyhow!("recv_ctrl_msg_filtered control_in failed: {e:?}")),
            };

            if reply.is_empty() {
                continue;
            }

            let msg = match postcard::from_bytes::<odyssey_protocol::control::device::DeviceMsg>(
                &reply,
            ) {
                Ok(msg) => msg,
                Err(e) => {
                    if start.elapsed() < timeout {
                        continue;
                    }
                    return Err(anyhow!("ctrl decode failed: {e}"));
                }
            };

            if accept(&msg) {
                return Ok(msg);
            }
        }
    }

    pub async fn read_version(&self) -> Result<odyssey_protocol::Version> {
        use odyssey_protocol::control::device::DeviceMsg;
        use odyssey_protocol::{PropKind, Props};
        self.send_ctrl_msg(DeviceMsg::ReadProp(PropKind::Version))
            .await?;
        match self.recv_ctrl_msg(Duration::from_secs(2)).await? {
            DeviceMsg::ReadPropResponse(Props::Version(version)) => Ok(version),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read the net core's release counter from the application, 0 when the net
    /// core reports none.
    ///
    /// Only applications with device protocol 0.1.5 or newer understand the
    /// request; older ones cannot decode it, so callers gate on the version.
    pub async fn read_netcore_release(&self) -> Result<u16> {
        use odyssey_protocol::control::device::DeviceMsg;
        use odyssey_protocol::{PropKind, Props};
        self.send_ctrl_msg(DeviceMsg::ReadProp(PropKind::NetCoreRelease))
            .await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::ReadPropResponse(Props::NetCoreRelease(_)))
            })
            .await?
        {
            DeviceMsg::ReadPropResponse(Props::NetCoreRelease(release)) => Ok(release),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read the net core's release counter through the packet stream, the path a
    /// unit reached over Bluetooth through a dongle answers on. Same semantics
    /// and protocol gate as [`Self::read_netcore_release`], which needs a USB
    /// control connection.
    pub async fn read_netcore_release_over_packets(&self) -> Result<u16> {
        use odyssey_protocol::{PropKind, Props};
        match self.read_prop(PropKind::NetCoreRelease).await? {
            Props::NetCoreRelease(release) => Ok(release),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read the UUID from the device via control endpoint.
    pub async fn read_uuid(&self) -> Result<[u8; 6]> {
        use odyssey_protocol::control::device::DeviceMsg;
        use odyssey_protocol::{PropKind, Props};
        self.send_ctrl_msg(DeviceMsg::ReadProp(PropKind::Uuid))
            .await?;
        match self.recv_ctrl_msg(Duration::from_secs(2)).await? {
            DeviceMsg::ReadPropResponse(Props::Uuid(uuid)) => Ok(uuid),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Get the current transport mode of the device (USB vs BLE).
    pub async fn get_transport_mode(&self) -> Result<TransportMode> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::GetTransportMode()).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::TransportModeStatus(_))
            })
            .await?
        {
            DeviceMsg::TransportModeStatus(mode) => Ok(mode),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Probe the transport mode of a device by issuing a control request on the vendor interface.
    /// This opens a temporary connection to query the device, then closes it.
    pub async fn probe_transport_mode(info: &DeviceInfo) -> Result<TransportMode> {
        use odyssey_protocol::control::device::DeviceMsg;

        // Open the device temporarily
        let device = info.open().await?;

        // Find the vendor-specific interface (class 0xFF)
        let vendor_iface_num = info
            .interfaces()
            .find(|iface| iface.class() == 0xFF)
            .map(nusb::InterfaceInfo::interface_number)
            .ok_or_else(|| anyhow!("No vendor interface (class 0xFF) found"))?;

        // Claim the control interface
        let ctrl_if = device
            .claim_interface(vendor_iface_num)
            .await
            .map_err(|e| anyhow!("Failed to claim control interface: {e}"))?;

        // Send GetTransportMode request
        let msg = DeviceMsg::GetTransportMode();
        let data = postcard::to_allocvec(&msg).map_err(|e| anyhow!("ctrl encode failed: {e}"))?;

        ctrl_if
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: Self::USB_DEVICE_CTRL_REQ_SEND,
                    value: 0,
                    index: u16::from(ctrl_if.interface_number()),
                    data: &data,
                },
                Duration::from_millis(1000),
            )
            .await
            .map_err(|e| anyhow!("ctrl send failed: {e}"))?;

        // Receive TransportModeStatus response
        let idx = u16::from(ctrl_if.interface_number());
        let timeout = Duration::from_secs(2);
        let start = std::time::Instant::now();

        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!("timeout waiting for transport mode response"));
            }
            let slice = (timeout - elapsed).min(Duration::from_millis(500));

            match ctrl_if
                .control_in(
                    ControlIn {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Interface,
                        request: Self::USB_DEVICE_CTRL_REQ_RECV,
                        value: 0,
                        index: idx,
                        length: 256,
                    },
                    slice,
                )
                .await
            {
                Ok(data) if !data.is_empty() => {
                    match postcard::from_bytes::<DeviceMsg>(&data) {
                        Ok(DeviceMsg::TransportModeStatus(mode)) => return Ok(mode),
                        Ok(_) => continue, // Ignore other messages
                        Err(e) => return Err(anyhow!("ctrl decode failed: {e}")),
                    }
                }
                Ok(_) => continue,                         // Empty response, retry
                Err(TransferError::Cancelled) => continue, // poll-slice timeout, retry
                Err(e) => return Err(anyhow!("get_transport_mode control_in failed: {e:?}")),
            }
        }
    }
    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<()> {
        use odyssey_protocol::control::device::{DeviceMsg, StartPairing};
        self.send_ctrl_msg(DeviceMsg::StartPairing(StartPairing { timeout_ms }))
            .await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::StartPairingResponse())
                    || matches!(msg, DeviceMsg::PairingResult(_))
            })
            .await?
        {
            DeviceMsg::StartPairingResponse() => Ok(()),
            DeviceMsg::PairingResult(odyssey_protocol::control::device::PairingOutcome::Ok(
                _addr,
            )) => {
                // Pairing finished immediately; treat as success so the caller can proceed
                Ok(())
            }
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    pub async fn wait_pairing_event(&self) -> Result<odyssey_protocol::control::BondedDevice> {
        use odyssey_protocol::control::device::{DeviceMsg, PairingError};
        loop {
            match self.recv_ctrl_msg(Duration::from_secs(120)).await? {
                DeviceMsg::PairingResult(
                    odyssey_protocol::control::device::PairingOutcome::Ok(device),
                ) => return Ok(device),
                DeviceMsg::PairingResult(
                    odyssey_protocol::control::device::PairingOutcome::Err(PairingError::Timeout),
                ) => return Err(anyhow!("Pairing timeout")),
                DeviceMsg::PairingResult(
                    odyssey_protocol::control::device::PairingOutcome::Err(PairingError::Cancelled),
                ) => return Err(anyhow!("Pairing cancelled")),
                _ => { /* ignore unrelated ctrl msgs */ }
            }
        }
    }

    pub async fn cancel_pairing(&self) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::CancelPairing()).await
    }

    pub async fn clear_bond(&self) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::ClearBond()).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::ClearBondResponse(_))
                    || matches!(msg, DeviceMsg::PairingResult(_))
            })
            .await?
        {
            DeviceMsg::ClearBondResponse(
                odyssey_protocol::control::device::ClearBondResult::Ok(),
            ) => Ok(()),
            DeviceMsg::ClearBondResponse(
                odyssey_protocol::control::device::ClearBondResult::Err(_),
            ) => Err(anyhow!("Failed to clear bond")),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    pub async fn add_bond(&self, entry: odyssey_protocol::control::BondEntry) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::AddBond(entry)).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(5), |msg| {
                matches!(msg, DeviceMsg::AddBondResponse(_))
            })
            .await?
        {
            DeviceMsg::AddBondResponse(odyssey_protocol::control::AddBondResult::Ok()) => Ok(()),
            DeviceMsg::AddBondResponse(odyssey_protocol::control::AddBondResult::Err(e)) => {
                Err(anyhow!("AddBond failed: {e:?}"))
            }
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read a single config value from the device via control endpoint.
    pub async fn read_config_ctrl(&self, kind: ConfigKind) -> Result<GeneralConfig> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::ReadConfig(kind)).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::ReadConfigResponse(_))
            })
            .await?
        {
            DeviceMsg::ReadConfigResponse(config) => Ok(config),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Write a single config value to the device via control endpoint.
    pub async fn write_config_ctrl(&self, config: GeneralConfig) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::WriteConfig(config)).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::WriteConfigAck())
            })
            .await?
        {
            DeviceMsg::WriteConfigAck() => Ok(()),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Flush settings to flash on the device via control endpoint.
    pub async fn flash_settings_ctrl(&self) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::FlashSettings()).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(5), |msg| {
                matches!(msg, DeviceMsg::FlashSettingsAck())
            })
            .await?
        {
            DeviceMsg::FlashSettingsAck() => Ok(()),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Request the device to reboot via control endpoint.
    /// Sends Reboot, waits for RebootAck, then the device resets itself.
    pub async fn reboot(&self) -> Result<()> {
        use odyssey_protocol::control::device::DeviceMsg;
        self.send_ctrl_msg(DeviceMsg::Reboot()).await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::RebootAck())
            })
            .await?
        {
            DeviceMsg::RebootAck() => Ok(()),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    /// Read all config values via control endpoint (for devices that don't support bulk config).
    pub async fn read_all_config_ctrl(&self) -> Result<GeneralSettings> {
        let impact_threshold = self.read_config_ctrl(ConfigKind::ImpactThreshold).await?;
        let suppress_ms = self.read_config_ctrl(ConfigKind::SuppressMs).await?;
        let accel_config = self.read_config_ctrl(ConfigKind::AccelConfig).await?;
        let gyro_config = self.read_config_ctrl(ConfigKind::GyroConfig).await?;
        let camera_model_nf = self.read_config_ctrl(ConfigKind::CameraModelNf).await?;
        let camera_model_wf = self.read_config_ctrl(ConfigKind::CameraModelWf).await?;
        let stereo_iso = self.read_config_ctrl(ConfigKind::StereoIso).await?;

        Ok(GeneralSettings {
            impact_threshold: match impact_threshold {
                GeneralConfig::ImpactThreshold(val) => val,
                _ => anyhow::bail!("Unexpected config variant for ImpactThreshold"),
            },
            suppress_ms: match suppress_ms {
                GeneralConfig::SuppressMs(val) => val,
                _ => anyhow::bail!("Unexpected config variant for SuppressMs"),
            },
            accel_config: match accel_config {
                GeneralConfig::AccelConfig(val) => val,
                _ => anyhow::bail!("Unexpected config variant for AccelConfig"),
            },
            gyro_config: match gyro_config {
                GeneralConfig::GyroConfig(val) => val,
                _ => anyhow::bail!("Unexpected config variant for GyroConfig"),
            },
            camera_model_nf: match camera_model_nf {
                GeneralConfig::CameraModelNf(val) => RosOpenCvIntrinsics::try_from(val)
                    .map_err(|e| anyhow!("invalid camera model: {e}"))?,
                _ => anyhow::bail!("Unexpected config variant for CameraModelNf"),
            },
            camera_model_wf: match camera_model_wf {
                GeneralConfig::CameraModelWf(val) => RosOpenCvIntrinsics::try_from(val)
                    .map_err(|e| anyhow!("invalid camera model: {e}"))?,
                _ => anyhow::bail!("Unexpected config variant for CameraModelWf"),
            },
            stereo_iso: match stereo_iso {
                GeneralConfig::StereoIso(val) => val,
                _ => anyhow::bail!("Unexpected config variant for StereoIso"),
            },
        })
    }

    pub async fn set_name(&self, name: &str) -> Result<()> {
        use odyssey_protocol::SetDeviceNameError;
        use odyssey_protocol::control::device::DeviceMsg;
        // Rejected here rather than truncated: silently renaming a device to a
        // prefix of what was asked for is worse than failing.
        let hname: heapless::String<32> = name
            .try_into()
            .map_err(|_| anyhow!("device name is longer than 32 bytes"))?;
        self.send_ctrl_msg(DeviceMsg::SetDeviceName(hname.into()))
            .await?;
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(3), |msg| {
                matches!(msg, DeviceMsg::SetDeviceNameResponse(_))
            })
            .await?
        {
            DeviceMsg::SetDeviceNameResponse(odyssey_protocol::SetDeviceNameResult::Ok()) => Ok(()),
            DeviceMsg::SetDeviceNameResponse(odyssey_protocol::SetDeviceNameResult::Err(
                SetDeviceNameError::TooLong,
            )) => Err(anyhow!("device rejected the name as too long")),
            DeviceMsg::SetDeviceNameResponse(odyssey_protocol::SetDeviceNameResult::Err(
                SetDeviceNameError::Failed,
            )) => Err(anyhow!("device failed to store the name")),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }

    pub async fn set_transport_mode(&self, usb_mode: bool) -> Result<TransportMode> {
        use odyssey_protocol::control::device::{DeviceMsg, TransportMode};
        let mode = if usb_mode {
            TransportMode::Usb
        } else {
            TransportMode::Ble
        };
        tracing::info!(
            "set_transport_mode: ctrl_if present={}, sending {:?}",
            self.ctrl_if.is_some(),
            mode
        );
        self.send_ctrl_msg(DeviceMsg::SetTransportMode(mode))
            .await?;
        tracing::info!(
            "set_transport_mode: send_ctrl_msg done, waiting for TransportModeStatus response"
        );
        match self
            .recv_ctrl_msg_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, DeviceMsg::TransportModeStatus(_))
            })
            .await?
        {
            DeviceMsg::TransportModeStatus(confirmed) => {
                tracing::info!(
                    "set_transport_mode: got TransportModeStatus({:?})",
                    confirmed
                );
                Ok(confirmed)
            }
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }
}
