//! Object mode: answers odyssey-protocol control packets from BLE L2CAP or USB and streams
//! marker, IMU, impact and battery reports back over the active transport.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub use donger_common::usb_device::{UsbConfigHandle, read_transfer, write_transfer};
use embassy_futures::join::join5;
use embassy_futures::select::{Either3, select3};
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Timer;
use embassy_usb::driver::EndpointError;
use nalgebra::Vector3;
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::wire::GeneralConfig;
use odyssey_protocol::{
    BatteryReport, ConfigKind, Packet, PacketData, PacketType, Props, ReadRegisterResponse,
};

use crate::ble::l2cap_bridge::{self, L2capChannels};
use crate::settings::PagSettings;
use crate::transport_mode;

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;

pub fn is_control_packet(pkt: &Packet) -> bool {
    l2cap_bridge::is_control_packet(pkt)
}

/// Request id and enable flag of one report stream.
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

#[derive(Default)]
struct StreamInfos {
    imu: StreamInfo,
    impact: StreamInfo,
    marker: StreamInfo,
    battery: StreamInfo,
}

impl StreamInfos {
    fn new() -> Self {
        Self::default()
    }

    fn disable_all(&self) {
        self.imu.enable.store(false, Ordering::Relaxed);
        self.impact.enable.store(false, Ordering::Relaxed);
        self.marker.enable.store(false, Ordering::Relaxed);
        self.battery.enable.store(false, Ordering::Relaxed);
    }

    fn enable(&self, req_id: u8, ty: PacketType) {
        defmt::info!("Enable stream {:?}, id = {=u8}", ty, req_id);
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::PocMarkersReport() => &self.marker,
            PacketType::BatteryReport() => &self.battery,
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
            PacketType::BatteryReport() => &self.battery,
            _ => return,
        };
        si.set_enable(false);
    }
}

/// Impact threshold and suppression time, shared between the receive loop and the IMU loop.
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

/// PAG7665QN vision sensor as seen by object mode.
#[allow(async_fn_in_trait)]
pub trait PagSensor {
    type Error: defmt::Format;

    async fn read_register(&mut self, bank: u8, address: u8) -> Result<u8, Self::Error>;

    async fn write_register(&mut self, bank: u8, address: u8, data: u8) -> Result<(), Self::Error>;

    /// Puts the sensor in object mode and reads the detected objects into `objs`.
    async fn get_objects(
        &mut self,
        objs: &mut [pag7665qn::types::Object; 16],
    ) -> Result<Option<u8>, Self::Error>;

    async fn switch_to_idle(&mut self) -> Result<(), Self::Error>;
}

#[allow(async_fn_in_trait)]
pub trait ImuSensor {
    type Error: defmt::Format;

    /// Reads one accel/gyro sample with its timestamp.
    async fn read_data(&mut self) -> Result<ImuData, Self::Error>;
}

pub struct ImuData {
    pub accel: [i32; 3],
    pub gyro: [i32; 3],
    pub timestamp_micros: u32,
}

#[allow(async_fn_in_trait)]
pub trait ImuInterrupt {
    /// Waits for the IMU data-ready interrupt.
    async fn wait(&mut self);
}

#[allow(async_fn_in_trait)]
pub trait PagInterrupt {
    /// Waits for the PAG data-ready interrupt.
    async fn wait(&mut self);
}

/// Reads the persisted PAG settings back from the sensor's bank 0 registers.
async fn read_pag_settings<P: PagSensor>(pag: &mut P) -> Result<PagSettings, P::Error> {
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

/// Hardware and settings handed to [`run`].
pub struct ObjectModeContext<'a, P, I, PI, II>
where
    P: PagSensor,
    I: ImuSensor,
    PI: PagInterrupt,
    II: ImuInterrupt,
{
    pub pag: &'a AsyncMutex<NoopRawMutex, P>,
    pub pag_int: PI,
    pub imu: I,
    pub imu_int: II,
    pub device_id: [u8; 6],
    pub product_id: u16,
    /// `[major, minor, patch]`
    pub firmware_version: [u16; 3],
    /// Config writes from the host land here and are flushed to flash on request.
    pub settings: &'a mut crate::settings::Settings,
    /// USB bulk IN: reports and responses to the host.
    pub usb_snd: Endpoint<'static, In>,
    /// USB bulk OUT: commands from the host.
    pub usb_rcv: Endpoint<'static, Out>,
    pub usb_configured: UsbConfigHandle,
}

/// Runs object mode forever: the receive, IMU, marker, battery and send loops run concurrently,
/// and packets go out over USB or BLE according to the current transport mode.
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

    let recv_task = recv_loop(
        ctx.pag,
        &stream_infos,
        &l2cap,
        ctx.device_id,
        ctx.product_id,
        ctx.firmware_version,
        &impact_settings,
        ctx.settings,
        &mut ctx.usb_rcv,
        ctx.usb_configured,
        pkt_channel.sender(),
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

    let stream_task = stream_loop(
        pkt_channel.receiver(),
        &l2cap,
        &mut ctx.usb_snd,
        ctx.usb_configured,
    );

    let battery_task = battery_loop(pkt_channel.sender(), &stream_infos);

    join5(recv_task, imu_task, obj_task, stream_task, battery_task).await;
}

async fn recv_loop<P: PagSensor, const N: usize>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    stream_infos: &StreamInfos,
    l2cap: &L2capChannels,
    device_id: [u8; 6],
    product_id: u16,
    firmware_version: [u16; 3],
    impact_settings: &ImpactSettings,
    settings: &mut crate::settings::Settings,
    usb_rcv: &mut Endpoint<'static, Out>,
    usb_configured: UsbConfigHandle,
    resp_snd: Sender<'_, Packet, N>,
) {
    let mut usb_buf = [0u8; 512];

    loop {
        let mode = transport_mode::get();
        let usb_enabled = usb_configured.is_configured() && matches!(mode, TransportMode::Usb);
        let ble_enabled = matches!(mode, TransportMode::Ble);

        let (from_usb, pkt) = match select3(
            async {
                loop {
                    if !usb_enabled {
                        if matches!(mode, TransportMode::Usb) {
                            // USB mode but the host has not configured the device yet. Poll
                            // instead of waiting on the config signal: Signal has a single
                            // consumer, and other tasks wait on it too.
                            embassy_time::Timer::after_millis(10).await;
                            // Once configured, go on to read_transfer.
                            if !usb_configured.is_configured() {
                                continue;
                            }
                        } else {
                            // BLE mode: USB never delivers a packet.
                            core::future::pending::<()>().await;
                        }
                    }
                    match read_transfer(usb_rcv, &mut usb_buf).await {
                        Ok(n) => {
                            match postcard::from_bytes::<Packet>(&usb_buf[..n]) {
                                Ok(pkt) => break pkt,
                                Err(e) => {
                                    defmt::error!("[object_mode] USB packet deserialization failed: {:?}", defmt::Debug2Format(&e));
                                    continue;
                                }
                            }
                        }
                        Err(EndpointError::Disabled) => {
                            defmt::debug!("[object_mode] USB endpoint disabled, waiting for reconfiguration...");
                            usb_configured.wait_for_event().await;
                            continue;
                        }
                        Err(e) => {
                            defmt::error!("[object_mode] USB read error: {:?}", defmt::Debug2Format(&e));
                            embassy_time::Timer::after_millis(10).await;
                            continue;
                        }
                    }
                }
            },
            async {
                if !ble_enabled {
                    core::future::pending::<Packet>().await
                } else {
                    l2cap.control_rx.receive().await
                }
            },
            async {
                if !ble_enabled {
                    core::future::pending::<Packet>().await
                } else {
                    l2cap.data_rx.receive().await
                }
            },
        )
        .await
        {
            Either3::First(pkt) => (true, pkt),
            Either3::Second(pkt) => (false, pkt),
            Either3::Third(pkt) => (false, pkt),
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
                use odyssey_protocol::StreamUpdateAction as S;
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
                use odyssey_protocol::PropKind;
                let prop = match kind {
                    PropKind::Uuid => Props::Uuid(device_id),
                    PropKind::ProductId => Props::ProductId(product_id),
                    PropKind::Name => Props::Name(settings.general.name.clone().into()),
                    PropKind::Version => {
                        Props::Version(odyssey_protocol::Version::new(firmware_version))
                    }
                    PropKind::NetCoreRelease => {
                        Props::NetCoreRelease(crate::ble::netcore::netcore_release())
                    }
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadPropResponse(prop),
                })
            }

            PacketData::ReadVersion() => Some(Packet {
                id: pkt.id,
                data: PacketData::ReadVersionResponse(odyssey_protocol::Version::new(
                    firmware_version,
                )),
            }),

            PacketData::WriteConfig(config) => {
                defmt::info!(
                    "[object_mode] WriteConfig: {:?}",
                    defmt::Debug2Format(&config)
                );
                let wire_config: GeneralConfig = config.into();
                // imu_loop reads impact settings from the atomics, not from `settings`.
                match &wire_config {
                    GeneralConfig::ImpactThreshold(v) => impact_settings.set_threshold(*v),
                    GeneralConfig::SuppressMs(v) => impact_settings.set_suppress_ms(*v),
                    _ => {}
                }
                // Persisted on the next FlashSettings.
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

                // Snapshot the PAG registers (NF sensor only) so they persist too.
                let pag_settings = {
                    let mut pag = pag.lock().await;
                    read_pag_settings(&mut *pag).await
                };

                if let Ok(pag_settings) = pag_settings {
                    settings.pag = pag_settings;
                } else {
                    defmt::error!("[object_mode] Failed to read PAG settings, saving general only");
                }

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
                // Object/image mode switching is not implemented; the request is just acked.
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::SetDeviceName(name) => {
                defmt::info!("[object_mode] SetDeviceName: {:?}", name.as_str());
                // No ADV_RESTART_SIGNAL: this may be running inside the dongle's BLE
                // connection, and a restart would drop it. The new name is advertised on the
                // next advertising cycle.
                crate::ble::peripheral::set_device_name_initial(name.as_str());
                settings.general.name = name.into();
                settings.general_write();
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::SetDeviceNameResponse(
                        odyssey_protocol::SetDeviceNameResult::Ok(),
                    ),
                })
            }

            // Responses and streaming reports are not commands.
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
            PacketData::BatteryReport(_) => None,
            PacketData::SetDeviceNameResponse(_) => None,
        };

        // Reply on the transport the command came in on. Id 255 asks for no reply.
        if let Some(r) = response {
            if r.id != 255 {
                if from_usb {
                    // stream_loop owns the USB IN endpoint. Await the send so a channel full
                    // of streaming data delays the response instead of dropping it.
                    resp_snd.send(r).await;
                } else {
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
}

/// Sends a battery report once a second while the battery stream is enabled.
async fn battery_loop<const N: usize>(
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
) -> ! {
    loop {
        embassy_time::Timer::after_secs(1).await;
        if stream_infos.battery.enabled() {
            let percent = crate::power_state::SOC_PERCENT.load(Ordering::Relaxed);
            let charging = crate::power_state::VBUS_PRESENT.load(Ordering::Relaxed);
            let id = stream_infos.battery.req_id();
            let pkt = Packet {
                id,
                data: PacketData::BatteryReport(BatteryReport { percent, charging }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("[battery_loop] channel full, dropping packet");
            }
        }
    }
}

/// Owns the outgoing side: writes queued packets to USB bulk IN, or to the matching L2CAP
/// channel in BLE mode. Packets are dropped when neither transport is active.
async fn stream_loop<const N: usize>(
    pkt_rcv: Receiver<'_, Packet, N>,
    l2cap: &L2capChannels,
    usb_snd: &mut Endpoint<'static, In>,
    usb_configured: UsbConfigHandle,
) {
    defmt::info!("[stream_loop] Started");
    let mut pkt_count: u32 = 0;
    let mut usb_snd_buf = [0u8; 1024];

    loop {
        let pkt = pkt_rcv.receive().await;
        pkt_count = pkt_count.wrapping_add(1);

        if pkt_count % 100 == 0 {
            defmt::debug!("[stream_loop] Processed {} packets", pkt_count);
        }

        let mode = transport_mode::get();
        let usb_active = usb_configured.is_configured() && matches!(mode, TransportMode::Usb);
        let ble_active = matches!(mode, TransportMode::Ble);

        if usb_active {
            if matches!(pkt.data, PacketData::ReadRegisterResponse(_)) {
                defmt::info!(
                    "[stream_loop] Sending ReadRegisterResponse id={=u8}",
                    pkt.id
                );
            }
            match postcard::to_slice(&pkt, &mut usb_snd_buf) {
                Ok(payload) => match write_transfer(usb_snd, payload, true).await {
                    Ok(()) => {
                        if matches!(pkt.data, PacketData::ReadRegisterResponse(_)) {
                            defmt::info!("[stream_loop] ReadRegisterResponse sent ok");
                        }
                    }
                    Err(e) => {
                        defmt::error!(
                            "[stream_loop] USB write error: {:?}",
                            defmt::Debug2Format(&e)
                        );
                    }
                },
                Err(e) => {
                    defmt::error!(
                        "[stream_loop] USB packet serialization failed: {:?}",
                        defmt::Debug2Format(&e)
                    );
                }
            }
        } else if ble_active {
            if is_control_packet(&pkt) {
                if l2cap.control_tx.try_send(pkt).is_err() {
                    defmt::warn!("[stream_loop] Dropped control packet - channel full");
                }
            } else {
                if l2cap.data_tx.try_send(pkt).is_err() {
                    defmt::warn!("[stream_loop] Dropped data packet - channel full");
                }
            }
        } else {
            defmt::debug!("[stream_loop] Dropping packet - no active transport");
        }
    }
}

/// Streams accel/gyro reports and raises impact reports when acceleration crosses the threshold.
async fn imu_loop<I: ImuSensor, II: ImuInterrupt, const N: usize>(
    imu: &mut I,
    imu_int: &mut II,
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
    impact_settings: &ImpactSettings,
) {
    const G: f32 = 9.806_65;

    // ICM-42688 sensitivity at the default full-scale ranges.
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

        // The ICM on lite1 (POC2) is rotated 90° CW relative to legacy atslite (POC1). Legacy maps
        // raw axes as [raw[0], -raw[1], -raw[2]]; with legacy_x = new_y and legacy_y = -new_x
        // that becomes [raw[1], raw[0], -raw[2]].
        let (ax, ay, az) = (acc[1], acc[0], -acc[2]);
        let (gx, gy, gz) = (gyr[1], gyr[0], -gyr[2]);

        // m/s² and rad/s.
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

        if stream_infos.imu.enabled() {
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: PacketData::AccelReport(odyssey_protocol::AccelReport {
                    timestamp: ts_micros,
                    accel,
                    gyro,
                }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("[object_mode] IMU channel full, dropping packet");
            }
        }

        // The threshold is in m/s²; impacts closer together than suppress_ms are dropped.
        if stream_infos.impact.enabled() {
            let accel_magnitude_ms2 = accel.magnitude();
            let threshold_ms2 = impact_settings.threshold() as f32;

            if accel_magnitude_ms2 >= threshold_ms2 {
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
                        data: PacketData::ImpactReport(odyssey_protocol::ImpactReport {
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

/// Streams PAG marker reports while the marker stream is enabled.
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

            // On timeout, loop around and re-check whether the stream is still enabled.
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
                data: PacketData::PocMarkersReport(odyssey_protocol::PocMarkersReport {
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
