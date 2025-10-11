use core::cell::RefCell;
use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::Format;
use embassy_futures::join::join4;
use embassy_futures::select::{Either, select};
use embassy_nrf::Peri;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::ppi::AnyConfigurableChannel;
use embassy_nrf::spim::Error;
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::watch::Watch;
use embassy_usb::driver::{Endpoint as _, EndpointIn as _, EndpointOut as _, EndpointError};
use embedded_hal_bus::spi::DeviceError;
use nalgebra::Vector3;
use paj7025::Paj7025Error;
use protodongers::{Packet, PacketData as P, PacketType, Props, ReadRegisterResponse};

use crate::fodtrigger::FodTrigger;
#[cfg(context = "vm")]
use crate::imu::bmi::{Imu, ImuInterrupt};
#[cfg(context = "atslite1")]
use crate::imu::icm::{Imu, ImuInterrupt};
use crate::settings::{PajsSettings, Settings};
use crate::utils::device_id;
use crate::{CommonContext, Paj, PajGroup};

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;
type ExitReceiver<'a> = embassy_sync::watch::Receiver<'a, NoopRawMutex, (), 4>;

#[cfg(context = "vm")]
#[allow(unreachable_code)]
pub async fn object_mode<'d, const N: usize, T: embassy_nrf::timer::Instance>(
    mut ctx: CommonContext<N>,
    timer: Peri<'d, T>,
) -> Result<CommonContext<N>, Paj7025Error<DeviceError<Error, Infallible>>> {
    defmt::info!("Entering Object Mode");
    let pkt_writer = PacketWriter {
        snd: &mut ctx.usb_snd,
    };
    let mut pkt_rcv = PacketReader::new(&mut ctx.usb_rcv);
    let pkt_channel = Channel::new();
    let stream_infos = StreamInfos::new();
    let exit = Watch::<_, _, 4>::new();
    let exit0 = exit.receiver().unwrap();
    let _exit1 = exit.receiver().unwrap();
    let _exit2 = exit.receiver().unwrap();
    let _exit3 = exit.receiver().unwrap();
    
    let a = usb_rcv_loop(
        ctx.paj7025r2_group.0,
        ctx.paj7025r3_group.0,
        &stream_infos,
        pkt_channel.sender(),
        &mut pkt_rcv,
        ctx.settings,
        ctx.nvmc,
    );
    let b = usb_snd_loop(pkt_writer, pkt_channel.receiver(), exit0);
    let c = imu_loop(&mut ctx.imu, &mut ctx.imu_int, pkt_channel.sender(), &stream_infos);
    let d = obj_loop(
        ctx.paj7025r2_group,
        ctx.paj7025r3_group,
        ctx.fod_set_ch,
        ctx.fod_clr_ch,
        timer,
        pkt_channel.sender(),
        &stream_infos,
    );
    // TODO turn these into tasks
    let r = join4(a, b, c, d).await;

    r.0?
    // ctx.paj = paj.into_inner().as_dynamic_mode();
    // ctx
}

#[cfg(context = "atslite1")]
#[allow(unreachable_code)]
pub async fn object_mode<'d, T: embassy_nrf::timer::Instance>(
    mut ctx: CommonContext,
    timer: Peri<'d, T>,
) -> Result<CommonContext, Paj7025Error<DeviceError<Error, Infallible>>> {
    defmt::info!("Entering Object Mode");
    let pkt_writer = PacketWriter {
        snd: &mut ctx.usb_snd,
    };
    let mut pkt_rcv = PacketReader::new(&mut ctx.usb_rcv);
    let pkt_channel = Channel::new();
    let stream_infos = StreamInfos::new();
    let exit = Watch::<_, _, 4>::new();
    let exit0 = exit.receiver().unwrap();
    let _exit1 = exit.receiver().unwrap();
    let _exit2 = exit.receiver().unwrap();
    let _exit3 = exit.receiver().unwrap();
    
    let a = usb_rcv_loop(
        ctx.paj7025r2_group.0,
        ctx.paj7025r3_group.0,
        &stream_infos,
        pkt_channel.sender(),
        &mut pkt_rcv,
        ctx.settings,
        ctx.nvmc,
    );
    let b = usb_snd_loop(pkt_writer, pkt_channel.receiver(), exit0);
    let c = imu_loop(&mut ctx.imu, &mut ctx.imu_int, pkt_channel.sender(), &stream_infos);
    let d = obj_loop(
        ctx.paj7025r2_group,
        ctx.paj7025r3_group,
        ctx.fod_set_ch,
        ctx.fod_clr_ch,
        timer,
        pkt_channel.sender(),
        &stream_infos,
    );
    // TODO turn these into tasks
    let r = join4(a, b, c, d).await;

    r.0?
    // ctx.paj = paj.into_inner().as_dynamic_mode();
    // ctx
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

async fn usb_rcv_loop(
    paj7025r2: &AsyncMutex<NoopRawMutex, Paj>,
    paj7025r3: &AsyncMutex<NoopRawMutex, Paj>,
    stream_infos: &StreamInfos,
    pkt_snd: Sender<'_, Packet, 4>,
    pkt_rcv: &mut PacketReader<'_>,
    settings: &mut Settings,
    nvmc: &RefCell<Nvmc<'static>>,
) -> Result<!, Paj7025Error<DeviceError<Error, Infallible>>> {
    loop {
        let pkt = match pkt_rcv.wait_for_packet().await {
            Err(WaitForPacketError::Usb(EndpointError::Disabled)) => {
                pkt_rcv.rcv.wait_enabled().await;
                continue;
            }
            Err(e) => {
                defmt::error!("Failed to read packet: {}", e);
                continue;
            }
            Ok(p) => p,
        };

        defmt::debug!("Received packet: {}", defmt::Debug2Format(&pkt));

        let response = match pkt.data {
            P::WriteRegister(register) => {
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
                // Some(Packet {
                //     id: pkt.id,
                //     data: P::Ack(),
                // })
                None
            }
            P::ReadRegister(register) => {
                let paj = match register.port {
                    protodongers::Port::Nf => paj7025r2,
                    protodongers::Port::Wf => paj7025r3,
                };
                let mut paj = paj.lock().await;
                let mut data: [u8; 1] = [0];
                paj.read_register(register.bank, register.address, &mut data).await?;
                Some(Packet {
                    id: pkt.id,
                    data: P::ReadRegisterResponse(ReadRegisterResponse {
                        bank: register.bank,
                        address: register.address,
                        data: data[0],
                    }),
                })
            }
            P::WriteConfig(config) => {
                settings.general.set_general_config(&config.into());
                // Some(Packet {
                //     id: pkt.id,
                //     data: P::Ack(),
                // })
                None
            }
            P::ReadConfig() => Some(Packet {
                id: pkt.id,
                data: P::ReadConfigResponse((*settings.general.general_config()).into()),
            }),
            P::FlashSettings() => {
                settings.pajs =
                    PajsSettings::read_from_pajs(&mut *paj7025r2.lock().await, &mut *paj7025r3.lock().await).await?;
                settings.write(nvmc);
                // Some(Packet {
                //     id: pkt.id,
                //     data: P::Ack(),
                // })
                None
            }
            P::ReadProps() => Some(Packet {
                id: pkt.id,
                data: P::ReadPropsResponse(Props {
                    uuid: device_id()[..6].try_into().unwrap(),
                    product_id: crate::usb::PID,
                }),
            }),
            P::StreamUpdate(s) => {
                use protodongers::StreamUpdateAction as S;
                match s.action {
                    S::Enable => stream_infos.enable(pkt.id, s.packet_id),
                    S::Disable => stream_infos.disable(s.packet_id),
                    S::DisableAll => stream_infos.disable_all(),
                }
                // Some(Packet {
                //     id: pkt.id,
                //     data: P::Ack(),
                // })
                None
            }
            P::WriteMode(mode) => {
                settings.transient.mode = mode;
                settings.transient_write(nvmc);
                // Some(Packet {
                //     id: pkt.id,
                //     data: P::Ack(),
                // })
                None
            }
            P::ReadVersion() => Some(Packet {
                id: pkt.id,
                data: P::ReadVersionResponse(protodongers::Version::new([MAJOR, MINOR, PATCH])),
            }),
            P::ObjectReportRequest() => None,
            P::Ack() => None,
            P::ReadConfigResponse(_) => None,
            P::ReadPropsResponse(_) => None,
            P::ObjectReport(_) => None,
            P::CombinedMarkersReport(_) => None,
            P::PocMarkersReport(_) => None,
            P::AccelReport(_) => None,
            P::ImpactReport(_) => None,
            P::Vendor(_, _) => None,
            P::ReadRegisterResponse(_) => None,
            P::ReadVersionResponse(_) => None,
        };

        if let Some(r) = response
            && r.id != 255
        {
            pkt_snd.send(r).await;
        }
    }
}

async fn usb_snd_loop(
    mut pkt_writer: PacketWriter<'_>,
    pkt_rcv: Receiver<'_, Packet, 4>,
    mut exit: ExitReceiver<'_>,
) -> ! {
    loop {
        let Either::First(pkt) = select(pkt_rcv.receive(), exit.get()).await else {
            break;
        };
        defmt::debug!("Sending packet: {}", defmt::Debug2Format(&pkt));
        let _ = pkt_writer.write_packet(&pkt).await;
    }
    todo!()
}

#[cfg(context = "vm")]
async fn imu_loop<const N: usize>(
    imu: &mut Imu<N>,
    imu_int: &mut ImuInterrupt,
    pkt_snd: Sender<'_, Packet, 4>,
    stream_infos: &StreamInfos,
) {
    // ---- Sensor time: 24-bit free-running counter, 39.0625 µs per tick ----
    const TS_BITS: u32 = 24;
    const TS_MASK: u32 = (1 << TS_BITS) - 1; // 0x00FF_FFFF
    const TICK_US_NUM: u32 = 625;
    const TICK_US_DEN_SHIFT: u32 = 4;

    // ---- Unit scales ----
    // Accelerometer full-scale: 16 g -> 2048 LSB/g
    const ACC_LSB_PER_G: f32 = 2048.0;
    const G: f32 = 9.806_65;

    // Gyro full-scale: 250 dps -> 16.384 LSB/(deg/s)
    const GYRO_LSB_PER_DPS: f32 = 65.536;
    const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

    let mut last_ts_raw: Option<u32> = None; // last 24-bit sensor-time value
    let mut ts_micros: u32 = 0; // accumulated time in µs (wraps ~71.6 min)

    loop {
        imu_int.wait().await;
        let pkt = imu.get_data().await.unwrap();

        let ts_raw = pkt.time & TS_MASK; // keep only 24 bits
        let dt_ticks_u32 = match last_ts_raw {
            Some(prev) => (ts_raw.wrapping_sub(prev)) & TS_MASK,
            None => 0,
        };
        last_ts_raw = Some(ts_raw);

        // ticks → µs: dt * (625/16)  (use u64 for the mul, then cast back)
        let dt_us = (((dt_ticks_u32 as u64) * (TICK_US_NUM as u64)) >> TICK_US_DEN_SHIFT) as u32;
        ts_micros = ts_micros.wrapping_add(dt_us);

        // --- convert to SI ---
        let accel = Vector3::new(-pkt.acc.x, -pkt.acc.y, pkt.acc.z).cast::<f32>() / ACC_LSB_PER_G * G;

        let gx = -(pkt.gyr.x as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gy = -(pkt.gyr.y as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gz = (pkt.gyr.z as f32 / GYRO_LSB_PER_DPS) * DEG_TO_RAD;

        if stream_infos.imu.enabled() {
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: P::AccelReport(protodongers::AccelReport {
                    timestamp: ts_micros, // µs since loop start (u32)
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

#[cfg(context = "atslite1")]
async fn imu_loop(
    imu: &mut Imu,
    imu_int: &mut ImuInterrupt,
    pkt_snd: Sender<'_, Packet, 4>,
    stream_infos: &StreamInfos,
) {
    static BUFFER: static_cell::ConstStaticCell<[u32; 521]> = static_cell::ConstStaticCell::new([0; 521]);
    let buffer = BUFFER.take();
    let mut ts_micros = 0;
    // TODO handle ODR change and impact stream
    loop {
        imu_int.wait().await;
        let _items = imu.read_fifo(buffer).await.unwrap();
        let pkt = bytemuck::from_bytes::<icm426xx::fifo::FifoPacket4>(&bytemuck::cast_slice(buffer)[4..24]);
        // CLKIN ticks (32 kHz)
        ts_micros += 1000 * (pkt.timestamp() as u32) / 32;
        // TODO make the driver handle scale
        // 32768 LSB/g
        let ax = pkt.accel_data_x();
        let ay = pkt.accel_data_y();
        let az = pkt.accel_data_z();
        // 262.144 LSB/(º/s)
        let gx = pkt.gyro_data_x();
        let gy = pkt.gyro_data_y();
        let gz = pkt.gyro_data_z();

        let (ax, ay, az) = (-ay, ax, az);
        let (gx, gy, gz) = (-gy, gx, gz);

        if stream_infos.imu.enabled() {
            // defmt::info!("IMU {=usize} fifo items, {}", items, pkt.fifo_header());
            // defmt::info!("ax={} ay={} az={} gx={} gy={} gz={}", ax, ay, az, gx, gy, gz);
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: P::AccelReport(protodongers::AccelReport {
                    timestamp: ts_micros,
                    accel: Vector3::new(ax, ay, az).cast() / 32768.0 * 9.806650,
                    gyro: Vector3::new(
                        (gx as f32 / 262.144).to_radians(),
                        (gy as f32 / 262.144).to_radians(),
                        (gz as f32 / 262.144).to_radians(),
                    ),
                }),
            };
            let r = pkt_snd.try_send(pkt);
            if r.is_err() {
                defmt::warn!("Failed to send IMU data due to full channel");
            }
        }
    }
}

async fn obj_loop<'d, T: embassy_nrf::timer::Instance>(
    r2_group: PajGroup<'_>,
    r3_group: PajGroup<'_>,
    ppi_set: Peri<'d, AnyConfigurableChannel>,
    ppi_clr: Peri<'d, AnyConfigurableChannel>,
    timer: Peri<'d, T>,
    pkt_snd: Sender<'_, Packet, 4>,
    stream_infos: &StreamInfos,
) -> Result<!, Paj7025Error<DeviceError<Error, Infallible>>> {
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

    // let mut r2_fod = embassy_nrf::gpio::Output::new(r2_fod, embassy_nrf::gpio::Level::Low, embassy_nrf::gpio::OutputDrive::Standard);
    // let mut r3_fod = embassy_nrf::gpio::Output::new(r3_fod, embassy_nrf::gpio::Level::Low, embassy_nrf::gpio::OutputDrive::Standard);

    loop {
        if stream_infos.marker.enabled() {
            // defmt::info!("Waiting tick");
            trigger.wait_tick().await;
            // defmt::info!("Tick waited");

            // r2_fod.set_high();
            // r3_fod.set_high();
            // embassy_time::Timer::after_micros(1).await;
            // r2_fod.set_low();
            // r3_fod.set_low();

            // embassy_time::Timer::after_millis(5).await;

            let mut objs = [[paj7025::types::ObjectFormat1::DEFAULT; 16]; 2];

            // TODO figure out why u2048x doesn't work or get rid of it
            // objs[0] = paj7025::parse_bank5(r2.lock().await.ll.output().bank_5().read_async().await?.value().to_le_bytes());
            // objs[1] = paj7025::parse_bank5(r3.lock().await.ll.output().bank_5().read_async().await?.value().to_le_bytes());
            let mut bytes: [u8; 256] = [0; 256];
            r2.lock().await.read_register(5, 0, &mut bytes).await?;
            objs[0] = paj7025::parse_bank5(bytes);

            r3.lock().await.read_register(5, 0, &mut bytes).await?;
            objs[1] = paj7025::parse_bank5(bytes);

            let id = stream_infos.marker.req_id();
            let pkt = Packet {
                id,
                data: P::CombinedMarkersReport(protodongers::CombinedMarkersReport {
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
        Self {
            rcv,
        }
    }

    pub async fn wait_for_packet(&mut self) -> Result<Packet, WaitForPacketError> {
        let mut data = [0; 1024];
        defmt::info!("Read transfer start");
        let n = self.rcv.read_transfer(&mut data).await?;
        defmt::info!("Read transfer done");
        Ok(postcard::from_bytes(&data[..n])?)
    }
}

pub struct PacketWriter<'a> {
    snd: &'a mut Endpoint<'static, In>,
}

impl PacketWriter<'_> {
    pub async fn write_packet(&mut self, pkt: &Packet) -> Result<(), WaitForPacketError> {
        let payload = postcard::to_vec::<_, 1024>(pkt)?;
        self.snd.write_transfer(&payload, true).await?;
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
        self.enable.load(Ordering::Relaxed)
    }

    fn req_id(&self) -> u8 {
        self.req_id.load(Ordering::Relaxed)
    }

    fn set_req_id(&self, req_id: u8) {
        self.req_id.store(req_id, Ordering::Relaxed);
    }

    fn set_enable(&self, enable: bool) {
        self.enable.store(enable, Ordering::Relaxed);
    }
}
