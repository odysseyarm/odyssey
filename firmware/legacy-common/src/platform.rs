//! Traits each board crate (legacy-vm, legacy-atslite) implements so the core
//! logic here stays board-independent.
use core::future::Future;

use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;

use crate::Paj;
use crate::odyssey_protocol::Packet;
use crate::settings::Settings;

/// One IMU sample.
pub struct ImuData {
    pub accel: [i32; 3],
    pub gyro: [i32; 3],
    pub timestamp_micros: u32,
}

pub trait Imu {
    type Error: core::fmt::Debug;
    type Interrupt;

    /// Reads accel, gyro and a timestamp. The timestamp is in microseconds and
    /// may wrap, so compare it with wrapping arithmetic.
    fn read_data(&mut self) -> impl Future<Output = Result<ImuData, Self::Error>>;
}

pub trait ImuInterrupt {
    fn wait(&mut self) -> impl Future<Output = ()>;
}

/// BLE L2CAP channels. Boards without BLE (the VM) supply a mock that never
/// receives and drops what it sends.
pub trait L2capChannels: Sized {
    type Receiver: L2capReceiver;
    type Sender: L2capSender;

    fn control_rx(&self) -> &Self::Receiver;
    fn data_rx(&self) -> &Self::Receiver;
    fn control_tx(&self) -> &Self::Sender;
    fn data_tx(&self) -> &Self::Sender;

    /// Changes whenever BLE queues are drained for a connection transition.
    /// Boards without BLE can return a constant.
    fn session_generation(&self) -> u32;
}

pub trait L2capReceiver {
    fn receive(&self) -> impl Future<Output = Packet>;
}

pub trait L2capSender {
    fn send(&self, pkt: Packet) -> impl Future<Output = ()>;
    fn try_send(&self, pkt: Packet) -> Result<(), ()>;
}

pub trait Platform {
    type Imu: Imu;
    type ImuInterrupt: ImuInterrupt;
    type L2capChannels: L2capChannels;
    type Timer: embassy_nrf::timer::Instance;

    const NAME: &'static str;

    /// IMU accelerometer scaling: LSB per g
    const ACC_LSB_PER_G: f32;

    /// IMU gyroscope scaling: LSB per degree/second
    const GYRO_LSB_PER_DPS: f32;

    /// Maps raw accel axes into the common frame.
    fn transform_accel(raw: [i32; 3]) -> [i32; 3];

    /// Maps raw gyro axes into the common frame.
    fn transform_gyro(raw: [i32; 3]) -> [i32; 3];

    /// Rounds an accelerometer ODR (Hz) to the nearest rate the sensor supports.
    fn round_accel_odr(hz: u16) -> u16;

    /// Device ID for this platform
    fn device_id() -> [u8; 8];

    /// Update the BLE advertising name. No-op on platforms without BLE.
    fn set_ble_name(_name: &str) {}

    /// Net-core release counter, 0 when unknown or when the platform has no net core.
    fn netcore_release() -> u16 {
        0
    }
}

/// Context passed to object_mode containing platform-specific state
pub struct PlatformContext<P: Platform> {
    pub paj7025r2: &'static AsyncMutex<NoopRawMutex, Paj>,
    pub paj7025r3: &'static AsyncMutex<NoopRawMutex, Paj>,
    pub imu: P::Imu,
    pub imu_int: P::ImuInterrupt,
    pub usb_snd: Endpoint<'static, In>,
    pub usb_rcv: Endpoint<'static, Out>,
    pub settings: &'static mut Settings,
    pub l2cap_channels: P::L2capChannels,
    pub timer: embassy_nrf::Peri<'static, P::Timer>,
    pub fod_set_ch: embassy_nrf::Peri<'static, embassy_nrf::ppi::AnyConfigurableChannel>,
    pub fod_clr_ch: embassy_nrf::Peri<'static, embassy_nrf::ppi::AnyConfigurableChannel>,
}
