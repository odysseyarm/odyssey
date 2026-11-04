use odyssey_protocol::Packet;
use odyssey_protocol::control::device::TransportMode;

pub(super) fn queued_transport(
    packet: &Packet,
    report_transport: TransportMode,
    usb_active: bool,
) -> Option<TransportMode> {
    if packet.is_control() {
        usb_active.then_some(TransportMode::Usb)
    } else {
        Some(report_transport)
    }
}

#[cfg(test)]
mod tests {
    use odyssey_protocol::{BatteryReport, PacketData};

    use super::*;

    #[test]
    fn usb_ack_follows_request_transport_not_report_transport() {
        let reply = Packet {
            id: 1,
            data: PacketData::Ack(),
        };
        assert_eq!(
            queued_transport(&reply, TransportMode::Ble, true),
            Some(TransportMode::Usb)
        );
        assert_eq!(queued_transport(&reply, TransportMode::Ble, false), None);
    }

    #[test]
    fn reports_still_follow_automatic_transport_changes() {
        let report = Packet {
            id: 1,
            data: PacketData::BatteryReport(BatteryReport {
                percent: 50,
                charging: false,
            }),
        };
        assert_eq!(
            queued_transport(&report, TransportMode::Ble, false),
            Some(TransportMode::Ble)
        );
        assert_eq!(
            queued_transport(&report, TransportMode::Usb, true),
            Some(TransportMode::Usb)
        );
    }
}
