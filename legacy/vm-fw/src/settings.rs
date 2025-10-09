//! Settings
//!
//! The current implementation uses three flash pages to store settings. One for PAJ settings,
//! one for other general settings like calibrations, and one for transient settings like current mode (TODO unused).
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
use core::convert::Infallible;
use core::sync::atomic::{AtomicU16, Ordering};

use bytemuck::{AnyBitPattern, CheckedBitPattern, NoUninit, bytes_of, bytes_of_mut, checked};
use defmt::Format;
use embassy_nrf::nvmc::PAGE_SIZE;
use embedded_hal_bus::spi::DeviceError;
use embedded_storage::nor_flash::NorFlash;
use paj7025::Paj7025Error;
use protodongers::wire::{CameraCalibrationParams, GeneralConfig};
pub use settings_region::get_settings_region;
use static_cell::StaticCell;

use crate::Paj;

static SETTINGS: StaticCell<Settings> = StaticCell::new();

pub async fn init_settings(
    flash: &RefCell<impl NorFlash>,
    pajr2: &mut Paj,
    pajr3: &mut Paj,
) -> Result<&'static mut Settings, Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
    let region = get_settings_region();
    assert!(region.len() >= 2 * PAGE_SIZE);
    let s = Settings::init(flash, region.start as u32);
    s.apply(pajr2, pajr3).await?;
    let s = SETTINGS.init(s);
    Ok(s)
}

pub struct Settings {
    pub pajs: PajsSettings,
    pub general: GeneralSettings,
    pub transient: TransientSettings,
    pajs_start: u32,
    general_start: u32,
    transient_start: u32,
}

impl Settings {
    fn init(flash: &RefCell<impl NorFlash>, start: u32) -> Self {
        let pajs_start = start;
        let general_start = start + PAGE_SIZE as u32;
        let transient_start = start + PAGE_SIZE as u32 * 2;
        let pajs = PajsSettings::init(flash, pajs_start);
        let general = GeneralSettings::init(flash, general_start);
        let transient = TransientSettings::init(flash, transient_start);
        Self {
            pajs,
            general,
            transient,
            pajs_start,
            general_start,
            transient_start,
        }
    }

    /// Make the settings take effect. Only applies to pajr2 for now
    pub async fn apply(
        &self,
        pajr2: &mut Paj,
        pajr3: &mut Paj,
    ) -> Result<(), Paj7025Error<DeviceError<embassy_nrf::spim::Error, Infallible>>> {
        self.pajs.apply(pajr2, pajr3).await
    }

    /// Write the settings to flash
    pub fn write(&self, flash: &RefCell<impl NorFlash>) {
        self.pajs.write(flash, self.pajs_start);
        self.general.write(flash, self.general_start);
    }

    /// Transient settings are written to flash independently of the others
    pub fn transient_write(&self, flash: &RefCell<impl NorFlash>) {
        self.transient.write(flash, self.transient_start);
    }
}

#[repr(C, align(4))]
#[derive(Clone, Copy, Format, NoUninit, AnyBitPattern)]
pub struct PajSettings {
    pub frame_period: u32,
    pub area_threshold_max: u16,
    pub exposure_time: u16,
    pub resolution_x: u16,
    pub resolution_y: u16,
    pub area_threshold_min: u8,
    pub brightness_threshold: u8,
    pub frame_subtraction: u8,
    pub gain_1: u8,
    pub gain_2: u8,
    pub max_object_cnt: u8,
    pub noise_threshold: u8,
    pub operation_mode: u8,
}

#[repr(C, align(4))]
#[derive(Clone, Copy, Format, NoUninit, AnyBitPattern)]
pub struct PajsSettings(PajSettings, PajSettings);

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

const fn max(a: u32, b: u32) -> u32 {
    if a < b { b } else { a }
}
impl PajsSettings {
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
        flash.write(start + Self::OFFSET, bytes_of(self)).unwrap();
    }

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
                        .. // ignore padding or future fields
                    } = $s;

                    $paj.ll.control().bank_0().cmd_oahb()
                        .write_async(|x| x.set_value(area_threshold_max)).await?;
                    $paj.ll.control().bank_c().cmd_oalb()
                        .write_async(|x| x.set_value(area_threshold_min)).await?;
                    $paj.ll.control().bank_c().cmd_thd()
                        .write_async(|x| x.set_value(brightness_threshold)).await?;
                    $paj.ll.control().bank_c().b_expo()
                        .write_async(|x| x.set_value(exposure_time)).await?;
                    $paj.ll.control().bank_c().cmd_frame_period()
                        .write_async(|x| x.set_value(frame_period)).await?;
                    $paj.ll.control().bank_0().cmd_frame_subtraction_on()
                        .write_async(|x| x.set_value(frame_subtraction)).await?;
                    $paj.ll.control().bank_c().b_global()
                        .write_async(|x| x.set_value(gain_1)).await?;
                    $paj.ll.control().bank_c().b_ggh()
                        .write_async(|x| x.set_value(gain_2)).await?;
                    $paj.ll.control().bank_0().cmd_max_object_num()
                        .write_async(|x| x.set_value(max_object_cnt)).await?;
                    $paj.ll.control().bank_0().cmd_nthd()
                        .write_async(|x| x.set_value(noise_threshold)).await?;
                    $paj.ll.control().bank_0().cmd_dsp_operation_mode()
                        .write_async(|x| x.set_value(operation_mode)).await?;
                    $paj.ll.control().bank_c().cmd_scale_resolution_x()
                        .write_async(|x| x.set_value(resolution_x)).await?;
                    $paj.ll.control().bank_c().cmd_scale_resolution_y()
                        .write_async(|x| x.set_value(resolution_y)).await?;

                    $paj.ll.control().bank_0().bank_0_sync_updated_flag().write_async(|x| x.set_value(1)).await?;
                    $paj.ll.control().bank_1().bank_1_sync_updated_flag().write_async(|x| x.set_value(1)).await?;

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
                        camera_matrix: [141.1052, 0.0, 49.0, 0.0, 141.1052, 49.0, 0.0, 0.0, 1.0],
                        dist_coeffs: [0.0, 0.0, 0.0, 0.0, 0.0],
                    },
                    camera_model_wf: CameraCalibrationParams {
                        camera_matrix: [34.341216, 0.0, 48.73854, 0.0, 34.39486, 49.98844, 0.0, 0.0, 1.0],
                        dist_coeffs: [0.039820534, -0.039933169, 0.00043006078, -0.0012057066, 0.005302234],
                    },
                    stereo_iso: Default::default(),
                    suppress_ms: 200,
                },
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
        ACCEL_ODR.store(self.general_config.inner.accel_config.accel_odr, Ordering::Relaxed);
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
            flash.borrow_mut().read(start + Self::OFFSET, &mut buf).unwrap();

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
