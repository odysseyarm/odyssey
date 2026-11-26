//! PAJ7025 sensor-master capture. Register SPI is never reclaimed here.
use core::convert::Infallible;

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_nrf::mode::Async;
use embassy_nrf::spim;
use embassy_nrf::spis::Spis;
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb::driver::EndpointError;
use embedded_hal_bus::spi::DeviceError;
use paj7025::Paj7025Error;

use crate::Paj;
use crate::object_mode::{PacketReader, PacketWriter, WaitForPacketError};
use crate::odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, ImageChunk, ImageCommand, ImageStatus, ImageStatusCode, ImageStreamConfig,
};
use crate::odyssey_protocol::{Packet, PacketData, Port};
use crate::platform::{L2capChannels, L2capReceiver, L2capSender};
use crate::usb::UsbConfigHandle;

/// Since: Odyssey Desktop 0.7.0.
pub struct ImageStart {
    /// Since: Odyssey Desktop 0.7.0.
    pub id: u8,
    /// Since: Odyssey Desktop 0.7.0.
    pub generation: u32,
    /// Since: Odyssey Desktop 0.7.0.
    pub config: ImageStreamConfig,
}

/// What object mode hands to the board when it leaves for image mode. It no longer refers to
/// the sensors, so the board can drop them and reuse their SPI peripheral and pins.
///
/// Since: Unreleased.
pub struct ImageHandoff<L> {
    /// Since: Unreleased.
    pub start: ImageStart,
    /// Whether the sensor reached image output. A failed transition still leaves register
    /// mode, so object mode cannot resume either way.
    ///
    /// Since: Unreleased.
    pub prepared: bool,
    /// Since: Unreleased.
    pub usb_snd: Endpoint<'static, In>,
    /// Since: Unreleased.
    pub usb_rcv: Endpoint<'static, Out>,
    /// Since: Unreleased.
    pub usb_configured: UsbConfigHandle,
    /// Since: Unreleased.
    pub l2cap_channels: L,
    /// Since: Unreleased.
    pub ota: Option<ota_channel::OtaHandle>,
}

/// Switches `sensor` to image output and releases its register SPI.
///
/// No flash write: tracking settings persist unchanged across recovery. This is the
/// established legacy image firmware setup, in native coordinates.
///
/// # Errors
///
/// Returns the SPI error of the first failed write. The transition is irreversible either way.
///
/// Since: Unreleased.
pub async fn prepare(
    sensor: &mut Paj<'_>,
    camera: Port,
) -> Result<(), Paj7025Error<DeviceError<spim::Error, Infallible>>> {
    sensor.init_image_settings(false).await?;
    sensor
        .ll
        .control()
        .bank_c()
        .b_global()
        .write_async(|r| r.set_value(0))
        .await?;
    sensor
        .ll
        .control()
        .bank_c()
        .b_ggh()
        .write_async(|r| r.set_value(0))
        .await?;
    sensor
        .ll
        .control()
        .bank_c()
        .cmd_frame_period()
        .write_async(|r| r.set_value(524288))
        .await?;
    sensor
        .set_image_exposure_ns(if camera == Port::Nf {
            1_600_000
        } else {
            4_000_000
        })
        .await?;
    sensor
        .ll
        .control()
        .bank_1()
        .bank_1_sync_updated_flag()
        .write_async(|r| r.set_value(1))
        .await?;
    sensor
        .ll
        .control()
        .bank_c()
        .cmd_thd()
        .write_async(|r| r.set_value(100))
        .await?;
    sensor
        .ll
        .control()
        .bank_0()
        .cmd_nthd()
        .write_async(|r| r.set_value(15))
        .await?;
    sensor
        .ll
        .control()
        .bank_c()
        .cmd_scale_resolution_x()
        .write_async(|r| r.set_value(4095))
        .await?;
    sensor
        .ll
        .control()
        .bank_c()
        .cmd_scale_resolution_y()
        .write_async(|r| r.set_value(4095))
        .await?;
    Timer::after_millis(100).await;
    sensor.release_for_image_mode().await
}

/// Streams frames from the sensor chosen in `handoff` until the host stops or disconnects.
/// `receiver` builds the image receiver for a camera; it runs only if the sensor reached image
/// output, after the board has dropped both register drivers.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn run<'r, L: L2capChannels>(
    handoff: ImageHandoff<L>,
    receiver: impl FnOnce(Port) -> Spis<'r, Async>,
) -> ! {
    let ImageHandoff {
        start,
        prepared,
        mut usb_snd,
        mut usb_rcv,
        usb_configured: usb,
        l2cap_channels,
        ota,
    } = handoff;
    let l2cap = &l2cap_channels;
    let mut writer = PacketWriter { snd: &mut usb_snd };
    let mut reader = PacketReader::new(&mut usb_rcv);
    // A cancelled USB read loses the packets the driver already copied, so one reader runs for
    // the rest of image mode and hands each result over through `inbox`.
    let inbox = Channel::<NoopRawMutex, Result<Packet, WaitForPacketError>, 1>::new();
    let usb_reader = async {
        loop {
            let result = reader.wait_for_packet().await;
            let disabled = matches!(
                result,
                Err(WaitForPacketError::Usb(EndpointError::Disabled))
            );
            inbox.send(result).await;
            if disabled {
                usb.wait_until_active().await;
            }
        }
    };
    let handler = async {
        if prepared {
            let mut spis = receiver(start.config.camera);
            status(
                &mut writer,
                start.id,
                start.generation,
                ImageStatusCode::Started,
                usb,
            )
            .await;
            // Static DMA storage outlives every cancellation. SPIS is dropped
            // (disabled) before recovery can accept further host requests.
            let raw = crate::utils::static_byte_buffer!(98 * 101);
            let mut frame_id = 0u32;
            let period = Duration::from_millis(1000 / u64::from(start.config.fps));
            'capture: loop {
                let frame_start = Instant::now();
                let len = {
                    let capture = with_timeout(Duration::from_secs(2), spis.read(raw));
                    let mut capture = core::pin::pin!(capture);
                    loop {
                        match select3(
                            select3(
                                inbox.receive(),
                                l2cap.control_rx().receive(),
                                l2cap.data_rx().receive(),
                            ),
                            usb.wait_until_inactive(),
                            capture.as_mut(),
                        )
                        .await
                        {
                            Either3::First(Either3::First(Ok(packet))) => {
                                if command(&mut writer, packet, &start, usb).await {
                                    break 'capture;
                                }
                            }
                            Either3::First(Either3::First(Err(_))) | Either3::Second(()) => {
                                break 'capture;
                            }
                            Either3::First(Either3::Second(packet))
                            | Either3::First(Either3::Third(packet)) => {
                                ota_busy_reply(l2cap, ota, packet).await;
                            }
                            Either3::Third(Ok(Ok(len))) => break len,
                            Either3::Third(_) => {
                                status(
                                    &mut writer,
                                    start.id,
                                    start.generation,
                                    ImageStatusCode::CaptureError,
                                    usb,
                                )
                                .await;
                                break 'capture;
                            }
                        }
                    }
                };
                // The recovered sensor receiver accounts for a four-bit start
                // alignment or one missing clock-byte. Never publish a short frame.
                if !align(raw, len) {
                    continue;
                }
                let timestamp_us = Instant::now().as_micros() as u32;
                for (index, pixels) in raw[..98 * 98].chunks(IMAGE_CHUNK_CAPACITY).enumerate() {
                    let mut data = [0; IMAGE_CHUNK_CAPACITY];
                    data[..pixels.len()].copy_from_slice(pixels);
                    let packet = Packet {
                        id: start.id,
                        data: PacketData::ImageChunk(ImageChunk {
                            generation: start.generation,
                            frame_id,
                            timestamp_us,
                            objects_timestamp_us: 0,
                            camera: start.config.camera,
                            width: 98,
                            height: 98,
                            object_count: 0,
                            offset: (index * IMAGE_CHUNK_CAPACITY) as u32,
                            len: pixels.len() as u16,
                            data,
                        }),
                    };
                    match select(
                        usb.wait_until_inactive(),
                        with_timeout(Duration::from_millis(500), writer.write_packet(&packet)),
                    )
                    .await
                    {
                        Either::Second(Ok(Ok(()))) => {}
                        _ => break 'capture,
                    }
                }
                frame_id = frame_id.wrapping_add(1);
                loop {
                    match select3(
                        select3(
                            inbox.receive(),
                            l2cap.control_rx().receive(),
                            l2cap.data_rx().receive(),
                        ),
                        usb.wait_until_inactive(),
                        Timer::at(frame_start + period),
                    )
                    .await
                    {
                        Either3::First(Either3::First(Ok(packet))) => {
                            if command(&mut writer, packet, &start, usb).await {
                                break 'capture;
                            }
                        }
                        Either3::First(Either3::First(Err(_))) | Either3::Second(()) => {
                            break 'capture;
                        }
                        Either3::First(Either3::Second(packet))
                        | Either3::First(Either3::Third(packet)) => {
                            ota_busy_reply(l2cap, ota, packet).await;
                        }
                        Either3::Third(()) => break,
                    }
                }
            }
            drop(spis);
        } else {
            // A partial transition is just as irreversible as a completed one.
            status(
                &mut writer,
                start.id,
                start.generation,
                ImageStatusCode::CaptureError,
                usb,
            )
            .await;
        }
        // No return to object_mode and no register requests after image entry, even
        // if USB disconnects/reconnects or startup failed halfway through.
        loop {
            match select3(
                inbox.receive(),
                l2cap.control_rx().receive(),
                l2cap.data_rx().receive(),
            )
            .await
            {
                Either3::First(Ok(packet)) => {
                    let (generation, code) = match packet.data {
                        PacketData::ImageCommand(ImageCommand::Renew { .. }) => continue,
                        PacketData::ImageCommand(ImageCommand::Stop { generation })
                            if generation != start.generation =>
                        {
                            (generation, ImageStatusCode::Stopped)
                        }
                        PacketData::ImageCommand(
                            ImageCommand::Start { generation, .. }
                            | ImageCommand::Stop { generation },
                        ) => (generation, ImageStatusCode::RebootRequired),
                        _ => (start.generation, ImageStatusCode::RebootRequired),
                    };
                    status(&mut writer, packet.id, generation, code, usb).await;
                }

                Either3::First(Err(_)) => Timer::after_millis(20).await,
                Either3::Second(packet) | Either3::Third(packet) => {
                    ota_busy_reply(l2cap, ota, packet).await;
                }
            }
        }
    };
    match select(handler, usb_reader).await {
        Either::First(never) | Either::Second(never) => never,
    }
}

async fn ota_busy_reply<L: L2capChannels>(
    l2cap: &L,
    ota: Option<ota_channel::OtaHandle>,
    packet: Packet,
) {
    let PacketData::OtaRequest(_) = packet.data else {
        return;
    };
    let Some(ota) = ota else {
        return;
    };
    let Some(reply) = ota.request(&[0]).await else {
        return;
    };
    let Ok(mut status) = odyssey_protocol::ota::Status::decode(&reply.status) else {
        return;
    };
    status.error = odyssey_protocol::ota::Code::Busy;
    let response = Packet {
        id: packet.id,
        data: PacketData::OtaStatus(status.encode()),
    };
    if response.is_control() {
        l2cap.control_tx().send(response).await;
    } else {
        l2cap.data_tx().send(response).await;
    }
}

async fn command(
    writer: &mut PacketWriter<'_>,
    packet: Packet,
    active: &ImageStart,
    usb: UsbConfigHandle,
) -> bool {
    let (generation, code, stop) = match packet.data {
        PacketData::ImageCommand(ImageCommand::Renew { .. }) => return false,
        PacketData::ImageCommand(ImageCommand::Stop { generation })
            if generation == active.generation =>
        {
            (generation, ImageStatusCode::RebootRequired, true)
        }
        PacketData::ImageCommand(ImageCommand::Stop { generation }) => {
            (generation, ImageStatusCode::Stopped, false)
        }
        PacketData::ImageCommand(ImageCommand::Start { generation, .. }) => {
            (generation, ImageStatusCode::Busy, false)
        }
        _ => (active.generation, ImageStatusCode::RebootRequired, false),
    };
    status(writer, packet.id, generation, code, usb).await;
    stop
}

async fn status(
    writer: &mut PacketWriter<'_>,
    id: u8,
    generation: u32,
    code: ImageStatusCode,
    usb: UsbConfigHandle,
) {
    let packet = Packet {
        id,
        data: PacketData::ImageStatus(ImageStatus {
            generation,
            status: code,
        }),
    };
    // Neither an absent reader nor a suspended host may retain capture forever.
    let _ = select(
        usb.wait_until_inactive(),
        with_timeout(Duration::from_millis(500), writer.write_packet(&packet)),
    )
    .await;
}

fn align(buffer: &mut [u8], len: usize) -> bool {
    if len == 9898 {
        buffer[0] <<= 4;
        for i in 1..buffer.len() {
            buffer[i - 1] |= buffer[i] >> 4;
            buffer[i] <<= 4;
        }
    } else if len == 9897 {
        for (i, byte) in [0x2d, 0x5a, 0xb4, 0x69, 0xd2, 0xa5, 0x4b]
            .iter()
            .enumerate()
        {
            if buffer[buffer.len() - 6] == *byte {
                let shift = i as u8 + 1;
                let mut previous = 0;
                for value in buffer.iter_mut() {
                    let original = *value;
                    *value = (*value >> shift) | (previous << (8 - shift));
                    previous = original;
                }
                break;
            }
        }
    } else {
        return false;
    }
    true
}
