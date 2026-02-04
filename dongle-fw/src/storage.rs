//! Storage backed by cfg-noodle for BLE bonds.

mod settings_region {
    unsafe extern "C" {
        static __settings_start: u32;
        static __settings_end: u32;
    }
    pub fn get_settings_region() -> core::ops::Range<u32> {
        let s = core::ptr::addr_of!(__settings_start) as u32;
        let e = core::ptr::addr_of!(__settings_end) as u32;
        s..e
    }
}

use bt_hci::param::BdAddr;
use cfg_noodle::NdlDataStorage;
use cfg_noodle::flash::Flash;
use cfg_noodle::minicbor::{CborLen, Decode, Encode};
use cfg_noodle::mutex::raw_impls::cs::CriticalSectionRawMutex as NoodleRawMutex;
use cfg_noodle::sequential_storage::cache::NoCache;
use cfg_noodle::{StorageList, StorageListNode};
use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_nrf::nvmc::PAGE_SIZE;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, WithTimeout};
use protodongers::mux::MAX_DEVICES;
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

pub static LIST: StorageList<NoodleRawMutex> = StorageList::new();

static NODE_BONDS: StorageListNode<BondsSettings> = StorageListNode::new("settings/bonds");

// Scan list is no longer persisted - it's derived from bonds on boot
// and only holds temporary pairing targets during runtime

static SETTINGS_FLUSH: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Flash access is coordinated through MPSL to avoid conflicts with radio operations

pub static SETTINGS_CELL: StaticCell<Settings> = StaticCell::new();

// Use a static scratch buffer for cfg-noodle to avoid large task stack usage
const STORAGE_BUF_SIZE: usize =
    <Flash<nrf_mpsl::Flash<'static>, NoCache> as NdlDataStorage>::MAX_ELEM_SIZE;
static STORAGE_BUF: StaticCell<[u8; STORAGE_BUF_SIZE]> = StaticCell::new();
static STORAGE_FLASH: StaticCell<Flash<nrf_mpsl::Flash<'static>, NoCache>> = StaticCell::new();

/// BLE bond information for a single device
#[derive(Debug, Clone, Copy, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct BondData {
    /// Device BD address
    #[n(0)]
    pub bd_addr: [u8; 6],
    /// Long Term Key
    #[n(1)]
    pub ltk: [u8; 16],
    /// Security level (0=none, 1=encrypted, 2=authenticated)
    #[n(2)]
    pub security_level: u8,
    /// Whether bonded
    #[n(3)]
    pub is_bonded: bool,
    /// Identity Resolving Key (optional)
    #[n(4)]
    pub irk: Option<[u8; 16]>,
}

/// Storage for all BLE bonds (up to MAX_DEVICES)
#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct BondsSettings {
    /// Array of bond slots (Some = bond present, None = empty slot)
    #[n(0)]
    pub slots: [Option<BondData>; MAX_DEVICES],
}

/// List of device addresses to scan for and connect to
#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct ScanListSettings {
    /// Array of addresses to scan for
    #[n(0)]
    pub addresses: [Option<[u8; 6]>; MAX_DEVICES],
}

impl Default for ScanListSettings {
    fn default() -> Self {
        Self {
            addresses: [None; MAX_DEVICES],
        }
    }
}

impl Default for BondsSettings {
    fn default() -> Self {
        Self {
            slots: [None; MAX_DEVICES],
        }
    }
}

#[embassy_executor::task]
async fn settings_worker(
    flash_dev: nrf_mpsl::Flash<'static>,
    region: core::ops::Range<u32>,
    auto_interval: Duration,
) {
    let flash = STORAGE_FLASH.init(Flash::new(flash_dev, region, NoCache::new()));

    // Buffer for cfg-noodle operations (static to prevent task stack overflow)
    let buffer = STORAGE_BUF.init([0u8; STORAGE_BUF_SIZE]);

    // Wait until SoftDevice task signals readiness before using flash
    // Start immediately; no need to wait for SoftDevice readiness.
    loop {
        let read_fut = async {
            LIST.needs_read().await;
            loop {
                if LIST
                    .needs_read()
                    .with_timeout(Duration::from_millis(100))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        };

        let write_fut = async {
            let wait_write = async {
                LIST.needs_write().await;
                loop {
                    if LIST
                        .needs_write()
                        .with_timeout(Duration::from_millis(1000))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            };
            // Queued write OR explicit flush signal to wake
            match select(wait_write, SETTINGS_FLUSH.wait()).await {
                Either::First(_) => {}
                Either::Second(_) => {}
            }
        };

        // Periodic tick so data eventually flushes without signals
        let tick_fut = Timer::after(auto_interval);

        match embassy_futures::select::select(read_fut, select(write_fut, tick_fut)).await {
            // Reads ready
            Either::First(_) => {
                defmt::debug!("Storage: starting process_reads");
                match LIST.process_reads(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::debug!("process_reads ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_reads err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_reads");
            }
            // Write wake (queued write or explicit flush)
            Either::Second(Either::First(_)) => {
                defmt::debug!("Storage: starting process_writes (triggered by write/tick)");
                match LIST.process_writes(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::debug!("process_writes ok: {:?}", rpt),
                    Err(e) => defmt::debug!("process_writes err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_writes");

                // GC after writes
                defmt::debug!("Storage: starting process_garbage");
                match LIST.process_garbage(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::trace!("process_garbage ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_garbage err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_garbage");
            }
            // Periodic tick: attempt a single write pass, same as vm-fw
            Either::Second(Either::Second(_)) => {
                defmt::debug!("Storage: starting process_writes (triggered by write/tick)");
                match LIST.process_writes(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::debug!("process_writes ok: {:?}", rpt),
                    Err(e) => defmt::debug!("process_writes err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_writes");

                // GC after writes
                defmt::debug!("Storage: starting process_garbage");
                match LIST.process_garbage(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::trace!("process_garbage ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_garbage err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_garbage");
            }
        }
    }
}

#[inline]
pub fn settings_flush_now() {
    SETTINGS_FLUSH.signal(());
}

pub struct Settings {
    #[allow(dead_code)]
    bonds: AsyncMutex<ThreadModeRawMutex, BondsSettings>,
    scan_list: AsyncMutex<ThreadModeRawMutex, ScanListSettings>,
}

impl Settings {
    /// Write bonds to flash
    #[allow(dead_code)]
    async fn bonds_write(&self) {
        let bonds = self.bonds.lock().await;
        let mut h = NODE_BONDS.attach(&LIST).await.unwrap();
        h.write(&*bonds).await.unwrap();
        drop(bonds);
        settings_flush_now();
    }

    /// Clear all bonds
    #[allow(dead_code)]
    pub async fn bonds_clear(&self) {
        let mut bonds = self.bonds.lock().await;
        *bonds = BondsSettings::default();
        drop(bonds);
        self.bonds_write().await;
    }

    /// Add or update a bond for a device
    #[allow(dead_code)]
    pub async fn add_bond(&self, bond: BondData) -> Result<(), ()> {
        let mut bonds = self.bonds.lock().await;

        // Find existing slot or empty slot
        for slot in bonds.slots.iter_mut() {
            if let Some(existing) = slot {
                if existing.bd_addr == bond.bd_addr {
                    // Update existing
                    *existing = bond;
                    info!("Updated bond for device {:02x}", bond.bd_addr);
                    drop(bonds);
                    self.bonds_write().await;
                    return Ok(());
                }
            }
        }

        // Find empty slot
        for slot in bonds.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(bond);
                info!("Added bond for device {:02x}", bond.bd_addr);
                drop(bonds);
                self.bonds_write().await;
                return Ok(());
            }
        }

        defmt::warn!("No empty bond slot available");
        Err(())
    }

    /// Remove a bond for a device
    #[allow(dead_code)]
    pub async fn remove_bond(&self, bd_addr: &[u8; 6]) -> Result<(), ()> {
        let mut bonds = self.bonds.lock().await;

        for slot in bonds.slots.iter_mut() {
            if let Some(bond) = slot {
                if &bond.bd_addr == bd_addr {
                    *slot = None;
                    info!("Removed bond for device {:02x}", bd_addr);
                    drop(bonds);
                    self.bonds_write().await;
                    return Ok(());
                }
            }
        }

        defmt::warn!("Bond for device {:02x} not found", bd_addr);
        Err(())
    }

    /// Get bond for a device
    #[allow(dead_code)]
    pub async fn get_bond(&self, bd_addr: BdAddr) -> Option<BondData> {
        let bonds = self.bonds.lock().await;
        for bond_opt in bonds.slots.iter() {
            if let Some(bond) = bond_opt {
                if bond.bd_addr == bd_addr.into_inner() {
                    return Some(*bond);
                }
            }
        }
        None
    }

    /// Get all bonds
    #[allow(dead_code)]
    pub async fn get_all_bonds(&self) -> BondsSettings {
        let bonds = self.bonds.lock().await;
        bonds.clone()
    }

    // scan_list_write removed - scan list is no longer persisted to flash

    /// Add an address to the scan list (runtime only, not persisted)
    #[allow(dead_code)]
    pub async fn add_scan_target(&self, address: [u8; 6]) -> Result<(), ()> {
        let mut scan_list = self.scan_list.lock().await;

        // Check if already exists
        for slot in scan_list.addresses.iter() {
            if let Some(addr) = slot {
                if addr == &address {
                    info!("Address {:02x} already in scan list", address);
                    return Ok(());
                }
            }
        }

        // Find empty slot, otherwise overwrite oldest entry (index 0) by shifting left
        if let Some(slot) = scan_list.addresses.iter_mut().find(|s| s.is_none()) {
            *slot = Some(address);
        } else {
            // shift left and place at end
            for i in 0..(scan_list.addresses.len() - 1) {
                scan_list.addresses[i] = scan_list.addresses[i + 1];
            }
            let last = scan_list.addresses.len() - 1;
            scan_list.addresses[last] = Some(address);
            defmt::warn!("Scan list full, overwrote oldest with {:02x}", address);
        }
        info!("Added {:02x} to scan list", address);
        Ok(())
    }

    /// Remove an address from the scan list (runtime only)
    #[allow(dead_code)]
    pub async fn remove_scan_target(&self, address: &[u8; 6]) -> Result<(), ()> {
        let mut scan_list = self.scan_list.lock().await;

        for slot in scan_list.addresses.iter_mut() {
            if let Some(addr) = slot {
                if addr == address {
                    *slot = None;
                    info!("Removed {:02x} from scan list", address);
                    return Ok(());
                }
            }
        }

        defmt::warn!("Address {:02x} not found in scan list", address);
        Err(())
    }

    /// Get all addresses in the scan list
    pub async fn get_scan_targets(&self) -> heapless::Vec<[u8; 6], MAX_DEVICES> {
        let scan_list = self.scan_list.lock().await;
        scan_list
            .addresses
            .iter()
            .filter_map(|&addr| addr)
            .collect()
    }

    /// Clear all scan targets (runtime only)
    pub async fn scan_list_clear(&self) {
        let mut scan_list = self.scan_list.lock().await;
        *scan_list = ScanListSettings::default();
        info!("Cleared all scan targets");
    }
}

/// Initialize settings subsystem
pub async fn init_settings(
    spawner: &Spawner,
    flash_dev: nrf_mpsl::Flash<'static>,
) -> &'static mut Settings {
    let region = get_settings_region();
    assert!((region.end - region.start) as usize >= 2 * PAGE_SIZE);

    info!(
        "Initializing settings at 0x{:08x}..0x{:08x}",
        region.start, region.end
    );
    info!(
        "cfg-noodle MAX_ELEM_SIZE={}, buf_size={}",
        STORAGE_BUF_SIZE, STORAGE_BUF_SIZE
    );

    // Start I/O worker first so it can service read hydration for attached nodes
    // TEMP: Use very long interval to avoid periodic flash writes during debugging
    spawner.spawn(unwrap!(settings_worker(
        flash_dev,
        region.clone(),
        Duration::from_secs(10),
    )));

    // Attach nodes (worker will hydrate once SoftDevice is ready)
    let h_bonds = NODE_BONDS
        .attach_with_default(&LIST, BondsSettings::default)
        .await
        .unwrap();

    let bonds_data = h_bonds.load();
    let bond_count = bonds_data.slots.iter().filter(|b| b.is_some()).count();
    info!("Loaded {} bonds from flash", bond_count);

    // Populate scan list from bonds (not persisted to flash)
    let mut scan_list_data = ScanListSettings::default();
    for (i, bond_opt) in bonds_data.slots.iter().enumerate() {
        if let Some(bond) = bond_opt {
            scan_list_data.addresses[i] = Some(bond.bd_addr);
        }
    }
    let scan_count = scan_list_data
        .addresses
        .iter()
        .filter(|a| a.is_some())
        .count();
    info!("Populated {} scan targets from bonds", scan_count);

    let settings = Settings {
        bonds: AsyncMutex::new(bonds_data),
        scan_list: AsyncMutex::new(scan_list_data),
    };

    SETTINGS_CELL.init(settings)
}

// Softdevice gating removed: storage starts immediately.
