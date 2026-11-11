//! DFU runtime interface exposed by the application firmware.
use embassy_boot::BlockingFirmwareState;
use embassy_time::{Duration, Instant};
use embassy_usb::control::{InResponse, OutResponse, Recipient, RequestType};
use embassy_usb::driver::Driver;
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, FunctionBuilder, Handler};
use embedded_storage::nor_flash::NorFlash;

use crate::Reset;
use crate::consts::{
    APPN_SPEC_SUBCLASS_DFU, DESC_DFU_FUNCTIONAL, DFU_PROTOCOL_RT, DfuAttributes, Request, State,
    Status, USB_CLASS_APPN_SPEC,
};

/// Tells the bootloader to enter USB DFU mode on the next boot.
///
/// Implemented for `BlockingFirmwareState<'d, STATE>`. Implement it directly when the DFU `Control` cannot own a
/// `BlockingFirmwareState`, for example when other code also needs it.
///
/// Since: Odyssey Desktop 0.6.0.
pub trait DfuMarker {
    /// Signal to the bootloader that DFU mode should be used on the next boot.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    fn mark_dfu(&mut self);
}

impl<'d, STATE: NorFlash> DfuMarker for BlockingFirmwareState<'d, STATE> {
    fn mark_dfu(&mut self) {
        self.mark_dfu()
            .expect("Failed to mark DFU mode in bootloader")
    }
}

/// DFU runtime state and USB control handler; [`usb_dfu`] registers it.
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Control<MARK: DfuMarker, RST: Reset> {
    dfu_marker: MARK,
    attrs: DfuAttributes,
    state: State,
    timeout: Option<Duration>,
    detach_start: Option<Instant>,
    reset: RST,
    /// Set when the descriptor is built. Control requests name an interface and
    /// a composite device has several, so without this the handler answers a
    /// detach meant for someone else and resets into the bootloader.
    interface: Option<InterfaceNumber>,
}

impl<MARK: DfuMarker, RST: Reset> Control<MARK, RST> {
    /// Starts in appIDLE. Pass it to [`usb_dfu`] to add the interface.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn new(dfu_marker: MARK, attrs: DfuAttributes, reset: RST) -> Self {
        Control {
            dfu_marker,
            attrs,
            state: State::AppIdle,
            detach_start: None,
            timeout: None,
            reset,
            interface: None,
        }
    }

    /// Whether a class request names the DFU interface.
    fn addressed_to_us(&self, req: &embassy_usb::control::Request) -> bool {
        if (req.request_type, req.recipient) != (RequestType::Class, Recipient::Interface) {
            return false;
        }
        match self.interface {
            Some(iface) => req.index == u8::from(iface) as u16,
            // The descriptor has not been built yet, so nothing can legitimately
            // be addressed here.
            None => false,
        }
    }
}

impl<MARK: DfuMarker, RST: Reset> Handler for Control<MARK, RST> {
    fn reset(&mut self) {
        if let Some(start) = self.detach_start {
            let delta = Instant::now() - start;
            let timeout = self.timeout.unwrap();
            trace!(
                "Received RESET with delta = {}, timeout = {}",
                delta.as_millis(),
                timeout.as_millis()
            );
            if delta < timeout {
                self.dfu_marker.mark_dfu();
                self.reset.sys_reset()
            }
        }
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        _: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        if !self.addressed_to_us(&req) {
            return None;
        }

        trace!("Received request {}", req);

        match Request::try_from(req.request) {
            Ok(Request::Detach) => {
                self.state = State::AppDetach;
                if self.attrs.contains(DfuAttributes::WILL_DETACH) {
                    trace!("Received DETACH, performing reset");
                    self.dfu_marker.mark_dfu();
                    self.reset.sys_reset();
                } else {
                    self.detach_start = Some(Instant::now());
                    self.timeout = Some(Duration::from_millis(req.value as u64));
                    trace!("Received DETACH, awaiting USB reset");
                }
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
            return None;
        }

        trace!("Received request {}", req);

        match Request::try_from(req.request) {
            Ok(Request::GetStatus) => {
                let Some(buf) = buf.get_mut(..6) else {
                    return Some(InResponse::Rejected);
                };
                buf.copy_from_slice(&[Status::Ok as u8, 0x32, 0x00, 0x00, self.state as u8, 0x00]);
                Some(InResponse::Accepted(buf))
            }
            _ => None,
        }
    }
}

/// Adds a USB DFU 1.1 runtime interface to `builder` and registers `handler` for it. The builder can be used as
/// normal afterwards.
///
/// The handler answers DFU_GETSTATUS and DFU_DETACH. A USB reset that arrives within the DETACH request's wTimeout
/// (immediately, if WILL_DETACH is set) calls [`DfuMarker::mark_dfu`] and resets the system. The bootloader must
/// recognise that mark and expose a DFU-mode device to carry out the transfer with the host.
///
/// `timeout` is advertised as wDetachTimeOut in the functional descriptor, in milliseconds truncated to `u16`.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn usb_dfu<'d, D: Driver<'d>, MARK: DfuMarker, RST: Reset>(
    builder: &mut Builder<'d, D>,
    handler: &'d mut Control<MARK, RST>,
    timeout: Duration,
    func_modifier: impl Fn(&mut FunctionBuilder<'_, 'd, D>),
) {
    let mut func = builder.function(0x00, 0x00, 0x00);

    // Function-level descriptors such as MSOS headers, for when DFU is part of a composite USB device.
    func_modifier(&mut func);

    let mut iface = func.interface();
    handler.interface = Some(iface.interface_number());
    let mut alt = iface.alt_setting(
        USB_CLASS_APPN_SPEC,
        APPN_SPEC_SUBCLASS_DFU,
        DFU_PROTOCOL_RT,
        None,
    );
    let timeout = timeout.as_millis() as u16;
    alt.descriptor(
        DESC_DFU_FUNCTIONAL,
        &[
            handler.attrs.bits(),
            (timeout & 0xff) as u8,
            ((timeout >> 8) & 0xff) as u8,
            0x40,
            0x00, // 64B control buffer size for application side
            0x10,
            0x01, // DFU 1.1
        ],
    );

    drop(func);
    builder.handler(handler);
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;

    struct Marker<'a>(&'a Cell<bool>);

    impl DfuMarker for Marker<'_> {
        fn mark_dfu(&mut self) {
            self.0.set(true);
        }
    }

    struct ResetFlag<'a>(&'a Cell<bool>);

    impl Reset for ResetFlag<'_> {
        fn sys_reset(&self) {
            self.0.set(true);
        }
    }

    #[test]
    fn immediate_detach_marks_and_resets_even_with_zero_timeout() {
        let marked = Cell::new(false);
        let reset = Cell::new(false);
        let mut handler = Control::new(
            Marker(&marked),
            DfuAttributes::WILL_DETACH,
            ResetFlag(&reset),
        );
        handler.interface = Some(InterfaceNumber(0));
        let unrelated =
            embassy_usb::control::Request::parse(&[0x21, Request::Detach as u8, 0, 0, 1, 0, 0, 0]);
        assert!(handler.control_out(unrelated, &[]).is_none());
        assert!(!marked.get());
        assert!(!reset.get());
        let detach =
            embassy_usb::control::Request::parse(&[0x21, Request::Detach as u8, 0, 0, 0, 0, 0, 0]);
        assert!(matches!(
            handler.control_out(detach, &[]),
            Some(OutResponse::Accepted)
        ));
        assert!(marked.get());
        assert!(reset.get());
        assert!(matches!(handler.state, State::AppDetach));
    }
}
