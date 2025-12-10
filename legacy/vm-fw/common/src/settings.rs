//! Settings backed by cfg-noodle.

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

use core::convert::Infallible;
use core::sync::atomic::{AtomicU16, Ordering};

use cfg_noodle::flash::Flash;
use cfg_noodle::minicbor::{CborLen, Decode, Encode};
use cfg_noodle::mutex::raw_impls::cs::CriticalSectionRawMutex as NoodleRawMutex;
use cfg_noodle::sequential_storage::cache::NoCache;
use cfg_noodle::{StorageList, StorageListNode};
use defmt::Format;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, WithTimeout as _};
use embedded_hal_bus::spi::DeviceError;
use paj7025::Paj7025Error;
use protodongers::wire::{CameraCalibrationParams, StereoCalibrationParams};
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

use crate::Paj;

pub static LIST: StorageList<NoodleRawMutex> = StorageList::new();

static NODE_PAJS: StorageListNode<PajsSettings> = StorageListNode::new("settings/pajs");
static NODE_GENERAL: StorageListNode<GeneralSettings> = StorageListNode::new("settings/general");
static NODE_TRANSIENT: StorageListNode<TransientSettings> = StorageListNode::new("settings/transient");

static NODE_BLE_BOND: StorageListNode<BleBondSettings> = StorageListNode::new("settings/ble_bond");

static SETTINGS_FLUSH: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub static SETTINGS_CELL: StaticCell<Settings> = StaticCell::new();

// Pointer to the settings for BLE access
static SETTINGS_PTR: core::sync::atomic::AtomicPtr<Settings> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Get a reference to settings. Only safe to call after init_settings has been called.
/// This is primarily for BLE peripheral to access settings.
pub unsafe fn get_settings() -> &'static mut Settings {
    let ptr = SETTINGS_PTR.load(core::sync::atomic::Ordering::Acquire);
    unsafe { &mut *ptr }
}

static ACCEL_ODR: AtomicU16 = AtomicU16::new(100);

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

pub async fn init_settings(
    spawner: &Spawner,
    flash_dev: embassy_embedded_hal::adapter::BlockingAsync<Nvmc<'static>>,
    pajr2: &mut Paj,
    pajr3: &mut Paj,
) -> Result<&'static mut Settings, Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
    let region = get_settings_region();
    assert!((region.end - region.start) as usize >= 2 * PAGE_SIZE);

    // Start I/O worker
    spawner.spawn(settings_worker(flash_dev, region.clone(), Duration::from_secs(10)).unwrap());

    // Attach nodes with defaults (hydrates via worker)
    let h_pajs = NODE_PAJS
        .attach_with_default(&LIST, PajsSettings::default)
        .await
        .unwrap();
    let h_general = NODE_GENERAL
        .attach_with_default(&LIST, GeneralSettings::default)
        .await
        .unwrap();
    let h_transient = NODE_TRANSIENT
        .attach_with_default(&LIST, TransientSettings::default)
        .await
        .unwrap();

    let h_ble_bond = NODE_BLE_BOND
        .attach_with_default(&LIST, BleBondSettings::default)
        .await
        .unwrap();

    let s = Settings {
        pajs: h_pajs.load(),
        general: {
            let g = h_general.load();
            ACCEL_ODR.store(g.accel_config.accel_odr, Ordering::Relaxed);
            g
        },
        transient: h_transient.load(),
        ble_bond: h_ble_bond.load(),
    };

    s.apply(pajr2, pajr3).await?;
    let settings_ref = SETTINGS_CELL.init(s);
    SETTINGS_PTR.store(settings_ref as *mut Settings, core::sync::atomic::Ordering::Release);
    Ok(settings_ref)
}

pub struct Settings {
    pub pajs: PajsSettings,
    pub general: GeneralSettings,
    pub transient: TransientSettings,
    pub ble_bond: BleBondSettings,
}

impl Settings {
    pub async fn apply(
        &self,
        pajr2: &mut Paj,
        pajr3: &mut Paj,
    ) -> Result<(), Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
        self.pajs.apply(pajr2, pajr3).await
    }

    pub fn write(&self) {
        embassy_futures::block_on(async {
            let mut hp = NODE_PAJS.attach(&LIST).await.unwrap();
            let mut hg = NODE_GENERAL.attach(&LIST).await.unwrap();
            hp.write(&self.pajs).await.unwrap();
            hg.write(&self.general).await.unwrap();
        });
        settings_flush_now();
    }

    pub fn transient_write(&self) {
        embassy_futures::block_on(async {
            let mut ht = NODE_TRANSIENT.attach(&LIST).await.unwrap();
            ht.write(&self.transient).await.unwrap();
        });
        settings_flush_now();
    }

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
}

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct PajSettings {
    #[n(0)]
    pub frame_period: u32,
    #[n(1)]
    pub area_threshold_max: u16,
    #[n(2)]
    pub exposure_time: u16,
    #[n(3)]
    pub resolution_x: u16,
    #[n(4)]
    pub resolution_y: u16,
    #[n(5)]
    pub area_threshold_min: u8,
    #[n(6)]
    pub brightness_threshold: u8,
    #[n(7)]
    pub frame_subtraction: u8,
    #[n(8)]
    pub gain_1: u8,
    #[n(9)]
    pub gain_2: u8,
    #[n(10)]
    pub max_object_cnt: u8,
    #[n(11)]
    pub noise_threshold: u8,
    #[n(12)]
    pub operation_mode: u8,
}

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct PajsSettings(#[n(0)] pub PajSettings, #[n(1)] pub PajSettings);

impl Default for PajsSettings {
    fn default() -> Self {
        Self(
            PajSettings {
                area_threshold_max: 9605,
                area_threshold_min: 10,
                brightness_threshold: 110,
                exposure_time: 8192,
                frame_period: 49780,
                frame_subtraction: 0,
                gain_1: 16,
                gain_2: 0,
                max_object_cnt: 16,
                noise_threshold: 15,
                operation_mode: 0,
                resolution_x: 4095,
                resolution_y: 4095,
            },
            PajSettings {
                area_threshold_max: 9605,
                area_threshold_min: 5,
                brightness_threshold: 110,
                exposure_time: 11365,
                frame_period: 49780,
                frame_subtraction: 0,
                gain_1: 8,
                gain_2: 3,
                max_object_cnt: 16,
                noise_threshold: 15,
                operation_mode: 0,
                resolution_x: 4095,
                resolution_y: 4095,
            },
        )
    }
}

impl PajsSettings {
    pub async fn apply(
        &self,
        pajr2: &mut Paj,
        pajr3: &mut Paj,
    ) -> Result<(), Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
        macro_rules! apply_one {
            ($paj:expr, $s:expr) => {
                async {
                    let PajSettings {
                        area_threshold_max,
                        area_threshold_min,
                        brightness_threshold,
                        exposure_time,
                        frame_period,
                        frame_subtraction,
                        gain_1,
                        gain_2,
                        max_object_cnt,
                        noise_threshold,
                        operation_mode,
                        resolution_x,
                        resolution_y,
                        ..
                    } = $s;

                    $paj.ll
                        .control()
                        .bank_0()
                        .cmd_oahb()
                        .write_async(|x| x.set_value(area_threshold_max))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .cmd_oalb()
                        .write_async(|x| x.set_value(area_threshold_min))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .cmd_thd()
                        .write_async(|x| x.set_value(brightness_threshold))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .b_expo()
                        .write_async(|x| x.set_value(exposure_time))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .cmd_frame_period()
                        .write_async(|x| x.set_value(frame_period))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_0()
                        .cmd_frame_subtraction_on()
                        .write_async(|x| x.set_value(frame_subtraction))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .b_global()
                        .write_async(|x| x.set_value(gain_1))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .b_ggh()
                        .write_async(|x| x.set_value(gain_2))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_0()
                        .cmd_max_object_num()
                        .write_async(|x| x.set_value(max_object_cnt))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_0()
                        .cmd_nthd()
                        .write_async(|x| x.set_value(noise_threshold))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_0()
                        .cmd_dsp_operation_mode()
                        .write_async(|x| x.set_value(operation_mode))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .cmd_scale_resolution_x()
                        .write_async(|x| x.set_value(resolution_x))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_c()
                        .cmd_scale_resolution_y()
                        .write_async(|x| x.set_value(resolution_y))
                        .await?;

                    $paj.ll
                        .control()
                        .bank_0()
                        .bank_0_sync_updated_flag()
                        .write_async(|x| x.set_value(1))
                        .await?;
                    $paj.ll
                        .control()
                        .bank_1()
                        .bank_1_sync_updated_flag()
                        .write_async(|x| x.set_value(1))
                        .await?;
                    Ok::<_, Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>>(())
                }
            };
        }

        apply_one!(pajr2, self.0).await?;
        apply_one!(pajr3, self.1).await
    }

    pub async fn read_from_pajs(
        pajr2: &mut Paj,
        pajr3: &mut Paj,
    ) -> Result<Self, Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
        macro_rules! read_one {
            ($paj:expr) => {{
                Ok(PajSettings {
                    area_threshold_max: $paj.ll.control().bank_0().cmd_oahb().read_async().await?.value(),
                    area_threshold_min: $paj.ll.control().bank_c().cmd_oalb().read_async().await?.value(),
                    brightness_threshold: $paj.ll.control().bank_c().cmd_thd().read_async().await?.value(),
                    exposure_time: $paj.ll.control().bank_1().b_expo_r().read_async().await?.value(),
                    frame_period: $paj
                        .ll
                        .control()
                        .bank_c()
                        .cmd_frame_period()
                        .read_async()
                        .await?
                        .value(),
                    frame_subtraction: $paj
                        .ll
                        .control()
                        .bank_0()
                        .cmd_frame_subtraction_on()
                        .read_async()
                        .await?
                        .value(),
                    gain_1: $paj
                        .ll
                        .control()
                        .bank_1()
                        .b_global_r()
                        .read_async()
                        .await?
                        .value(),
                    gain_2: $paj.ll.control().bank_1().b_ggh_r().read_async().await?.value(),
                    max_object_cnt: $paj
                        .ll
                        .control()
                        .bank_0()
                        .cmd_max_object_num()
                        .read_async()
                        .await?
                        .value(),
                    noise_threshold: $paj.ll.control().bank_0().cmd_nthd().read_async().await?.value(),
                    operation_mode: $paj
                        .ll
                        .control()
                        .bank_0()
                        .cmd_dsp_operation_mode()
                        .read_async()
                        .await?
                        .value(),
                    resolution_x: $paj
                        .ll
                        .control()
                        .bank_c()
                        .cmd_scale_resolution_x()
                        .read_async()
                        .await?
                        .value(),
                    resolution_y: $paj
                        .ll
                        .control()
                        .bank_c()
                        .cmd_scale_resolution_y()
                        .read_async()
                        .await?
                        .value(),
                })
            }};
        }
        Ok(Self(read_one!(pajr2)?, read_one!(pajr3)?))
    }
}

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct GeneralSettings {
    #[n(0)]
    pub impact_threshold: u8,
    #[n(1)]
    pub suppress_ms: u8,
    #[n(2)]
    pub accel_config: protodongers::AccelConfig,
    #[n(3)]
    pub gyro_config: protodongers::GyroConfig,
    #[n(4)]
    pub camera_model_nf: CameraCalibrationParams,
    #[n(5)]
    pub camera_model_wf: CameraCalibrationParams,
    #[n(6)]
    pub stereo_iso: StereoCalibrationParams,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            impact_threshold: 5,
            suppress_ms: 200,
            accel_config: Default::default(),
            gyro_config: Default::default(),
            camera_model_nf: CameraCalibrationParams {
                camera_matrix: [5896.181, 0.0, 2047.5, 0.0, 5896.181, 2047.5, 0.0, 0.0, 1.0],
                dist_coeffs: [0.0, 0.0, 0.0, 0.0, 0.0],
            },
            camera_model_wf: CameraCalibrationParams {
                camera_matrix: [1434.9723, 0.0, 2036.575, 0.0, 1437.214, 2088.8027, 0.0, 0.0, 1.0],
                dist_coeffs: [0.039820533, -0.03993317, 0.00043006078, -0.0012057066, 0.005302235],
            },
            stereo_iso: Default::default(),
        }
    }
}

impl GeneralSettings {
    pub fn set_general_config(&mut self, config: &protodongers::wire::GeneralConfig) {
        use protodongers::wire::GeneralConfig as GC;
        match config {
            GC::ImpactThreshold(v) => self.impact_threshold = *v,
            GC::SuppressMs(v) => self.suppress_ms = *v,
            GC::AccelConfig(v) => {
                self.accel_config = v.clone().into();
                ACCEL_ODR.store(v.accel_odr, Ordering::Relaxed);
            }
            GC::GyroConfig(v) => self.gyro_config = v.clone(),
            GC::CameraModelNf(v) => self.camera_model_nf = v.clone(),
            GC::CameraModelWf(v) => self.camera_model_wf = v.clone(),
            GC::StereoIso(v) => self.stereo_iso = v.clone(),
        }
    }
}

#[derive(Debug, Clone, Encode, Decode, CborLen, Format)]
pub struct TransientSettings {
    #[n(0)]
    pub mode: protodongers::Mode,
}

impl Default for TransientSettings {
    fn default() -> Self {
        Self {
            mode: protodongers::Mode::Object,
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
