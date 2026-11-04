use super::semver::semver_at_least;

/// First device protocol whose firmware picks its data link itself.
const AUTO_TRANSPORT_PROTOCOL: [u16; 3] = [0, 1, 6];

/// Who decides which link a unit streams its reports over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransportSelection {
    /// The firmware: USB while a host has the cable configured, BLE otherwise. Stream
    /// subscriptions are shared by both links, `SetTransportMode` changes nothing, and
    /// the BLE link stays connected (and idle) while USB carries the reports.
    Automatic,
    /// The user: the mode stored in the unit's settings, changed by `SetTransportMode`
    /// and applied after a reboot.
    Manual,
}

impl TransportSelection {
    /// `protocol` is the device protocol version the unit reports, on either the control
    /// endpoint or the packet interface; both carry the same value.
    pub(super) fn for_protocol(protocol: [u16; 3]) -> Self {
        if semver_at_least(protocol, AUTO_TRANSPORT_PROTOCOL) {
            Self::Automatic
        } else {
            Self::Manual
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_from_protocol_0_1_6_picks_its_own_transport() {
        assert_eq!(
            TransportSelection::for_protocol([0, 1, 5]),
            TransportSelection::Manual
        );
        assert_eq!(
            TransportSelection::for_protocol([0, 0, 0]),
            TransportSelection::Manual
        );
        assert_eq!(
            TransportSelection::for_protocol([0, 1, 6]),
            TransportSelection::Automatic
        );
        assert_eq!(
            TransportSelection::for_protocol([0, 2, 0]),
            TransportSelection::Automatic
        );
    }
}
