use defmt::info;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::Vec;
use nrf_softdevice::ble::security::{IoCapabilities, SecurityHandler};
use nrf_softdevice::ble::{Connection, EncryptionInfo, IdentityKey, MasterId, SecurityMode};
use protodongers::hub::MAX_DEVICES;

use crate::storage::{BondData, Settings};

/// Signal to notify async task that a new bond needs to be stored
pub static BOND_TO_STORE: Signal<CriticalSectionRawMutex, BondData> = Signal::new();

/// In-memory cache of bonds for fast synchronous lookup
/// Loaded at startup and updated when bonds are added/removed
pub static BOND_CACHE: Mutex<CriticalSectionRawMutex, Vec<BondData, MAX_DEVICES>> =
    Mutex::new(Vec::new());

pub struct DongleSecurityHandler {
    #[allow(dead_code)]
    settings: &'static Settings,
}

impl DongleSecurityHandler {
    pub const fn new(settings: &'static Settings) -> Self {
        Self { settings }
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

impl SecurityHandler for DongleSecurityHandler {
    fn io_capabilities(&self) -> IoCapabilities {
        IoCapabilities::None // No display or keyboard - Just Works pairing
    }

    fn can_bond(&self, _conn: &Connection) -> bool {
        true // We want to bond with all devices
    }

    fn display_passkey(&self, passkey: &[u8; 6]) {
        info!("Passkey for bonding: {:?}", passkey);
    }

    fn on_bonded(
        &self,
        _conn: &Connection,
        _master_id: MasterId,
        enc_info: EncryptionInfo,
        peer_id: IdentityKey,
    ) {
        info!("Bonded! Storing keys...");

        let address = peer_id.addr;
        let bd_addr: [u8; 6] = address.bytes;

        let bond = BondData {
            bd_addr,
            ltk: enc_info.ltk,
            security_level: 2, // Authenticated
            is_bonded: true,
            irk: Some(peer_id.irk.as_raw().irk),
        };

        // Signal async task to store bond to flash
        // This is safe to call from sync context
        BOND_TO_STORE.signal(bond);
        info!("Bond queued for storage: device {:02x}", bd_addr);
    }

    fn get_key(&self, _conn: &Connection, _master_id: MasterId) -> Option<EncryptionInfo> {
        // Peripheral role - not used in our central-only implementation
        None
    }

    fn get_peripheral_key(&self, conn: &Connection) -> Option<(MasterId, EncryptionInfo)> {
        // This is called when reconnecting to a bonded peripheral
        // Look up the bond from our in-memory cache
        let peer_addr = conn.peer_address();
        let bd_addr: [u8; 6] = peer_addr.bytes;

        info!("Looking up bond for device {:02x}", bd_addr);

        // Try to lock the cache (should be fast with CriticalSectionRawMutex)
        if let Ok(bonds) = BOND_CACHE.try_lock() {
            for bond in bonds.iter() {
                if bond.bd_addr == bd_addr {
                    info!("Found bond for device {:02x}", bd_addr);

                    // Create MasterId (all zeros since we don't track EDIV/RAND)
                    let master_id = MasterId::from_raw(nrf_softdevice::raw::ble_gap_master_id_t {
                        ediv: 0,
                        rand: [0; 8],
                    });

                    // Create EncryptionInfo from stored LTK
                    // flags byte: bit 0 = lesc, bit 1 = auth, bits 2-7 = ltk_len
                    let flags = if bond.security_level >= 2 { 0x02 } else { 0x00 }; // auth bit
                    let flags = flags | (16 << 2); // ltk_len = 16 bytes (128 bits)

                    let enc_info = EncryptionInfo {
                        ltk: bond.ltk,
                        flags,
                    };

                    return Some((master_id, enc_info));
                }
            }
        }

        info!("No bond found for device {:02x}", bd_addr);
        None
    }

    fn on_security_update(&self, _conn: &Connection, security_mode: SecurityMode) {
        info!("Security updated: {:?}", security_mode);
    }
}
