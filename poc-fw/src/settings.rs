//! Settings
//!
//! The current implementation uses two flash pages to store settings. One for PAG settings, and
//! one for other general settings like calibrations.
mod settings_region {
    unsafe extern "C" {
        static __settings_start: u32;
        static __settings_end: u32;
    }
    pub fn get_settings_region() -> core::ops::Range<usize> {
        (&raw const __settings_start) as usize..(&raw const __settings_end) as usize
    }
}

use core::cell::RefCell;
use core::sync::atomic::{AtomicU16, Ordering};

use bytemuck::checked;
use bytemuck::{AnyBitPattern, CheckedBitPattern, NoUninit, bytes_of, bytes_of_mut};
use defmt::Format;
use embassy_nrf::nvmc::PAGE_SIZE;
use embedded_storage::nor_flash::NorFlash;
use pag7661qn::mode;
use protodongers::wire::{CameraCalibrationParams, GeneralConfig};
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

use crate::Pag;

static SETTINGS: StaticCell<Settings> = StaticCell::new();

pub async fn init_settings(
    flash: &RefCell<impl NorFlash>,
    pag: &mut Pag<mode::Idle>,
) -> &'static mut Settings {
    let region = get_settings_region();
    assert!(region.len() >= 2 * PAGE_SIZE);
    let s = Settings::init(&flash, region.start as u32);
    s.apply(pag).await;
    let s = SETTINGS.init(s);
    s
}

pub struct Settings {
    pub pag: PagSettings,
    pub general: GeneralSettings,
    pub transient: TransientSettings,
    pag_start: u32,
    general_start: u32,
    transient_start: u32,
}

impl Settings {
    fn init(flash: &RefCell<impl NorFlash>, start: u32) -> Self {
        let pag_start = start;
        let general_start = start + PAGE_SIZE as u32;
        let transient_start = start + PAGE_SIZE as u32 * 2;
        let pag = PagSettings::init(&flash, pag_start);
        let general = GeneralSettings::init(&flash, general_start);
        let transient = TransientSettings::init(&flash, transient_start);
        Self {
            pag,
            general,
            transient,
            pag_start,
            general_start,
            transient_start,
        }
    }

    /// Make the settings take effect. Only relevent for PAG atm.
    pub async fn apply(&self, pag: &mut Pag<mode::Idle>) {
        self.pag.apply(pag).await;
    }

    /// Write the settings to flash
    pub fn write(&self, flash: &RefCell<impl NorFlash>) {
        self.pag.write(&flash, self.pag_start);
        self.general.write(&flash, self.general_start);
    }

    /// Transient settings are written to flash independently of the others
    pub fn transient_write(&self, flash: &RefCell<impl NorFlash>) {
        self.transient.write(&flash, self.transient_start);
    }
}

#[repr(C, align(4))]
#[derive(Clone, Copy, Format, NoUninit, AnyBitPattern)]
pub struct PagSettings {
    pub sensor_fps: u8,
    pub sensor_gain: u8,
    pub sensor_exposure_us: u16,
    pub area_lower_bound: u16,
    pub area_upper_bound: u16,
    pub light_threshold: u8,
    pub _padding: [u8; 3],
}

impl Default for PagSettings {
    fn default() -> Self {
        Self {
            sensor_fps: 180,
            // Xavier says do not go below 3
            sensor_gain: 3,
            // Xavier says do not go above 5300
            sensor_exposure_us: 2000,
            area_lower_bound: 10,
            area_upper_bound: 500,
            light_threshold: 120,
            _padding: [0; 3],
        }
    }
}

const fn max(a: u32, b: u32) -> u32 {
    if a < b { b } else { a }
}
impl PagSettings {
    const MAGIC: [u8; 4] = 0x6160fbc9_u32.to_le_bytes();
    const MAGIC_SIZE: u32 = size_of_val(&Self::MAGIC) as u32;
    const OFFSET: u32 = max(align_of::<Self>() as u32, Self::MAGIC_SIZE);
    const __: () = {
        assert!(size_of::<Self>() <= PAGE_SIZE);
    };

    fn init(flash: &RefCell<impl NorFlash>, start: u32) -> Self {
        let mut magic = Self::MAGIC;
        flash.borrow_mut().read(start, &mut magic).unwrap();
        let mut r = Self::default();
        if magic != Self::MAGIC {
            r.write(flash, start);
        } else {
            flash
                .borrow_mut()
                .read(start + Self::OFFSET, bytes_of_mut(&mut r))
                .unwrap();
        }
        r
    }

    fn write(&self, flash: &RefCell<impl NorFlash>, start: u32) {
        let mut flash = flash.borrow_mut();
        flash.erase(start, start + PAGE_SIZE as u32).unwrap();
        flash.write(start, &Self::MAGIC).unwrap();
        flash
            .write(start + Self::OFFSET as u32, bytes_of(self))
            .unwrap();
    }

    pub async fn apply(&self, pag: &mut Pag<mode::Idle>) {
        let Self {
            sensor_fps,
            sensor_gain,
            sensor_exposure_us,
            area_lower_bound,
            area_upper_bound,
            light_threshold,
            _padding,
        } = self;
        pag.set_sensor_fps(*sensor_fps).await.unwrap();
        pag.set_sensor_gain(*sensor_gain).await.unwrap();
        if *sensor_exposure_us % 100 == 0 && *sensor_exposure_us <= 12700 {
            pag.set_sensor_exposure_us(true, *sensor_exposure_us)
                .await
                .unwrap();
        } else {
            defmt::warn!(
                "sensor_exposure_us is an invalid value: {=u16}",
                sensor_exposure_us
            );
        }
        pag.set_area_lower_bound(*area_lower_bound).await.unwrap();
        pag.set_area_upper_bound(*area_upper_bound).await.unwrap();
        pag.set_light_threshold(*light_threshold).await.unwrap();
    }

    pub async fn read_from_pag(pag: &mut Pag<mode::Mode>) -> Self {
        Self {
            sensor_fps: pag.sensor_fps().await.unwrap(),
            sensor_gain: pag.sensor_gain().await.unwrap(),
            sensor_exposure_us: pag.sensor_exposure_us().await.unwrap().1,
            area_lower_bound: pag.area_lower_bound().await.unwrap(),
            area_upper_bound: pag.area_upper_bound().await.unwrap(),
            light_threshold: pag.light_threshold().await.unwrap(),
            _padding: [0; 3],
        }
    }
}

#[repr(C, align(4))]
#[derive(Clone, Copy, NoUninit, AnyBitPattern)]
struct AlignedGeneralConfig {
    inner: GeneralConfig,
}

static ACCEL_ODR: AtomicU16 = AtomicU16::new(200);
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GeneralSettings {
    general_config: AlignedGeneralConfig,
    // Atomic copy of accel odr from GeneralConfig
    pub accel_odr: &'static AtomicU16,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            general_config: AlignedGeneralConfig {
                inner: GeneralConfig {
                    impact_threshold: 5,
                    accel_config: Default::default(),
                    gyro_config: Default::default(),
                    camera_model_nf: CameraCalibrationParams {
                        camera_matrix: [
                            13182.6602, 0.0, 8941.14648, 0.0, 13172.4834, 8340.88477, 0.0, 0.0, 1.0,
                        ],
                        dist_coeffs: [-0.0183234196, 0.0833942071, 0.0, 0.0, -0.0357658006],
                    },
                    camera_model_wf: CameraCalibrationParams {
                        camera_matrix: [
                            13182.6602, 0.0, 8941.14648, 0.0, 13172.4834, 8340.88477, 0.0, 0.0, 1.0,
                        ],
                        dist_coeffs: [-0.0183234196, 0.0833942071, 0.0, 0.0, -0.0357658006],
                    },
                    stereo_iso: Default::default(),
                    suppress_ms: 100,
                    _padding: [0; 40],
                }
                .into(),
            },
            accel_odr: &ACCEL_ODR,
        }
    }
}

impl GeneralSettings {
    const MAGIC: [u8; 4] = 0x5bbddf6d_u32.to_le_bytes();
    const MAGIC_SIZE: u32 = size_of_val(&Self::MAGIC) as u32;
    const OFFSET: u32 = max(align_of::<Self>() as u32, Self::MAGIC_SIZE);
    const __: () = {
        assert!(size_of::<Self>() <= PAGE_SIZE);
    };

    fn init(flash: &RefCell<impl NorFlash>, start: u32) -> Self {
        let mut magic = Self::MAGIC;
        flash.borrow_mut().read(start, &mut magic).unwrap();
        let mut r = Self::default();
        if magic != Self::MAGIC {
            r.write(flash, start);
        } else {
            flash
                .borrow_mut()
                .read(start + Self::OFFSET, bytes_of_mut(&mut r.general_config))
                .unwrap();
        }
        r
    }

    fn write(&self, flash: &RefCell<impl NorFlash>, start: u32) {
        let mut flash = flash.borrow_mut();
        flash.erase(start, start + PAGE_SIZE as u32).unwrap();
        flash.write(start, &Self::MAGIC).unwrap();
        flash
            .write(start + Self::OFFSET, bytes_of(&self.general_config))
            .unwrap();
    }

    pub fn general_config(&self) -> &GeneralConfig {
        &self.general_config.inner
    }

    pub fn set_general_config(&mut self, general_config: &GeneralConfig) {
        self.general_config.inner = *general_config;
        ACCEL_ODR.store(
            self.general_config.inner.accel_config.accel_odr,
            Ordering::Relaxed,
        );
    }
}

#[repr(C, align(4))]
#[derive(Clone, Copy, CheckedBitPattern, NoUninit)]
pub struct TransientSettings {
    pub mode: protodongers::Mode,
    _padding: [u8; 3],
}

impl Default for TransientSettings {
    fn default() -> Self {
        Self {
            mode: protodongers::Mode::Object,
            _padding: [0; 3],
        }
    }
}

impl TransientSettings {
    const MAGIC: [u8; 4] = 0x42006969_u32.to_le_bytes();
    const MAGIC_SIZE: u32 = size_of_val(&Self::MAGIC) as u32;
    const OFFSET: u32 = max(align_of::<Self>() as u32, Self::MAGIC_SIZE);
    const __: () = {
        assert!(size_of::<Self>() <= PAGE_SIZE);
    };

    fn init(flash: &RefCell<impl NorFlash>, start: u32) -> Self {
        let mut magic = Self::MAGIC;
        flash.borrow_mut().read(start, &mut magic).unwrap();
        let mut r = Self::default();
        if magic != Self::MAGIC {
            r.write(flash, start);
        } else {
            let mut buf = [0u8; core::mem::size_of::<Self>()];
            flash
                .borrow_mut()
                .read(start + Self::OFFSET, &mut buf)
                .unwrap();

            r = match checked::try_from_bytes::<TransientSettings>(&buf) {
                Ok(v) => *v,
                Err(_) => {
                    defmt::info!("Failed to read TransientSettings, using default");
                    Self::default()
                }
            };

            defmt::info!("Mode is read as {}", r.mode as u8);
        }
        r
    }

    fn write(&self, flash: &RefCell<impl NorFlash>, start: u32) {
        let mut flash = flash.borrow_mut();
        flash.erase(start, start + PAGE_SIZE as u32).unwrap();
        flash.write(start, &Self::MAGIC).unwrap();
        flash.write(start + Self::OFFSET, bytes_of(self)).unwrap();
    }
}
