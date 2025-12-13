use core::convert::Infallible;
use core::future::Future;

use bmi2::bmi2_async::Bmi2;
use bmi2::config;
use bmi2::interface::SpiInterface;
use bmi2::types::{Burst, Error, IntIoCtrl, IntLatch, IntMapData, MapData, PwrCtrl};
use embassy_nrf::Peri;
use embassy_nrf::gpio::{AnyPin, Input, Output, Pull};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::DeviceError;

type Bmi2Type<const N: usize> =
    Bmi2<SpiInterface<embedded_hal_bus::spi::ExclusiveDevice<Spim<'static>, Output<'static>, Delay>>, Delay, N>;

pub struct VmImu<const N: usize> {
    inner: Bmi2Type<N>,
}

pub struct VmImuInterrupt {
    irq: Input<'static>,
}

// Implement the Imu trait from common::platform
impl<const N: usize> common::platform::Imu for VmImu<N> {
    type Error = Error<DeviceError<spim::Error, Infallible>>;
    type Interrupt = VmImuInterrupt;

    fn read_accel(&mut self) -> impl Future<Output = Result<[i16; 3], Self::Error>> {
        async move {
            let data = self.inner.get_acc_data().await?;
            Ok([data.x, data.y, data.z])
        }
    }

    fn read_gyro(&mut self) -> impl Future<Output = Result<[i16; 3], Self::Error>> {
        async move {
            let data = self.inner.get_gyr_data().await?;
            Ok([data.x, data.y, data.z])
        }
    }
}

// Implement the ImuInterrupt trait
impl common::platform::ImuInterrupt for VmImuInterrupt {
    fn wait(&mut self) -> impl Future<Output = ()> {
        self.irq.wait_for_low()
    }
}

pub async fn init<SPI, IRQ, const N: usize, const MAX: u16>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: common::utils::SpiPins,
    irq: Peri<'static, AnyPin>,
) -> Result<(VmImu<N>, VmImuInterrupt), Error<DeviceError<spim::Error, Infallible>>>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'static,
{
    let irq = Input::new(irq, Pull::None);

    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M8;
    spim_config.mode = spim::MODE_0;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;

    let spi_device = common::utils::make_spi_dev(spi, irqs, pins, spim_config);

    let mut imu: Bmi2Type<N> = Bmi2::new_spi(spi_device, Delay, Burst::new(MAX)).await;

    let chip_id = imu.get_chip_id().await.unwrap();
    defmt::info!("BMI270 chip id: {}", chip_id);

    imu.init(&config::BMI270_CONFIG_FILE).await?;
    let acc_conf = {
        use bmi2::types::{AccBwp, AccConf, Odr, PerfMode};
        AccConf {
            odr: Odr::Odr100,
            bwp: AccBwp::Osr4Avg1,
            filter_perf: PerfMode::Perf,
        }
    };
    let gyr_conf = {
        use bmi2::types::{GyrBwp, GyrConf, Odr, PerfMode};
        GyrConf {
            odr: Odr::Odr100,
            bwp: GyrBwp::Osr4,
            filter_perf: PerfMode::Perf,
            noise_perf: PerfMode::Perf,
        }
    };
    imu.disable_power_save().await?;
    imu.set_acc_conf(acc_conf).await?;
    imu.set_gyr_conf(gyr_conf).await?;
    imu.set_acc_range(bmi2::types::AccRange::Range16g).await?;
    imu.set_gyr_range(bmi2::types::GyrRange {
        range: bmi2::types::GyrRangeVal::Range500,
        ois_range: bmi2::types::OisRange::Range250,
    })
    .await?;
    imu.set_int_map_data(IntMapData {
        int1: MapData {
            ffull: false,
            fwm: false,
            drdy: true,
            err: false,
        },
        int2: MapData {
            ffull: false,
            fwm: false,
            drdy: false,
            err: false,
        },
    })
    .await?;
    imu.set_int1_io_ctrl(IntIoCtrl {
        level: bmi2::types::OutputLevel::ActiveLow,
        od: bmi2::types::OutputBehavior::PushPull,
        output_en: true,
        input_en: false,
    })
    .await?;
    imu.set_pwr_ctrl(PwrCtrl {
        acc_en: true,
        gyr_en: true,
        aux_en: false,
        temp_en: false,
    })
    .await?;
    imu.set_int_latch(IntLatch::None).await?;

    Ok((VmImu { inner: imu }, VmImuInterrupt { irq }))
}
