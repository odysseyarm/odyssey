use core::future::Future;
use core::task::{Context, Poll, Waker};

use embedded_hal_async::spi::{Error, ErrorKind, ErrorType, SpiDevice};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusFault;

impl Error for BusFault {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

struct Sensor {
    registers: [[u8; 256]; 16],
    bank: usize,
    transactions: usize,
    fail_at: Option<usize>,
    pending_at: Option<usize>,
    released: bool,
}

impl Sensor {
    fn new() -> Self {
        Self {
            registers: [[0; 256]; 16],
            bank: 0,
            transactions: 0,
            fail_at: None,
            pending_at: None,
            released: false,
        }
    }
}

// A register-backed sensor model: assertions inspect the resulting sensor state, not
// copies of the driver's transactions. Borrowing it also exercises non-'static devices.
struct Device<'a>(&'a mut Sensor);

impl Drop for Device<'_> {
    fn drop(&mut self) {
        self.0.released = true;
    }
}

impl ErrorType for Device<'_> {
    type Error = BusFault;
}

impl SpiDevice for Device<'_> {
    async fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), BusFault> {
        let index = self.0.transactions;
        self.0.transactions += 1;
        if self.0.fail_at == Some(index) {
            return Err(BusFault);
        }
        if self.0.pending_at == Some(index) {
            core::future::pending::<()>().await;
        }
        let [Operation::Write(command), Operation::Write(data)] = operations else {
            panic!("unexpected sensor operation");
        };
        let [mode, register] = **command else {
            panic!("invalid sensor command");
        };
        assert_eq!(mode & CMD_READ, 0);
        if register == REG_BANK_SELECT {
            assert_eq!(data.len(), 1);
            self.0.bank = data[0] as usize;
        } else {
            if data.len() > 1 {
                assert_ne!(mode & CMD_MULTI, 0);
            }
            let start = register as usize;
            self.0.registers[self.0.bank][start..start + data.len()].copy_from_slice(data);
        }
        Ok(())
    }
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("unexpected pending transaction"),
    }
}

#[test]
fn image_exposure_preserves_exact_boundary_values() {
    for (nanoseconds, raw) in [(20_000, 25u16), (20_800, 26), (52_428_000, u16::MAX)] {
        let mut sensor = Sensor::new();
        let mut driver = ready(Paj7025::init(Device(&mut sensor))).unwrap();
        ready(driver.set_image_exposure_ns(nanoseconds)).unwrap();
        drop(driver);
        assert_eq!(&sensor.registers[0x0C][0x0F..=0x10], &raw.to_le_bytes());
    }
}

#[test]
fn invalid_image_exposure_never_changes_sensor_state() {
    for nanoseconds in [0, 19_200, 19_999, 20_001, 52_428_001, 52_428_800, u32::MAX] {
        let mut sensor = Sensor::new();
        sensor.registers[0x0C][0x0F] = 0x55;
        sensor.registers[0x0C][0x10] = 0xAA;
        let mut driver = ready(Paj7025::init(Device(&mut sensor))).unwrap();
        assert_eq!(
            ready(driver.set_image_exposure_ns(nanoseconds)),
            Err(Paj7025Error::InvalidImageExposure { nanoseconds }),
        );
        drop(driver);
        assert_eq!(sensor.transactions, 0);
        assert_eq!(&sensor.registers[0x0C][0x0F..=0x10], &[0x55, 0xAA]);
    }
}

#[test]
fn image_transition_releases_device_and_enables_image_output() {
    let mut sensor = Sensor::new();
    let driver = ready(Paj7025::init(Device(&mut sensor))).unwrap();
    ready(driver.into_image_mode()).unwrap();
    assert!(sensor.released);
    assert_eq!(sensor.registers[0][0x2F], 7);
    assert_eq!(sensor.registers[0][0x30], 1);
    assert_eq!(sensor.registers[0][0x01], 1);
    assert_eq!(&sensor.registers[0x0C][0x6D..=0x70], &[6; 4]);
}

#[test]
fn image_transition_stops_on_each_spi_error_and_releases_device() {
    for fail_at in 0..17 {
        let mut sensor = Sensor::new();
        sensor.fail_at = Some(fail_at);
        let driver = ready(Paj7025::init(Device(&mut sensor))).unwrap();
        assert_eq!(
            ready(driver.into_image_mode()),
            Err(Paj7025Error::Spi(BusFault))
        );
        assert!(sensor.released);
        assert_eq!(sensor.transactions, fail_at + 1);
    }
}

#[test]
fn cancelling_image_transition_releases_device_without_rolling_back() {
    let mut sensor = Sensor::new();
    sensor.pending_at = Some(7);
    let driver = ready(Paj7025::init(Device(&mut sensor))).unwrap();
    {
        let mut transition = core::pin::pin!(driver.into_image_mode());
        assert!(
            transition
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert!(sensor.released);
    assert_eq!(sensor.registers[0][0x2F], 7);
    assert_eq!(sensor.registers[0][0x30], 0);
    assert_eq!(sensor.registers[0][0x01], 0);
}
