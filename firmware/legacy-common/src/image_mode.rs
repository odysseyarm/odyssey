//! PAJ7025 sensor-master capture. Register SPI is never reclaimed here.
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_nrf::gpio::{AnyPin, Pin};
use embassy_nrf::spis::Spis;
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer, with_timeout};

use crate::Paj;
use crate::object_mode::{PacketReader, PacketWriter};
use crate::odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, ImageChunk, ImageCommand, ImageStatus, ImageStatusCode, ImageStreamConfig,
};
use crate::odyssey_protocol::{Packet, PacketData, Port};
use crate::usb::UsbConfigHandle;
use crate::utils::SpiPins;

pub struct ImageStart {
    pub id: u8,
    pub generation: u32,
    pub config: ImageStreamConfig,
}

pub struct ImagePins {
    pub cs: embassy_nrf::Peri<'static, AnyPin>,
    pub sck: embassy_nrf::Peri<'static, AnyPin>,
    pub mosi: embassy_nrf::Peri<'static, AnyPin>,
}

pub struct ImageReceiver {
    pins: ImagePins,
    init: fn(ImagePins) -> Spis<'static>,
}
impl ImageReceiver {
    /// Reserve the image-side aliases; activate only after register SPI is released.
    pub fn reserve(pins: &SpiPins, init: fn(ImagePins) -> Spis<'static>) -> Self {
        // These aliases remain dormant while SPIM owns the pins. The only
        // activation site follows release_for_image_mode and cancellation of
        // every register task; the board callback reclaims the same peripheral.
        // SAFETY: aliases are not activated until the original register driver is released.
        let clone_pin = |pin: &embassy_nrf::Peri<'static, AnyPin>| unsafe {
            AnyPin::steal(pin.port() as u8 * 32 + pin.pin())
        };
        Self {
            pins: ImagePins {
                cs: clone_pin(&pins.cs),
                sck: clone_pin(&pins.sck),
                mosi: clone_pin(&pins.mosi),
            },
            init,
        }
    }
}
pub struct ImageReceivers {
    pub near: ImageReceiver,
    pub wide: ImageReceiver,
}

pub async fn run(
    near: &Mutex<NoopRawMutex, Paj>,
    wide: &Mutex<NoopRawMutex, Paj>,
    receivers: ImageReceivers,
    mut snd: Endpoint<'static, In>,
    mut rcv: Endpoint<'static, Out>,
    usb: UsbConfigHandle,
    start: ImageStart,
) -> ! {
    let mut writer = PacketWriter { snd: &mut snd };
    let mut reader = PacketReader::new(&mut rcv);
    let sensor = if start.config.camera == Port::Nf {
        near
    } else {
        wide
    };
    let receiver = if start.config.camera == Port::Nf {
        receivers.near
    } else {
        receivers.wide
    };
    // No flash write: tracking settings persist unchanged across recovery. This
    // is the established legacy image firmware setup, in native coordinates.
    let prepared = async {
        let mut sensor = sensor.lock().await;
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
            .set_image_exposure_ns(if start.config.camera == Port::Nf {
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
    .await;
    if prepared.is_ok() {
        let mut spis = (receiver.init)(receiver.pins);
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
                        reader.wait_for_packet(),
                        usb.wait_until_inactive(),
                        capture.as_mut(),
                    )
                    .await
                    {
                        Either3::First(Ok(packet)) => {
                            if command(&mut writer, packet, &start, usb).await {
                                break 'capture;
                            }
                        }
                        Either3::First(Err(_)) | Either3::Second(()) => break 'capture,
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
                    reader.wait_for_packet(),
                    usb.wait_until_inactive(),
                    Timer::at(frame_start + period),
                )
                .await
                {
                    Either3::First(Ok(packet)) => {
                        if command(&mut writer, packet, &start, usb).await {
                            break 'capture;
                        }
                    }
                    Either3::First(Err(_)) | Either3::Second(()) => break 'capture,
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
        usb.wait_until_active().await;
        match reader.wait_for_packet().await {
            Ok(packet) => {
                let (generation, code) = match packet.data {
                    PacketData::ImageCommand(ImageCommand::Renew { .. }) => continue,
                    PacketData::ImageCommand(ImageCommand::Stop { generation })
                        if generation != start.generation =>
                    {
                        (generation, ImageStatusCode::Stopped)
                    }
                    PacketData::ImageCommand(
                        ImageCommand::Start { generation, .. } | ImageCommand::Stop { generation },
                    ) => (generation, ImageStatusCode::RebootRequired),
                    _ => (start.generation, ImageStatusCode::RebootRequired),
                };
                status(&mut writer, packet.id, generation, code, usb).await;
            }
            Err(_) => Timer::after_millis(20).await,
        }
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
