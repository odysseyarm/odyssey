//! Object mode - handles protodongers protocol packets for vision sensor streaming
//!
//! This module processes incoming control packets (via BLE L2CAP or USB) and
//! manages streaming of object/marker data, IMU data, and impact events.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_futures::join::join4;
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Timer;
use nalgebra::Vector3;
use protodongers::control::device::TransportMode;
use protodongers::wire::GeneralConfig;
use protodongers::{ConfigKind, Packet, PacketData, PacketType, Props, ReadRegisterResponse};

use crate::ble::l2cap_bridge::{self, L2capChannels};
use crate::settings::PagSettings;
use crate::transport_mode;

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;

/// Determines if a packet is a control packet (vs streaming data)
pub fn is_control_packet(pkt: &Packet) -> bool {
    l2cap_bridge::is_control_packet(pkt)
}

/// Stream info tracking for each stream type
#[derive(Default)]
struct StreamInfo {
    req_id: AtomicU8,
    enable: AtomicBool,
}

impl StreamInfo {
    fn enabled(&self) -> bool {
        self.enable.load(Ordering::Acquire)
    }

    fn req_id(&self) -> u8 {
        self.req_id.load(Ordering::Relaxed)
    }

    fn set_req_id(&self, req_id: u8) {
        self.req_id.store(req_id, Ordering::Relaxed);
    }

    fn set_enable(&self, enable: bool) {
        self.enable.store(enable, Ordering::Release);
    }
}

/// Stream infos for all stream types
#[derive(Default)]
struct StreamInfos {
    imu: StreamInfo,
    impact: StreamInfo,
    marker: StreamInfo,
}

impl StreamInfos {
    fn new() -> Self {
        Self::default()
    }

    fn disable_all(&self) {
        self.imu.enable.store(false, Ordering::Relaxed);
        self.impact.enable.store(false, Ordering::Relaxed);
        self.marker.enable.store(false, Ordering::Relaxed);
    }

    fn enable(&self, req_id: u8, ty: PacketType) {
        defmt::info!("Enable stream {:?}, id = {=u8}", ty, req_id);
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::PocMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_req_id(req_id);
        si.set_enable(true);
    }

    fn disable(&self, ty: PacketType) {
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::PocMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_enable(false);
    }
}

/// Impact detection settings (atomically updatable)
#[allow(dead_code)]
struct ImpactSettings {
    threshold: AtomicU8,
    suppress_ms: AtomicU8,
}

#[allow(dead_code)]
impl ImpactSettings {
    fn new(threshold: u8, suppress_ms: u8) -> Self {
        Self {
            threshold: AtomicU8::new(threshold),
            suppress_ms: AtomicU8::new(suppress_ms),
        }
    }

    fn threshold(&self) -> u8 {
        self.threshold.load(Ordering::Relaxed)
    }

    fn suppress_ms(&self) -> u8 {
        self.suppress_ms.load(Ordering::Relaxed)
    }

    fn set_threshold(&self, val: u8) {
        self.threshold.store(val, Ordering::Relaxed);
    }

    fn set_suppress_ms(&self, val: u8) {
        self.suppress_ms.store(val, Ordering::Relaxed);
    }
}

/// Firmware version extracted from Cargo.toml
const MAJOR: u16 = match u16::from_str_radix(env!("CARGO_PKG_VERSION_MAJOR"), 10) {
    Ok(v) => v,
    Err(_) => 0,
};
const MINOR: u16 = match u16::from_str_radix(env!("CARGO_PKG_VERSION_MINOR"), 10) {
    Ok(v) => v,
    Err(_) => 0,
};
const PATCH: u16 = match u16::from_str_radix(env!("CARGO_PKG_VERSION_PATCH"), 10) {
    Ok(v) => v,
    Err(_) => 0,
};

/// Trait for PAG sensor interface (PAG7665QN)
#[allow(async_fn_in_trait)]
pub trait PagSensor {
    type Error: defmt::Format;

    /// Read a register from the sensor
    async fn read_register(&mut self, bank: u8, address: u8) -> Result<u8, Self::Error>;

    /// Write a register to the sensor
    async fn write_register(&mut self, bank: u8, address: u8, data: u8) -> Result<(), Self::Error>;

    /// Switch to object mode and get objects
    async fn get_objects(
        &mut self,
        objs: &mut [pag7665qn::types::Object; 16],
    ) -> Result<Option<u8>, Self::Error>;

    /// Switch to idle mode
    async fn switch_to_idle(&mut self) -> Result<(), Self::Error>;
}

/// Trait for IMU sensor interface
#[allow(async_fn_in_trait)]
pub trait ImuSensor {
    type Error: defmt::Format;

    /// Read IMU data (returns accel, gyro, timestamp)
    async fn read_data(&mut self) -> Result<ImuData, Self::Error>;
}

/// IMU data structure
pub struct ImuData {
    pub accel: [i32; 3],
    pub gyro: [i32; 3],
    pub timestamp_micros: u32,
}

/// Trait for IMU interrupt
#[allow(async_fn_in_trait)]
pub trait ImuInterrupt {
    /// Wait for data ready interrupt
    async fn wait(&mut self);
}

/// Trait for PAG interrupt
#[allow(async_fn_in_trait)]
pub trait PagInterrupt {
    /// Wait for data ready interrupt
    async fn wait(&mut self);
}

/// Read current PAG sensor settings from registers
async fn read_pag_settings<P: PagSensor>(pag: &mut P) -> Result<PagSettings, P::Error> {
    // Read all relevant registers from bank 0
    let exposure = pag.read_register(0, 0x67).await?;
    let gain = pag.read_register(0, 0x68).await?;
    let light_threshold = pag.read_register(0, 0x6D).await?;
    let area_min_lo = pag.read_register(0, 0x6E).await?;
    let area_min_hi = pag.read_register(0, 0x6F).await?;
    let area_max_lo = pag.read_register(0, 0x70).await?;
    let area_max_hi = pag.read_register(0, 0x71).await?;
    let circle_r_min = pag.read_register(0, 0x23).await?;
    let circle_r_max = pag.read_register(0, 0x24).await?;
    let circle_k_min = pag.read_register(0, 0x77).await?;
    let circle_k_max = pag.read_register(0, 0x78).await?;
    let frame_rate = pag.read_register(0, 0x13).await?;

    Ok(PagSettings {
        exposure,
        gain,
        light_threshold,
        area_min: u16::from_le_bytes([area_min_lo, area_min_hi]),
        area_max: u16::from_le_bytes([area_max_lo, area_max_hi]),
        circle_r_min,
        circle_r_max,
        circle_k_min,
        circle_k_max,
        frame_rate,
    })
}

/// Context for object mode
pub struct ObjectModeContext<'a, P, I, PI, II>
where
    P: PagSensor,
    I: ImuSensor,
    PI: PagInterrupt,
    II: ImuInterrupt,
{
    /// PAG sensor (mutex protected)
    pub pag: &'a AsyncMutex<NoopRawMutex, P>,
    /// PAG interrupt
    pub pag_int: PI,
    /// IMU sensor
    pub imu: I,
    /// IMU interrupt
    pub imu_int: II,
    /// Device ID (6 bytes)
    pub device_id: [u8; 6],
    /// Product ID
    pub product_id: u16,
    /// Settings reference for persisting config changes
    pub settings: &'a mut crate::settings::Settings,
}

/// Run object mode - the main entry point
///
/// This function runs the object mode loop, handling:
/// - Incoming control packets from BLE L2CAP
/// - Outgoing streaming data (markers, IMU, impact)
/// - Protocol handling (ReadConfig, WriteConfig, StreamUpdate, etc.)
pub async fn run<P, I, PI, II>(mut ctx: ObjectModeContext<'_, P, I, PI, II>)
where
    P: PagSensor,
    I: ImuSensor,
    PI: PagInterrupt,
    II: ImuInterrupt,
{
    defmt::info!("[object_mode] Starting object mode");

    let pkt_channel = Channel::<Packet, 64>::new();
    let stream_infos = StreamInfos::new();
    let impact_settings = ImpactSettings::new(
        ctx.settings.general.impact_threshold,
        ctx.settings.general.suppress_ms,
    );

    let l2cap = l2cap_bridge::get_or_init();

    // Run four concurrent tasks:
    // 1. Receive and process control packets
    // 2. Stream IMU data
    // 3. Stream marker data
    // 4. Send streaming data to BLE
    let recv_task = recv_loop(
        ctx.pag,
        &stream_infos,
        &l2cap,
        ctx.device_id,
        ctx.product_id,
        &impact_settings,
        ctx.settings,
    );

    let imu_task = imu_loop(
        &mut ctx.imu,
        &mut ctx.imu_int,
        pkt_channel.sender(),
        &stream_infos,
        &impact_settings,
    );

    let obj_task = obj_loop(
        ctx.pag,
        &mut ctx.pag_int,
        pkt_channel.sender(),
        &stream_infos,
    );

    let stream_task = stream_loop(pkt_channel.receiver(), &l2cap);

    // Join all tasks - they run forever
    join4(recv_task, imu_task, obj_task, stream_task).await;
}

/// Receive loop - handles incoming packets from BLE L2CAP channels
async fn recv_loop<P: PagSensor>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    stream_infos: &StreamInfos,
    l2cap: &L2capChannels,
    device_id: [u8; 6],
    product_id: u16,
    impact_settings: &ImpactSettings,
    settings: &mut crate::settings::Settings,
) {
    loop {
        // Wait for transport mode to be BLE
        while transport_mode::get() != TransportMode::Ble {
            defmt::debug!("[object_mode] Waiting for BLE transport mode...");
            transport_mode::wait_for_change().await;
        }

        // Listen on both control and data channels
        let pkt = match select(l2cap.control_rx.receive(), l2cap.data_rx.receive()).await {
            Either::First(pkt) => pkt,
            Either::Second(pkt) => pkt,
        };

        let response = match pkt.data {
            PacketData::WriteRegister(register) => {
                let mut pag = pag.lock().await;
                match pag
                    .write_register(register.bank, register.address, register.data)
                    .await
                {
                    Ok(()) => Some(Packet {
                        id: pkt.id,
                        data: PacketData::Ack(),
                    }),
                    Err(e) => {
                        defmt::error!("[object_mode] WriteRegister failed: {:?}", e);
                        None
                    }
                }
            }

            PacketData::ReadRegister(register) => {
                let mut pag = pag.lock().await;
                match pag.read_register(register.bank, register.address).await {
                    Ok(data) => Some(Packet {
                        id: pkt.id,
                        data: PacketData::ReadRegisterResponse(ReadRegisterResponse {
                            bank: register.bank,
                            address: register.address,
                            data,
                        }),
                    }),
                    Err(e) => {
                        defmt::error!("[object_mode] ReadRegister failed: {:?}", e);
                        None
                    }
                }
            }

            PacketData::StreamUpdate(s) => {
                use protodongers::StreamUpdateAction as S;
                defmt::info!(
                    "[object_mode] StreamUpdate: action={:?}, packet_id={:?}",
                    defmt::Debug2Format(&s.action),
                    defmt::Debug2Format(&s.packet_id)
                );
                match s.action {
                    S::Enable => stream_infos.enable(pkt.id, s.packet_id),
                    S::Disable => stream_infos.disable(s.packet_id),
                    S::DisableAll => {
                        defmt::info!("[object_mode] DisableAll streams");
                        stream_infos.disable_all();
                    }
                }
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::ReadProp(kind) => {
                use protodongers::PropKind;
                let prop = match kind {
                    PropKind::Uuid => Props::Uuid(device_id),
                    PropKind::ProductId => Props::ProductId(product_id),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadPropResponse(prop),
                })
            }

            PacketData::ReadVersion() => Some(Packet {
                id: pkt.id,
                data: PacketData::ReadVersionResponse(protodongers::Version::new([
                    MAJOR, MINOR, PATCH,
                ])),
            }),

            PacketData::WriteConfig(config) => {
                defmt::info!(
                    "[object_mode] WriteConfig: {:?}",
                    defmt::Debug2Format(&config)
                );
                let wire_config: GeneralConfig = config.into();
                // Update atomic impact settings for imu_loop
                match &wire_config {
                    GeneralConfig::ImpactThreshold(v) => impact_settings.set_threshold(*v),
                    GeneralConfig::SuppressMs(v) => impact_settings.set_suppress_ms(*v),
                    _ => {}
                }
                // Store into settings struct for persistence
                settings.general.set_general_config(&wire_config);
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::ReadConfig(kind) => {
                defmt::info!("[object_mode] ReadConfig: {:?}", kind);
                let g = &settings.general;
                let wire_config = match kind {
                    ConfigKind::ImpactThreshold => {
                        GeneralConfig::ImpactThreshold(g.impact_threshold)
                    }
                    ConfigKind::SuppressMs => GeneralConfig::SuppressMs(g.suppress_ms),
                    ConfigKind::AccelConfig => {
                        GeneralConfig::AccelConfig(g.accel_config.clone().into())
                    }
                    ConfigKind::GyroConfig => GeneralConfig::GyroConfig(g.gyro_config.clone()),
                    ConfigKind::CameraModelNf => {
                        GeneralConfig::CameraModelNf(g.camera_model_nf.clone())
                    }
                    ConfigKind::CameraModelWf => {
                        GeneralConfig::CameraModelWf(g.camera_model_wf.clone())
                    }
                    ConfigKind::StereoIso => GeneralConfig::StereoIso(g.stereo_iso.clone()),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadConfigResponse(wire_config.into()),
                })
            }

            PacketData::FlashSettings() => {
                defmt::info!(
                    "[object_mode] FlashSettings received. impact_threshold={}, suppress_ms={}",
                    settings.general.impact_threshold,
                    settings.general.suppress_ms,
                );

                // Read current PAG sensor settings (NF only)
                let pag_settings = {
                    let mut pag = pag.lock().await;
                    read_pag_settings(&mut *pag).await
                };

                if let Ok(pag_settings) = pag_settings {
                    settings.pag = pag_settings;
                } else {
                    defmt::error!("[object_mode] Failed to read PAG settings, saving general only");
                }

                // Save general + PAG settings to flash
                settings.general_write();
                settings.pag_write();
                defmt::info!("[object_mode] Settings saved to flash");

                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::WriteMode(mode) => {
                defmt::info!("[object_mode] WriteMode: {:?}", mode);
                // TODO: Handle mode switching (object/image mode)
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            // Ignore response packets and streaming data
            PacketData::ObjectReportRequest() => None,
            PacketData::Ack() => None,
            PacketData::ReadConfigResponse(_) => None,
            PacketData::ReadPropResponse(_) => None,
            PacketData::ObjectReport(_) => None,
            PacketData::CombinedMarkersReport(_) => None,
            PacketData::PocMarkersReport(_) => None,
            PacketData::AccelReport(_) => None,
            PacketData::ImpactReport(_) => None,
            PacketData::Vendor(_, _) => None,
            PacketData::ReadRegisterResponse(_) => None,
            PacketData::ReadVersionResponse(_) => None,
        };

        // Send response back via appropriate channel
        if let Some(r) = response {
            if r.id != 255 {
                if is_control_packet(&r) {
                    if l2cap.control_tx.try_send(r).is_err() {
                        defmt::warn!("[object_mode] Dropped control response - channel full");
                    }
                } else {
                    if l2cap.data_tx.try_send(r).is_err() {
                        defmt::warn!("[object_mode] Dropped data response - channel full");
                    }
                }
            }
        }
    }
}

/// Stream loop - sends streaming data packets to BLE
async fn stream_loop<const N: usize>(pkt_rcv: Receiver<'_, Packet, N>, l2cap: &L2capChannels) {
    defmt::info!("[stream_loop] Started");
    let mut pkt_count: u32 = 0;

    loop {
        let pkt = pkt_rcv.receive().await;
        pkt_count = pkt_count.wrapping_add(1);

        // Log every 100th packet to avoid spam
        if pkt_count % 100 == 0 {
            defmt::debug!("[stream_loop] Processed {} packets", pkt_count);
        }

        // Route to correct L2CAP channel based on packet type
        if is_control_packet(&pkt) {
            if l2cap.control_tx.try_send(pkt).is_err() {
                defmt::warn!("[stream_loop] Dropped control packet - channel full");
            }
        } else {
            if l2cap.data_tx.try_send(pkt).is_err() {
                defmt::warn!("[stream_loop] Dropped data packet - channel full");
            }
        }
    }
}

/// IMU loop - streams accelerometer and gyro data, detects impacts
async fn imu_loop<I: ImuSensor, II: ImuInterrupt, const N: usize>(
    imu: &mut I,
    imu_int: &mut II,
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
    impact_settings: &ImpactSettings,
) {
    const G: f32 = 9.806_65;

    // ICM-42688 scales:
    // Accel: 32768 LSB/g at default FS
    // Gyro: 262.144 LSB/(°/s) at default FS
    const ACC_LSB_PER_G: f32 = 32768.0;
    const GYRO_LSB_PER_DPS: f32 = 262.144;

    let mut last_impact_ts_micros: Option<u32> = None;

    loop {
        imu_int.wait().await;

        let imu_data = match imu.read_data().await {
            Ok(data) => data,
            Err(e) => {
                defmt::error!("[object_mode] IMU read failed: {:?}", e);
                continue;
            }
        };

        let acc = imu_data.accel;
        let gyr = imu_data.gyro;
        let ts_micros = imu_data.timestamp_micros;

        // Apply axis transforms (POC2/lite1 specific)
        // ICM on lite1 is rotated 90° CW vs legacy atslite (POC1).
        // Legacy transform: [raw[0], -raw[1], -raw[2]]
        // Rotation mapping: legacy_x = new_y, legacy_y = -new_x
        // Combined: [raw[1], raw[0], -raw[2]]
        let (ax, ay, az) = (acc[1], acc[0], -acc[2]);
        let (gx, gy, gz) = (gyr[1], gyr[0], -gyr[2]);

        // Convert to SI units
        let accel = Vector3::new(
            ax as f32 / ACC_LSB_PER_G * G,
            ay as f32 / ACC_LSB_PER_G * G,
            az as f32 / ACC_LSB_PER_G * G,
        );
        let gyro = Vector3::new(
            (gx as f32 / GYRO_LSB_PER_DPS).to_radians(),
            (gy as f32 / GYRO_LSB_PER_DPS).to_radians(),
            (gz as f32 / GYRO_LSB_PER_DPS).to_radians(),
        );

        // Send accel report if stream enabled
        if stream_infos.imu.enabled() {
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: PacketData::AccelReport(protodongers::AccelReport {
                    timestamp: ts_micros,
                    accel,
                    gyro,
                }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("[object_mode] IMU channel full, dropping packet");
            }
        }

        // Impact detection
        if stream_infos.impact.enabled() {
            let accel_magnitude_g = accel.magnitude() / G;
            let threshold_g = impact_settings.threshold() as f32;

            if accel_magnitude_g >= threshold_g {
                let suppress_us = (impact_settings.suppress_ms() as u32) * 1000;
                let should_send = match last_impact_ts_micros {
                    Some(last_ts) => ts_micros.wrapping_sub(last_ts) >= suppress_us,
                    None => true,
                };

                if should_send {
                    last_impact_ts_micros = Some(ts_micros);
                    let id = stream_infos.impact.req_id();
                    let pkt = Packet {
                        id,
                        data: PacketData::ImpactReport(protodongers::ImpactReport {
                            timestamp: ts_micros,
                        }),
                    };
                    if pkt_snd.try_send(pkt).is_err() {
                        defmt::warn!("[object_mode] Impact channel full, dropping packet");
                    }
                }
            }
        }
    }
}

/// Object loop - streams marker data from PAG sensor
async fn obj_loop<P: PagSensor, PI: PagInterrupt, const N: usize>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    pag_int: &mut PI,
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
) {
    defmt::info!("[obj_loop] Started");
    let mut last_enabled_log = false;

    loop {
        let enabled = stream_infos.marker.enabled();

        // Log state changes
        if enabled != last_enabled_log {
            if enabled {
                defmt::info!("[obj_loop] Marker stream ENABLED, starting to read objects");
            } else {
                defmt::info!("[obj_loop] Marker stream DISABLED, sleeping");
            }
            last_enabled_log = enabled;
        }

        if enabled {
            let mut objs = [pag7665qn::types::Object::DEFAULT; 16];

            // Wait for interrupt with timeout
            match embassy_time::with_timeout(
                embassy_time::Duration::from_millis(100),
                pag_int.wait(),
            )
            .await
            {
                Ok(_) => {}
                Err(_) => {
                    defmt::trace!("[obj_loop] PAG interrupt timeout");
                    continue;
                }
            }

            {
                let mut pag = pag.lock().await;
                match pag.get_objects(&mut objs).await {
                    Ok(Some(_)) => {}
                    Ok(None) => continue,
                    Err(e) => {
                        defmt::error!("[obj_loop] get_objects failed: {:?}", e);
                        continue;
                    }
                }
            };

            let id = stream_infos.marker.req_id();
            let pkt = Packet {
                id,
                data: PacketData::PocMarkersReport(protodongers::PocMarkersReport {
                    points: objs.map(|o| nalgebra::Point2::new(o.x(), o.y())),
                }),
            };

            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("[obj_loop] Marker channel full, dropping packet");
            }
        } else {
            Timer::after_millis(50).await;
        }
    }
}
