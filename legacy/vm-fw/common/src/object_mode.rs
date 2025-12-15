use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::Format;
use embassy_futures::join::join4;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_nrf::ppi::AnyConfigurableChannel;
use embassy_nrf::spim::Error;
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::watch::Watch;
use embassy_usb::driver::EndpointError;
use embedded_hal_bus::spi::DeviceError;
use nalgebra::Vector3;
use paj7025::Paj7025Error;

use crate::fodtrigger::FodTrigger;
use crate::platform::{Imu, ImuInterrupt, L2capChannels, L2capReceiver, L2capSender, Platform, is_control_packet};
use crate::protodongers::{
    self, ConfigKind, Packet, PacketData, PacketType, PropKind, Props, ReadRegisterResponse, wire,
};
use crate::settings::{PajsSettings, Settings};
use crate::usb::{self, UsbConfigHandle};
use crate::utils::impl_from;
use crate::{Paj, PajGroup};

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;
type ExitReceiver<'a> = embassy_sync::watch::Receiver<'a, NoopRawMutex, (), 4>;

/// Context structure for object_mode - platform-agnostic
pub struct ObjectModeContext<'d, P: Platform, C1: embassy_nrf::gpiote::Channel, C2: embassy_nrf::gpiote::Channel> {
    pub paj7025r2_group: PajGroup<'d, C1>,
    pub paj7025r3_group: PajGroup<'d, C2>,
    pub fod_set_ch: embassy_nrf::Peri<'d, AnyConfigurableChannel>,
    pub fod_clr_ch: embassy_nrf::Peri<'d, AnyConfigurableChannel>,
    pub timer: embassy_nrf::Peri<'d, P::Timer>,
    pub imu: P::Imu,
    pub imu_int: P::ImuInterrupt,
    pub usb_snd: Endpoint<'static, In>,
    pub usb_rcv: Endpoint<'static, Out>,
    pub usb_configured: UsbConfigHandle,
    pub settings: &'d mut Settings,
    pub l2cap_channels: P::L2capChannels,
}

#[allow(unreachable_code)]
pub async fn object_mode<'d, P: Platform, C1: embassy_nrf::gpiote::Channel, C2: embassy_nrf::gpiote::Channel>(
    pid: u16,
    mut ctx: ObjectModeContext<'d, P, C1, C2>,
) -> Result<ObjectModeContext<'d, P, C1, C2>, Paj7025Error<DeviceError<Error, Infallible>>> {
    defmt::info!("Entering Object Mode on {}", P::NAME);
    let pkt_writer = PacketWriter { snd: &mut ctx.usb_snd };
    let mut pkt_rcv = PacketReader::new(&mut ctx.usb_rcv);
    let pkt_channel = Channel::<Packet, 64>::new();
    let stream_infos = StreamInfos::new();
    let exit = Watch::<_, _, 4>::new();
    let exit0 = exit.receiver().unwrap();
    let _exit1 = exit.receiver().unwrap();
    let _exit2 = exit.receiver().unwrap();
    let _exit3 = exit.receiver().unwrap();

    let a = usb_rcv_loop(
        pid,
        ctx.paj7025r2_group.0,
        ctx.paj7025r3_group.0,
        &stream_infos,
        pkt_channel.sender(),
        &mut pkt_rcv,
        ctx.usb_configured,
        ctx.settings,
        &ctx.l2cap_channels,
        P::device_id(),
    );
    let b = usb_snd_loop(
        pkt_writer,
        pkt_channel.receiver(),
        exit0,
        ctx.usb_configured,
        &ctx.l2cap_channels,
    );
    let c = imu_loop::<P>(&mut ctx.imu, &mut ctx.imu_int, pkt_channel.sender(), &stream_infos);
    let d = obj_loop(
        ctx.paj7025r2_group,
        ctx.paj7025r3_group,
        ctx.fod_set_ch,
        ctx.fod_clr_ch,
        ctx.timer,
        pkt_channel.sender(),
        &stream_infos,
    );
    // TODO turn these into tasks
    let r = join4(a, b, c, d).await;

    // All tasks return Result<Infallible, Error>, meaning they never succeed
    // If any task returns, it's because of an error
    match r.0 {
        Ok(_) => unreachable!("usb_rcv_loop should never return Ok"),
        Err(e) => Err(e),
    }
}

const MAJOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MAJOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MAJOR"),
};
const MINOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MINOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MINOR"),
};
const PATCH: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_PATCH"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_PATCH"),
};

#[derive(Debug, Clone, Copy)]
enum PacketSource {
    Usb,
    Ble,
}

async fn usb_rcv_loop<L: L2capChannels>(
    pid: u16,
    paj7025r2: &AsyncMutex<NoopRawMutex, Paj>,
    paj7025r3: &AsyncMutex<NoopRawMutex, Paj>,
    stream_infos: &StreamInfos,
    pkt_snd: Sender<'_, Packet, 64>,
    pkt_rcv: &mut PacketReader<'_>,
    usb_configured: UsbConfigHandle,
    settings: &mut Settings,
    l2cap_channels: &L,
    device_id: [u8; 8],
) -> Result<Infallible, Paj7025Error<DeviceError<Error, Infallible>>> {
    loop {
        // USB takes priority - when USB is configured, ignore BLE completely
        // When USB is not configured, only listen to BLE
        let (src, pkt) = match select3(
            async {
                loop {
                    match pkt_rcv.wait_for_packet().await {
                        Ok(pkt) => break pkt,
                        Err(WaitForPacketError::Usb(EndpointError::Disabled)) => {
                            defmt::info!("USB not configured; waiting for host connection...");
                            usb_configured.wait_until_configured().await;
                            defmt::info!("USB configured; resuming packet handling");
                            continue;
                        }
                        Err(e) => {
                            defmt::error!("Failed to read USB packet: {}", e);
                            embassy_time::Timer::after_millis(100).await;
                        }
                    }
                }
            },
            async {
                // Only listen to BLE control when USB is not configured
                if usb_configured.is_configured() {
                    core::future::pending().await
                } else {
                    l2cap_channels.control_rx().receive().await
                }
            },
            async {
                // Only listen to BLE data when USB is not configured
                if usb_configured.is_configured() {
                    core::future::pending().await
                } else {
                    l2cap_channels.data_rx().receive().await
                }
            },
        )
        .await
        {
            Either3::First(pkt) => (PacketSource::Usb, pkt),
            Either3::Second(pkt) => (PacketSource::Ble, pkt),
            Either3::Third(pkt) => (PacketSource::Ble, pkt),
        };

        match src {
            PacketSource::Usb => defmt::info!("[app] src=USB"),
            PacketSource::Ble => defmt::info!("[app] src=BLE"),
        }

        defmt::info!(
            "[app] Processing packet with id={}, data={:?}",
            pkt.id,
            defmt::Debug2Format(&pkt.data)
        );

        let response = match pkt.data {
            PacketData::WriteRegister(register) => {
                let paj = match register.port {
                    protodongers::Port::Nf => paj7025r2,
                    protodongers::Port::Wf => paj7025r3,
                };
                let mut paj = paj.lock().await;
                paj.write_register(register.bank, register.address, &[register.data])
                    .await?;
                paj.ll
                    .control()
                    .bank_0()
                    .bank_0_sync_updated_flag()
                    .write_async(|x| x.set_value(1))
                    .await?;
                paj.ll
                    .control()
                    .bank_1()
                    .bank_1_sync_updated_flag()
                    .write_async(|x| x.set_value(1))
                    .await?;
                None
            }
            PacketData::ReadRegister(register) => {
                defmt::debug!("[app] ReadRegister: acquiring paj lock");
                let paj = match register.port {
                    protodongers::Port::Nf => paj7025r2,
                    protodongers::Port::Wf => paj7025r3,
                };
                let mut paj = paj.lock().await;
                defmt::debug!(
                    "[app] ReadRegister: lock acquired, reading bank={}, addr={}",
                    register.bank,
                    register.address
                );
                let mut data: [u8; 1] = [0];
                paj.read_register(register.bank, register.address, &mut data).await?;
                defmt::debug!("[app] ReadRegister: read complete, data={}", data[0]);
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadRegisterResponse(ReadRegisterResponse {
                        bank: register.bank,
                        address: register.address,
                        data: data[0],
                    }),
                })
            }
            PacketData::WriteConfig(config) => {
                let wire_config: wire::GeneralConfig = config.into();
                settings.general.set_general_config(&wire_config);
                None
            }
            PacketData::ReadConfig(kind) => {
                defmt::info!("[app] building ReadConfig response for {:?}", kind);
                let wire_config = match kind {
                    ConfigKind::ImpactThreshold => {
                        wire::GeneralConfig::ImpactThreshold(settings.general.impact_threshold)
                    }
                    ConfigKind::SuppressMs => wire::GeneralConfig::SuppressMs(settings.general.suppress_ms),
                    ConfigKind::AccelConfig => wire::GeneralConfig::AccelConfig(settings.general.accel_config.into()),
                    ConfigKind::GyroConfig => wire::GeneralConfig::GyroConfig(settings.general.gyro_config.clone()),
                    ConfigKind::CameraModelNf => {
                        wire::GeneralConfig::CameraModelNf(settings.general.camera_model_nf.clone())
                    }
                    ConfigKind::CameraModelWf => {
                        wire::GeneralConfig::CameraModelWf(settings.general.camera_model_wf.clone())
                    }
                    ConfigKind::StereoIso => wire::GeneralConfig::StereoIso(settings.general.stereo_iso.clone()),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadConfigResponse(wire_config.into()),
                })
            }
            PacketData::FlashSettings() => {
                settings.pajs =
                    PajsSettings::read_from_pajs(&mut *paj7025r2.lock().await, &mut *paj7025r3.lock().await).await?;
                settings.write();
                None
            }
            PacketData::ReadProp(kind) => {
                let prop = match kind {
                    PropKind::Uuid => Props::Uuid(device_id[..6].try_into().unwrap()),
                    PropKind::ProductId => Props::ProductId(pid),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadPropResponse(prop),
                })
            }
            PacketData::StreamUpdate(s) => {
                use protodongers::StreamUpdateAction as S;
                match s.action {
                    S::Enable => stream_infos.enable(pkt.id, s.packet_id),
                    S::Disable => stream_infos.disable(s.packet_id),
                    S::DisableAll => stream_infos.disable_all(),
                }
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }
            PacketData::WriteMode(mode) => {
                settings.transient.mode = mode;
                settings.transient_write();
                None
            }
            PacketData::ReadVersion() => Some(Packet {
                id: pkt.id,
                data: PacketData::ReadVersionResponse(protodongers::Version::new([MAJOR, MINOR, PATCH])),
            }),
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

        if let Some(r) = response
            && r.id != 255
        {
            match src {
                PacketSource::Usb => {
                    defmt::debug!(
                        "[app] Sending response via USB: id={}, data={:?}",
                        r.id,
                        defmt::Debug2Format(&r.data)
                    );
                    pkt_snd.send(r).await;
                    defmt::debug!("[app] Response sent via USB");
                }
                PacketSource::Ble => {
                    // Route to correct L2CAP channel - each has dedicated L2CAP connection
                    // Use try_send to avoid blocking if channel is full (BLE task not draining)
                    if is_control_packet(&r) {
                        if l2cap_channels.control_tx().try_send(r).is_err() {
                            defmt::warn!("[app] Dropped control response - channel full");
                        } else {
                            defmt::debug!("[app] Control response queued for BLE");
                        }
                    } else {
                        if l2cap_channels.data_tx().try_send(r).is_err() {
                            defmt::warn!("[app] Dropped data response - channel full");
                        } else {
                            defmt::debug!("[app] Data response queued for BLE");
                        }
                    }
                }
            }
        }
        defmt::debug!("[app] Packet processing complete, looping...");
    }
}

async fn usb_snd_loop<L: L2capChannels>(
    mut pkt_writer: PacketWriter<'_>,
    pkt_rcv: Receiver<'_, Packet, 64>,
    mut exit: ExitReceiver<'_>,
    usb_configured: UsbConfigHandle,
    l2cap_channels: &L,
) -> ! {
    loop {
        let Either::First(pkt) = select(pkt_rcv.receive(), exit.get()).await else {
            break;
        };
        defmt::trace!(
            "[usb_snd_loop] Broadcasting streaming packet: {}",
            defmt::Debug2Format(&pkt)
        );

        // USB takes priority - if USB is connected, only send to USB
        if usb_configured.is_configured() {
            let _ = pkt_writer.write_packet(&pkt).await;
        } else {
            // USB not connected, send to BLE only
            if is_control_packet(&pkt) {
                let _ = l2cap_channels.control_tx().try_send(pkt);
            } else {
                let _ = l2cap_channels.data_tx().try_send(pkt);
            }
        }
    }
    todo!()
}

async fn imu_loop<P: Platform>(
    imu: &mut P::Imu,
    imu_int: &mut P::ImuInterrupt,
    pkt_snd: Sender<'_, Packet, 64>,
    stream_infos: &StreamInfos,
) {
    // ---- Sensor time: simple counter for timestamp ----
    // Platform-specific timing can be added via trait methods if needed
    let mut ts_micros: u32 = 0;

    // ---- Unit scales (default for BMI270, platforms may differ) ----
    // Accelerometer full-scale: 16 g -> 2048 LSB/g
    const ACC_LSB_PER_G: f32 = 2048.0;
    const G: f32 = 9.806_65;

    // Gyro full-scale: 250 dps -> 65.536 LSB/(deg/s)
    const GYRO_LSB_PER_DPS: f32 = 65.536;
    const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

    loop {
        imu_int.wait().await;

        // Read accelerometer and gyroscope data from the platform-specific IMU
        let acc = match imu.read_accel().await {
            Ok(data) => data,
            Err(e) => {
                defmt::error!("Failed to read accelerometer: {}", defmt::Debug2Format(&e));
                continue;
            }
        };

        let gyr = match imu.read_gyro().await {
            Ok(data) => data,
            Err(e) => {
                defmt::error!("Failed to read gyroscope: {}", defmt::Debug2Format(&e));
                continue;
            }
        };

        // Simple timestamp increment (~100Hz sampling = 10ms per sample)
        // TODO: Platform-specific timestamp handling via trait methods
        ts_micros = ts_micros.wrapping_add(10_000);

        // Convert to SI units (using BMI270 scaling - platforms may override)
        let accel = Vector3::new(-acc[0], -acc[1], acc[2]).cast::<f32>() / ACC_LSB_PER_G * G;

        let gx = -(gyr[0] as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gy = -(gyr[1] as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gz = (gyr[2] as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;

        if stream_infos.imu.enabled() {
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: PacketData::AccelReport(protodongers::AccelReport {
                    timestamp: ts_micros,
                    accel,
                    gyro: Vector3::new(gx, gy, gz),
                }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("Failed to send IMU data due to full channel");
            }
        }
    }
}

async fn obj_loop<
    T: embassy_nrf::timer::Instance,
    C1: embassy_nrf::gpiote::Channel,
    C2: embassy_nrf::gpiote::Channel,
>(
    r2_group: PajGroup<'_, C1>,
    r3_group: PajGroup<'_, C2>,
    ppi_set: embassy_nrf::Peri<'_, AnyConfigurableChannel>,
    ppi_clr: embassy_nrf::Peri<'_, AnyConfigurableChannel>,
    timer: embassy_nrf::Peri<'_, T>,
    pkt_snd: Sender<'_, Packet, 64>,
    stream_infos: &StreamInfos,
) -> Result<Infallible, Paj7025Error<DeviceError<Error, Infallible>>> {
    let (r2, r2_fod, nf_ch) = r2_group;
    let (r3, r3_fod, wf_ch) = r3_group;

    let mut trigger = FodTrigger::new(
        r2_fod,
        r3_fod,
        nf_ch,
        wf_ch,
        timer,
        ppi_set,
        ppi_clr,
        embassy_nrf::timer::Frequency::F1MHz,
        5,
        2,
    );

    // TODO freezes everything when used with raw usb driver
    trigger.start();

    loop {
        if stream_infos.marker.enabled() {
            trigger.wait_tick().await;

            let mut objs = [[paj7025::types::ObjectFormat1::DEFAULT; 16]; 2];

            let mut bytes: [u8; 256] = [0; 256];
            r2.lock().await.read_register(5, 0, &mut bytes).await?;
            objs[0] = paj7025::parse_bank5(bytes);

            r3.lock().await.read_register(5, 0, &mut bytes).await?;
            objs[1] = paj7025::parse_bank5(bytes);

            let id = stream_infos.marker.req_id();
            let pkt = Packet {
                id,
                data: PacketData::CombinedMarkersReport(protodongers::CombinedMarkersReport {
                    nf_points: objs[0].map(|o| {
                        if o.cx().value() == 4095 && o.cy().value() == 4095 {
                            nalgebra::Point2::origin()
                        } else {
                            nalgebra::Point2::new(o.cx().value(), o.cy().value())
                        }
                    }),
                    wf_points: objs[1].map(|o| {
                        if o.cx().value() == 4095 && o.cy().value() == 4095 {
                            nalgebra::Point2::origin()
                        } else {
                            nalgebra::Point2::new(4095 - o.cx().value(), 4095 - o.cy().value())
                        }
                    }),
                }),
            };
            let r = pkt_snd.try_send(pkt);
            if r.is_err() {
                defmt::warn!("Failed to send marker data due to full channel");
            }
        } else {
            embassy_time::Timer::after_millis(50).await;
        }
    }
}

pub struct PacketReader<'a> {
    rcv: &'a mut Endpoint<'static, Out>,
}

impl<'a> PacketReader<'a> {
    pub fn new(rcv: &'a mut Endpoint<'static, Out>) -> Self {
        Self { rcv }
    }

    pub async fn wait_for_packet(&mut self) -> Result<Packet, WaitForPacketError> {
        let mut data = [0; 1024];
        defmt::debug!("Read transfer start");
        let n = usb::read_transfer(&mut self.rcv, &mut data).await?;
        defmt::debug!("Read transfer done");
        Ok(postcard::from_bytes(&data[..n])?)
    }
}

pub struct PacketWriter<'a> {
    snd: &'a mut Endpoint<'static, In>,
}

impl PacketWriter<'_> {
    pub async fn write_packet(&mut self, pkt: &Packet) -> Result<(), WaitForPacketError> {
        let payload = postcard::to_vec::<_, 1024>(pkt)?;
        defmt::debug!("[USB] write_packet: payload_len={}, pkt_id={}", payload.len(), pkt.id);
        usb::write_transfer(self.snd, &payload, true).await?;
        Ok(())
    }
}

#[derive(Format)]
pub enum WaitForPacketError {
    Postcard(postcard::Error),
    Packet(protodongers::Error),
    Usb(EndpointError),
}

impl_from!(WaitForPacketError::Postcard(postcard::Error));
impl_from!(WaitForPacketError::Packet(protodongers::Error));
impl_from!(WaitForPacketError::Usb(EndpointError));

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
        defmt::info!("Enable {}, id = {=u8}", ty, req_id);
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::CombinedMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_req_id(req_id);
        si.set_enable(true);
    }

    fn disable(&self, ty: PacketType) {
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::CombinedMarkersReport() => &self.marker,
            _ => return,
        };
        si.set_enable(false);
    }
}

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
