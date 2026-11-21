//! The system Bluetooth adapter as a host for ATS Lite units: pairing a unit with it and
//! finding the bonded units that are in reach. Addresses are `[u8; 6]` in the unit's own
//! order, least significant byte first, the order of a unit's uuid and `BondSlot::bd_addr`.
//! A unit's address is `odyssey_protocol::control::ble_address` of its uuid. `BlueZ` prints
//! and parses the reverse.

#[cfg(target_os = "linux")]
mod bluez {
    use std::pin::pin;
    use std::time::Duration;

    use bluez_async::{
        AddressType, BluetoothError, BluetoothEvent, BluetoothSession, DeviceEvent, DeviceInfo,
        MacAddress,
    };
    use futures::StreamExt as _;

    /// Appearance of a unit's GAP service: the firmware sets
    /// `appearance::power_device::GENERIC_POWER_DEVICE`, category 0x1E, sub-category 0, so
    /// `0x1E << 6`.
    const UNIT_APPEARANCE: u16 = 0x1E << 6;

    const BLUEZ_CALL_TIMEOUT: Duration = Duration::from_secs(8);
    /// The unit advertises in pairing mode for as long as the user left it, so this only
    /// bounds a unit that is out of range or not in pairing mode.
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
    const PAIR_TIMEOUT: Duration = Duration::from_secs(25);

    fn mac_address(unit: [u8; 6]) -> MacAddress {
        let mut display_order = unit;
        display_order.reverse();
        MacAddress::from(display_order)
    }

    fn unit_address(mac: MacAddress) -> [u8; 6] {
        let mut unit = <[u8; 6]>::from(mac);
        unit.reverse();
        unit
    }

    async fn call<T>(
        what: &'static str,
        limit: Duration,
        future: impl Future<Output = Result<T, BluetoothError>>,
    ) -> eros::Result<T> {
        match tokio::time::timeout(limit, future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(eros::error!("{what}: {e}")),
            Err(_elapsed) => Err(eros::error!("{what}: timed out")),
        }
    }

    async fn session() -> eros::Result<BluetoothSession> {
        let (_, session) = BluetoothSession::new()
            .await
            .map_err(|e| eros::error!("BlueZ is unavailable: {e}"))?;
        Ok(session)
    }

    /// Linux needs an explicit `BlueZ` `Pair` request, btleplug exposes no bonding API.
    pub(crate) async fn pair_device(
        session: &BluetoothSession,
        device: &DeviceInfo,
    ) -> eros::Result<()> {
        if !device.paired {
            call(
                "BlueZ pairing",
                PAIR_TIMEOUT,
                session.pair_with_timeout(&device.id, PAIR_TIMEOUT),
            )
            .await?;
        }
        let paired = call(
            "BlueZ device lookup",
            BLUEZ_CALL_TIMEOUT,
            session.get_device_info(&device.id),
        )
        .await?
        .paired;
        eros::ensure!(paired, "BlueZ did not establish a bond");
        Ok(())
    }

    pub(crate) async fn pair(unit: [u8; 6]) -> eros::Result<()> {
        let session = session().await?;
        let mac = mac_address(odyssey_protocol::control::ble_address(unit));
        // Subscribe before looking, so a device found in between is not missed.
        let events = session
            .event_stream()
            .await
            .map_err(|e| eros::error!("BlueZ events: {e}"))?;
        let mut events = pin!(events);
        call(
            "BlueZ discovery",
            BLUEZ_CALL_TIMEOUT,
            session.start_discovery(),
        )
        .await?;
        let known = call(
            "BlueZ device list",
            BLUEZ_CALL_TIMEOUT,
            session.get_devices(),
        )
        .await?
        .into_iter()
        .find(|device| device.mac_address == mac);
        let device = if let Some(device) = known {
            device
        } else {
            let found = tokio::time::timeout(DISCOVERY_TIMEOUT, async {
                while let Some(event) = events.next().await {
                    let BluetoothEvent::Device {
                        id,
                        event: DeviceEvent::Discovered,
                    } = event
                    else {
                        continue;
                    };
                    if let Ok(info) = session.get_device_info(&id).await
                        && info.mac_address == mac
                    {
                        return Some(info);
                    }
                }
                None
            })
            .await;
            found
                .ok()
                .flatten()
                .ok_or_else(|| eros::error!("The unit is not advertising"))?
        };
        pair_device(&session, &device).await
    }

    pub(crate) async fn adapter_address() -> Option<[u8; 6]> {
        let session = session().await.ok()?;
        let adapters = call(
            "BlueZ adapter list",
            BLUEZ_CALL_TIMEOUT,
            session.get_adapters(),
        )
        .await
        .ok()?;
        adapters
            .iter()
            .find(|adapter| adapter.powered)
            .or(adapters.first())
            .map(|adapter| unit_address(adapter.mac_address))
    }

    /// Whether `BlueZ` knows `device` as a unit this host is bonded to.
    ///
    /// Nothing in a unit's advertisement is specific to it outside pairing mode and its
    /// name is user-chosen, so the GAP appearance `BlueZ` cached from the unit's GATT
    /// service is what identifies it.
    fn is_bonded_unit(device: &DeviceInfo) -> bool {
        device.paired
            && device.address_type == AddressType::Random
            && device.appearance == Some(UNIT_APPEARANCE)
    }

    /// A `BlueZ` session that lists the units bonded to this host.
    pub(crate) struct BondedUnits {
        session: BluetoothSession,
        discovering: bool,
    }

    impl BondedUnits {
        pub(crate) async fn connect() -> eros::Result<Self> {
            Ok(Self {
                session: session().await?,
                discovering: false,
            })
        }

        /// Bonded units whose advertisement `BlueZ` has seen or that it holds a link to.
        ///
        /// Starts discovery the first time a bonded unit exists, so users with none do not
        /// pay for a permanent scan.
        pub(crate) async fn in_reach(&mut self) -> eros::Result<Vec<[u8; 6]>> {
            let devices = call(
                "BlueZ device list",
                BLUEZ_CALL_TIMEOUT,
                self.session.get_devices(),
            )
            .await?;
            let units: Vec<_> = devices
                .iter()
                .filter(|device| is_bonded_unit(device))
                .collect();
            if !units.is_empty() && !self.discovering {
                call(
                    "BlueZ discovery",
                    BLUEZ_CALL_TIMEOUT,
                    self.session.start_discovery(),
                )
                .await?;
                self.discovering = true;
            }
            Ok(units
                .into_iter()
                .filter(|device| device.connected || device.rssi.is_some())
                .map(|device| unit_address(device.mac_address))
                .collect())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unit_order_is_the_reverse_of_the_bluez_display_order() {
            let unit = [0x01, 0x02, 0x03, 0x04, 0x05, 0xC6];
            let mac = mac_address(unit);
            assert_eq!(mac.to_string(), "C6:05:04:03:02:01");
            assert_eq!(unit_address(mac), unit);
        }

        #[test]
        fn appearance_is_the_generic_power_device() {
            assert_eq!(UNIT_APPEARANCE, 0x0780);
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use bluez::{BondedUnits, pair_device};

/// Pairs the system Bluetooth adapter with the unit whose BLE address is `unit`. The unit
/// must already be in pairing mode.
///
/// # Errors
/// Returns an error when `BlueZ` is unavailable, the unit does not advertise within the
/// discovery window, or the pairing is refused.
#[cfg(target_os = "linux")]
pub(crate) fn pair(unit: [u8; 6]) -> impl Future<Output = eros::Result<()>> + Send {
    bluez::pair(unit)
}

/// Identity address of the system Bluetooth adapter, if there is one.
#[cfg(target_os = "linux")]
pub(crate) fn adapter_address() -> impl Future<Output = Option<[u8; 6]>> + Send {
    bluez::adapter_address()
}

/// Pairs the system Bluetooth adapter with the unit whose BLE address is `unit`.
///
/// # Errors
/// Always: only Linux reaches units through the system Bluetooth adapter.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn pair(_unit: [u8; 6]) -> eros::Result<()> {
    eros::bail!("Pairing through the system Bluetooth adapter is only supported on Linux")
}

/// Identity address of the system Bluetooth adapter. Always `None` off Linux.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn adapter_address() -> Option<[u8; 6]> {
    None
}
