//! Sensor trait implementations for lite1 board
//!
//! Implements the object_mode traits for PAG7665QN and ICM-42688-P

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

/// PAG7665QN sensor type
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

/// Simple error wrapper for defmt formatting
#[derive(Format)]
pub enum SensorError {
    Spi,
    InvalidData,
}

/// PAG sensor wrapper implementing PagSensor trait
pub struct Pag7665QnSensor {
    pub driver: PagDriver<mode::Mode>,
}

impl Pag7665QnSensor {
    pub fn new(driver: PagDriver<mode::Mode>) -> Self {
        Self { driver }
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
}

/// PAG interrupt wrapper
pub struct Pag7665QnInterrupt {
    pub int_o: Input<'static>,
}

impl PagInterrupt for Pag7665QnInterrupt {
    async fn wait(&mut self) {
        self.int_o.wait_for_high().await;
    }
}

/// IMU driver type
pub type ImuDriver = icm426xx::ICM42688<
    ExclusiveDevice<embassy_nrf::spim::Spim<'static>, embassy_nrf::gpio::Output<'static>, Delay>,
    icm426xx::Ready,
>;

/// IMU wrapper implementing ImuSensor trait
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

        // CLKIN ticks (32 kHz)
        self.ts_micros = self
            .ts_micros
            .wrapping_add(1000 * (pkt.timestamp() as u32) / 32);

        Ok(ImuData {
            accel: [
                pkt.accel_data_x() as i16,
                pkt.accel_data_y() as i16,
                pkt.accel_data_z() as i16,
            ],
            gyro: [
                pkt.gyro_data_x() as i16,
                pkt.gyro_data_y() as i16,
                pkt.gyro_data_z() as i16,
            ],
            timestamp_micros: self.ts_micros,
        })
    }
}

/// IMU interrupt wrapper implementing ImuInterrupt trait
pub struct Icm42688Interrupt {
    pub int1: Input<'static>,
}

impl ImuInterrupt for Icm42688Interrupt {
    async fn wait(&mut self) {
        self.int1.wait_for_low().await;
    }
}
