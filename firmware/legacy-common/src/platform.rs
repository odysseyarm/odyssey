//! Traits each board crate (legacy-vm, legacy-atslite) implements so the core
//! logic here stays board-independent.
use core::future::Future;

use crate::odyssey_protocol::Packet;

/// One IMU sample.
///
/// Since: Odyssey Desktop 0.7.0.
pub struct ImuData {
    /// Since: Odyssey Desktop 0.7.0.
    pub accel: [i32; 3],
    /// Since: Odyssey Desktop 0.7.0.
    pub gyro: [i32; 3],
    /// Since: Odyssey Desktop 0.7.0.
    pub timestamp_micros: u32,
}

/// Since: Odyssey Desktop 0.7.0.
pub trait Imu {
    /// Since: Odyssey Desktop 0.7.0.
    type Error: core::fmt::Debug;
    /// Since: Odyssey Desktop 0.7.0.
    type Interrupt;

    /// Reads accel, gyro and a timestamp. The timestamp is in microseconds and
    /// may wrap, so compare it with wrapping arithmetic.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn read_data(&mut self) -> impl Future<Output = Result<ImuData, Self::Error>>;
}

/// Since: Odyssey Desktop 0.7.0.
pub trait ImuInterrupt {
    /// Since: Odyssey Desktop 0.7.0.
    fn wait(&mut self) -> impl Future<Output = ()>;
}

/// BLE L2CAP channels. Boards without BLE (the VM) supply a mock that never
/// receives and drops what it sends.
///
/// Since: Odyssey Desktop 0.7.0.
pub trait L2capChannels: Sized {
    /// Since: Odyssey Desktop 0.7.0.
    type Receiver: L2capReceiver;
    /// Since: Odyssey Desktop 0.7.0.
    type Sender: L2capSender;

    /// Since: Odyssey Desktop 0.7.0.
    fn control_rx(&self) -> &Self::Receiver;
    /// Since: Odyssey Desktop 0.7.0.
    fn data_rx(&self) -> &Self::Receiver;
    /// Since: Odyssey Desktop 0.7.0.
    fn control_tx(&self) -> &Self::Sender;
    /// Since: Odyssey Desktop 0.7.0.
    fn data_tx(&self) -> &Self::Sender;

    /// Changes whenever BLE queues are drained for a connection transition.
    /// Boards without BLE can return a constant.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn session_generation(&self) -> u32;
}

/// Since: Odyssey Desktop 0.7.0.
pub trait L2capReceiver {
    /// Since: Odyssey Desktop 0.7.0.
    fn receive(&self) -> impl Future<Output = Packet>;
}

/// Since: Odyssey Desktop 0.7.0.
pub trait L2capSender {
    /// Since: Odyssey Desktop 0.7.0.
    fn send(&self, pkt: Packet) -> impl Future<Output = ()>;
    /// Since: Odyssey Desktop 0.7.0.
    fn try_send(&self, pkt: Packet) -> Result<(), ()>;
}

/// Since: Odyssey Desktop 0.7.0.
pub trait Platform {
    /// Since: Odyssey Desktop 0.7.0.
    type Imu: Imu;
    /// Since: Odyssey Desktop 0.7.0.
    type ImuInterrupt: ImuInterrupt;
    /// Since: Odyssey Desktop 0.7.0.
    type L2capChannels: L2capChannels;
    /// Since: Odyssey Desktop 0.7.0.
    type Timer: embassy_nrf::timer::Instance;

    /// Since: Odyssey Desktop 0.7.0.
    const NAME: &'static str;

    /// OTA service supplied by this board's library, absent on USB-only boards.
    const OTA: Option<ota_channel::OtaHandle>;

    /// IMU accelerometer scaling: LSB per g
    ///
    /// Since: Odyssey Desktop 0.7.0.
    const ACC_LSB_PER_G: f32;

    /// IMU gyroscope scaling: LSB per degree/second
    ///
    /// Since: Odyssey Desktop 0.7.0.
    const GYRO_LSB_PER_DPS: f32;

    /// Maps raw accel axes into the common frame.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn transform_accel(raw: [i32; 3]) -> [i32; 3];

    /// Maps raw gyro axes into the common frame.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn transform_gyro(raw: [i32; 3]) -> [i32; 3];

    /// Rounds an accelerometer ODR (Hz) to the nearest rate the sensor supports.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn round_accel_odr(hz: u16) -> u16;

    /// Device ID for this platform
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn device_id() -> [u8; 8];

    /// Waits for this board's settings worker before resetting flash hardware.
    fn settings_wait_settled(timeout: embassy_time::Duration) -> impl Future<Output = ()>;

    /// Update the BLE advertising name. No-op on platforms without BLE.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn set_ble_name(_name: &str) {}

    /// Net-core release counter, 0 when unknown or when the platform has no net core.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    fn netcore_release() -> u16 {
        0
    }

    /// Stage2 bootloader version, `[0, 0, 0]` when unknown or when the platform has no
    /// versioned bootloader.
    ///
    /// Since: Unreleased.
    fn bootloader_version() -> [u32; 3] {
        [0, 0, 0]
    }
}
