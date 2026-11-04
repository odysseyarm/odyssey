//! object_mode sensor traits for the lite1 PAG7665QN and ICM-42688-P.

use bytemuck::{cast_slice, from_bytes};
use defmt::Format;
use embassy_nrf::gpio::Input;
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::fifo::FifoPacket4;
use pag7665qn::mode::{self, ModeT};
use pag7665qn::spi::Pag7665QnSpi;
use pag7665qn::{Interface, Pag7665Qn};
use static_cell::ConstStaticCell;

use crate::object_mode::{ImuData, ImuInterrupt, ImuSensor, PagInterrupt, PagSensor};

/// PAG7665QN driver on its own SPIM bus.
pub type PagDriver<M> = Pag7665Qn<
    Pag7665QnSpi<
        ExclusiveDevice<
            embassy_nrf::spim::Spim<'static>,
            embassy_nrf::gpio::Output<'static>,
            Delay,
        >,
    >,
    Delay,
    M,
>;

/// Sensor failure, kept field-less so defmt can print it.
#[derive(Format)]
pub enum SensorError {
    Spi,
    InvalidData,
}

/// The PAG7665QN as an object_mode [`PagSensor`].
pub struct Pag7665QnSensor {
    pub driver: PagDriver<mode::Mode>,
    image_roi: Option<u8>,
}

impl Pag7665QnSensor {
    pub fn new(driver: PagDriver<mode::Mode>) -> Self {
        Self {
            driver,
            image_roi: None,
        }
    }
}

impl PagSensor for Pag7665QnSensor {
    type Error = SensorError;

    async fn read_register(&mut self, bank: u8, address: u8) -> Result<u8, Self::Error> {
        self.driver
            .set_bank(bank)
            .await
            .map_err(|_| SensorError::Spi)?;
        self.driver
            .bus_unchecked()
            .read_byte(address)
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn write_register(&mut self, bank: u8, address: u8, data: u8) -> Result<(), Self::Error> {
        self.driver
            .set_bank(bank)
            .await
            .map_err(|_| SensorError::Spi)?;
        self.driver
            .bus_unchecked()
            .write(address, &[data])
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn get_objects(
        &mut self,
        objs: &mut [pag7665qn::types::Object; 16],
    ) -> Result<Option<u8>, Self::Error> {
        self.driver
            .switch_mode_mut(mode::Object.mode())
            .await
            .map_err(|_| SensorError::Spi)?;
        self.driver
            .try_get_objects::<core::convert::Infallible>(objs)
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn switch_to_idle(&mut self) -> Result<(), Self::Error> {
        self.driver
            .switch_mode_mut(mode::Mode::Idle)
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn switch_to_image(&mut self) -> Result<(), Self::Error> {
        self.switch_to_idle().await?;
        // ROI_CTRL bit 0 is the documented ROI enable. Disable that known restriction and
        // restore the exact byte before the next object snapshot. SENSOR_SIZE_SEL's mapping
        // is undocumented here, so it is not guessed or changed.
        let roi = match self.image_roi {
            Some(roi) => roi,
            None => {
                let roi = self.read_register(0, 0x72).await?;
                self.image_roi = Some(roi);
                roi
            }
        };
        self.write_register(0, 0x72, roi & !1).await?;
        self.driver
            .switch_mode_mut(mode::Mode::Image)
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn try_get_image(&mut self, image: &mut [u8; 320 * 240]) -> Result<bool, Self::Error> {
        self.driver
            .try_get_image::<core::convert::Infallible>(image)
            .await
            .map_err(|_| SensorError::Spi)
    }

    async fn restore_object(&mut self) -> Result<(), Self::Error> {
        let roi = async {
            if let Some(roi) = self.image_roi {
                self.switch_to_idle().await?;
                self.write_register(0, 0x72, roi).await?;
                self.image_roi = None;
            }
            Ok::<(), SensorError>(())
        }
        .await;
        let object = self
            .driver
            .restore_object_mode()
            .await
            .map_err(|_| SensorError::Spi);
        roi?;
        object
    }
}

/// The PAG7665QN INT_O line as an object_mode [`PagInterrupt`]; waits for it to go high.
pub struct Pag7665QnInterrupt {
    pub int_o: Input<'static>,
}

impl PagInterrupt for Pag7665QnInterrupt {
    async fn wait(&mut self) {
        self.int_o.wait_for_high().await;
    }
}

/// The ready ICM-42688-P on its own SPI bus.
pub type ImuDriver = icm426xx::ICM42688<
    ExclusiveDevice<embassy_nrf::spim::Spim<'static>, embassy_nrf::gpio::Output<'static>, Delay>,
    icm426xx::Ready,
>;

/// The ICM-42688-P as an object_mode [`ImuSensor`], timestamped from its FIFO.
pub struct Icm42688Sensor {
    pub driver: ImuDriver,
    buffer: &'static mut [u32; 521],
    ts_micros: u32,
}

impl Icm42688Sensor {
    pub fn new(driver: ImuDriver) -> Self {
        static BUFFER: ConstStaticCell<[u32; 521]> = ConstStaticCell::new([0; 521]);
        Self {
            driver,
            buffer: BUFFER.take(),
            ts_micros: 0,
        }
    }
}

impl ImuSensor for Icm42688Sensor {
    type Error = SensorError;

    async fn read_data(&mut self) -> Result<ImuData, Self::Error> {
        let _items = self
            .driver
            .read_fifo(self.buffer)
            .await
            .map_err(|_| SensorError::Spi)?;
        let pkt = from_bytes::<FifoPacket4>(&cast_slice(self.buffer)[4..24]);

        // FIFO timestamps count CLKIN ticks at 32 kHz.
        self.ts_micros = self
            .ts_micros
            .wrapping_add(1000 * (pkt.timestamp() as u32) / 32);

        Ok(ImuData {
            accel: [pkt.accel_data_x(), pkt.accel_data_y(), pkt.accel_data_z()],
            gyro: [pkt.gyro_data_x(), pkt.gyro_data_y(), pkt.gyro_data_z()],
            timestamp_micros: self.ts_micros,
        })
    }
}

/// The ICM-42688-P INT1 line as an object_mode [`ImuInterrupt`].
pub struct Icm42688Interrupt {
    pub int1: Input<'static>,
}

impl ImuInterrupt for Icm42688Interrupt {
    async fn wait(&mut self) {
        self.int1.wait_for_low().await;
    }
}
