use defmt::info;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::Vec;
use protodongers::hub::MAX_DEVICES;

use crate::storage::{BondData, Settings};

/// Signal to notify async task that a new bond needs to be stored
pub static BOND_TO_STORE: Signal<CriticalSectionRawMutex, BondData> = Signal::new();

/// In-memory cache of bonds for fast synchronous lookup
/// Loaded at startup and updated when bonds are added/removed
pub static BOND_CACHE: Mutex<CriticalSectionRawMutex, Vec<BondData, MAX_DEVICES>> =
    Mutex::new(Vec::new());

// Security is handled by trouble-host's built-in manager; no extra handler needed here.

/// Convert trouble-host BondInformation into our persisted BondData format.
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

/// Convert our persisted BondData into trouble-host BondInformation.
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

/// Load all bonds from flash into the in-memory cache
/// Should be called at startup before BLE operations begin
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

/// Update the bond cache with a new or updated bond
/// Should be called after storing a bond to flash
pub async fn update_bond_cache(bond: BondData) {
    let mut cache = BOND_CACHE.lock().await;

    // Check if bond already exists (update)
    for existing in cache.iter_mut() {
        if existing.bd_addr == bond.bd_addr {
            *existing = bond;
            info!("Updated bond in cache for device {:02x}", bond.bd_addr);
            return;
        }
    }

    // Add new bond
    if let Err(_) = cache.push(bond) {
        defmt::error!(
            "Bond cache full - cannot add bond for device {:02x}",
            bond.bd_addr
        );
    } else {
        info!("Added bond to cache for device {:02x}", bond.bd_addr);
    }
}

/// Clear all bonds from the cache
/// Should be called when clearing bonds to prevent stale bonds from being used
pub async fn clear_bond_cache() {
    let mut cache = BOND_CACHE.lock().await;
    cache.clear();
    info!("Cleared bond cache");
}
