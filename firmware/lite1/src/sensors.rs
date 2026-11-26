//! object_mode sensor traits for Lite1 vision and interchangeable IMUs.

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
///
/// Since: Odyssey Desktop 0.6.0.
pub type PagDriver<M> = Pag7665Qn<
    Pag7665QnSpi<
        ExclusiveDevice<
            embassy_nrf::spim::Spim<'static, embassy_nrf::mode::Async>,
            embassy_nrf::gpio::Output<'static>,
            Delay,
        >,
    >,
    Delay,
    M,
>;

/// Sensor failure, kept field-less so defmt can print it.
///
/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug, Format)]
pub enum SensorError {
    /// Since: Odyssey Desktop 0.6.0.
    Spi,
    /// Since: Odyssey Desktop 0.6.0.
    InvalidData,
    Lsm(crate::lsm::LsmError),
}

/// The PAG7665QN as an object_mode [`PagSensor`].
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Pag7665QnSensor {
    /// Since: Odyssey Desktop 0.6.0.
    pub driver: PagDriver<mode::Mode>,
    image_roi: Option<u8>,
}

impl Pag7665QnSensor {
    /// Since: Odyssey Desktop 0.6.0.
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
        // ROI_CTRL bit 0 is the documented ROI enable. Clear it, remembering the original byte
        // so the next object snapshot can restore it. SENSOR_SIZE_SEL's mapping is
        // undocumented here, so it is left alone.
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

/// The PAG7665QN INT_O line as an object_mode [`PagInterrupt`]. Waits for it to go high.
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Pag7665QnInterrupt {
    /// Since: Odyssey Desktop 0.6.0.
    pub int_o: Input<'static, embassy_nrf::mode::Async>,
}

impl PagInterrupt for Pag7665QnInterrupt {
    async fn wait(&mut self) {
        self.int_o.wait_for_high().await;
    }
}

/// Boot-selected IMU; each adapter reports board-frame SI measurements.
pub enum Imu {
    Icm(Icm42688Sensor),
    Lsm(crate::lsm::Lsm6dsv320xSensor),
}

impl ImuSensor for Imu {
    type Error = SensorError;

    async fn read_data(&mut self) -> Result<ImuData, Self::Error> {
        match self {
            Self::Icm(sensor) => sensor.read_data().await,
            Self::Lsm(sensor) => sensor.read_data().await.map_err(SensorError::Lsm),
        }
    }

    fn has_pending_data(&self) -> bool {
        match self {
            Self::Icm(sensor) => sensor.has_pending_data(),
            Self::Lsm(sensor) => sensor.has_pending_data(),
        }
    }
}

/// The ICM-42688-P with buffered FIFO samples and its 32 kHz timestamp.
pub struct Icm42688Sensor {
    driver: crate::imu::IcmDriver,
    buffer: &'static mut [u32; 521],
    packet_count: usize,
    next_packet: usize,
    ts_micros: u32,
    ts_quarters: u32,
}

impl Icm42688Sensor {
    pub fn new(driver: crate::imu::IcmDriver) -> Self {
        static BUFFER: ConstStaticCell<[u32; 521]> = ConstStaticCell::new([0; 521]);
        Self {
            driver,
            buffer: BUFFER.take(),
            packet_count: 0,
            next_packet: 0,
            ts_micros: 0,
            ts_quarters: 0,
        }
    }
}

impl ImuSensor for Icm42688Sensor {
    type Error = SensorError;

    async fn read_data(&mut self) -> Result<ImuData, Self::Error> {
        if !self.has_pending_data() {
            self.packet_count = self
                .driver
                .read_fifo(self.buffer)
                .await
                .map_err(|_| SensorError::Spi)?;
            self.next_packet = 0;
            if self.packet_count == 0 || self.packet_count > (self.buffer.len() * 4 - 4) / 20 {
                self.packet_count = 0;
                return Err(SensorError::InvalidData);
            }
        }
        let start = 4 + self.next_packet * 20;
        let pkt = from_bytes::<FifoPacket4>(&cast_slice(self.buffer)[start..start + 20]);
        self.next_packet += 1;
        (self.ts_micros, self.ts_quarters) =
            advance_icm_timestamp(self.ts_micros, self.ts_quarters, pkt.timestamp());
        icm_data(pkt, self.ts_micros)
    }

    fn has_pending_data(&self) -> bool {
        self.next_packet < self.packet_count
    }
}

fn advance_icm_timestamp(micros: u32, fraction: u32, ticks: u16) -> (u32, u32) {
    let quarters = fraction + 125 * u32::from(ticks);
    (micros.wrapping_add(quarters / 4), quarters % 4)
}

#[inline(never)]
fn icm_data(pkt: &FifoPacket4, timestamp_micros: u32) -> Result<ImuData, SensorError> {
    let accel = [pkt.accel_data_x(), pkt.accel_data_y(), pkt.accel_data_z()];
    let gyro = [pkt.gyro_data_x(), pkt.gyro_data_y(), pkt.gyro_data_z()];
    if pkt.fifo_header & 0xf0 != 0x70 || accel.contains(&-524_288) || gyro.contains(&-524_288) {
        return Err(SensorError::InvalidData);
    }
    let accel = accel.map(|raw| raw as f32 / 32768.0 * 9.806_65);
    let gyro = gyro.map(|raw| (raw as f32 / 262.144).to_radians());
    Ok(ImuData {
        accel: [accel[1], accel[0], -accel[2]],
        gyro: [gyro[1], gyro[0], -gyro[2]],
        timestamp_micros,
    })
}

/// Active-low data-ready line of the selected IMU.
pub struct SensorInterrupt {
    pub int1: Input<'static, embassy_nrf::mode::Async>,
}

impl ImuInterrupt for SensorInterrupt {
    async fn wait(&mut self) {
        self.int1.wait_for_low().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icm_maps_signed_motion_to_board_si_units() {
        let packet = FifoPacket4 {
            fifo_header: 0x78,
            accel_data_y1: 0x08,
            accel_data_z1: 0x10,
            gyro_data_x1: 0x10,
            ..FifoPacket4::default()
        };
        let sample = icm_data(&packet, 123).unwrap();
        assert!((sample.accel[0] - 9.806_65).abs() < 0.000_01);
        assert_eq!(sample.accel[1], 0.0);
        assert!((sample.accel[2] + 2.0 * 9.806_65).abs() < 0.000_01);
        assert!((sample.gyro[1] - 250.0_f32.to_radians()).abs() < 0.000_01);
        assert_eq!(sample.timestamp_micros, 123);
    }

    #[test]
    fn icm_rejects_empty_or_incomplete_fifo_samples() {
        assert!(matches!(
            icm_data(&FifoPacket4::default(), 0),
            Err(SensorError::InvalidData)
        ));
        let packet = FifoPacket4 {
            fifo_header: 0x78,
            accel_data_x1: 0x80,
            ..FifoPacket4::default()
        };
        assert!(matches!(
            icm_data(&packet, 0),
            Err(SensorError::InvalidData)
        ));
    }

    #[test]
    fn icm_timestamp_retains_fraction_and_wraps_microseconds() {
        let (mut micros, mut fraction) = (0, 0);
        for _ in 0..4 {
            (micros, fraction) = advance_icm_timestamp(micros, fraction, 1);
        }
        assert_eq!((micros, fraction), (125, 0));
        assert_eq!(advance_icm_timestamp(u32::MAX - 10, 0, 1), (20, 1));
    }
}
