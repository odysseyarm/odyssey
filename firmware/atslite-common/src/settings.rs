//! Flash-backed settings for atslite boards, stored with cfg-noodle: the BLE bond, general
//! device settings and PAG sensor settings.

mod settings_region {
    unsafe extern "C" {
        static __settings_start: u32;
        static __settings_end: u32;
    }
    /// Since: Odyssey Desktop 0.7.0.
    pub fn get_settings_region() -> core::ops::Range<u32> {
        let s = core::ptr::addr_of!(__settings_start) as u32;
        let e = core::ptr::addr_of!(__settings_end) as u32;
        s..e
    }
}

use core::cell::RefCell;

use cfg_noodle::flash::Flash;
use cfg_noodle::minicbor::{CborLen, Decode, Encode};
use cfg_noodle::mutex::raw_impls::cs::CriticalSectionRawMutex as NoodleRawMutex;
use cfg_noodle::sequential_storage::cache::NoCache;
use cfg_noodle::{StorageList, StorageListNode, minicbor};
use defmt::Format;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, WithTimeout as _};
/// Since: Odyssey Desktop 0.7.0.
pub use settings_region::get_settings_region;

/// Since: Odyssey Desktop 0.7.0.
pub static LIST: StorageList<NoodleRawMutex> = StorageList::new();

// Firmware before multi-bond support kept one bond here. Read once to migrate it.
static NODE_BLE_BOND: StorageListNode<BleBondSettings> = StorageListNode::new("settings/ble_bond");
static NODE_BLE_BONDS: StorageListNode<BleBonds> = StorageListNode::new("settings/ble_bonds");
static NODE_GENERAL: StorageListNode<GeneralSettings> = StorageListNode::new("settings/general");
static NODE_PAG: StorageListNode<PagSettings> = StorageListNode::new("settings/pag");

static SETTINGS_FLUSH: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Raised after the worker has finished a write pass. A reset between the
/// request and this point interrupts an NVMC erase, which loses the change and
/// can leave the node undecodable.
static SETTINGS_SETTLED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// Access is synchronous on the single-threaded application executor: a settings borrow never
// survives an await and cannot alias the object-mode, USB-control or BLE handlers.
static SETTINGS: Mutex<ThreadModeRawMutex, RefCell<Option<Settings>>> =
    Mutex::new(RefCell::new(None));

/// Since: Odyssey Desktop 0.7.0.
pub fn publish(settings: Settings) {
    SETTINGS.lock(|cell| {
        let mut slot = cell.borrow_mut();
        assert!(slot.is_none(), "settings already initialized");
        *slot = Some(settings);
    });
}

/// Since: Odyssey Desktop 0.7.0.
pub fn with_settings<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    SETTINGS.lock(|cell| {
        let mut slot = cell.borrow_mut();
        f(slot.as_mut().expect("settings not initialized"))
    })
}

/// Since: Odyssey Desktop 0.7.0.
pub fn is_initialized() -> bool {
    SETTINGS.lock(|cell| cell.borrow().is_some())
}

#[embassy_executor::task]
async fn settings_worker(
    flash_dev: embassy_embedded_hal::adapter::BlockingAsync<Nvmc<'static>>,
    region: core::ops::Range<u32>,
    auto_interval: Duration,
) {
    let mut flash = Flash::new(flash_dev, region, NoCache::new());

    let mut buffer = [0u8; 4080];

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
            // Woken by queued writes or by an explicit flush.
            match select(wait_write, SETTINGS_FLUSH.wait()).await {
                Either::First(_) => {}
                Either::Second(_) => {}
            }
        };

        // Write anyway every `auto_interval`, so changes land even if nobody signals.
        let tick_fut = Timer::after(auto_interval);

        match embassy_futures::select::select(read_fut, select(write_fut, tick_fut)).await {
            Either::First(_) => {
                let start = Instant::now();
                match LIST.process_reads(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::debug!("process_reads ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_reads err: {:?}", e),
                }
                defmt::trace!("reads in {:?}", start.elapsed());
            }
            Either::Second(Either::First(_)) | Either::Second(Either::Second(_)) => {
                let start = Instant::now();
                match LIST.process_writes(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::debug!("process_writes ok: {:?}", rpt),
                    Err(e) => defmt::debug!("process_writes err: {:?}", e),
                }
                defmt::trace!("writes in {:?}", start.elapsed());

                // Garbage-collect after every write pass.
                let start = Instant::now();
                match LIST.process_garbage(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::trace!("process_garbage ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_garbage err: {:?}", e),
                }
                defmt::trace!("gc in {:?}", start.elapsed());
                SETTINGS_SETTLED.signal(());
            }
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[inline]
pub fn settings_flush_now() {
    SETTINGS_FLUSH.signal(());
}

/// Wait until the worker has completed a write pass, up to `timeout`.
///
/// Callers that reset the chip must go through this. Otherwise the reset can
/// land in the middle of a page erase.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn settings_wait_settled(timeout: Duration) {
    // A completion from an earlier pass says nothing about writes queued since then.
    SETTINGS_SETTLED.reset();
    settings_flush_now();
    if embassy_time::with_timeout(timeout, SETTINGS_SETTLED.wait())
        .await
        .is_err()
    {
        defmt::warn!("settings did not settle within {:?}, continuing", timeout);
    }
}

/// Spawns the settings I/O worker. Call before `init_nodes()` or any `attach_with_default` on
/// `LIST`. Attaches no nodes. Call `publish` after loading the board's nodes.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn init_core(
    spawner: &Spawner,
    flash_dev: embassy_embedded_hal::adapter::BlockingAsync<Nvmc<'static>>,
) {
    let region = get_settings_region();
    defmt::info!(
        "[settings] Settings flash region: {:#010x}..{:#010x} (size={}KB)",
        region.start,
        region.end,
        (region.end - region.start) / 1024
    );
    assert!((region.end - region.start) as usize >= 2 * PAGE_SIZE);

    spawner.spawn(settings_worker(flash_dev, region.clone(), Duration::from_secs(10)).unwrap());
}

/// Loads the BLE bonds into owned settings, moving a bond stored by single-bond firmware into
/// the first slot. Call after `init_core()`, then load any board-specific nodes before
/// publishing the settings for the application tasks.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn init_nodes() -> Settings {
    let mut h_legacy = NODE_BLE_BOND
        .attach_with_default(&LIST, ble_bond_default)
        .await
        .unwrap();
    let mut h_bonds = NODE_BLE_BONDS
        .attach_with_default(&LIST, BleBonds::default)
        .await
        .unwrap();

    let mut bonds = h_bonds.load();
    let legacy = h_legacy.load();
    if legacy.is_bonded {
        if bonds.slots.iter().all(|slot| !slot.is_bonded) {
            bonds.slots[0] = legacy;
            h_bonds.write(&bonds).await.unwrap();
        }
        h_legacy.write(&ble_bond_default()).await.unwrap();
        settings_flush_now();
    }

    Settings {
        ble_bonds: bonds,
        general: GeneralSettings::default(),
        pag: PagSettings::default(),
    }
}

/// Attaches the general and PAG nodes and loads them into `settings`. For boards that use this
/// crate's `GeneralSettings`/`PagSettings`, such as lite1. Call after `init_nodes()`.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn init_board_nodes(settings: &mut Settings) {
    let h_general = NODE_GENERAL
        .attach_with_default(&LIST, GeneralSettings::default)
        .await
        .unwrap();

    let h_pag = NODE_PAG
        .attach_with_default(&LIST, PagSettings::default)
        .await
        .unwrap();

    settings.general = h_general.load();
    settings.pag = h_pag.load();
}

/// Since: Odyssey Desktop 0.7.0.
pub struct Settings {
    /// Since: Unreleased.
    pub ble_bonds: BleBonds,
    /// Since: Odyssey Desktop 0.7.0.
    pub general: GeneralSettings,
    /// Since: Odyssey Desktop 0.7.0.
    pub pag: PagSettings,
}

/// Outcome of [`Settings::bond_store`] when no slot can take the bond.
///
/// Since: Unreleased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Format)]
pub struct BondsFull;

impl Settings {
    /// The stored bonds, ready to load into the BLE host.
    ///
    /// Since: Unreleased.
    pub fn bond_infos(&self) -> impl Iterator<Item = trouble_host::prelude::BondInformation> + '_ {
        ble_bond_infos(&self.ble_bonds)
    }

    /// Whether `identity` belongs to a stored bond.
    ///
    /// Since: Unreleased.
    pub fn is_bonded_peer(&self, identity: &trouble_host::prelude::Identity) -> bool {
        self.bond_infos()
            .any(|bond| bond.identity.match_identity(identity))
    }

    /// Whether every slot holds a bond.
    ///
    /// Since: Unreleased.
    pub fn bonds_full(&self) -> bool {
        self.ble_bonds.slots.iter().all(|slot| slot.is_bonded)
    }

    /// Stores `entry`, replacing a bond with the same identity or filling a free slot, and
    /// writes the bonds to flash.
    ///
    /// # Errors
    ///
    /// [`BondsFull`] when `entry` is a new host and every slot is in use. Nothing changes.
    ///
    /// Since: Unreleased.
    pub fn bond_store(&mut self, entry: BleBondSettings) -> Result<(), BondsFull> {
        self.bond_store_with_kind(entry, None)
    }

    /// Stores a bond learned from an authenticated BLE connection.
    ///
    /// Since: Unreleased.
    pub fn bond_store_info(
        &mut self,
        info: &trouble_host::prelude::BondInformation,
    ) -> Result<(), BondsFull> {
        self.bond_store_with_kind(
            ble_bond_from_bond_info(info),
            Some(info.identity.addr.kind.as_raw()),
        )
    }
    /// Records the address kind only after Trouble confirms the stored bond key.
    ///
    /// Since: Unreleased.
    pub fn learn_bond_address(&mut self, info: &trouble_host::prelude::BondInformation) -> bool {
        if !learn_bond_address_kind(&mut self.ble_bonds, info) {
            return false;
        }
        self.ble_bonds_write();
        true
    }

    fn bond_store_with_kind(
        &mut self,
        entry: BleBondSettings,
        address_kind: Option<u8>,
    ) -> Result<(), BondsFull> {
        let address_kinds = self
            .ble_bonds
            .address_kinds
            .unwrap_or([None; odyssey_protocol::control::MAX_BONDS]);
        let slots = &self.ble_bonds.slots;
        let relation: [slot_table::Slot; odyssey_protocol::control::MAX_BONDS] =
            core::array::from_fn(|i| {
                let same = ble_bond_to_bond_infos(&entry, address_kind).any(|new| {
                    ble_bond_to_bond_infos(&slots[i], address_kinds[i])
                        .any(|stored| stored.identity.match_identity(&new.identity))
                });
                if same {
                    slot_table::Slot::Same
                } else if slots[i].is_bonded {
                    slot_table::Slot::Other
                } else {
                    slot_table::Slot::Free
                }
            });
        let index = slot_table::slot_for(&relation).ok_or(BondsFull)?;
        self.ble_bonds.slots[index] = entry;
        let mut address_kinds = address_kinds;
        address_kinds[index] = address_kind.or(address_kinds[index]);
        self.ble_bonds.address_kinds = Some(address_kinds);
        self.ble_bonds_write();
        Ok(())
    }

    /// Removes the bond whose identity address is `bd_addr` and writes the bonds to flash.
    /// Returns whether one was found.
    ///
    /// Since: Unreleased.
    pub fn bond_remove(&mut self, bd_addr: [u8; 6]) -> bool {
        let Some(index) = self
            .ble_bonds
            .slots
            .iter()
            .position(|slot| slot.is_bonded && slot.bd_addr == bd_addr)
        else {
            return false;
        };
        self.ble_bonds.slots[index] = ble_bond_default();
        if let Some(address_kinds) = &mut self.ble_bonds.address_kinds {
            address_kinds[index] = None;
        }
        self.ble_bonds_write();
        true
    }

    /// Removes every bond and writes the empty slots to flash.
    ///
    /// Since: Unreleased.
    pub fn bonds_clear(&mut self) {
        self.ble_bonds = BleBonds::default();
        self.ble_bonds_write();
    }

    fn ble_bonds_write(&self) {
        embassy_futures::block_on(async {
            let mut hb = NODE_BLE_BONDS.attach(&LIST).await.unwrap();
            hb.write(&self.ble_bonds).await.unwrap();
        });
        settings_flush_now();
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn general_write(&self) {
        embassy_futures::block_on(async {
            let mut hg = NODE_GENERAL.attach(&LIST).await.unwrap();
            hg.write(&self.general).await.unwrap();
        });
        settings_flush_now();
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn pag_write(&self) {
        embassy_futures::block_on(async {
            let mut hp = NODE_PAG.attach(&LIST).await.unwrap();
            hp.write(&self.pag).await.unwrap();
        });
        settings_flush_now();
    }
}

mod heapless_str32_cbor {
    use cfg_noodle::minicbor::decode::Error;
    use cfg_noodle::minicbor::encode::{Error as EncodeError, Write};
    use cfg_noodle::minicbor::{Decoder, Encoder};

    /// Since: Odyssey Desktop 0.7.0.
    pub fn encode<Ctx, W: Write>(
        s: &heapless::String<32>,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), EncodeError<W::Error>> {
        e.str(s.as_str())?;
        Ok(())
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn decode<'b, Ctx>(
        d: &mut Decoder<'b>,
        _ctx: &mut Ctx,
    ) -> Result<heapless::String<32>, Error> {
        let s = d.str()?;
        let mut hs: heapless::String<32> = heapless::String::new();
        for c in s.chars() {
            if hs.push(c).is_err() {
                break;
            }
        }
        Ok(hs)
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn cbor_len<Ctx>(s: &heapless::String<32>, _ctx: &mut Ctx) -> usize {
        let n = s.as_bytes().len();
        // CBOR text string: 1-byte header for n ≤ 23, 2-byte header for n ≤ 255
        let header = if n <= 23 { 1 } else { 2 };
        header + n
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct GeneralSettings {
    /// Ignored: the device picks USB or BLE from the USB state at run time. The field stays so
    /// settings stored by earlier firmware keep decoding with the same CBOR indices.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(0)]
    pub transport_mode: odyssey_protocol::control::device::TransportMode,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(1)]
    pub impact_threshold: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(2)]
    pub suppress_ms: u8,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(3)]
    pub accel_config: odyssey_protocol::AccelConfig,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(4)]
    pub gyro_config: odyssey_protocol::GyroConfig,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(5)]
    pub camera_model_nf: odyssey_protocol::wire::CameraCalibrationParams,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(6)]
    pub camera_model_wf: odyssey_protocol::wire::CameraCalibrationParams,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(7)]
    pub stereo_iso: odyssey_protocol::wire::StereoCalibrationParams,
    /// Since: Odyssey Desktop 0.7.0.
    #[n(8)]
    #[cbor(with = "heapless_str32_cbor")]
    pub name: heapless::String<32>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            transport_mode: odyssey_protocol::control::device::TransportMode::Ble,
            // m/s². SI equivalent of the legacy 5 g setting.
            impact_threshold: 25,
            suppress_ms: 100,
            accel_config: Default::default(),
            gyro_config: Default::default(),
            camera_model_nf: odyssey_protocol::wire::CameraCalibrationParams {
                camera_matrix: [12645.0, 0.0, 10240.0, 0.0, 12601.0, 7680.0, 0.0, 0.0, 1.0],
                dist_coeffs: [0.0; 5],
            },
            camera_model_wf: odyssey_protocol::wire::CameraCalibrationParams {
                camera_matrix: [0.0; 9],
                dist_coeffs: [0.0; 5],
            },
            stereo_iso: odyssey_protocol::wire::StereoCalibrationParams {
                r: [0.0; 9],
                t: [0.0; 3],
            },
            name: heapless::String::new(),
        }
    }
}

impl GeneralSettings {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn set_general_config(&mut self, config: &odyssey_protocol::wire::GeneralConfig) {
        use odyssey_protocol::wire::GeneralConfig as GC;
        match config {
            GC::ImpactThreshold(v) => self.impact_threshold = *v,
            GC::SuppressMs(v) => self.suppress_ms = *v,
            GC::AccelConfig(v) => self.accel_config = v.clone().into(),
            GC::GyroConfig(v) => self.gyro_config = v.clone(),
            GC::CameraModelNf(v) => self.camera_model_nf = v.clone(),
            GC::CameraModelWf(v) => self.camera_model_wf = v.clone(),
            GC::StereoIso(v) => self.stereo_iso = v.clone(),
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub type BleBondSettings = odyssey_protocol::control::BondEntry;

/// Every bond slot. An entry with `is_bonded == false` is free.
///
/// Since: Unreleased.
#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct BleBonds {
    /// Since: Unreleased.
    #[n(0)]
    pub slots: [BleBondSettings; odyssey_protocol::control::MAX_BONDS],
    /// Address kinds parallel to `slots`; `None` keeps legacy untyped bonds ambiguous.
    #[n(1)]
    pub address_kinds: Option<[Option<u8>; odyssey_protocol::control::MAX_BONDS]>,
}

impl Default for BleBonds {
    fn default() -> Self {
        Self {
            slots: [ble_bond_default(); odyssey_protocol::control::MAX_BONDS],
            address_kinds: None,
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub fn ble_bond_default() -> BleBondSettings {
    BleBondSettings {
        bd_addr: [0; 6],
        ltk: [0; 16],
        security_level: 0,
        is_bonded: false,
        irk: None,
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub fn ble_bond_from_bond_info(info: &trouble_host::prelude::BondInformation) -> BleBondSettings {
    let mut bd_addr = [0u8; 6];
    bd_addr.copy_from_slice(info.identity.addr.addr.raw());
    BleBondSettings {
        bd_addr,
        ltk: info.ltk.to_le_bytes(),
        security_level: match info.security_level {
            trouble_host::prelude::SecurityLevel::NoEncryption => 0,
            trouble_host::prelude::SecurityLevel::Encrypted => 1,
            trouble_host::prelude::SecurityLevel::EncryptedAuthenticated => 2,
        },
        is_bonded: info.is_bonded,
        irk: info.identity.irk.map(|irk| irk.to_le_bytes()),
    }
}

/// Since: Unreleased.
pub fn ble_bond_to_bond_infos(
    b: &BleBondSettings,
    address_kind: Option<u8>,
) -> impl Iterator<Item = trouble_host::prelude::BondInformation> + '_ {
    let stored_irk = b
        .irk
        .and_then(trouble_host::prelude::IdentityResolvingKey::from_le_bytes);
    let invalid_irk = b.irk.is_some() && stored_irk.is_none();
    let address_kinds = match (b.is_bonded, invalid_irk, address_kind) {
        (false, _, _) | (true, true, _) => [(None, false), (None, false)],
        (true, false, Some(kind)) => [(Some(kind), true), (None, false)],
        (true, false, None) => [
            (
                Some(trouble_host::prelude::AddrKind::PUBLIC.as_raw()),
                false,
            ),
            (Some(trouble_host::prelude::AddrKind::RANDOM.as_raw()), true),
        ],
    };
    address_kinds
        .into_iter()
        .filter_map(move |(kind, include_irk)| {
            let kind = kind?;
            Some(trouble_host::prelude::BondInformation {
                identity: trouble_host::prelude::Identity {
                    addr: trouble_host::prelude::Address::new(
                        trouble_host::prelude::AddrKind::new(kind),
                        trouble_host::prelude::BdAddr::new(b.bd_addr),
                    ),
                    irk: if include_irk { stored_irk } else { None },
                },
                security_level: match b.security_level {
                    0 => trouble_host::prelude::SecurityLevel::NoEncryption,
                    1 => trouble_host::prelude::SecurityLevel::Encrypted,
                    2 => trouble_host::prelude::SecurityLevel::EncryptedAuthenticated,
                    _ => trouble_host::prelude::SecurityLevel::NoEncryption,
                },
                is_bonded: b.is_bonded,
                ltk: trouble_host::prelude::LongTermKey::from_le_bytes(b.ltk),
            })
        })
}

fn ble_bond_infos(
    bonds: &BleBonds,
) -> impl Iterator<Item = trouble_host::prelude::BondInformation> + '_ {
    let address_kinds = bonds
        .address_kinds
        .unwrap_or([None; odyssey_protocol::control::MAX_BONDS]);
    bonds
        .slots
        .iter()
        .zip(address_kinds)
        .flat_map(|(bond, kind)| ble_bond_to_bond_infos(bond, kind))
}

fn untyped_bond_index(
    bonds: &BleBonds,
    observed: &trouble_host::prelude::BondInformation,
) -> Option<usize> {
    let address_kinds = bonds
        .address_kinds
        .unwrap_or([None; odyssey_protocol::control::MAX_BONDS]);
    bonds
        .slots
        .iter()
        .zip(address_kinds)
        .enumerate()
        .find_map(|(index, (bond, kind))| {
            (kind.is_none()
                && ble_bond_to_bond_infos(bond, None).any(|candidate| candidate == *observed))
            .then_some(index)
        })
}

fn learn_bond_address_kind(
    bonds: &mut BleBonds,
    observed: &trouble_host::prelude::BondInformation,
) -> bool {
    let Some(index) = untyped_bond_index(bonds, observed) else {
        return false;
    };
    let mut address_kinds = bonds
        .address_kinds
        .unwrap_or([None; odyssey_protocol::control::MAX_BONDS]);
    address_kinds[index] = Some(observed.identity.addr.kind.as_raw());
    bonds.address_kinds = Some(address_kinds);
    true
}

/// PAG7665QN register values kept across power cycles.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct PagSettings {
    /// Exposure register (0x67): bit[7]=LED_Always_ON, bits[6:0]=time (units of 100µs)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(0)]
    pub exposure: u8,
    /// Analog gain register (0x68)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(1)]
    pub gain: u8,
    /// Light threshold register (0x6D)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(2)]
    pub light_threshold: u8,
    /// Area lower bound registers (0x6E-0x6F)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(3)]
    pub area_min: u16,
    /// Area upper bound registers (0x70-0x71)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(4)]
    pub area_max: u16,
    /// Circle R lower bound register (0x23)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(5)]
    pub circle_r_min: u8,
    /// Circle R upper bound register (0x24)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(6)]
    pub circle_r_max: u8,
    /// Circle K lower bound register (0x77)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(7)]
    pub circle_k_min: u8,
    /// Circle K upper bound register (0x78)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(8)]
    pub circle_k_max: u8,
    /// Frame rate register (0x13)
    ///
    /// Since: Odyssey Desktop 0.7.0.
    #[n(9)]
    pub frame_rate: u8,
}

impl Default for PagSettings {
    fn default() -> Self {
        Self {
            // From Xavier's recommendations and poc-fw.
            exposure: 0x80 | 20, // 2000 µs, LED always on
            gain: 3,             // Xavier: never below 3
            light_threshold: 120,
            area_min: 10,
            area_max: 500,
            circle_r_min: 0x1A, // datasheet default
            circle_r_max: 0x26, // datasheet default
            circle_k_min: 0x17, // datasheet default
            circle_k_max: 0x1D, // datasheet default
            frame_rate: 180,    // FPS
        }
    }
}

#[cfg(test)]
mod tests {
    use cfg_noodle::minicbor::{self, Encode};
    use trouble_host::prelude::{
        AddrKind, Address, BdAddr, BondInformation, Identity, IdentityResolvingKey, LongTermKey,
        SecurityLevel,
    };

    use super::{
        BleBondSettings, BleBonds, ble_bond_default, ble_bond_from_bond_info, ble_bond_infos,
        ble_bond_to_bond_infos, learn_bond_address_kind, untyped_bond_index,
    };

    const MAX_BONDS: usize = odyssey_protocol::control::MAX_BONDS;

    fn bond(kind: AddrKind, key: u8) -> BondInformation {
        BondInformation::new(
            Identity {
                addr: Address::new(kind, BdAddr::new([1, 2, 3, 4, 5, 6])),
                irk: Some(IdentityResolvingKey::from_le_bytes([7; 16]).unwrap()),
            },
            LongTermKey::from_le_bytes([key; 16]),
            SecurityLevel::EncryptedAuthenticated,
            true,
        )
    }

    fn same_slots(a: &BleBonds, b: &BleBonds) -> bool {
        a.slots.iter().zip(&b.slots).all(|(a, b)| {
            a.bd_addr == b.bd_addr
                && a.ltk == b.ltk
                && a.security_level == b.security_level
                && a.is_bonded == b.is_bonded
                && a.irk == b.irk
        })
    }

    #[derive(Encode)]
    struct LegacyBleBonds {
        #[n(0)]
        slots: [BleBondSettings; MAX_BONDS],
    }

    #[test]
    fn old_cbor_without_address_kinds_decodes_as_unknown() {
        let mut output = [0; 256];
        let remaining = {
            let mut encoder = minicbor::Encoder::new(&mut output[..]);
            encoder
                .encode(LegacyBleBonds {
                    slots: [ble_bond_default(); MAX_BONDS],
                })
                .unwrap();
            encoder.into_writer().len()
        };
        let encoded = &output[..output.len() - remaining];
        let decoded: BleBonds = minicbor::decode(encoded).unwrap();
        assert_eq!(decoded.address_kinds, None);
    }

    #[test]
    fn untyped_bonds_expand_to_both_address_kinds_at_full_capacity() {
        let entry = ble_bond_from_bond_info(&bond(AddrKind::PUBLIC, 1));
        let bonds = BleBonds {
            slots: [entry; MAX_BONDS],
            address_kinds: None,
        };
        let mut candidates = ble_bond_to_bond_infos(&entry, None);
        let public_alias = candidates.next().unwrap();
        let random_alias = candidates.next().unwrap();
        assert_eq!(public_alias.identity.irk, None);
        assert_eq!(
            random_alias.identity.irk,
            entry.irk.and_then(IdentityResolvingKey::from_le_bytes)
        );
        assert_eq!(ble_bond_to_bond_infos(&entry, None).count(), 2);
        assert_eq!(
            ble_bond_to_bond_infos(&entry, Some(AddrKind::PUBLIC.as_raw())).count(),
            1
        );
        assert_eq!(ble_bond_infos(&bonds).count(), 2 * MAX_BONDS);
    }

    #[test]
    fn address_learning_requires_exact_candidate_and_preserves_entry() {
        let observed = bond(AddrKind::RANDOM, 1);
        let entry = ble_bond_from_bond_info(&observed);
        let mut untyped = BleBonds::default();
        untyped.slots[0] = entry;
        let wrong_key = bond(AddrKind::RANDOM, 2);
        assert_eq!(untyped_bond_index(&untyped, &wrong_key), None);
        let wrong_security = BondInformation {
            security_level: SecurityLevel::Encrypted,
            ..observed.clone()
        };
        assert_eq!(untyped_bond_index(&untyped, &wrong_security), None);

        for candidate in ble_bond_to_bond_infos(&entry, None) {
            assert_eq!(untyped_bond_index(&untyped, &candidate), Some(0));
            let mut learned = untyped.clone();
            assert!(learn_bond_address_kind(&mut learned, &candidate));
            assert!(same_slots(&learned, &untyped));
            assert_eq!(
                learned.address_kinds.unwrap()[0],
                Some(candidate.identity.addr.kind.as_raw())
            );
            assert!(!learn_bond_address_kind(&mut learned, &candidate));
            let typed_candidate =
                ble_bond_to_bond_infos(&learned.slots[0], learned.address_kinds.unwrap()[0])
                    .next()
                    .unwrap();
            assert_eq!(typed_candidate.identity.irk, observed.identity.irk);
        }

        let typed = BleBonds {
            slots: [entry; MAX_BONDS],
            address_kinds: Some([Some(AddrKind::RANDOM.as_raw()); MAX_BONDS]),
        };
        assert_eq!(untyped_bond_index(&typed, &observed), None);
    }

    #[test]
    fn invalid_persisted_irk_does_not_downgrade_to_unresolved_bond() {
        let mut entry = ble_bond_default();
        entry.is_bonded = true;
        entry.irk = Some([0; 16]);
        assert_eq!(ble_bond_to_bond_infos(&entry, None).count(), 0);
    }
}
