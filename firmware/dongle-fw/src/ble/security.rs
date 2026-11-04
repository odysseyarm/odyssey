use defmt::info;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::Vec;
use odyssey_protocol::mux::MAX_DEVICES;

use crate::storage::{BondData, Settings};

/// Bond waiting to be written to flash by the storage task.
pub static BOND_TO_STORE: Signal<CriticalSectionRawMutex, BondData> = Signal::new();

/// Bonds mirrored from flash for synchronous lookup. Filled at startup and
/// kept in step as bonds are added or removed.
pub static BOND_CACHE: Mutex<CriticalSectionRawMutex, Vec<BondData, MAX_DEVICES>> =
    Mutex::new(Vec::new());

// trouble-host's built-in security manager handles pairing; there is no
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

/// Replaces the cache with the bonds in flash. Call at startup before any BLE
/// operation; bonds past `MAX_DEVICES` are logged and skipped.
pub async fn load_bond_cache(settings: &Settings) {
    info!("Loading bonds into cache...");

    let bonds_settings = settings.get_all_bonds().await;
    let mut cache = BOND_CACHE.lock().await;

    cache.clear();

    for bond_opt in bonds_settings.slots.iter() {
        if let Some(bond) = bond_opt {
            if let Err(_) = cache.push(*bond) {
                defmt::error!("Bond cache full - some bonds may not be loaded");
                break;
            }
            info!("Loaded bond for device {:02x}", bond.bd_addr);
        }
    }

    info!("Loaded {} bonds into cache", cache.len());
}

/// Inserts or replaces the cached bond for `bond.bd_addr`. Call after the bond
/// is stored to flash.
pub async fn update_bond_cache(bond: BondData) {
    let mut cache = BOND_CACHE.lock().await;

    for existing in cache.iter_mut() {
        if existing.bd_addr == bond.bd_addr {
            *existing = bond;
            info!("Updated bond in cache for device {:02x}", bond.bd_addr);
            return;
        }
    }

    if let Err(_) = cache.push(bond) {
        defmt::error!(
            "Bond cache full - cannot add bond for device {:02x}",
            bond.bd_addr
        );
    } else {
        info!("Added bond to cache for device {:02x}", bond.bd_addr);
    }
}

/// Empties the cache. Call when clearing bonds, or reconnects would use the
/// stale keys.
pub async fn clear_bond_cache() {
    let mut cache = BOND_CACHE.lock().await;
    cache.clear();
    info!("Cleared bond cache");
}
