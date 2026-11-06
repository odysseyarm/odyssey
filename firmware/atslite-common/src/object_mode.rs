//! Tracking and reversible USB image capture for the main firmware.
//! Reports use USB while configured, otherwise BLE; image capture is USB-only and
//! retains Image mode for the session without acquiring object snapshots.

use core::future::Future;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use donger_common::usb_device;
pub use donger_common::usb_device::{UsbConfigHandle, read_transfer, write_transfer};
use embassy_futures::join::{join, join5};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::{NoopRawMutex, ThreadModeRawMutex};
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::driver::{Endpoint as _, EndpointError};
use nalgebra::Vector3;
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, IMAGE_LEASE_TIMEOUT_MS, ImageChunk, ImageCommand, ImageStatus,
    ImageStatusCode, ImageStreamConfig,
};
use odyssey_protocol::wire::GeneralConfig;
use odyssey_protocol::{
    BatteryReport, ConfigKind, Packet, PacketData, PacketType, Props, ReadRegisterResponse,
};

use crate::ble::l2cap_bridge::{self, L2capChannels};
use crate::settings::PagSettings;

mod image_state;
use image_state::ImageOwnership;

type Channel<T, const N: usize> = embassy_sync::channel::Channel<ThreadModeRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, ThreadModeRawMutex, T, N>;
type Receiver<'a, T, const N: usize> =
    embassy_sync::channel::Receiver<'a, ThreadModeRawMutex, T, N>;

/// USB requests read but not handled yet. USB is read in its own loop so a multi-packet bulk
/// OUT transfer is never cancelled half-way when a BLE request wins the race.
const USB_REQUEST_QUEUE: usize = 2;

/// Replies to USB requests waiting for the bulk IN endpoint. They bypass the report queue, so
/// a backlog of reports does not delay them.
const USB_REPLY_QUEUE: usize = 4;

/// A single static frame avoids both a 76 KiB task-stack temporary and heap allocation.
const IMAGE_WIDTH: u16 = 320;
const IMAGE_HEIGHT: u16 = 240;
const IMAGE_PIXELS: usize = IMAGE_WIDTH as usize * IMAGE_HEIGHT as usize;
const IMAGE_QUEUE: usize = 2;
// Preserve live (not merely flashed) settings, including ROI and sensor-size selection.
// The meaning of SENSOR_SIZE_SEL (0x12) is not documented in this repository. Preserve it
// verbatim rather than guessing a 320×240 selector; capture follows the existing driver API.
const IMAGE_SAVED_REGISTERS: [u8; 18] = [
    0x67, 0x68, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x23, 0x24, 0x77, 0x78, 0x13, 0x12, 0x72, 0x73, 0x74,
    0x75, 0x76,
];

/// USB packets retain the connection that received/produced them across queue backpressure.
struct UsbPacket {
    packet: Packet,
    generation: u32,
}

#[derive(Clone, Copy)]
struct UsbReplies<'a, const N: usize> {
    sender: Sender<'a, UsbPacket, N>,
    generation: u32,
}

impl<const N: usize> UsbReplies<'_, N> {
    fn try_send(&self, packet: Packet) -> Result<(), ()> {
        self.sender
            .try_send(UsbPacket {
                packet,
                generation: self.generation,
            })
            .map_err(|_| ())
    }
}

struct ImageRequest {
    id: u8,
    command: ImageCommand,
    usb_generation: u32,
}

impl ImageRequest {
    fn belongs_to(&self, usb_generation: u32) -> bool {
        ImageOwnership::request_is_current(self.usb_generation, usb_generation)
    }
}

struct ImageState {
    ownership: ImageOwnership,
    abort: Signal<NoopRawMutex, ()>,
}

impl ImageState {
    fn new() -> Self {
        Self {
            ownership: ImageOwnership::new(),
            abort: Signal::new(),
        }
    }
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

    /// Enters Image with ROI temporarily disabled; `restore_object` reinstates the live ROI.
    async fn switch_to_image(&mut self) -> Result<(), Self::Error>;

    /// Polls one full-resolution native-orientation frame; never waits for a future frame.
    async fn try_get_image(&mut self, image: &mut [u8; IMAGE_PIXELS]) -> Result<bool, Self::Error>;

    /// Releases a report-buffer lock and restores Object even after a failed transition.
    async fn restore_object(&mut self) -> Result<(), Self::Error>;
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

/// Hardware handed to [`run`]. Settings are borrowed synchronously through `with_settings`.
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
    /// USB bulk IN: reports and responses to the host.
    pub usb_snd: Endpoint<'static, In>,
    /// USB bulk OUT: commands from the host.
    pub usb_rcv: Endpoint<'static, Out>,
    pub usb_configured: UsbConfigHandle,
}

/// Runs object mode forever: the USB reader and the receive, IMU, marker, battery and send
/// loops run concurrently. Which streams are enabled is one set for the whole device, so a
/// stream enabled over either transport keeps flowing when reports move to the other.
pub async fn run<P, I, PI, II>(mut ctx: ObjectModeContext<'_, P, I, PI, II>)
where
    P: PagSensor,
    I: ImuSensor,
    PI: PagInterrupt,
    II: ImuInterrupt,
{
    defmt::info!("[object_mode] Starting object mode");

    // Packet queues live in statics: built as locals they inflate main's poll frame enough
    // to overflow the startup stack once task spawns stack on top of it.
    static REPORTS: Channel<Packet, 64> = Channel::new();
    static USB_REQUESTS: Channel<UsbPacket, USB_REQUEST_QUEUE> = Channel::new();
    static USB_REPLIES: Channel<UsbPacket, USB_REPLY_QUEUE> = Channel::new();
    static IMAGE_REQUESTS: Channel<ImageRequest, 2> = Channel::new();
    static IMAGE_DATA: Channel<Packet, IMAGE_QUEUE> = Channel::new();
    let (usb_requests, usb_replies, image_requests, image_data) =
        (&USB_REQUESTS, &USB_REPLIES, &IMAGE_REQUESTS, &IMAGE_DATA);
    let image_state = ImageState::new();
    let stream_infos = StreamInfos::new();

    let l2cap = l2cap_bridge::channels();

    let usb_read_task = usb_read_loop(&mut ctx.usb_rcv, usb_requests.sender(), ctx.usb_configured);

    let recv_task = recv_loop(
        ctx.pag,
        &stream_infos,
        &l2cap,
        ctx.device_id,
        ctx.product_id,
        ctx.firmware_version,
        usb_requests.receiver(),
        usb_replies.sender(),
        image_requests.sender(),
        &image_state,
        ctx.usb_configured,
    );

    let imu_task = imu_loop(
        &mut ctx.imu,
        &mut ctx.imu_int,
        REPORTS.sender(),
        &stream_infos,
    );

    let obj_task = obj_loop(
        ctx.pag,
        &mut ctx.pag_int,
        REPORTS.sender(),
        &stream_infos,
        image_requests.receiver(),
        usb_replies.sender(),
        image_data,
        &image_state,
        ctx.usb_configured,
    );

    let stream_task = stream_loop(
        REPORTS.receiver(),
        usb_replies.receiver(),
        image_data.receiver(),
        &image_state,
        &l2cap,
        &mut ctx.usb_snd,
        ctx.usb_configured,
    );

    let battery_task = battery_loop(REPORTS.sender(), &stream_infos);

    join(
        usb_read_task,
        join5(recv_task, imu_task, obj_task, stream_task, battery_task),
    )
    .await;
}

/// Reads requests from USB bulk OUT into `requests` while a host has the endpoint enabled.
async fn usb_read_loop<const N: usize>(
    usb_rcv: &mut Endpoint<'static, Out>,
    requests: Sender<'_, UsbPacket, N>,
    usb_configured: UsbConfigHandle,
) {
    let mut usb_buf = [0u8; 512];
    loop {
        let generation = usb_configured.generation();
        let transfer = match select(
            usb_configured.wait_until_generation_changes(generation),
            read_transfer(usb_rcv, &mut usb_buf),
        )
        .await
        {
            Either::First(()) => continue,
            Either::Second(result) => result,
        };
        match transfer {
            Ok(n) => match postcard::from_bytes::<Packet>(&usb_buf[..n]) {
                Ok(pkt) if generation == usb_configured.generation() => {
                    requests
                        .send(UsbPacket {
                            packet: pkt,
                            generation,
                        })
                        .await;
                }
                Ok(_) => {}
                Err(e) => defmt::error!(
                    "[object_mode] USB packet deserialization failed: {:?}",
                    defmt::Debug2Format(&e)
                ),
            },
            Err(EndpointError::Disabled) => {
                defmt::debug!("[object_mode] USB endpoint disabled, waiting for a host");
                usb_rcv.wait_enabled().await;
            }
            Err(e) => {
                defmt::error!(
                    "[object_mode] USB read error: {:?}",
                    defmt::Debug2Format(&e)
                );
                Timer::after_millis(10).await;
            }
        }
    }
}

/// Handles requests from both transports as they arrive. Neither transport is ever switched
/// off here: a host may send over BLE while USB carries the reports, and is answered over BLE.
async fn recv_loop<P: PagSensor, const Q: usize, const R: usize>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    stream_infos: &StreamInfos,
    l2cap: &L2capChannels,
    device_id: [u8; 6],
    product_id: u16,
    firmware_version: [u16; 3],
    usb_requests: Receiver<'_, UsbPacket, Q>,
    usb_replies: Sender<'_, UsbPacket, R>,
    image_requests: Sender<'_, ImageRequest, 2>,
    image_state: &ImageState,
    usb_configured: UsbConfigHandle,
) {
    loop {
        let (request_transport, pkt, usb_generation) = match select3(
            usb_requests.receive(),
            l2cap.control_rx.receive(),
            l2cap.data_rx.receive(),
        )
        .await
        {
            Either3::First(request) => (TransportMode::Usb, request.packet, request.generation),
            Either3::Second(pkt) | Either3::Third(pkt) => (TransportMode::Ble, pkt, 0),
        };
        if request_transport == TransportMode::Usb && usb_generation != usb_configured.generation()
        {
            continue;
        }
        let ble_session = match request_transport {
            TransportMode::Usb => 0,
            TransportMode::Ble => l2cap_bridge::session_generation(),
        };

        let response = match pkt.data {
            PacketData::WriteRegister(_) if !image_state.ownership.settings_writable() => {
                Some(image_status(
                    pkt.id,
                    image_state.ownership.generation(),
                    ImageStatusCode::Busy,
                ))
            }
            PacketData::WriteRegister(register) => {
                let mut pag = pag.lock().await;
                // The ownership state may have changed while this request waited on the
                // sensor mutex. Never ACK a write that cleanup would subsequently overwrite.
                if !image_state.ownership.settings_writable() {
                    Some(image_status(
                        pkt.id,
                        image_state.ownership.generation(),
                        ImageStatusCode::Busy,
                    ))
                } else {
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
                    PropKind::Name => Props::Name(crate::settings::with_settings(|s| {
                        s.general.name.clone().into()
                    })),
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
                // Shared by USB control and object mode; imu_loop reads the same live settings.
                // Persisted on the next FlashSettings.
                crate::settings::with_settings(|s| s.general.set_general_config(&wire_config));
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::ReadConfig(kind) => {
                defmt::info!("[object_mode] ReadConfig: {:?}", kind);
                let wire_config = crate::settings::with_settings(|s| {
                    let g = &s.general;
                    match kind {
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
                    }
                });
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadConfigResponse(wire_config.into()),
                })
            }

            PacketData::FlashSettings() => {
                defmt::info!("[object_mode] FlashSettings received");

                // Snapshot the PAG registers (NF sensor only) so they persist too.
                let pag_settings = {
                    let mut pag = pag.lock().await;
                    read_pag_settings(&mut *pag).await
                };

                crate::settings::with_settings(|settings| {
                    if let Ok(pag_settings) = pag_settings {
                        settings.pag = pag_settings;
                        settings.pag_write();
                    } else {
                        defmt::error!(
                            "[object_mode] Failed to read PAG settings, saving general only"
                        );
                    }
                    settings.general_write();
                });
                defmt::info!("[object_mode] Settings queued for flash");

                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }

            PacketData::WriteMode(_) => Some(image_status(pkt.id, 0, ImageStatusCode::Unsupported)),

            // Renew is never answered: a reply on the stream id would end the host session.
            PacketData::ImageCommand(command @ ImageCommand::Renew { .. }) => {
                if request_transport == TransportMode::Usb {
                    let _ = image_requests.try_send(ImageRequest {
                        id: pkt.id,
                        command,
                        usb_generation,
                    });
                }
                None
            }
            PacketData::ImageCommand(command) => {
                let generation = match command {
                    ImageCommand::Start { generation, .. }
                    | ImageCommand::Stop { generation }
                    | ImageCommand::Renew { generation } => generation,
                };
                if request_transport != TransportMode::Usb {
                    Some(image_status(
                        pkt.id,
                        generation,
                        ImageStatusCode::UsbRequired,
                    ))
                } else if let ImageCommand::Start { config, .. } = command
                    && config.camera != odyssey_protocol::Port::Nf
                {
                    Some(image_status(
                        pkt.id,
                        generation,
                        ImageStatusCode::UnsupportedCamera,
                    ))
                } else if let ImageCommand::Start { config, .. } = command
                    && !(1..=15).contains(&config.fps)
                {
                    Some(image_status(
                        pkt.id,
                        generation,
                        ImageStatusCode::CaptureError,
                    ))
                } else if image_requests
                    .try_send(ImageRequest {
                        id: pkt.id,
                        command,
                        usb_generation,
                    })
                    .is_err()
                {
                    Some(image_status(pkt.id, generation, ImageStatusCode::Busy))
                } else {
                    // The sensor owner responds only after it has actually changed/restored mode.
                    None
                }
            }

            PacketData::SetDeviceName(name) => {
                defmt::info!("[object_mode] SetDeviceName: {:?}", name.as_str());
                // No ADV_RESTART_SIGNAL: this may be running inside the dongle's BLE
                // connection, and a restart would drop it. The new name is advertised on the
                // next advertising cycle.
                crate::ble::peripheral::set_device_name_initial(name.as_str());
                crate::settings::with_settings(|settings| {
                    settings.general.name = name.into();
                    settings.general_write();
                });
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
            PacketData::ImageStatus(_) => None,
            PacketData::ImageChunk(_) => None,
            PacketData::ImageObject(_) => None,
        };

        // Id 255 asks for no reply.
        let Some(r) = response.filter(|r| r.id != 255) else {
            continue;
        };
        let queued =
            match atslite_transport::reply_transport(request_transport, usb_device::is_active()) {
                // stream_loop owns the USB IN endpoint. try_send keeps a USB host that stopped
                // reading from stalling the BLE requests behind it.
                Some(TransportMode::Usb) => usb_replies
                    .try_send(UsbPacket {
                        packet: r,
                        generation: usb_generation,
                    })
                    .is_ok(),
                Some(TransportMode::Ble) if ble_session == l2cap_bridge::session_generation() => {
                    l2cap.try_send(r).is_ok()
                }
                Some(TransportMode::Ble) => {
                    // The handler awaited sensor I/O across a disconnect. Its reply belongs to
                    // the old session, even if another central has already reconnected.
                    defmt::debug!("[object_mode] BLE session changed before reply, dropping it");
                    true
                }
                None => {
                    defmt::debug!("[object_mode] USB went away before the reply, dropping it");
                    true
                }
            };
        if !queued {
            defmt::warn!(
                "[object_mode] Dropped {:?} reply - queue full",
                request_transport
            );
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

/// Owns the outgoing side. Reports go over USB while a USB host is using the device and to the
/// L2CAP queues otherwise; replies to USB requests go over USB or nowhere. A USB write in
/// progress is abandoned the moment USB goes away, so reports move to BLE without waiting for
/// a stalled transfer.
async fn stream_loop<const N: usize, const R: usize>(
    reports: Receiver<'_, Packet, N>,
    usb_replies: Receiver<'_, UsbPacket, R>,
    image_data: Receiver<'_, Packet, IMAGE_QUEUE>,
    image_state: &ImageState,
    l2cap: &L2capChannels,
    usb_snd: &mut Endpoint<'static, In>,
    usb_configured: UsbConfigHandle,
) {
    defmt::info!("[stream_loop] Started");
    let mut pkt_count: u32 = 0;
    let mut usb_snd_buf = [0u8; 1024];

    loop {
        let (pkt, transport, usb_generation) = match select3(
            usb_replies.receive(),
            reports.receive(),
            image_data.receive(),
        )
        .await
        {
            Either3::First(reply) => {
                if reply.generation != usb_configured.generation() {
                    continue;
                }
                (
                    reply.packet,
                    atslite_transport::reply_transport(TransportMode::Usb, usb_device::is_active()),
                    reply.generation,
                )
            }
            Either3::Second(pkt) => (
                pkt,
                Some(atslite_transport::report_transport(usb_device::is_active())),
                usb_configured.generation(),
            ),
            Either3::Third(pkt) => {
                let generation = match &pkt.data {
                    PacketData::ImageChunk(chunk) => chunk.generation,
                    _ => continue,
                };
                if !image_state
                    .ownership
                    .accepts(generation, usb_configured.generation())
                {
                    continue;
                }
                (
                    pkt,
                    atslite_transport::reply_transport(TransportMode::Usb, usb_device::is_active()),
                    image_state.ownership.usb_generation(),
                )
            }
        };
        pkt_count = pkt_count.wrapping_add(1);
        if pkt_count % 100 == 0 {
            defmt::debug!("[stream_loop] Processed {} packets", pkt_count);
        }

        match transport {
            Some(TransportMode::Usb) => {
                // Reports queued for BLE before USB came up would still go out over BLE next to
                // the newer ones on USB, so they are dropped. The data queue holds only reports;
                // replies to BLE requests are on the control queue and still go out.
                l2cap.data_tx.clear();
                let payload = match postcard::to_slice(&pkt, &mut usb_snd_buf) {
                    Ok(payload) => payload,
                    Err(e) => {
                        defmt::error!(
                            "[stream_loop] USB packet serialization failed: {:?}",
                            defmt::Debug2Format(&e)
                        );
                        continue;
                    }
                };
                match select(
                    usb_configured.wait_until_generation_changes(usb_generation),
                    write_transfer(usb_snd, payload, true),
                )
                .await
                {
                    Either::First(()) => {
                        defmt::info!("[stream_loop] USB epoch changed mid-write, packet dropped");
                    }
                    Either::Second(Ok(())) => {}
                    Either::Second(Err(e)) => {
                        image_state.abort.signal(());
                        defmt::error!(
                            "[stream_loop] USB write error: {:?}",
                            defmt::Debug2Format(&e)
                        );
                    }
                }
            }
            Some(TransportMode::Ble) => {
                if l2cap.try_send(pkt).is_err() {
                    defmt::warn!("[stream_loop] Dropped packet - L2CAP queue full");
                }
            }
            None => defmt::debug!("[stream_loop] Dropped USB reply - USB went away"),
        }
    }
}

/// Streams accel/gyro reports and raises impact reports when acceleration crosses the threshold.
async fn imu_loop<I: ImuSensor, II: ImuInterrupt, const N: usize>(
    imu: &mut I,
    imu_int: &mut II,
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
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
            let (threshold, suppress_ms) = crate::settings::with_settings(|s| {
                (s.general.impact_threshold, s.general.suppress_ms)
            });
            let threshold_ms2 = threshold as f32;

            if accel_magnitude_ms2 >= threshold_ms2 {
                let suppress_us = u32::from(suppress_ms) * 1000;
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

/// Sensor ownership loop. Image transitions/capture/cleanup never block USB receive or IMU.
/// No PAG mutex guard survives USB delivery. Normal markers keep their existing all-centroid
/// semantics outside image sessions.
async fn obj_loop<P: PagSensor, PI: PagInterrupt, const N: usize>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    pag_int: &mut PI,
    pkt_snd: Sender<'_, Packet, N>,
    stream_infos: &StreamInfos,
    image_requests: Receiver<'_, ImageRequest, 2>,
    usb_replies: Sender<'_, UsbPacket, USB_REPLY_QUEUE>,
    image_data: &Channel<Packet, IMAGE_QUEUE>,
    image_state: &ImageState,
    usb_configured: UsbConfigHandle,
) {
    static IMAGE: static_cell::ConstStaticCell<[u8; IMAGE_PIXELS]> =
        static_cell::ConstStaticCell::new([0; IMAGE_PIXELS]);
    let image = IMAGE.take();
    let mut objs = [pag7665qn::types::Object::DEFAULT; 16];
    let mut frame_id = 0u32;
    let mut restore_pending = false;
    let mut saved_image_settings = [0; IMAGE_SAVED_REGISTERS.len()];

    loop {
        if restore_pending {
            restore_pending = restore_image_settings(pag, &saved_image_settings)
                .await
                .is_err();
            image_state.ownership.finish_restore(!restore_pending);
        }
        let request = match select(image_requests.receive(), async {
            if stream_infos.marker.enabled() && !restore_pending {
                let _ = with_timeout(Duration::from_millis(50), pag_int.wait()).await;
            } else {
                Timer::after_millis(50).await;
            }
        })
        .await
        {
            Either::First(request) => Some(request),
            Either::Second(_) => None,
        };
        if let Some(request) = request {
            if !request.belongs_to(usb_configured.generation()) || !usb_device::is_active() {
                continue;
            }
            let usb_replies = UsbReplies {
                sender: usb_replies,
                generation: request.usb_generation,
            };
            match request.command {
                ImageCommand::Renew { .. } => {}
                ImageCommand::Stop { generation } => {
                    let status = if restore_pending {
                        ImageStatusCode::CaptureError
                    } else {
                        ImageStatusCode::Stopped
                    };
                    let _ = usb_replies.try_send(image_status(request.id, generation, status));
                }
                ImageCommand::Start { generation, config } => {
                    if !usb_device::is_active() {
                        continue;
                    }
                    if restore_pending {
                        let _ = usb_replies.try_send(image_status(
                            request.id,
                            generation,
                            ImageStatusCode::CaptureError,
                        ));
                        continue;
                    }
                    image_state.abort.reset();
                    image_data.clear();
                    image_state
                        .ownership
                        .begin(generation, request.usb_generation);
                    let snapshot = async {
                        let mut sensor = pag.lock().await;
                        for (index, register) in IMAGE_SAVED_REGISTERS.iter().enumerate() {
                            saved_image_settings[index] =
                                sensor.read_register(0, *register).await?;
                        }
                        Ok::<(), P::Error>(())
                    }
                    .await;
                    if let Err(e) = snapshot {
                        image_state.ownership.finish_capture();
                        image_state.ownership.finish_restore(true);
                        defmt::error!("[image] Cannot snapshot live settings: {:?}", e);
                        let _ = usb_replies.try_send(image_status(
                            request.id,
                            generation,
                            ImageStatusCode::CaptureError,
                        ));
                        continue;
                    }
                    let end = image_session(
                        pag,
                        image,
                        &mut frame_id,
                        request.id,
                        generation,
                        request.usb_generation,
                        config,
                        image_requests,
                        usb_replies,
                        image_data,
                        image_state,
                        usb_configured,
                    )
                    .await;
                    image_state.ownership.finish_capture();
                    image_data.clear();
                    let restored = restore_image_settings(pag, &saved_image_settings).await;
                    restore_pending = restored.is_err();
                    image_state.ownership.finish_restore(!restore_pending);
                    if let Err(e) = &restored {
                        defmt::error!("[obj_loop] Failed to restore Object: {:?}", e);
                    }
                    match end {
                        ImageEnd::Stop { id, generation } => {
                            let status = if restored.is_ok() {
                                ImageStatusCode::Stopped
                            } else {
                                ImageStatusCode::CaptureError
                            };
                            let _ = usb_replies.try_send(image_status(id, generation, status));
                        }
                        ImageEnd::LeaseExpired => {
                            let status = if restored.is_ok() {
                                ImageStatusCode::LeaseExpired
                            } else {
                                ImageStatusCode::CaptureError
                            };
                            let _ =
                                usb_replies.try_send(image_status(request.id, generation, status));
                        }
                        ImageEnd::CaptureError => {
                            let _ = usb_replies.try_send(image_status(
                                request.id,
                                generation,
                                ImageStatusCode::CaptureError,
                            ));
                        }
                        ImageEnd::Disconnected => {}
                    }
                }
            }
            continue;
        }
        if !stream_infos.marker.enabled() || restore_pending {
            continue;
        }
        match pag.lock().await.get_objects(&mut objs).await {
            Ok(Some(_)) => {
                send_markers(pkt_snd, stream_infos, &objs);
            }
            Ok(None) => {}
            Err(e) => defmt::error!("[obj_loop] get_objects failed: {:?}", e),
        }
    }
}

/// Reapply the exact pre-session registers in Idle and attempt Object restoration even if
/// cleanup or a settings write fails. A failed attempt remains pending in the sensor loop.
async fn restore_image_settings<P: PagSensor>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    settings: &[u8; IMAGE_SAVED_REGISTERS.len()],
) -> Result<(), P::Error> {
    let mut sensor = pag.lock().await;
    let restore = async {
        sensor.restore_object().await?;
        sensor.switch_to_idle().await?;
        for (register, value) in IMAGE_SAVED_REGISTERS.iter().zip(settings) {
            sensor.write_register(0, *register, *value).await?;
        }
        Ok::<(), P::Error>(())
    }
    .await;
    let object = sensor.restore_object().await;
    restore?;
    object
}

fn send_markers<const N: usize>(
    reports: Sender<'_, Packet, N>,
    streams: &StreamInfos,
    objs: &[pag7665qn::types::Object; 16],
) {
    if streams.marker.enabled() {
        let _ = reports.try_send(Packet {
            id: streams.marker.req_id(),
            data: PacketData::PocMarkersReport(odyssey_protocol::PocMarkersReport {
                points: objs.map(|o| nalgebra::Point2::new(o.x(), o.y())),
            }),
        });
    }
}

fn image_status(id: u8, generation: u32, status: ImageStatusCode) -> Packet {
    Packet {
        id,
        data: PacketData::ImageStatus(ImageStatus { generation, status }),
    }
}

enum ImageEnd {
    Stop { id: u8, generation: u32 },
    Disconnected,
    LeaseExpired,
    CaptureError,
}

/// Waits without starving stop/disconnect/lease expiry while an image packet is backpressured.
async fn image_wait<F: Future>(
    future: F,
    generation: u32,
    usb_generation: u32,
    lease: &mut Instant,
    requests: Receiver<'_, ImageRequest, 2>,
    replies: UsbReplies<'_, USB_REPLY_QUEUE>,
    state: &ImageState,
    usb: UsbConfigHandle,
) -> Result<F::Output, ImageEnd> {
    let mut future = core::pin::pin!(future);
    loop {
        if usb.generation() != usb_generation || !usb_device::is_active() {
            return Err(ImageEnd::Disconnected);
        }
        // Check the epoch cancellation first. A brief deconfigure/reconfigure must win over
        // a new host's already queued command and over a ready packet-send future.
        // Cancellation and expiry are polled before requests, so a Renew queued after the
        // deadline cannot revive the session.
        match select3(
            select3(
                state.abort.wait(),
                usb.wait_until_generation_changes(usb_generation),
                Timer::at(*lease),
            ),
            requests.receive(),
            future.as_mut(),
        )
        .await
        {
            Either3::First(Either3::First(())) => return Err(ImageEnd::CaptureError),
            Either3::First(Either3::Second(())) => return Err(ImageEnd::Disconnected),
            Either3::First(Either3::Third(())) => return Err(ImageEnd::LeaseExpired),
            Either3::Second(request) => {
                if !request.belongs_to(usb_generation) {
                    continue;
                }
                match request.command {
                    ImageCommand::Renew {
                        generation: renewed,
                    } => {
                        if renewed == generation {
                            *lease = Instant::now() + Duration::from_millis(IMAGE_LEASE_TIMEOUT_MS);
                        }
                    }
                    ImageCommand::Stop {
                        generation: stopped,
                    } if stopped == generation => {
                        return Err(ImageEnd::Stop {
                            id: request.id,
                            generation,
                        });
                    }
                    ImageCommand::Stop {
                        generation: stopped,
                    } => {
                        let _ = replies.try_send(image_status(
                            request.id,
                            stopped,
                            ImageStatusCode::Stopped,
                        ));
                    }
                    ImageCommand::Start {
                        generation: started,
                        ..
                    } => {
                        let _ = replies.try_send(image_status(
                            request.id,
                            started,
                            ImageStatusCode::Busy,
                        ));
                    }
                }
            }
            Either3::Third(value) => return Ok(value),
        }
    }
}

async fn image_session<P: PagSensor>(
    pag: &AsyncMutex<NoopRawMutex, P>,
    image: &mut [u8; IMAGE_PIXELS],
    frame_id: &mut u32,
    id: u8,
    generation: u32,
    usb_generation: u32,
    config: ImageStreamConfig,
    requests: Receiver<'_, ImageRequest, 2>,
    replies: UsbReplies<'_, USB_REPLY_QUEUE>,
    data: &Channel<Packet, IMAGE_QUEUE>,
    state: &ImageState,
    usb: UsbConfigHandle,
) -> ImageEnd {
    let period = Duration::from_millis(1000 / u64::from(config.fps));
    let mut lease = Instant::now() + Duration::from_millis(IMAGE_LEASE_TIMEOUT_MS);
    if let Err(e) = pag.lock().await.switch_to_image().await {
        defmt::error!("[image] Image transition failed: {:?}", e);
        return ImageEnd::CaptureError;
    }
    if usb.generation() != usb_generation || !usb_device::is_active() {
        return ImageEnd::Disconnected;
    }
    if replies
        .try_send(image_status(id, generation, ImageStatusCode::Started))
        .is_err()
    {
        return ImageEnd::CaptureError;
    }
    loop {
        let frame_start = Instant::now();
        let capture_deadline = Instant::now() + Duration::from_secs(1);
        let timestamp_us = loop {
            match pag.lock().await.try_get_image(image).await {
                Ok(true) => break Instant::now().as_micros() as u32,
                Ok(false) if Instant::now() < capture_deadline => {}
                Ok(false) => return ImageEnd::CaptureError,
                Err(e) => {
                    defmt::error!("[image] Capture failed: {:?}", e);
                    return ImageEnd::CaptureError;
                }
            }
            if let Err(end) = image_wait(
                Timer::after_millis(5),
                generation,
                usb_generation,
                &mut lease,
                requests,
                replies,
                state,
                usb,
            )
            .await
            {
                return end;
            }
        };
        *frame_id = frame_id.wrapping_add(1);
        for (index, pixels) in image.chunks(IMAGE_CHUNK_CAPACITY).enumerate() {
            let mut chunk = ImageChunk {
                generation,
                frame_id: *frame_id,
                timestamp_us,
                objects_timestamp_us: 0,
                camera: config.camera,
                width: IMAGE_WIDTH,
                height: IMAGE_HEIGHT,
                object_count: 0,
                offset: (index * IMAGE_CHUNK_CAPACITY) as u32,
                len: pixels.len() as u16,
                data: [0; IMAGE_CHUNK_CAPACITY],
            };
            chunk.data[..pixels.len()].copy_from_slice(pixels);
            match image_wait(
                with_timeout(
                    Duration::from_millis(500),
                    data.send(Packet {
                        id,
                        data: PacketData::ImageChunk(chunk),
                    }),
                ),
                generation,
                usb_generation,
                &mut lease,
                requests,
                replies,
                state,
                usb,
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return ImageEnd::CaptureError,
                Err(end) => return end,
            }
        }
        if let Err(end) = image_wait(
            Timer::at(frame_start + period),
            generation,
            usb_generation,
            &mut lease,
            requests,
            replies,
            state,
            usb,
        )
        .await
        {
            return end;
        }
    }
}
