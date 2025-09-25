use core::convert::Infallible;

use bmi2::bmi2_async::Bmi2;
use bmi2::config;
use bmi2::interface::SpiInterface;
use bmi2::types::{Burst, Error, FifoConf, IntIoCtrl, IntLatch, IntMapData, IntMapFeat, MapData, PwrConf, PwrCtrl};
use embassy_nrf::Peri;
use embassy_nrf::gpio::{AnyPin, Input, Output, Pull};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::DeviceError;

use crate::pins;
use crate::utils::make_spi_dev;

pub type Imu<const N: usize> =
    Bmi2<SpiInterface<embedded_hal_bus::spi::ExclusiveDevice<Spim<'static>, Output<'static>, Delay>>, Delay, N>;

pub struct ImuInterrupt {
    irq: Input<'static>,
}

impl ImuInterrupt {
    pub fn wait(&mut self) -> impl Future<Output = ()> {
        self.irq.wait_for_low()
    }
}

pub async fn init<SPI, IRQ, const N: usize, const MAX: u16>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: pins::Spi,
    irq: Peri<'static, AnyPin>,
) -> Result<(Imu<N>, ImuInterrupt), Error<DeviceError<spim::Error, Infallible>>>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'static,
{
    let irq = Input::new(irq, Pull::None);

    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M8;
    spim_config.mode = spim::MODE_0;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;

    let spi_device = make_spi_dev(spi, irqs, pins, spim_config);

    let mut imu: Bmi2<_, _, N> = Bmi2::new_spi(spi_device, Delay, Burst::new(MAX)).await;

    let chip_id = imu.get_chip_id().await.unwrap();
    defmt::info!("chip id: {}", chip_id);

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

    Ok((imu, ImuInterrupt { irq }))
}
