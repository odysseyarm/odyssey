//! Flash-backed settings (bonds and device names) stored with cfg-noodle.

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

use core::sync::atomic::{AtomicBool, Ordering};

use bt_hci::param::BdAddr;
use cfg_noodle::flash::Flash;
use cfg_noodle::minicbor::{CborLen, Decode, Encode};
use cfg_noodle::mutex::raw_impls::cs::CriticalSectionRawMutex as NoodleRawMutex;
use cfg_noodle::sequential_storage::cache::NoCache;
use cfg_noodle::{NdlDataStorage, StorageList, StorageListNode};
use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_nrf::nvmc::PAGE_SIZE;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, WithTimeout};
use odyssey_protocol::mux::MAX_DEVICES;
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

pub static LIST: StorageList<NoodleRawMutex> = StorageList::new();

static NODE_BONDS: StorageListNode<BondsSettings> = StorageListNode::new("settings/bonds");
static NODE_ACCESSORY_BONDS: StorageListNode<AccessoryBondsSettings> =
    StorageListNode::new("settings/accessory_bonds");
static COMMIT_PENDING: AtomicBool = AtomicBool::new(false);
static COMMIT_RESULT: Signal<ThreadModeRawMutex, Result<(), ()>> = Signal::new();
static ACCESSORY_STORES: embassy_sync::channel::Channel<
    ThreadModeRawMutex,
    (BondData, usize, u64),
    3,
> = embassy_sync::channel::Channel::new();
static ACCESSORY_STORE_RESULTS: [Signal<ThreadModeRawMutex, (u64, Result<(), ()>)>; 3] =
    [const { Signal::new() }; 3];

pub async fn persist_accessory_bond(bond: BondData, slot: usize, token: u64) -> Result<(), ()> {
    ACCESSORY_STORES
        .try_send((bond, slot, token))
        .map_err(|_| ())?;
    loop {
        let (completed, result) = ACCESSORY_STORE_RESULTS[slot].wait().await;
        if completed == token {
            return result;
        }
    }
}

#[embassy_executor::task]
pub async fn accessory_bond_worker(settings: &'static Settings) {
    loop {
        let (bond, slot, token) = ACCESSORY_STORES.receive().await;
        let result = settings.store_accessory_bond(bond).await;
        ACCESSORY_STORE_RESULTS[slot].signal((token, result));
    }
}

pub const ACCESSORY_BOND_CAPACITY: usize = 6;

#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct AccessoryBondsSettings {
    #[n(0)]
    pub slots: [Option<BondData>; ACCESSORY_BOND_CAPACITY],
}

impl Default for AccessoryBondsSettings {
    fn default() -> Self {
        Self {
            slots: [None; ACCESSORY_BOND_CAPACITY],
        }
    }
}
static NODE_DEVICE_NAMES: StorageListNode<DeviceNamesSettings> =
    StorageListNode::new("settings/device_names");

// The scan list is not persisted: it is rebuilt from the bonds at boot and
// otherwise only holds pairing targets added at runtime.

static SETTINGS_FLUSH: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Flash goes through nrf_mpsl::Flash so NVMC operations do not collide with
// radio events.

pub static SETTINGS_CELL: StaticCell<Settings> = StaticCell::new();

// cfg-noodle's scratch buffer is static. On the task stack it would overflow.
const STORAGE_BUF_SIZE: usize =
    <Flash<nrf_mpsl::Flash<'static>, NoCache> as NdlDataStorage>::MAX_ELEM_SIZE;
static STORAGE_BUF: StaticCell<[u8; STORAGE_BUF_SIZE]> = StaticCell::new();
static STORAGE_FLASH: StaticCell<Flash<nrf_mpsl::Flash<'static>, NoCache>> = StaticCell::new();

/// One device's bond: address, LTK, IRK and security level.
pub type BondData = odyssey_protocol::control::BondEntry;

/// Bond slots for up to `MAX_DEVICES` devices.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct BondsSettings {
    /// `None` marks a free slot.
    #[n(0)]
    pub slots: [Option<BondData>; MAX_DEVICES],
}

/// Addresses the central connects to. Runtime only, never written to flash.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct ScanListSettings {
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

/// A device's BLE address and its UTF-8 name, up to 32 bytes.
#[derive(Debug, Clone, Copy, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct DeviceNameEntry {
    #[n(0)]
    pub bd_addr: [u8; 6],
    #[n(1)]
    pub name_bytes: [u8; 32],
    #[n(2)]
    pub name_len: u8,
}

impl DeviceNameEntry {
    pub fn name(&self) -> heapless::String<32> {
        let len = self.name_len.min(32) as usize;
        let mut s = heapless::String::new();
        if let Ok(text) = core::str::from_utf8(&self.name_bytes[..len]) {
            for c in text.chars().take(32) {
                let _ = s.push(c);
            }
        }
        s
    }
}

/// Name slots for up to `MAX_DEVICES` devices.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cbor(map)]
pub struct DeviceNamesSettings {
    #[n(0)]
    pub entries: [Option<DeviceNameEntry>; MAX_DEVICES],
}

impl Default for DeviceNamesSettings {
    fn default() -> Self {
        Self {
            entries: [None; MAX_DEVICES],
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

    // Static rather than on the task stack, which it would overflow.
    let buffer = STORAGE_BUF.init([0u8; STORAGE_BUF_SIZE]);

    // The worker starts at once. There is no SoftDevice readiness to wait for.
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
            // Wakes on a queued write or an explicit flush.
            match select(wait_write, SETTINGS_FLUSH.wait()).await {
                Either::First(_) => {}
                Either::Second(_) => {}
            }
        };

        // Flushes eventually even if nothing signals.
        let tick_fut = Timer::after(auto_interval);

        match embassy_futures::select::select(read_fut, select(write_fut, tick_fut)).await {
            Either::First(_) => {
                defmt::debug!("Storage: starting process_reads");
                match LIST.process_reads(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::debug!("process_reads ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_reads err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_reads");
            }
            Either::Second(Either::First(_) | Either::Second(_)) => {
                defmt::debug!("Storage: starting process_writes (triggered by write/tick)");
                let commit_started = COMMIT_PENDING.swap(false, Ordering::AcqRel);
                let write_result = LIST.process_writes(&mut *flash, &mut buffer[..]).await;
                match &write_result {
                    Ok(rpt) => defmt::debug!("process_writes ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_writes err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_writes");

                defmt::debug!("Storage: starting process_garbage");
                match LIST.process_garbage(&mut *flash, &mut buffer[..]).await {
                    Ok(rpt) => defmt::trace!("process_garbage ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_garbage err: {:?}", e),
                }
                defmt::debug!("Storage: finished process_garbage");

                let committed = if write_result.is_err() {
                    LIST.process_writes(&mut *flash, &mut buffer[..])
                        .await
                        .is_ok()
                } else {
                    true
                };
                if commit_started {
                    COMMIT_RESULT.signal(if committed { Ok(()) } else { Err(()) });
                }
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
    accessory_bonds: AsyncMutex<ThreadModeRawMutex, AccessoryBondsSettings>,
    scan_list: AsyncMutex<ThreadModeRawMutex, ScanListSettings>,
    device_names: AsyncMutex<ThreadModeRawMutex, DeviceNamesSettings>,
}

impl Settings {
    pub async fn get_accessory_bonds(&self) -> AccessoryBondsSettings {
        self.accessory_bonds.lock().await.clone()
    }

    pub async fn get_accessory_bond(&self, address: [u8; 6]) -> Option<BondData> {
        self.accessory_bonds
            .lock()
            .await
            .slots
            .iter()
            .flatten()
            .find(|b| b.bd_addr == address)
            .copied()
    }

    async fn store_accessory_bond(&self, bond: BondData) -> Result<(), ()> {
        let mut saved = self.accessory_bonds.lock().await;
        let mut next = saved.clone();
        let slot = next
            .slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|b| b.bd_addr == bond.bd_addr))
            .or_else(|| next.slots.iter().position(Option::is_none))
            .ok_or(())?;
        next.slots[slot] = Some(bond);
        let mut node = NODE_ACCESSORY_BONDS.attach(&LIST).await.map_err(|_| ())?;
        node.write(&next).await.map_err(|_| ())?;
        COMMIT_RESULT.reset();
        COMMIT_PENDING.store(true, Ordering::Release);
        settings_flush_now();
        let result = COMMIT_RESULT.wait().await;
        if result.is_ok() {
            *saved = next;
            Ok(())
        } else {
            COMMIT_PENDING.store(false, Ordering::Release);
            node.write(&*saved).await.map_err(|_| ())?;
            settings_flush_now();
            Err(())
        }
    }

    #[allow(dead_code)]
    async fn bonds_write(&self) {
        let bonds = self.bonds.lock().await;
        let mut h = NODE_BONDS.attach(&LIST).await.unwrap();
        h.write(&*bonds).await.unwrap();
        drop(bonds);
        settings_flush_now();
    }

    #[allow(dead_code)]
    pub async fn bonds_clear(&self) {
        let mut bonds = self.bonds.lock().await;
        *bonds = BondsSettings::default();
        drop(bonds);
        self.bonds_write().await;
    }

    /// Replaces the bond for `bond.bd_addr` or takes a free slot, then queues a
    /// flash write.
    ///
    /// # Errors
    ///
    /// Fails when every slot is taken by another device.
    #[allow(dead_code)]
    pub async fn add_bond(&self, bond: BondData) -> Result<(), ()> {
        let mut bonds = self.bonds.lock().await;

        for slot in bonds.slots.iter_mut() {
            if let Some(existing) = slot {
                if existing.bd_addr == bond.bd_addr {
                    *existing = bond;
                    info!("Updated bond for device {:02x}", bond.bd_addr);
                    drop(bonds);
                    self.bonds_write().await;
                    return Ok(());
                }
            }
        }

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

    /// Frees the slot for `bd_addr` and queues a flash write.
    ///
    /// # Errors
    ///
    /// Fails when no bond for `bd_addr` exists.
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

    #[allow(dead_code)]
    pub async fn get_all_bonds(&self) -> BondsSettings {
        let bonds = self.bonds.lock().await;
        bonds.clone()
    }

    /// Adds `address` to the in-memory scan list. When the list is full the
    /// oldest entry (index 0) is dropped. Always succeeds.
    #[allow(dead_code)]
    pub async fn add_scan_target(&self, address: [u8; 6]) -> Result<(), ()> {
        let mut scan_list = self.scan_list.lock().await;

        for slot in scan_list.addresses.iter() {
            if let Some(addr) = slot {
                if addr == &address {
                    info!("Address {:02x} already in scan list", address);
                    return Ok(());
                }
            }
        }

        if let Some(slot) = scan_list.addresses.iter_mut().find(|s| s.is_none()) {
            *slot = Some(address);
        } else {
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

    /// Removes `address` from the in-memory scan list.
    ///
    /// # Errors
    ///
    /// Fails when `address` is not in the list.
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

    pub async fn get_scan_targets(&self) -> heapless::Vec<[u8; 6], MAX_DEVICES> {
        let scan_list = self.scan_list.lock().await;
        scan_list
            .addresses
            .iter()
            .filter_map(|&addr| addr)
            .collect()
    }

    pub async fn scan_list_clear(&self) {
        let mut scan_list = self.scan_list.lock().await;
        *scan_list = ScanListSettings::default();
        info!("Cleared all scan targets");
    }

    async fn device_names_write(&self) {
        let names = self.device_names.lock().await;
        let mut h = NODE_DEVICE_NAMES.attach(&LIST).await.unwrap();
        h.write(&*names).await.unwrap();
        drop(names);
        settings_flush_now();
    }

    pub async fn set_device_name(&self, bd_addr: [u8; 6], name: &heapless::String<32>) {
        let mut names = self.device_names.lock().await;
        for entry in names.entries.iter_mut() {
            if let Some(e) = entry {
                if e.bd_addr == bd_addr {
                    let bytes = name.as_bytes();
                    let len = bytes.len().min(32) as u8;
                    e.name_bytes[..len as usize].copy_from_slice(&bytes[..len as usize]);
                    e.name_len = len;
                    drop(names);
                    self.device_names_write().await;
                    return;
                }
            }
        }
        for entry in names.entries.iter_mut() {
            if entry.is_none() {
                let bytes = name.as_bytes();
                let len = bytes.len().min(32) as u8;
                let mut name_bytes = [0u8; 32];
                name_bytes[..len as usize].copy_from_slice(&bytes[..len as usize]);
                *entry = Some(DeviceNameEntry {
                    bd_addr,
                    name_bytes,
                    name_len: len,
                });
                drop(names);
                self.device_names_write().await;
                return;
            }
        }
        defmt::warn!("No empty device name slot for {:02x}", bd_addr);
    }

    pub async fn get_device_name(&self, bd_addr: &[u8; 6]) -> heapless::String<32> {
        let names = self.device_names.lock().await;
        for entry in names.entries.iter() {
            if let Some(e) = entry {
                if &e.bd_addr == bd_addr {
                    let name = e.name();
                    defmt::info!("get_device_name {:02x} -> {:?}", bd_addr, name.as_str());
                    return name;
                }
            }
        }
        defmt::info!("get_device_name {:02x} -> (not found in flash)", bd_addr);
        heapless::String::new()
    }

    pub async fn clear_device_names(&self) {
        let mut names = self.device_names.lock().await;
        *names = DeviceNamesSettings::default();
        drop(names);
        self.device_names_write().await;
    }
}

/// Spawns the flash worker, loads bonds and names, and seeds the scan list
/// from the bonds. Call once.
///
/// # Panics
///
/// Panics if the linker-provided settings region is smaller than two flash
/// pages, or if called twice.
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

    // Spawn the worker before attaching nodes so it can service their initial reads.
    // Periodic write pass every 10 s.
    spawner.spawn(unwrap!(settings_worker(
        flash_dev,
        region.clone(),
        Duration::from_secs(10),
    )));

    // The worker fills these in from flash as they attach.
    let h_bonds = NODE_BONDS
        .attach_with_default(&LIST, BondsSettings::default)
        .await
        .unwrap();
    let h_device_names = NODE_DEVICE_NAMES
        .attach_with_default(&LIST, DeviceNamesSettings::default)
        .await
        .unwrap();
    let h_accessory_bonds = NODE_ACCESSORY_BONDS
        .attach_with_default(&LIST, AccessoryBondsSettings::default)
        .await
        .unwrap();

    let bonds_data = h_bonds.load();
    let bond_count = bonds_data.slots.iter().filter(|b| b.is_some()).count();
    info!("Loaded {} bonds from flash", bond_count);

    let names_data = h_device_names.load();
    let names_count = names_data.entries.iter().filter(|e| e.is_some()).count();
    info!("Loaded {} device names from flash", names_count);

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
        accessory_bonds: AsyncMutex::new(h_accessory_bonds.load()),
        scan_list: AsyncMutex::new(scan_list_data),
        device_names: AsyncMutex::new(names_data),
    };

    SETTINGS_CELL.init(settings)
}
