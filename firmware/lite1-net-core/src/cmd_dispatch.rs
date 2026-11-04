use core::cell::RefCell;

use bt_hci::cmd;
use bt_hci::cmd::CmdReturnBuf;
use bt_hci::cmd::controller_baseband::*;
use bt_hci::cmd::info::*;
use bt_hci::cmd::le::*;
use bt_hci::cmd::link_control::*;
use bt_hci::cmd::status::*;
use bt_hci::event::{CommandComplete, EventParams};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use nrf_sdc::vendor::*;

pub type CmdErr = cmd::Error<nrf_sdc::Error>;

macro_rules! dispatch_cmd {
    (
        $send:expr, $ctrl:expr, $opcode:expr, $payload:expr,
        [ $( $(@ $async:tt)? $ty:ty ),* $(,)? ]
    ) => {{
        match $opcode {
            $(
                < $ty as bt_hci::cmd::Cmd >::OPCODE =>
                    dispatch_cmd!(@arm $ctrl, $payload, { $($async)? }, $ty, $send),
            )*
            _ => Err(bt_hci::cmd::Error::Hci(bt_hci::param::Error::UNKNOWN_CMD)),
        }
    }};

    // Async commands: the controller reports them through its own events, so the sender is unused.
    (@arm $ctrl:expr, $payload:expr, {async}, $ty:path, $_sender:expr) => {{
        let params =
            <<$ty as bt_hci::cmd::Cmd>::Params as bt_hci::FromHciBytes>
            ::from_hci_bytes_complete($payload)
            .map_err(|_| bt_hci::cmd::Error::Hci(bt_hci::param::Error::INVALID_HCI_PARAMETERS))?;
        let cmd_val = <$ty as From<<$ty as bt_hci::cmd::Cmd>::Params>>::from(params);
        bt_hci::cmd::AsyncCmd::exec(&cmd_val, $ctrl).await
    }};

    // Sync commands: the SDC hands back the return parameters, and we wrap them in a
    // Command Complete event ourselves.
    (@arm $ctrl:expr, $payload:expr, {}, $ty:path, $sender:expr) => {{
        let params =
            <<$ty as bt_hci::cmd::Cmd>::Params as bt_hci::FromHciBytes>
            ::from_hci_bytes_complete($payload)
            .map_err(|_| bt_hci::cmd::Error::Hci(bt_hci::param::Error::INVALID_HCI_PARAMETERS))?;
        let cmd_val = <$ty as From<<$ty as bt_hci::cmd::Cmd>::Params>>::from(params);

        let ret = bt_hci::cmd::SyncCmd::exec(&cmd_val, $ctrl).await;

        const RLEN: usize = < $ty as bt_hci::cmd::SyncCmd >::ReturnBuf::LEN;
        const HEAD: usize = 7; // H4 indicator, event code, plen, credits, opcode (2), status
        let param_len: u8 = (1 + 2 + 1 + RLEN) as u8;

        let mut buf = [0u8; HEAD + RLEN];

        buf[0] = bt_hci::PacketKind::Event as u8;

        buf[1] = CommandComplete::EVENT_CODE;

        buf[2] = param_len;

        // One command credit.
        buf[3] = 1;

        // Opcode, little-endian.
        let op16 = < $ty as bt_hci::cmd::Cmd >::OPCODE.into_inner();
        buf[4] = (op16 & 0xFF) as u8;
        buf[5] = (op16 >> 8) as u8;

        match ret {
            Ok(ret) => {
                buf[6] = bt_hci::param::Status::SUCCESS.into_inner();

                let bytes = bt_hci::AsHciBytes::as_hci_bytes(&ret);
                buf[HEAD..HEAD + RLEN].copy_from_slice(bytes);
            }
            Err(bt_hci::cmd::Error::Hci(e)) => {
                buf[6] = e.to_status().into_inner();
                // CommandComplete must echo the command handle even on error so the host can
                // match the response to an outstanding command with a nonzero handle.
                let handle = bt_hci::cmd::SyncCmd::param_handle(&cmd_val);
                let bytes = bt_hci::AsHciBytes::as_hci_bytes(&handle);
                buf[HEAD..HEAD + bytes.len()].copy_from_slice(bytes);
            }
            Err(e) => {
                return Err(e);
            }
        }

        defmt::info!("[cmd_dispatch] Sending {} bytes: {=[u8]:x}", buf.len(), &buf[..]);
        $sender.lock(|x| x.borrow_mut().send(&buf)).map_err(|_| bt_hci::cmd::Error::Io(nrf_sdc::Error::EIO))
    }};

    (@arm $_c:expr, $_p:expr, {$bad:tt}, $_ty:path, $_s:expr) => {
        compile_error!("unknown marker after '@': use '@async' or omit it")
    };
}

use crate::IpcNotify;
use crate::icmsg_config::ALIGN;

pub async fn exec_cmd_by_opcode<'d, E>(
    send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    ctrl: &crate::sdc::SoftdeviceController<'d>,
    opcode: bt_hci::cmd::Opcode,
    payload: &[u8],
) -> Result<(), CmdErr>
where
    E: core::fmt::Debug,
{
    dispatch_cmd!(send, ctrl, opcode, payload, [
        // §7.1 Link Control
        Disconnect,
        @async ReadRemoteVersionInformation,

        // §7.3 Controller & Baseband
        Reset,
        SetEventMask,
        ReadTransmitPowerLevel,
        SetControllerToHostFlowControl,
        HostBufferSize,
        SetEventMaskPage2,
        ReadAuthenticatedPayloadTimeout,
        WriteAuthenticatedPayloadTimeout,
        HostNumberOfCompletedPackets,

        // §7.4 Informational parameters
        ReadLocalVersionInformation,
        ReadLocalSupportedCmds,
        ReadLocalSupportedFeatures,
        ReadBdAddr,

        // §7.5 Status parameters
        ReadRssi,

        // §7.8 LE Controller (legacy and extended)
        LeSetAdvParams,
        LeReadAdvPhysicalChannelTxPower,
        LeSetAdvData,
        LeSetScanResponseData,
        LeSetAdvEnable,
        LeSetScanParams,
        LeSetScanEnable,
        @async LeCreateConn,

        LeSetExtAdvParams,
        LeSetExtAdvParamsV2,
        LeReadMaxAdvDataLength,
        LeReadNumberOfSupportedAdvSets,
        LeRemoveAdvSet,
        LeClearAdvSets,
        LeSetPeriodicAdvParams,
        LeSetPeriodicAdvParamsV2,
        LeSetPeriodicAdvEnable,
        LeSetExtScanEnable,
        @async LePeriodicAdvCreateSync,
        LePeriodicAdvCreateSyncCancel,
        LePeriodicAdvTerminateSync,
        LeAddDeviceToPeriodicAdvList,
        LeRemoveDeviceFromPeriodicAdvList,
        LeClearPeriodicAdvList,
        LeReadPeriodicAdvListSize,
        LeSetPeriodicAdvSyncTransferParams,
        LeSetDefaultPeriodicAdvSyncTransferParams,

        LeSetEventMask,
        LeReadBufferSize,
        LeReadLocalSupportedFeatures,
        LeSetRandomAddr,
        LeCreateConnCancel,
        LeReadFilterAcceptListSize,
        LeClearFilterAcceptList,
        LeAddDeviceToFilterAcceptList,
        LeRemoveDeviceFromFilterAcceptList,
        @async LeConnUpdate,
        LeSetHostChannelClassification,
        LeReadChannelMap,
        @async LeReadRemoteFeatures,
        LeEncrypt,
        LeRand,
        @async LeEnableEncryption,
        LeLongTermKeyRequestReply,
        LeLongTermKeyRequestNegativeReply,
        LeReadSupportedStates,
        LeSetDataLength,
        LeReadSuggestedDefaultDataLength,
        LeWriteSuggestedDefaultDataLength,
        LeAddDeviceToResolvingList,
        LeRemoveDeviceFromResolvingList,
        LeClearResolvingList,
        LeReadResolvingListSize,
        LeSetAddrResolutionEnable,
        LeSetResolvablePrivateAddrTimeout,
        LeReadMaxDataLength,
        LeReadPhy,
        LeSetDefaultPhy,
        @async LeSetPhy,
        LeSetAdvSetRandomAddr,
        LeReadTransmitPower,
        LeReadRfPathCompensation,
        LeWriteRfPathCompensation,
        LeSetPrivacyMode,
        LeSetConnectionlessCteTransmitEnable,
        LeConnCteResponseEnable,
        LeReadAntennaInformation,
        LeSetPeriodicAdvReceiveEnable,
        LePeriodicAdvSyncTransfer,
        LePeriodicAdvSetInfoTransfer,
        @async LeRequestPeerSca,
        LeEnhancedReadTransmitPowerLevel,
        @async LeReadRemoteTransmitPowerLevel,
        LeSetPathLossReportingParams,
        LeSetPathLossReportingEnable,
        LeSetTransmitPowerReportingEnable,
        LeSetDataRelatedAddrChanges,
        LeSetHostFeature,
        LeSetHostFeatureV2,

        // Zephyr and Nordic vendor commands
        ZephyrReadVersionInfo,
        ZephyrReadSupportedCommands,
        ZephyrWriteBdAddr,
        ZephyrReadKeyHierarchyRoots,
        ZephyrReadChipTemp,
        ZephyrWriteTxPower,
        ZephyrReadTxPower,

        NordicLlpmModeSet,
        NordicConnUpdate,
        NordicConnEventExtend,
        NordicQosConnEventReportEnable,
        NordicEventLengthSet,
        NordicPeriodicAdvEventLengthSet,
        NordicPeripheralLatencyModeSet,
        NordicWriteRemoteTxPower,
        NordicSetAdvRandomness,
        NordicCompatModeWindowOffsetSet,
        NordicQosChannelSurveyEnable,
        NordicSetPowerControlRequestParams,
        NordicReadAverageRssi,
        NordicCentralAclEventSpacingSet,
        NordicGetNextConnEventCounter,
        NordicAllowParallelConnectionEstablishments,
        NordicMinValOfMaxAclTxPayloadSet,
        NordicIsoReadTxTimestamp,
        NordicBigReservedTimeSet,
        NordicCigReservedTimeSet,
        NordicCisSubeventLengthSet,
        NordicScanChannelMapSet,
        NordicScanAcceptExtAdvPacketsSet,
        NordicSetRolePriority,
        NordicSetEventStartTask,
        NordicConnAnchorPointUpdateEventReportEnable,

        ZephyrReadStaticAddrs,
    ])
}
