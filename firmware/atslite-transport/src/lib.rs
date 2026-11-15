//! Transport choice for packets leaving an ATSlite.
//!
//! The device has no stored transport mode. Streamed reports follow the USB
//! state: they go over USB while a USB host has the device configured and over
//! BLE otherwise, like a wireless mouse that switches to its cable when plugged
//! in. The BLE link stays up meanwhile, so unplugging moves the reports back
//! without reconnecting. Replies are different: each one goes back over the
//! transport its request arrived on.

#![no_std]

use odyssey_protocol::control::device::TransportMode;

/// Transport that carries streamed reports, and the one `GetTransportMode`
/// reports: USB while a USB host has the device configured, BLE otherwise.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn report_transport(usb_configured: bool) -> TransportMode {
    if usb_configured {
        TransportMode::Usb
    } else {
        TransportMode::Ble
    }
}

/// Transport for the reply to a request that arrived on `request_transport`.
///
/// Returns `None` for a USB request whose USB host has gone away since: the
/// reply is dropped instead of rerouted, because the request's sender waits for
/// it on USB and fails over by sending the request again on BLE.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn reply_transport(
    request_transport: TransportMode,
    usb_configured: bool,
) -> Option<TransportMode> {
    match request_transport {
        TransportMode::Usb => usb_configured.then_some(TransportMode::Usb),
        TransportMode::Ble => Some(TransportMode::Ble),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_go_over_usb_only_while_usb_is_configured() {
        assert_eq!(report_transport(true), TransportMode::Usb);
        assert_eq!(report_transport(false), TransportMode::Ble);
    }

    #[test]
    fn a_ble_request_is_answered_over_ble_while_usb_carries_reports() {
        assert_eq!(
            reply_transport(TransportMode::Ble, true),
            Some(TransportMode::Ble)
        );
        assert_eq!(
            reply_transport(TransportMode::Ble, false),
            Some(TransportMode::Ble)
        );
    }

    #[test]
    fn a_usb_reply_is_dropped_after_unplug_instead_of_sent_over_ble() {
        assert_eq!(
            reply_transport(TransportMode::Usb, true),
            Some(TransportMode::Usb)
        );
        assert_eq!(reply_transport(TransportMode::Usb, false), None);
    }
}
