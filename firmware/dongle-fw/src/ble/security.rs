use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use odyssey_protocol::mux::MAX_DEVICES;

use crate::storage::BondData;

/// Commands for the single task that owns bond persistence. Clearing through
/// the same queue prevents an in-flight store from resurrecting a cleared bond.
pub enum BondCommand {
    Store(BondData),
    Clear,
}

/// One slot per device task, so simultaneous pairing completions all survive.
pub static BOND_COMMANDS: Channel<CriticalSectionRawMutex, BondCommand, MAX_DEVICES> =
    Channel::new();

// trouble-host's built-in security manager handles pairing. There is no
// handler of our own.

/// Converts a trouble-host bond into the form persisted in flash.
pub fn bonddata_from_info(info: &trouble_host::prelude::BondInformation) -> BondData {
    let mut bd_addr = [0u8; 6];
    bd_addr.copy_from_slice(info.identity.bd_addr.raw());

    let security_level = match info.security_level {
        trouble_host::prelude::SecurityLevel::NoEncryption => 0,
        trouble_host::prelude::SecurityLevel::Encrypted => 1,
        trouble_host::prelude::SecurityLevel::EncryptedAuthenticated => 2,
    };

    let irk = info.identity.irk.map(|irk| irk.to_le_bytes());

    BondData {
        bd_addr,
        ltk: info.ltk.to_le_bytes(),
        security_level,
        is_bonded: info.is_bonded,
        irk,
    }
}

/// Converts a persisted bond back into trouble-host's form. An unknown
/// `security_level` maps to `NoEncryption`.
pub fn info_from_bonddata(bd: &BondData) -> trouble_host::prelude::BondInformation {
    let security_level = match bd.security_level {
        0 => trouble_host::prelude::SecurityLevel::NoEncryption,
        1 => trouble_host::prelude::SecurityLevel::Encrypted,
        2 => trouble_host::prelude::SecurityLevel::EncryptedAuthenticated,
        _ => trouble_host::prelude::SecurityLevel::NoEncryption,
    };
    let irk = bd
        .irk
        .map(|irk| trouble_host::prelude::IdentityResolvingKey::from_le_bytes(irk));

    trouble_host::prelude::BondInformation {
        identity: trouble_host::prelude::Identity {
            bd_addr: trouble_host::prelude::BdAddr::new(bd.bd_addr),
            irk,
        },
        security_level,
        is_bonded: bd.is_bonded,
        ltk: trouble_host::prelude::LongTermKey::from_le_bytes(bd.ltk),
    }
}
