//! Settings backed by cfg-noodle for atslite boards.
//!
//! This module provides persistent storage for:
//! - BLE bond information (for reconnection without re-pairing)
//! - General device settings (future)

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

use cfg_noodle::flash::Flash;
use cfg_noodle::minicbor;
use cfg_noodle::minicbor::{CborLen, Decode, Encode};
use cfg_noodle::mutex::raw_impls::cs::CriticalSectionRawMutex as NoodleRawMutex;
use cfg_noodle::sequential_storage::cache::NoCache;
use cfg_noodle::{StorageList, StorageListNode};
use defmt::Format;
use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, WithTimeout as _};
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

pub static LIST: StorageList<NoodleRawMutex> = StorageList::new();

static NODE_BLE_BOND: StorageListNode<BleBondSettings> = StorageListNode::new("settings/ble_bond");
static NODE_GENERAL: StorageListNode<GeneralSettings> = StorageListNode::new("settings/general");
static NODE_PAG: StorageListNode<PagSettings> = StorageListNode::new("settings/pag");

static SETTINGS_FLUSH: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub static SETTINGS_CELL: StaticCell<Settings> = StaticCell::new();

// Pointer to the settings for BLE access
static SETTINGS_PTR: core::sync::atomic::AtomicPtr<Settings> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Get a reference to settings. Only safe to call after init_settings has been called.
/// This is primarily for BLE peripheral to access settings.
///
/// # Safety
/// Must only be called after `init_settings` has completed.
pub unsafe fn get_settings() -> &'static mut Settings {
    let ptr = SETTINGS_PTR.load(core::sync::atomic::Ordering::Acquire);
    unsafe { &mut *ptr }
}

/// Persist the transport mode to flash via the atslite-common general settings node.
/// Intended to be registered as the `transport_mode_persist_fn` on lite1.
pub fn persist_transport_mode(mode: protodongers::control::device::TransportMode) {
    let settings = unsafe { get_settings() };
    settings.general.transport_mode = mode;
    settings.general_write();
}

/// Check if settings have been initialized
pub fn is_initialized() -> bool {
    !SETTINGS_PTR
        .load(core::sync::atomic::Ordering::Acquire)
        .is_null()
}

#[embassy_executor::task]
async fn settings_worker(
    flash_dev: embassy_embedded_hal::adapter::BlockingAsync<Nvmc<'static>>,
    region: core::ops::Range<u32>,
    auto_interval: Duration,
) {
    let mut flash = Flash::new(flash_dev, region, NoCache::new());

    // Buffer for cfg-noodle operations
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
                let start = Instant::now();
                match LIST.process_reads(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::debug!("process_reads ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_reads err: {:?}", e),
                }
                defmt::trace!("reads in {:?}", start.elapsed());
            }
            // Writes or tick
            Either::Second(Either::First(_)) | Either::Second(Either::Second(_)) => {
                let start = Instant::now();
                match LIST.process_writes(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::debug!("process_writes ok: {:?}", rpt),
                    Err(e) => defmt::debug!("process_writes err: {:?}", e),
                }
                defmt::trace!("writes in {:?}", start.elapsed());

                // GC after writes
                let start = Instant::now();
                match LIST.process_garbage(&mut flash, &mut buffer).await {
                    Ok(rpt) => defmt::trace!("process_garbage ok: {:?}", rpt),
                    Err(e) => defmt::error!("process_garbage err: {:?}", e),
                }
                defmt::trace!("gc in {:?}", start.elapsed());
            }
        }
    }
}

#[inline]
pub fn settings_flush_now() {
    SETTINGS_FLUSH.signal(());
}

/// Start the settings I/O worker task. Must be called before `init_nodes()` or
/// any `attach_with_default` on `LIST`. Does NOT attach any nodes or set `SETTINGS_PTR`.
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

/// Attach the BLE bond node to `LIST`, construct the `Settings` struct,
/// and store it in `SETTINGS_PTR`. Board-specific nodes (general, PAG, etc.)
/// should be attached separately by each board before or after this call.
/// Must be called after `init_core()`.
pub async fn init_nodes() -> &'static mut Settings {
    let h_ble_bond = NODE_BLE_BOND
        .attach_with_default(&LIST, BleBondSettings::default)
        .await
        .unwrap();

    let s = Settings {
        ble_bond: h_ble_bond.load(),
        general: GeneralSettings::default(),
        pag: PagSettings::default(),
    };

    let settings_ref = SETTINGS_CELL.init(s);
    SETTINGS_PTR.store(
        settings_ref as *mut Settings,
        core::sync::atomic::Ordering::Release,
    );
    settings_ref
}

/// Attach the general and PAG nodes to `LIST` and update the Settings struct.
/// Called by boards that use atslite-common's GeneralSettings/PagSettings
/// (e.g. lite1). Must be called after `init_nodes()`.
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

pub struct Settings {
    pub ble_bond: BleBondSettings,
    pub general: GeneralSettings,
    pub pag: PagSettings,
}

impl Settings {
    pub fn ble_bond_write(&self) {
        embassy_futures::block_on(async {
            let mut hb = NODE_BLE_BOND.attach(&LIST).await.unwrap();
            hb.write(&self.ble_bond).await.unwrap();
        });
        settings_flush_now();
    }

    #[allow(dead_code)]
    pub fn ble_bond_clear(&mut self) {
        self.ble_bond = BleBondSettings::default();
        embassy_futures::block_on(async {
            let mut hb = NODE_BLE_BOND.attach(&LIST).await.unwrap();
            hb.write(&self.ble_bond).await.unwrap();
        });
        settings_flush_now();
    }

    pub fn general_write(&self) {
        embassy_futures::block_on(async {
            let mut hg = NODE_GENERAL.attach(&LIST).await.unwrap();
            hg.write(&self.general).await.unwrap();
        });
        settings_flush_now();
    }


    pub fn pag_write(&self) {
        embassy_futures::block_on(async {
            let mut hp = NODE_PAG.attach(&LIST).await.unwrap();
            hp.write(&self.pag).await.unwrap();
        });
        settings_flush_now();
    }
}

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct GeneralSettings {
    #[n(0)]
    pub transport_mode: protodongers::control::device::TransportMode,
    #[n(1)]
    pub impact_threshold: u8,
    #[n(2)]
    pub suppress_ms: u8,
    #[n(3)]
    pub accel_config: protodongers::AccelConfig,
    #[n(4)]
    pub gyro_config: protodongers::GyroConfig,
    #[n(5)]
    pub camera_model_nf: protodongers::wire::CameraCalibrationParams,
    #[n(6)]
    pub camera_model_wf: protodongers::wire::CameraCalibrationParams,
    #[n(7)]
    pub stereo_iso: protodongers::wire::StereoCalibrationParams,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            transport_mode: protodongers::control::device::TransportMode::Ble,
            // 5g legacy equivalent in SI units.
            impact_threshold: 49,
            suppress_ms: 100,
            accel_config: Default::default(),
            gyro_config: Default::default(),
            camera_model_nf: protodongers::wire::CameraCalibrationParams {
                camera_matrix: [12645.0, 0.0, 10240.0, 0.0, 12601.0, 7680.0, 0.0, 0.0, 1.0],
                dist_coeffs: [0.0; 5],
            },
            camera_model_wf: protodongers::wire::CameraCalibrationParams {
                camera_matrix: [0.0; 9],
                dist_coeffs: [0.0; 5],
            },
            stereo_iso: protodongers::wire::StereoCalibrationParams {
                r: [0.0; 9],
                t: [0.0; 3],
            },
        }
    }
}

impl GeneralSettings {
    pub fn set_general_config(&mut self, config: &protodongers::wire::GeneralConfig) {
        use protodongers::wire::GeneralConfig as GC;
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

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct BleBondSettings {
    #[n(0)]
    pub bd_addr: [u8; 6],
    #[n(1)]
    pub ltk: [u8; 16],
    #[n(2)]
    pub security_level: u8,
    #[n(3)]
    pub is_bonded: bool,
    #[n(4)]
    pub irk: Option<[u8; 16]>,
}

impl Default for BleBondSettings {
    fn default() -> Self {
        Self {
            bd_addr: [0; 6],
            ltk: [0; 16],
            security_level: 0,
            is_bonded: false,
            irk: None,
        }
    }
}

impl BleBondSettings {
    pub fn from_bond_info(info: &trouble_host::prelude::BondInformation) -> Self {
        let mut bd_addr = [0u8; 6];
        bd_addr.copy_from_slice(info.identity.bd_addr.raw());
        Self {
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

    pub fn to_bond_info(&self) -> Option<trouble_host::prelude::BondInformation> {
        if !self.is_bonded {
            return None;
        }

        Some(trouble_host::prelude::BondInformation {
            identity: trouble_host::prelude::Identity {
                bd_addr: trouble_host::prelude::BdAddr::new(self.bd_addr),
                irk: self
                    .irk
                    .map(|irk| trouble_host::prelude::IdentityResolvingKey::from_le_bytes(irk)),
            },
            security_level: match self.security_level {
                0 => trouble_host::prelude::SecurityLevel::NoEncryption,
                1 => trouble_host::prelude::SecurityLevel::Encrypted,
                2 => trouble_host::prelude::SecurityLevel::EncryptedAuthenticated,
                _ => trouble_host::prelude::SecurityLevel::NoEncryption,
            },
            is_bonded: self.is_bonded,
            ltk: trouble_host::prelude::LongTermKey::from_le_bytes(self.ltk),
        })
    }
}

/// PAG7665QN sensor settings that persist across power cycles
#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct PagSettings {
    /// Exposure register (0x67): bit[7]=LED_Always_ON, bits[6:0]=time (units of 100µs)
    #[n(0)]
    pub exposure: u8,
    /// Analog gain register (0x68)
    #[n(1)]
    pub gain: u8,
    /// Light threshold register (0x6D)
    #[n(2)]
    pub light_threshold: u8,
    /// Area lower bound registers (0x6E-0x6F)
    #[n(3)]
    pub area_min: u16,
    /// Area upper bound registers (0x70-0x71)
    #[n(4)]
    pub area_max: u16,
    /// Circle R lower bound register (0x23)
    #[n(5)]
    pub circle_r_min: u8,
    /// Circle R upper bound register (0x24)
    #[n(6)]
    pub circle_r_max: u8,
    /// Circle K lower bound register (0x77)
    #[n(7)]
    pub circle_k_min: u8,
    /// Circle K upper bound register (0x78)
    #[n(8)]
    pub circle_k_max: u8,
    /// Frame rate register (0x13)
    #[n(9)]
    pub frame_rate: u8,
}

impl Default for PagSettings {
    fn default() -> Self {
        Self {
            // Defaults from Xavier's recommendations and poc-fw
            exposure: 0x80 | 20,  // 2000µs (20 * 100), LED always on
            gain: 3,              // Xavier says do not go below 3
            light_threshold: 120, // Light threshold
            area_min: 10,         // Minimum area
            area_max: 500,        // Maximum area
            circle_r_min: 0x1A,   // R lower bound (datasheet default)
            circle_r_max: 0x26,   // R upper bound (datasheet default)
            circle_k_min: 0x17,   // K lower bound (datasheet default)
            circle_k_max: 0x1D,   // K upper bound (datasheet default)
            frame_rate: 180,      // 180 FPS
        }
    }
}
