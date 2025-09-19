use core::{
    cell::RefCell, convert::Infallible, mem::{MaybeUninit, transmute}, sync::atomic::{AtomicBool, AtomicU8, Ordering}
};

use defmt::Format;
use embassy_futures::{
    join::join4,
    select::{Either, select},
};
use embassy_nrf::{Peri, gpio::{self, AnyPin}, nvmc::Nvmc, peripherals, spim::{self, Error}};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex as AsyncMutex, watch::Watch};
use embassy_usb::{class::cdc_acm, driver::EndpointError};
use embedded_hal_bus::spi::DeviceError;
use paj7025::Paj7025Error;
use protodongers::{
    Packet, PacketData as P, PacketType, Props, ReadRegisterResponse,
};
use static_cell::ConstStaticCell;

use crate::{
    CommonContext, Paj, UsbDriver,
    imu::{Imu, ImuInterrupt},
    settings::{PajsSettings, Settings},
    usb::write_serial, utils::device_id,
};

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;
type ExitReceiver<'a> = embassy_sync::watch::Receiver<'a, NoopRawMutex, (), 4>;

type PajGroup<'a> = (&'a AsyncMutex<NoopRawMutex, Paj>, gpio::Output<'a>);

#[allow(unreachable_code)]
pub async fn object_mode<const N: usize>(mut ctx: CommonContext<N>) -> Result<CommonContext<N>, Paj7025Error<DeviceError<Error, Infallible>>> {
    defmt::info!("Entering Object Mode");
    let paj7025r2 = AsyncMutex::<NoopRawMutex, _>::new(ctx.paj7025r2);
    let paj7025r3 = AsyncMutex::<NoopRawMutex, _>::new(ctx.paj7025r3);
    let pkt_writer = PacketWriter {
        snd: &mut ctx.usb_snd,
        packet_serialize_buffer: ctx.obj.packet_serialize_buffer,
        packet_cobs_encode_buffer: ctx.obj.packet_cobs_encode_buffer,
    };
    let mut pkt_rcv = PacketReader::new(&mut ctx.usb_rcv, ctx.obj.packet_receive_buffer);
    let pkt_channel = Channel::new();
    let stream_infos = StreamInfos::new();
    let exit = Watch::<_, _, 4>::new();
    let exit0 = exit.receiver().unwrap();
    let _exit1 = exit.receiver().unwrap();
    let _exit2 = exit.receiver().unwrap();
    let _exit3 = exit.receiver().unwrap();

    let a = usb_rcv_loop(
        &paj7025r2,
        &paj7025r3,
        &stream_infos,
        pkt_channel.sender(),
        &mut pkt_rcv,
        ctx.settings,
        &ctx.nvmc,
    );
    let b = usb_snd_loop(pkt_writer, pkt_channel.receiver(), exit0);
    let c = imu_loop(
        &mut ctx.imu,
        &mut ctx.imu_int,
        pkt_channel.sender(),
        &stream_infos,
    );
    let d = obj_loop((&paj7025r2, ctx.paj7025r2_fod), (&paj7025r3, ctx.paj7025r3_fod), pkt_channel.sender(), &stream_infos);
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
                pkt_rcv.rcv.wait_connection().await;
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
                paj.write_register(register.bank, register.address, &[register.data]).await?;
                paj.ll.control().bank_0().bank_0_sync_updated_flag().write_async(|x| x.set_value(1)).await?;
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
                let data: u8 = 0;
                paj.read_register(register.bank, register.address, &mut [data]);
                Some(Packet {
                    id: pkt.id,
                    data: P::ReadRegisterResponse(ReadRegisterResponse {
                        bank: register.bank,
                        address: register.address,
                        data,
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
                settings.pajs = PajsSettings::read_from_pajs(&mut *paj7025r2.lock().await, &mut *paj7025r3.lock().await).await?;
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

        if let Some(r) = response {
            if r.id != 255 {
                pkt_snd.send(r).await;
            }
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

async fn imu_loop<const N: usize>(
    imu: &mut Imu<N>,
    imu_int: &mut ImuInterrupt,
    pkt_snd: Sender<'_, Packet, 4>,
    stream_infos: &StreamInfos,
) {
    static BUFFER: ConstStaticCell<[u32; 521]> = ConstStaticCell::new([0; 521]);
    let buffer = BUFFER.take();
    let mut ts_micros = 0;
    // TODO handle ODR change and impact stream
    loop {
        imu_int.wait().await;
        // let _items = imu.read_fifo(buffer).await.unwrap();
        // let pkt = from_bytes::<FifoPacket4>(&cast_slice(buffer)[4..24]);
        // CLKIN ticks (32 kHz)
        // ts_micros += 1000 * (pkt.timestamp() as u32) / 32;
        // TODO make the driver handle scale
        // 32768 LSB/g
        // let ax = pkt.accel_data_x();
        // let ay = pkt.accel_data_y();
        // let az = pkt.accel_data_z();
        // // 262.144 LSB/(º/s)
        // let gx = pkt.gyro_data_x();
        // let gy = pkt.gyro_data_y();
        // let gz = pkt.gyro_data_z();

        // let (ax, ay, az) = (-ay, ax, az);
        // let (gx, gy, gz) = (-gy, gx, gz);

        // if stream_infos.imu.enabled() {
        //     // defmt::info!("IMU {=usize} fifo items, {}", items, pkt.fifo_header());
        //     // defmt::info!("ax={} ay={} az={} gx={} gy={} gz={}", ax, ay, az, gx, gy, gz);
        //     let id = stream_infos.imu.req_id();
        //     let pkt = Packet {
        //         id,
        //         data: P::AccelReport(protodongers::AccelReport {
        //             timestamp: ts_micros,
        //             accel: Vector3::new(ax, ay, az).cast() / 32768.0 * 9.806650,
        //             gyro: Vector3::new(
        //                 (gx as f32 / 262.144).to_radians(),
        //                 (gy as f32 / 262.144).to_radians(),
        //                 (gz as f32 / 262.144).to_radians(),
        //             ),
        //         }),
        //     };
        //     let r = pkt_snd.try_send(pkt);
        //     if r.is_err() {
        //         defmt::warn!("Failed to send IMU data due to full channel");
        //     }
        // }
    }
}

async fn obj_loop(
    r2_group: PajGroup<'_>,
    r3_group: PajGroup<'_>,
    pkt_snd: Sender<'_, Packet, 4>,
    stream_infos: &StreamInfos,
) -> Result<!, Paj7025Error<DeviceError<Error, Infallible>>> {
    let (r2, mut r2_fod) = r2_group;
    let (r3, mut r3_fod) = r3_group;
    loop {
        if stream_infos.marker.enabled() {
            let mut objs = [[paj7025::types::ObjectFormat1::default(); 16]; 2];

            embassy_time::Timer::after_millis(5).await;
            r2_fod.set_high();
            r3_fod.set_high();

            objs[0] = paj7025::parse_bank5(r2.lock().await.ll.output().bank_5().read_async().await?.value().to_le_bytes());
            objs[1] = paj7025::parse_bank5(r3.lock().await.ll.output().bank_5().read_async().await?.value().to_le_bytes());

            let id = stream_infos.marker.req_id();
            let pkt = Packet {
                id,
                data: P::CombinedMarkersReport(protodongers::CombinedMarkersReport {
                    nf_points: objs[0].map(|o| nalgebra::Point2::new(o.cx, o.cy)),
                    wf_points: objs[1].map(|o| nalgebra::Point2::new(o.cx, o.cy)),
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

/// Singleton to hold static resources.
pub struct Context {
    pub packet_receive_buffer: &'static mut [u8; 1024],
    pub packet_serialize_buffer: &'static mut [u8; 1024],
    pub packet_cobs_encode_buffer: &'static mut [u8; 2049],
}

impl Context {
    /// Can only be called once
    pub fn take() -> Self {
        Self {
            packet_receive_buffer: static_byte_buffer!(1024),
            packet_serialize_buffer: static_byte_buffer!(1024),
            packet_cobs_encode_buffer: static_byte_buffer!(2049),
        }
    }
}

pub struct PacketReader<'a> {
    rcv: &'a mut cdc_acm::Receiver<'static, UsbDriver>,
    buf: &'a mut [u8; 1024],
    n: usize,
    start: usize,
}

impl<'a> PacketReader<'a> {
    /// Keep the same constructor signature
    pub fn new(
        rcv: &'a mut cdc_acm::Receiver<'static, UsbDriver>,
        buf: &'a mut [u8; 1024],
    ) -> Self {
        Self { rcv, buf, n: 0, start: 0 }
    }

    /// Ensure at least `need` bytes are buffered contiguously at buf[start .. start+n]
    async fn fill_min(&mut self, need: usize) -> Result<(), WaitForPacketError> {
        while self.n < need {
            if self.start + self.n == self.buf.len() {
                if self.n == 0 {
                    self.start = 0;
                } else {
                    // compact once when we run out of tail space
                    self.buf.copy_within(self.start .. self.start + self.n, 0);
                    self.start = 0;
                }
            }
            let wrote = self
                .rcv
                .read_packet(&mut self.buf[self.start + self.n ..])
                .await?;
            self.n += wrote;
        }
        Ok(())
    }

    /// Read a single COBS-framed packet: COBS(payload) followed by a single 0x00 delimiter.
    pub async fn wait_for_packet(&mut self) -> Result<Packet, WaitForPacketError> {
        loop {
            // ensure at least one byte so we can search for the delimiter
            self.fill_min(1).await?;

            // find the 0x00 delimiter in the current window
            let mut zero_pos = None;
            for i in 0..self.n {
                if self.buf[self.start + i] == 0x00 {
                    zero_pos = Some(i);
                    break;
                }
            }

            if let Some(frame_len) = zero_pos {
                // drop empty frames (possible if a stray delimiter arrives)
                if frame_len == 0 {
                    self.start += 1;
                    self.n -= 1;
                    if self.n == 0 { self.start = 0; }
                    continue;
                }

                // decode COBS in-place within the window
                let window = &mut self.buf[self.start .. self.start + frame_len];
                let decoded_len = cobs::decode_in_place(window).map_err(WaitForPacketError::Cobs)?;

                // parse the decoded payload
                let mut slice = &window[..decoded_len];
                let pkt = Packet::parse(&mut slice)?;

                // consume this frame + its trailing 0x00 delimiter
                self.start += frame_len + 1;
                self.n -= frame_len + 1;
                if self.n == 0 { self.start = 0; }

                return Ok(pkt);
            }

            // no delimiter found yet; pull at least one more byte
            self.fill_min(self.n + 1).await?;
        }
    }
}

pub struct PacketWriter<'a> {
    snd: &'a mut cdc_acm::Sender<'static, UsbDriver>,
    packet_serialize_buffer: &'a mut [u8; 1024],
    packet_cobs_encode_buffer: &'a mut [u8; 2049],
}

impl PacketWriter<'_> {
    pub async fn write_packet(&mut self, pkt: &Packet) -> Result<(), WaitForPacketError> {
        // Serialize the payload into the serialize buffer (no copies/allocs)
        let ser_len = {
            // serialize_raw writes into [MaybeUninit<u8>]
            let ser = unsafe {
                transmute::<&mut [u8], &mut [MaybeUninit<u8>]>(self.packet_serialize_buffer)
            };
            pkt.serialize_raw(ser)
        };
        let payload = &self.packet_serialize_buffer[..ser_len];

        // COBS-encode into the encode buffer
        // Worst-case encoded_len <= input_len + input_len/254 + 1
        let enc_len = cobs::encode(payload, self.packet_cobs_encode_buffer);

        // Append the single 0x00 delimiter after encoded data
        self.packet_cobs_encode_buffer[enc_len] = 0x00;
        let frame = &self.packet_cobs_encode_buffer[..enc_len + 1];

        // Send the frame
        write_serial(self.snd, frame).await;

        Ok(())
    }
}

#[derive(Format)]
pub enum WaitForPacketError {
    Cobs(cobs::DecodeError),
    Packet(protodongers::Error),
    Usb(EndpointError),
}

impl_from!(WaitForPacketError::Cobs(cobs::DecodeError));
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
