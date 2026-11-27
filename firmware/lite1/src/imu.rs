//! ICM-first IMU selection; LSM6DSV320X fallback on its independent SPI bus.

use embassy_nrf::Peri;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pin, Port, Pull};
use embassy_nrf::gpiote::{self, OutputChannelPolarity};
use embassy_nrf::peripherals::{SERIAL0, SERIAL1};
use embassy_nrf::ppi::{self, Ppi};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::timer::{self, Timer};
use embassy_time::{Delay, Duration, with_timeout};
use embedded_hal_async::spi::SpiDevice;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::{ICM42688, Ready};

use crate::pins::{Icm42688p, Lsm6dsv320x};
use crate::sensors::{Imu, SensorInterrupt};
use crate::{Irqs, lsm};

/// The ready ICM-42688-P on its own SPI bus.
///
/// Since: Odyssey Desktop 0.6.0.
pub type IcmDriver = ICM42688<
    ExclusiveDevice<Spim<'static, embassy_nrf::mode::Async>, Output<'static>, Delay>,
    Ready,
>;

/// IMU startup failed before a usable motion stream was available.
#[derive(Debug, defmt::Format)]
pub enum InitError {
    Spi,
    Lsm(lsm::LsmError),
}

const VALID_ODRS_ICM42688: &[(u16, icm426xx::config::AccelOdr)] = {
    use icm426xx::config::AccelOdr;
    &[
        (2, AccelOdr::_1_5625Hz),
        (3, AccelOdr::_3_125Hz),
        (6, AccelOdr::_6_25Hz),
        (13, AccelOdr::_12_5Hz),
        (25, AccelOdr::_25Hz),
        (50, AccelOdr::_50Hz),
        (100, AccelOdr::_100Hz),
        (200, AccelOdr::_200Hz),
        (500, AccelOdr::_500Hz),
        (1000, AccelOdr::_1kHz),
        (2000, AccelOdr::_2kHz),
        (4000, AccelOdr::_4kHz),
        (8000, AccelOdr::_8kHz),
        (16000, AccelOdr::_16kHz),
        (32000, AccelOdr::_32kHz),
    ]
};

fn nearest_odr_index(hz: u16) -> usize {
    let mut best = 0;
    for (i, &(val, _)) in VALID_ODRS_ICM42688.iter().enumerate() {
        let curr_diff = (hz as i32 - VALID_ODRS_ICM42688[best].0 as i32).unsigned_abs();
        let new_diff = (hz as i32 - val as i32).unsigned_abs();
        if new_diff < curr_diff {
            best = i;
        }
    }
    best
}

/// Rounds `hz` to the nearest accelerometer ODR the ICM-42688-P supports.
///
/// Since: Odyssey Desktop 0.6.0.
pub fn round_accel_odr_hz(hz: u16) -> u16 {
    VALID_ODRS_ICM42688[nearest_odr_index(hz)].0
}

/// Selects ICM when its identity matches, otherwise initializes the LSM.
///
/// Unpopulated MISO lines are pulled down and inactive interrupt inputs are
/// disconnected. Startup is bounded; CLKIN is retained only for a ready ICM.
#[expect(
    clippy::too_many_arguments,
    reason = "Each peripheral parameter owns a distinct IMU resource"
)]
pub async fn init<T: timer::Instance, C: gpiote::Channel>(
    icm_instance: Peri<'static, SERIAL0>,
    lsm_instance: Peri<'static, SERIAL1>,
    icm_pins: Icm42688p,
    lsm_pins: Lsm6dsv320x,
    clkin_timer_instance: Peri<'static, T>,
    clkin_ppi_ch: Peri<'static, impl ppi::ConfigurableChannel>,
    clkin_gpiote_ch: Peri<'static, C>,
) -> Result<(Imu, SensorInterrupt), InitError> {
    let icm_int = Input::new_async(icm_pins.int1, Irqs, Pull::Up);
    let lsm_int = Input::new_async(lsm_pins.int1, Irqs, Pull::Up);
    let _lsm_int2 = Input::new(lsm_pins.int2, Pull::Down);
    let mut config = spim::Config::default();
    config.frequency = spim::Frequency::M8;
    config.mode = spim::MODE_3;
    config.bit_order = spim::BitOrder::MsbFirst;
    let miso_pins = [
        (icm_pins.miso.port(), icm_pins.miso.pin()),
        (lsm_pins.miso.port(), lsm_pins.miso.pin()),
    ];
    let icm_spim = Spim::new(
        icm_instance,
        icm_pins.sck,
        icm_pins.mosi,
        icm_pins.miso,
        Irqs,
        config.clone(),
    );
    // The LSM adapter only makes blocking transfers. An async-mode Spim serves them too and
    // shares its code with the ICM bus; a blocking-mode Spim costs 640 B of flash we lack.
    let lsm_spim = Spim::new(
        lsm_instance,
        lsm_pins.sck,
        lsm_pins.mosi,
        lsm_pins.miso,
        Irqs,
        config,
    );
    // SPIM clears GPIO pulls; hold unpopulated MISO lines at a known level.
    for (port, pin) in miso_pins {
        let gpio = match port {
            Port::Port0 => embassy_nrf::pac::P0,
            Port::Port1 => embassy_nrf::pac::P1,
        };
        gpio.pin_cnf(usize::from(pin)).modify(|w| {
            w.set_pull(embassy_nrf::pac::gpio::vals::Pull::Pulldown);
        });
    }
    let Ok(mut icm_spi) = ExclusiveDevice::new(
        icm_spim,
        Output::new(icm_pins.cs, Level::High, OutputDrive::Standard),
        Delay,
    );
    let Ok(mut lsm_spi) = ExclusiveDevice::new(
        lsm_spim,
        Output::new(lsm_pins.cs, Level::High, OutputDrive::Standard),
        Delay,
    );

    let mut identity = [0x75 | 0x80, 0];
    icm_spi
        .transfer_in_place(&mut identity)
        .await
        .map_err(|_| InitError::Spi)?;
    // Match the WHO_AM_I value accepted by the locked icm426xx driver.
    if identity[1] == 0xdb {
        let clkin_ch = gpiote::OutputChannel::new(
            clkin_gpiote_ch,
            icm_pins.clkin,
            Level::Low,
            OutputDrive::Standard,
            OutputChannelPolarity::Toggle,
        );
        // 8 MHz / 125 gives 64 kHz toggles, a 32 kHz CLKIN square wave.
        let timer = Timer::new(clkin_timer_instance);
        timer.set_frequency(timer::Frequency::F8MHz);
        let cc = timer.cc(0);
        cc.write(125);
        cc.short_compare_clear();
        let mut ppi_ch = Ppi::new_one_to_one(clkin_ppi_ch, cc.event_compare(), clkin_ch.task_out());
        ppi_ch.enable();
        timer.start();

        let mut imu_config = icm426xx::Config::default();
        {
            use icm426xx::config::{AccelMode, Drive, GyroOdr, Pin9Function, Polarity};
            // Sampled faster than reported; object mode sums the samples into each report.
            imu_config.gyro.odr = GyroOdr::_1kHz;
            imu_config.accel.odr = icm426xx::config::AccelOdr::_1kHz;
            imu_config.accel.mode = AccelMode::LowNoise;
            imu_config.int1.drive = Drive::PushPull;
            imu_config.int1.polarity = Polarity::ActiveLow;
            imu_config.pin9.function = Pin9Function::CLKIN;
            imu_config.fifo_watermark = 1;
        }
        if let Ok(Ok(driver)) = with_timeout(
            Duration::from_millis(250),
            ICM42688::new(icm_spi).initialize(Delay, imu_config),
        )
        .await
        {
            match lsm::Lsm6dsv320xSensor::standby(&mut lsm_spi) {
                Ok(()) | Err(lsm::LsmError::WrongWhoAmI) => {}
                Err(error) => return Err(InitError::Lsm(error)),
            }
            timer.persist();
            ppi_ch.persist();
            clkin_ch.persist();
            defmt::info!("ICM42688 selected");
            return Ok((
                Imu::Icm(crate::sensors::Icm42688Sensor::new(driver)),
                SensorInterrupt { int1: icm_int },
            ));
        }
        defmt::warn!("ICM initialization failed; trying LSM");
    }

    let sensor = lsm::Lsm6dsv320xSensor::initialize(lsm_spi).map_err(InitError::Lsm)?;
    defmt::info!("LSM6DSV320X selected at 120 Hz");
    Ok((Imu::Lsm(sensor), SensorInterrupt { int1: lsm_int }))
}
