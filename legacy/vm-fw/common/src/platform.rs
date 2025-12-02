/// Platform abstraction traits for board-specific implementations
///
/// Each board crate (vm-board, atslite-board) implements these traits
/// to provide platform-specific functionality while keeping the core
/// logic in this common library platform-agnostic.

use core::future::Future;

use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;

use crate::protodongers::{Packet, PacketType};

use crate::Paj;
use crate::settings::Settings;

/// IMU (Inertial Measurement Unit) abstraction
pub trait Imu {
    type Error: core::fmt::Debug;
    type Interrupt;

    /// Read accelerometer data [x, y, z] in raw ADC units
    fn read_accel(&mut self) -> impl Future<Output = Result<[i16; 3], Self::Error>>;

    /// Read gyroscope data [x, y, z] in raw ADC units
    fn read_gyro(&mut self) -> impl Future<Output = Result<[i16; 3], Self::Error>>;
}

/// IMU interrupt source abstraction
pub trait ImuInterrupt {
    /// Wait for the next IMU interrupt
    fn wait(&mut self) -> impl Future<Output = ()>;
}

/// L2CAP channel abstraction for BLE communication
/// For platforms without BLE (like VM), this is a no-op mock
pub trait L2capChannels: Sized {
    type Receiver: L2capReceiver;
    type Sender: L2capSender;

    fn control_rx(&self) -> &Self::Receiver;
    fn data_rx(&self) -> &Self::Receiver;
    fn control_tx(&self) -> &Self::Sender;
    fn data_tx(&self) -> &Self::Sender;
}

pub trait L2capReceiver {
    fn receive(&self) -> impl Future<Output = Packet>;
}

pub trait L2capSender {
    fn send(&self, pkt: Packet) -> impl Future<Output = ()>;
    fn try_send(&self, pkt: Packet) -> Result<(), ()>;
}

/// Platform-specific configuration and initialization
pub trait Platform {
    type Imu: Imu;
    type ImuInterrupt: ImuInterrupt;
    type L2capChannels: L2capChannels;
    type Timer: embassy_nrf::timer::Instance;

    /// Platform name for logging
    const NAME: &'static str;

    /// Device ID for this platform
    fn device_id() -> [u8; 8];
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

/// Helper function to check if a packet should go to the control channel
pub fn is_control_packet(pkt: &Packet) -> bool {
    match pkt.ty() {
        PacketType::ObjectReport()
        | PacketType::CombinedMarkersReport()
        | PacketType::PocMarkersReport()
        | PacketType::AccelReport()
        | PacketType::ImpactReport() => false,
        _ => true,
    }
}
