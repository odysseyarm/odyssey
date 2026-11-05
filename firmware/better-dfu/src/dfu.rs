//! DFU-mode side of the protocol, run by the bootloader.
use core::cell::Cell;

use embassy_boot::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterError};
use embassy_usb::control::{InResponse, OutResponse, Recipient, RequestType};
use embassy_usb::driver::Driver;
use embassy_usb::types::{InterfaceNumber, StringIndex};
use embassy_usb::{Builder, FunctionBuilder, Handler};
use embedded_storage::nor_flash::{NorFlash, NorFlashErrorKind};

use crate::Reset;
use crate::consts::{
    APPN_SPEC_SUBCLASS_DFU, DESC_DFU_FUNCTIONAL, DFU_PROTOCOL_DFU, DfuAttributes, Request, State,
    Status, USB_CLASS_APPN_SPEC,
};

/// Image a DFU download is written to, selected by the interface alternate setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DfuTarget {
    /// Application core bank A (direct XIP)
    AppBankA,
    /// Application core bank B (direct XIP)
    AppBankB,
    /// Network core (swap-based, forwarded via IPC)
    NetCore,
    /// Stage2 bootloader (swap-based self-update)
    Bootloader,
}

/// Forwards firmware chunks to a target this core cannot flash directly, such as the network core over IPC.
pub trait DfuForwarder {
    /// Called at the start of a firmware download to the alternate target
    fn start_forward(&mut self, total_size: usize) -> Result<(), ()>;

    /// Called for each chunk of firmware data to be forwarded
    fn forward_chunk(&mut self, offset: usize, data: &[u8]) -> Result<(), ()>;

    /// Called when firmware download is complete
    fn finish_forward(&mut self) -> Result<(), ()>;
}

/// Writes downloaded firmware to the flash region of a [`DfuTarget`].
pub trait DualBankWriter {
    /// Writes `data` at byte `offset` within `target`'s region.
    fn write_for_target(&mut self, target: DfuTarget, offset: usize, data: &[u8])
    -> Result<(), ()>;

    /// Marks `target` as updated once the final block has been written.
    fn mark_updated_for_target(&mut self, target: DfuTarget) -> Result<(), ()>;
}

/// Forwarder for app-only DFU; every call fails.
pub struct NullForwarder;

impl DfuForwarder for NullForwarder {
    fn start_forward(&mut self, _total_size: usize) -> Result<(), ()> {
        Err(())
    }

    fn forward_chunk(&mut self, _offset: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn finish_forward(&mut self) -> Result<(), ()> {
        Err(())
    }
}

/// Writer for systems without dual-bank support; every call fails.
pub struct NullDualBankWriter;

impl DualBankWriter for NullDualBankWriter {
    fn write_for_target(
        &mut self,
        _target: DfuTarget,
        _offset: usize,
        _data: &[u8],
    ) -> Result<(), ()> {
        Err(())
    }

    fn mark_updated_for_target(&mut self, _target: DfuTarget) -> Result<(), ()> {
        Err(())
    }
}
/// DFU-mode state machine and USB control handler; [`usb_dfu`] registers it.
pub struct Control<
    'd,
    DFU: NorFlash,
    STATE: NorFlash,
    RST: Reset,
    FWD: DfuForwarder,
    DBW: DualBankWriter,
    const BLOCK_SIZE: usize,
> {
    _updater: BlockingFirmwareUpdater<'d, DFU, STATE>,
    attrs: DfuAttributes,
    state: State,
    status: Status,
    offset: usize,
    buf: AlignedBuffer<BLOCK_SIZE>,
    reset: RST,

    target: DfuTarget,
    _forwarder: FWD,
    dual_bank_writer: DBW,
    _total_size: usize,
    active_bank_is_a: bool,
    single_bank_capacity: Option<usize>,
    app_string: Option<StringIndex>,
    net_string: Option<StringIndex>,
    bootloader_string: Option<StringIndex>,
    app_label: &'static str,
    bootloader_label: &'static str,
    state_flagger: Option<&'d mut dyn FnMut()>,
    download_started_flag: Option<&'d Cell<bool>>,
    interface: Option<InterfaceNumber>,

    #[cfg(feature = "_verify")]
    public_key: &'static [u8; 32],
}

impl<
    'd,
    DFU: NorFlash,
    STATE: NorFlash,
    RST: Reset,
    FWD: DfuForwarder,
    DBW: DualBankWriter,
    const BLOCK_SIZE: usize,
> Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE>
{
    /// The default download target is the app bank that is not running, per `active_bank_is_a`.
    pub fn new(
        updater: BlockingFirmwareUpdater<'d, DFU, STATE>,
        attrs: DfuAttributes,
        reset: RST,
        forwarder: FWD,
        dual_bank_writer: DBW,
        active_bank_is_a: bool,
        #[cfg(feature = "_verify")] public_key: &'static [u8; 32],
    ) -> Self {
        Self {
            _updater: updater,
            attrs,
            state: State::DfuIdle,
            status: Status::Ok,
            offset: 0,
            buf: AlignedBuffer([0; BLOCK_SIZE]),
            reset,
            target: if active_bank_is_a {
                DfuTarget::AppBankB
            } else {
                DfuTarget::AppBankA
            },
            _forwarder: forwarder,
            dual_bank_writer,
            _total_size: 0,
            active_bank_is_a,
            single_bank_capacity: None,
            app_string: None,
            net_string: None,
            bootloader_string: None,
            app_label: "APP",
            bootloader_label: "BOOTLOADER",
            state_flagger: None,
            download_started_flag: None,
            interface: None,

            #[cfg(feature = "_verify")]
            public_key,
        }
    }

    /// Sets the string descriptors for alt settings 0 (labelled `app_label`), 1 (NET) and 2 (bootloader).
    /// [`usb_dfu`] calls this.
    pub fn set_interface_strings(
        &mut self,
        app_string: StringIndex,
        net_string: StringIndex,
        bootloader_string: StringIndex,
        app_label: &'static str,
    ) {
        self.app_string = Some(app_string);
        self.net_string = Some(net_string);
        self.bootloader_string = Some(bootloader_string);
        self.app_label = app_label;
    }

    /// Override the alt 2 string with one carrying this bootloader's version,
    /// e.g. `concat!("BOOTLOADER ", env!("CARGO_PKG_VERSION"))`.
    ///
    /// The host reads this to decide whether the installed bootloader predates
    /// the one in the firmware manifest. A bootloader that keeps the default
    /// label reports no version, and the host treats it as older than
    /// anything: bootloaders built without this call report the same label.
    pub fn set_bootloader_label(&mut self, label: &'static str) {
        self.bootloader_label = label;
    }

    /// Callback run on DFU_DETACH before the reset, e.g. to write SWAP_MAGIC.
    pub fn set_state_flagger(&mut self, f: &'d mut dyn FnMut()) {
        self.state_flagger = Some(f);
    }

    /// Sets a sticky flag on any addressed DNLOAD request when CAN_DOWNLOAD is enabled,
    /// before validating or writing its contents, including empty or rejected downloads.
    /// This handler never clears the flag, including on Abort, USB reset, errors or alt changes.
    /// Register before serving requests; registration leaves the flag's current value unchanged.
    pub fn set_download_started_flag(&mut self, flag: &'d Cell<bool>) {
        self.download_started_flag = Some(flag);
    }

    fn reset_boot_state(&mut self) {
        self.state = State::DfuIdle;
        self.status = Status::Ok;
        self.offset = 0;
        // Default to the inactive app bank.
        self.target = if self.active_bank_is_a {
            DfuTarget::AppBankB
        } else {
            DfuTarget::AppBankA
        };
        self._total_size = 0;
    }

    fn reset_state(&mut self) {
        self.offset = 0;
        self.state = State::DfuIdle;
        self.status = Status::Ok;
        self.target = if self.active_bank_is_a {
            DfuTarget::AppBankB
        } else {
            DfuTarget::AppBankA
        };
        self._total_size = 0;
    }

    fn write_download(&mut self, len: usize) -> Result<(), Status> {
        if let Some(capacity) = self.single_bank_capacity {
            let end = self.offset.checked_add(len).ok_or(Status::ErrAddress)?;
            if end > capacity || !self.offset.is_multiple_of(BLOCK_SIZE) {
                return Err(Status::ErrAddress);
            }
            let padded = len.next_multiple_of(DFU::WRITE_SIZE);
            self.buf.as_mut()[len..padded].fill(0xff);
            self._updater
                .write_firmware(self.offset, &self.buf.as_ref()[..padded])
                .map_err(Status::from)
        } else {
            self.dual_bank_writer
                .write_for_target(self.target, self.offset, &self.buf.as_ref()[..len])
                .map_err(|()| Status::ErrWrite)
        }
    }

    fn finish_download(&mut self) -> Result<(), Status> {
        if self.single_bank_capacity.is_none() {
            return self
                .dual_bank_writer
                .mark_updated_for_target(self.target)
                .map_err(|()| Status::ErrWrite);
        }
        if self.offset == 0 {
            return Err(Status::ErrNotDone);
        }
        #[cfg(not(feature = "_verify"))]
        return self._updater.mark_updated().map_err(Status::from);
        #[cfg(feature = "_verify")]
        {
            const SIGNATURE_LEN: usize = 64;
            let len = self
                .offset
                .checked_sub(SIGNATURE_LEN)
                .ok_or(Status::ErrVerify)?;
            let len = u32::try_from(len).map_err(|_| Status::ErrAddress)?;
            let mut signature = [0; SIGNATURE_LEN];
            self._updater
                .read_dfu(len, &mut signature)
                .map_err(Status::from)?;
            self._updater
                .verify_and_mark_updated(self.public_key, &signature, len)
                .map_err(Status::from)
        }
    }

    fn addressed_to_us(&self, req: &embassy_usb::control::Request) -> bool {
        (req.request_type, req.recipient) == (RequestType::Class, Recipient::Interface)
            && self
                .interface
                .is_some_and(|iface| req.index == u8::from(iface) as u16)
    }
}

impl<'d, DFU: NorFlash, STATE: NorFlash, RST: Reset, const BLOCK_SIZE: usize>
    Control<'d, DFU, STATE, RST, NullForwarder, NullDualBankWriter, BLOCK_SIZE>
{
    /// Writes one application through embassy-boot's scratch-swap updater.
    ///
    /// `active_capacity` excludes the DFU partition's scratch page. Only APP
    /// alternate setting 0 is exposed; the bootloader cannot replace itself.
    ///
    /// # Panics
    ///
    /// Panics if the USB block size is zero or not flash-write aligned.
    pub fn new_single_bank(
        updater: BlockingFirmwareUpdater<'d, DFU, STATE>,
        attrs: DfuAttributes,
        reset: RST,
        active_capacity: usize,
        #[cfg(feature = "_verify")] public_key: &'static [u8; 32],
    ) -> Self {
        assert!(BLOCK_SIZE != 0 && BLOCK_SIZE.is_multiple_of(DFU::WRITE_SIZE));
        let mut control = Self::new(
            updater,
            attrs,
            reset,
            NullForwarder,
            NullDualBankWriter,
            false,
            #[cfg(feature = "_verify")]
            public_key,
        );
        control.single_bank_capacity = Some(active_capacity);
        control
    }
}

impl From<FirmwareUpdaterError> for Status {
    fn from(e: FirmwareUpdaterError) -> Self {
        match e {
            FirmwareUpdaterError::Flash(e) => match e {
                NorFlashErrorKind::NotAligned => Status::ErrWrite,
                NorFlashErrorKind::OutOfBounds => Status::ErrAddress,
                _ => Status::ErrUnknown,
            },
            FirmwareUpdaterError::Signature(_) => Status::ErrVerify,
            FirmwareUpdaterError::BadState => Status::ErrUnknown,
        }
    }
}

impl<
    'd,
    DFU: NorFlash,
    STATE: NorFlash,
    RST: Reset,
    FWD: DfuForwarder,
    DBW: DualBankWriter,
    const BLOCK_SIZE: usize,
> Handler for Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE>
{
    fn set_alternate_setting(&mut self, interface: InterfaceNumber, alternate: u8) {
        if self.interface != Some(interface) {
            return;
        }
        if self.single_bank_capacity.is_some() && alternate != 0 {
            return;
        }
        match alternate {
            0 => {
                // Alt 0 is whichever app bank is not running.
                self.target = if self.active_bank_is_a {
                    info!("Alt setting 0 selected: Bank B (inactive, active=A)");
                    DfuTarget::AppBankB
                } else {
                    info!("Alt setting 0 selected: Bank A (inactive, active=B)");
                    DfuTarget::AppBankA
                };
            }
            1 => {
                info!("Alt setting 1 selected: Network Core");
                self.target = DfuTarget::NetCore;
            }
            2 => {
                info!("Alt setting 2 selected: Bootloader");
                self.target = DfuTarget::Bootloader;
            }
            _ => {
                warn!("Unknown alt setting {}", alternate);
            }
        }
    }

    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        if let Some(app_idx) = self.app_string {
            if index == app_idx {
                return Some(self.app_label);
            }
        }

        if let Some(net_idx) = self.net_string {
            if index == net_idx {
                return Some("NET");
            }
        }

        if let Some(bootloader_idx) = self.bootloader_string {
            if index == bootloader_idx {
                return Some(self.bootloader_label);
            }
        }

        None
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        if !self.addressed_to_us(&req) {
            debug!("Unknown out request: {:?}", req);
            return None;
        }
        match Request::try_from(req.request) {
            Ok(Request::Abort) => {
                info!("Abort requested");
                self.reset_state();
                self.reset_boot_state();
                Some(OutResponse::Accepted)
            }
            Ok(Request::Dnload) if self.attrs.contains(DfuAttributes::CAN_DOWNLOAD) => {
                if let Some(flag) = self.download_started_flag {
                    flag.set(true);
                }
                if self.single_bank_capacity.is_some()
                    && usize::from(req.value) != self.offset.div_ceil(BLOCK_SIZE)
                {
                    self.status = Status::ErrAddress;
                    self.state = State::Error;
                    return Some(OutResponse::Rejected);
                }
                if req.value == 0 {
                    info!("Download starting to {:?}", self.target);
                    self.state = State::Download;
                    self.offset = 0;
                }

                if self.state != State::Download {
                    error!("Unexpected DNLOAD while chip is waiting for a GETSTATUS");
                    self.status = Status::ErrUnknown;
                    self.state = State::Error;
                    return Some(OutResponse::Rejected);
                }

                if data.len() > BLOCK_SIZE || data.len() != req.length as usize {
                    error!("USB data length does not match the download request or block size");
                    self.status = Status::ErrUnknown;
                    self.state = State::Error;
                    return Some(OutResponse::Rejected);
                }

                debug!("Copying {} bytes to buffer", data.len());
                self.buf.as_mut()[..data.len()].copy_from_slice(data);

                let final_transfer = req.length == 0;
                if final_transfer {
                    debug!("Receiving final transfer for {:?}", self.target);
                    let finished_target = self.target;
                    match self.finish_download() {
                        Ok(_) => {
                            self.reset_boot_state();
                            self.status = Status::Ok;
                            self.state = State::ManifestSync;
                            info!("{:?} update complete", finished_target);
                        }
                        Err(status) => {
                            error!("Error completing {:?} update", self.target);
                            self.state = State::Error;
                            self.status = status;
                        }
                    }
                } else {
                    debug!(
                        "Writing {} bytes at offset {} to {:?}",
                        data.len(),
                        self.offset,
                        self.target
                    );
                    match self.write_download(data.len()) {
                        Ok(_) => {
                            debug!("Write successful, new offset: {}", self.offset + data.len());
                            self.status = Status::Ok;
                            self.state = State::DlSync;
                            self.offset += data.len();
                        }
                        Err(status) => {
                            error!(
                                "Error writing firmware at offset {}: {:?}",
                                self.offset, self.target
                            );
                            self.state = State::Error;
                            self.status = status;
                        }
                    }
                }

                Some(OutResponse::Accepted)
            }
            Ok(Request::Detach) => {
                if let Some(f) = self.state_flagger.as_mut() {
                    f();
                }
                // Already in DFU mode, so DETACH only runs the state flagger and resets. ResetImmediate does
                // not return, so the Accepted status stage is never sent.
                let resp = Some(OutResponse::Accepted);
                self.reset.sys_reset();
                resp
            }
            Ok(Request::ClrStatus) => {
                info!("Clear status requested");
                self.reset_state();
                Some(OutResponse::Accepted)
            }
            _ => None,
        }
    }

    fn control_in<'a>(
        &'a mut self,
        req: embassy_usb::control::Request,
        buf: &'a mut [u8],
    ) -> Option<embassy_usb::control::InResponse<'a>> {
        if !self.addressed_to_us(&req) {
            debug!("Unknown in request: {:?}", req);
            return None;
        }
        match Request::try_from(req.request) {
            Ok(Request::GetStatus) => {
                let Some(buf) = buf.get_mut(..6) else {
                    return Some(InResponse::Rejected);
                };
                match self.state {
                    State::DlSync => self.state = State::Download,
                    State::ManifestSync => {
                        self.state = State::ManifestWaitReset;

                        // If WILL_DETACH is set, perform the detach/reset immediately.
                        // The host may see a transient error because the device disappears.
                        if self.attrs.contains(DfuAttributes::WILL_DETACH) {
                            self.reset.sys_reset();
                        }
                    }
                    _ => {}
                }

                // bStatus, bwPollTimeout = 0x000032 (50 ms, fixed), bState, iString = 0 (no vendor error string).
                buf.copy_from_slice(&[self.status as u8, 0x32, 0x00, 0x00, self.state as u8, 0x00]);
                Some(InResponse::Accepted(buf))
            }
            Ok(Request::GetState) => {
                let Some(buf) = buf.get_mut(..1) else {
                    return Some(InResponse::Rejected);
                };
                buf[0] = self.state as u8;
                Some(InResponse::Accepted(buf))
            }
            Ok(Request::Upload) if self.attrs.contains(DfuAttributes::CAN_UPLOAD) => {
                // FirmwareUpdater can read the active partition, so upload could be supported; it is not.
                Some(InResponse::Rejected)
            }
            _ => None,
        }
    }

    fn reset(&mut self) {
        // Only reset after the manifest phase is complete. This keeps normal bus resets
        // (e.g., re-enumeration) from rebooting the device.
        if matches!(self.state, State::ManifestSync | State::ManifestWaitReset) {
            self.reset.sys_reset()
        }
    }
}

/// Adds the USB DFU 1.1 DFU-mode interface to `builder` and registers `handler` for its control requests.
///
/// Dual-bank handlers expose inactive APP_A/APP_B, NET and BOOTLOADER alternatives.
/// [`Control::new_single_bank`] exposes only APP and stages it for scratch swap.
/// Downloaded blocks go to the configured writer; the final manifestation status
/// resets the device when WILL_DETACH is set, otherwise the next USB reset does.
///
/// `func_modifier` can add function-level descriptors such as MSOS headers, for when DFU is part of a
/// composite USB device.
pub fn usb_dfu<
    'd,
    D: Driver<'d>,
    DFU: NorFlash,
    STATE: NorFlash,
    RST: Reset,
    FWD: DfuForwarder,
    DBW: DualBankWriter,
    const BLOCK_SIZE: usize,
>(
    builder: &mut Builder<'d, D>,
    handler: &'d mut Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE>,
    func_modifier: impl Fn(&mut FunctionBuilder<'_, 'd, D>),
) {
    // Allocated before builder.function(), whose FunctionBuilder borrows builder mutably until dropped.
    let inactive_str_idx = builder.string();
    let extra_strings = if handler.single_bank_capacity.is_some() {
        None
    } else {
        Some((builder.string(), builder.string()))
    };

    let mut func = builder.function(
        USB_CLASS_APPN_SPEC,
        APPN_SPEC_SUBCLASS_DFU,
        DFU_PROTOCOL_DFU,
    );

    // Function-level descriptors such as MSOS headers, for when DFU is part of a composite USB device.
    func_modifier(&mut func);

    // Alt 0: inactive app bank (APP_B when A is active, APP_A when B is active)
    // Alt 1: NET
    // Alt 2: BOOTLOADER
    if let Some((net_str_idx, bootloader_str_idx)) = extra_strings {
        let inactive_app_label = if handler.active_bank_is_a {
            "APP_B"
        } else {
            "APP_A"
        };
        handler.set_interface_strings(
            inactive_str_idx,
            net_str_idx,
            bootloader_str_idx,
            inactive_app_label,
        );
    } else {
        handler.app_string = Some(inactive_str_idx);
        handler.app_label = "APP";
    }
    let mut iface = func.interface();
    handler.interface = Some(iface.interface_number());

    // alt=0: Inactive application bank (download goes here)
    let mut alt = iface.alt_setting(
        USB_CLASS_APPN_SPEC,
        APPN_SPEC_SUBCLASS_DFU,
        DFU_PROTOCOL_DFU,
        Some(inactive_str_idx),
    );
    alt.descriptor(
        DESC_DFU_FUNCTIONAL,
        &[
            handler.attrs.bits(),
            0xc4,
            0x09, // wDetachTimeOut 0x09c4 = 2500 ms
            (BLOCK_SIZE & 0xff) as u8,
            ((BLOCK_SIZE & 0xff00) >> 8) as u8,
            0x10,
            0x01, // DFU 1.1
        ],
    );

    if let Some((net_str_idx, bootloader_str_idx)) = extra_strings {
        // alt=1: Network Core
        let mut alt = iface.alt_setting(
            USB_CLASS_APPN_SPEC,
            APPN_SPEC_SUBCLASS_DFU,
            DFU_PROTOCOL_DFU,
            Some(net_str_idx),
        );
        alt.descriptor(
            DESC_DFU_FUNCTIONAL,
            &[
                handler.attrs.bits(),
                0xc4,
                0x09,
                (BLOCK_SIZE & 0xff) as u8,
                ((BLOCK_SIZE & 0xff00) >> 8) as u8,
                0x10,
                0x01, // DFU 1.1
            ],
        );

        // alt=2: Bootloader (swap-based self-update)
        let mut alt = iface.alt_setting(
            USB_CLASS_APPN_SPEC,
            APPN_SPEC_SUBCLASS_DFU,
            DFU_PROTOCOL_DFU,
            Some(bootloader_str_idx),
        );
        alt.descriptor(
            DESC_DFU_FUNCTIONAL,
            &[
                handler.attrs.bits(),
                0xc4,
                0x09,
                (BLOCK_SIZE & 0xff) as u8,
                ((BLOCK_SIZE & 0xff00) >> 8) as u8,
                0x10,
                0x01, // DFU 1.1
            ],
        );
    }

    drop(iface);
    drop(func);

    builder.handler(handler);
}

#[cfg(test)]
mod tests {
    use embassy_boot::FirmwareUpdaterConfig;
    use embedded_storage::nor_flash::{ErrorType, ReadNorFlash};

    use super::*;

    struct UnusedFlash;

    impl ErrorType for UnusedFlash {
        type Error = NorFlashErrorKind;
    }

    impl ReadNorFlash for UnusedFlash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, _: u32, _: &mut [u8]) -> Result<(), Self::Error> {
            panic!("Control must not access its unused updater");
        }

        fn capacity(&self) -> usize {
            4096
        }
    }

    impl NorFlash for UnusedFlash {
        const WRITE_SIZE: usize = 1;
        const ERASE_SIZE: usize = 4096;

        fn write(&mut self, _: u32, _: &[u8]) -> Result<(), Self::Error> {
            panic!("Control must not access its unused updater");
        }

        fn erase(&mut self, _: u32, _: u32) -> Result<(), Self::Error> {
            panic!("Control must not access its unused updater");
        }
    }

    struct NoReset;

    impl Reset for NoReset {
        fn sys_reset(&self) {
            panic!("Unexpected system reset");
        }
    }

    #[test]
    fn download_attempt_stays_recorded_after_errors_abort_reset_and_alt_changes() {
        for (value, length, data) in [
            (0, 1, &[1][..]),
            (0, 2, &[1][..]),
            (1, 0, &[][..]),
            (0, 0, &[][..]),
        ] {
            let flag = Cell::new(false);
            let mut aligned = AlignedBuffer([0; 1]);
            let updater = BlockingFirmwareUpdater::new(
                FirmwareUpdaterConfig {
                    dfu: UnusedFlash,
                    state: UnusedFlash,
                },
                aligned.as_mut(),
            );
            let mut control = Control::<_, _, _, _, _, 4>::new(
                updater,
                DfuAttributes::CAN_DOWNLOAD,
                NoReset,
                NullForwarder,
                NullDualBankWriter,
                true,
                #[cfg(feature = "_verify")]
                &[0; 32],
            );
            control.interface = Some(InterfaceNumber(0));
            control.set_download_started_flag(&flag);
            let mut req = embassy_usb::control::Request::parse(&[
                0x21,
                Request::Dnload as u8,
                0,
                0,
                0,
                0,
                0,
                0,
            ]);
            req.value = value;
            req.length = length;

            req.index = 1;
            assert!(control.control_out(req, data).is_none());
            assert!(!flag.get());
            req.index = 0;
            control.attrs = DfuAttributes::empty();
            assert!(control.control_out(req, data).is_none());
            assert!(!flag.get());
            control.attrs = DfuAttributes::CAN_DOWNLOAD;

            let _response = control.control_out(req, data);
            assert!(flag.get());
            req.request = Request::Abort as u8;
            req.length = 0;
            assert!(matches!(
                control.control_out(req, &[]),
                Some(OutResponse::Accepted)
            ));
            assert!(flag.get());
            control.reset();
            assert!(flag.get());
            control.set_alternate_setting(InterfaceNumber(0), 1);
            assert!(flag.get());
        }
    }
}

#[cfg(all(test, not(feature = "_verify")))]
#[path = "dfu/single_bank_tests.rs"]
mod single_bank_tests;
