//! DFU bootloader part of DFU logic
use embassy_boot::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterError};
use embassy_usb::control::{InResponse, OutResponse, Recipient, RequestType};
use embassy_usb::driver::Driver;
use embassy_usb::types::{InterfaceNumber, StringIndex};
use embassy_usb::{Builder, FunctionBuilder, Handler};
use embedded_storage::nor_flash::{NorFlash, NorFlashErrorKind};

use crate::Reset;
use crate::consts::{
    APPN_SPEC_SUBCLASS_DFU, DESC_DFU_FUNCTIONAL, DFU_PROTOCOL_DFU, DfuAttributes, Request, State, Status,
    USB_CLASS_APPN_SPEC,
};


/// Target for DFU firmware update
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

/// Trait for forwarding firmware chunks to alternate target (e.g., network core via IPC)
pub trait DfuForwarder {
    /// Called at the start of a firmware download to the alternate target
    fn start_forward(&mut self, total_size: usize) -> Result<(), ()>;

    /// Called for each chunk of firmware data to be forwarded
    fn forward_chunk(&mut self, offset: usize, data: &[u8]) -> Result<(), ()>;

    /// Called when firmware download is complete
    fn finish_forward(&mut self) -> Result<(), ()>;
}

/// Trait for handling dual-bank flash writes based on DFU target
pub trait DualBankWriter {
    /// Write firmware data to the appropriate bank based on target
    fn write_for_target(&mut self, target: DfuTarget, offset: usize, data: &[u8]) -> Result<(), ()>;

    /// Mark the appropriate bank as updated based on target
    fn mark_updated_for_target(&mut self, target: DfuTarget) -> Result<(), ()>;
}

/// Dummy forwarder for single-target (app-only) DFU
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

/// Null dual-bank writer for systems that don't need dual-bank support
pub struct NullDualBankWriter;

impl DualBankWriter for NullDualBankWriter {
    fn write_for_target(&mut self, _target: DfuTarget, _offset: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn mark_updated_for_target(&mut self, _target: DfuTarget) -> Result<(), ()> {
        Err(())
    }
}
/// Internal state for USB DFU
pub struct Control<'d, DFU: NorFlash, STATE: NorFlash, RST: Reset, FWD: DfuForwarder, DBW: DualBankWriter, const BLOCK_SIZE: usize> {
    _updater: BlockingFirmwareUpdater<'d, DFU, STATE>,
    attrs: DfuAttributes,
    state: State,
    status: Status,
    offset: usize,
    buf: AlignedBuffer<BLOCK_SIZE>,
    reset: RST,

    // Dual-alt support
    target: DfuTarget,
    _forwarder: FWD,
    dual_bank_writer: DBW,
    _total_size: usize,
    active_bank_is_a: bool,
    app_string: Option<StringIndex>,
    net_string: Option<StringIndex>,
    bootloader_string: Option<StringIndex>,
    app_label: &'static str,
    state_flagger: Option<&'d mut dyn FnMut()>,

    #[cfg(feature = "_verify")]
    public_key: &'static [u8; 32],
}

impl<'d, DFU: NorFlash, STATE: NorFlash, RST: Reset, FWD: DfuForwarder, DBW: DualBankWriter, const BLOCK_SIZE: usize> Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE> {
    /// Create a new DFU instance to handle DFU transfers.
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
            app_string: None,
            net_string: None,
            bootloader_string: None,
            app_label: "APP",
            state_flagger: None,

            #[cfg(feature = "_verify")]
            public_key,
        }
    }

    /// Provide string indices and labels for alternate settings.
    pub fn set_interface_strings(&mut self, app_string: StringIndex, net_string: StringIndex, bootloader_string: StringIndex, app_label: &'static str) {
        self.app_string = Some(app_string);
        self.net_string = Some(net_string);
        self.bootloader_string = Some(bootloader_string);
        self.app_label = app_label;
    }

    /// Provide a callback used to flag DFU requested (e.g., write SWAP_MAGIC).
    pub fn set_state_flagger(&mut self, f: &'d mut dyn FnMut()) {
        self.state_flagger = Some(f);
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

impl<'d, DFU: NorFlash, STATE: NorFlash, RST: Reset, FWD: DfuForwarder, DBW: DualBankWriter, const BLOCK_SIZE: usize> Handler
    for Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE>
{
    fn set_alternate_setting(&mut self, _interface: InterfaceNumber, alternate: u8) {
        match alternate {
            0 => {
                // Alt 0 = Inactive bank. Pick the opposite of the active bank directly.
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
                return Some("BOOTLOADER");
            }
        }

        None
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        if (req.request_type, req.recipient) != (RequestType::Class, Recipient::Interface) {
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

                if data.len() > BLOCK_SIZE {
                    error!("USB data len exceeded block size");
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
                    match self.dual_bank_writer.mark_updated_for_target(finished_target) {
                        Ok(_) => {
                            self.reset_boot_state();
                            self.status = Status::Ok;
                            self.state = State::ManifestSync;
                            info!("{:?} update complete", finished_target);
                        }
                        Err(_) => {
                            error!("Error completing {:?} update", self.target);
                            self.state = State::Error;
                            self.status = Status::ErrWrite;
                        }
                    }
                } else {
                    debug!(
                        "Writing {} bytes at offset {} to {:?}",
                        data.len(),
                        self.offset,
                        self.target
                    );
                    match self.dual_bank_writer.write_for_target(
                        self.target,
                        self.offset,
                        &self.buf.as_ref()[..data.len()],
                    ) {
                        Ok(_) => {
                            debug!("Write successful, new offset: {}", self.offset + data.len());
                            self.status = Status::Ok;
                            self.state = State::DlSync;
                            self.offset += data.len();
                        }
                        Err(_) => {
                            error!(
                                "Error writing firmware at offset {}: {:?}",
                                self.offset, self.target
                            );
                            self.state = State::Error;
                            self.status = Status::ErrWrite;
                        }
                    }
                }

                Some(OutResponse::Accepted)
            }
            Ok(Request::Detach) => {
                if let Some(f) = self.state_flagger.as_mut() {
                    f();
                }
                // Send response then reset to honor WILL_DETACH semantics.
                let resp = Some(OutResponse::Accepted);
                self.reset.sys_reset();
                resp
            } // Device is already in DFU mode
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
        if (req.request_type, req.recipient) != (RequestType::Class, Recipient::Interface) {
            debug!("Unknown in request: {:?}", req);
            return None;
        }
        match Request::try_from(req.request) {
            Ok(Request::GetStatus) => {
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

                //TODO: Configurable poll timeout, ability to add string for Vendor error
                buf[0..6].copy_from_slice(&[self.status as u8, 0x32, 0x00, 0x00, self.state as u8, 0x00]);
                Some(InResponse::Accepted(&buf[0..6]))
            }
            Ok(Request::GetState) => {
                buf[0] = self.state as u8;
                Some(InResponse::Accepted(&buf[0..1]))
            }
            Ok(Request::Upload) if self.attrs.contains(DfuAttributes::CAN_UPLOAD) => {
                //TODO: FirmwareUpdater provides a way of reading the active partition so we could in theory add functionality to upload firmware.
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

/// An implementation of the USB DFU 1.1 protocol
///
/// This function will add a DFU interface descriptor to the provided Builder, and register the provided Control as a handler for the USB device
/// The handler is responsive to DFU GetState, GetStatus, Abort, and ClrStatus commands, as well as Download if configured by the user.
///
/// Once the host has initiated a DFU download operation, the chunks sent by the host will be written to the DFU partition.
/// Once the final sync in the manifestation phase has been received, the handler will trigger a system reset to swap the new firmware.
pub fn usb_dfu<'d, D: Driver<'d>, DFU: NorFlash, STATE: NorFlash, RST: Reset, FWD: DfuForwarder, DBW: DualBankWriter, const BLOCK_SIZE: usize>(
    builder: &mut Builder<'d, D>,
    handler: &'d mut Control<'d, DFU, STATE, RST, FWD, DBW, BLOCK_SIZE>,
    func_modifier: impl Fn(&mut FunctionBuilder<'_, 'd, D>),
) {
    // Allocate string indices up front to avoid borrowing builder twice.
    let inactive_str_idx = builder.string();
    let net_str_idx = builder.string();
    let bootloader_str_idx = builder.string();

    let mut func = builder.function(USB_CLASS_APPN_SPEC, APPN_SPEC_SUBCLASS_DFU, DFU_PROTOCOL_DFU);

    // Here we give users the opportunity to add their own function level MSOS headers for instance.
    // This is useful when DFU functionality is part of a composite USB device.
    func_modifier(&mut func);

    // Allocate string indices for alternate settings and attach readable names.
    // Alt 0: inactive app bank (APP_B when A is active, APP_A when B is active)
    // Alt 1: NET
    // Alt 2: BOOTLOADER
    let inactive_app_label = if handler.active_bank_is_a { "APP_B" } else { "APP_A" };
    handler.set_interface_strings(inactive_str_idx, net_str_idx, bootloader_str_idx, inactive_app_label);
    // Default target to the inactive app bank.
    handler.target = if handler.active_bank_is_a {
        DfuTarget::AppBankB
    } else {
        DfuTarget::AppBankA
    };

    let mut iface = func.interface();

    // Only show the inactive bank based on which bank is active
    // If Bank A is active, show "Bank B" (for uploading to inactive bank B)
    // If Bank B is active, show "Bank A" (for uploading to inactive bank A)

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
            0x09, // 2500ms timeout
            (BLOCK_SIZE & 0xff) as u8,
            ((BLOCK_SIZE & 0xff00) >> 8) as u8,
            0x10,
            0x01, // DFU 1.1
        ],
    );

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

    drop(iface);
    drop(func);

    builder.handler(handler);
}
