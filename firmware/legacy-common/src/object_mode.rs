//! Concurrent sensor/control handling. Bulk request assembly survives BLE
//! preemption. A terminating sensor/control error cancels the background loops.
//! PAJ7025 image entry cancels tracking and releases register SPI permanently.
//! Stop/disconnect ends reception. Only reboot/power-cycle restores tracking.
//!
use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::Format;
use embassy_futures::join::join3;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_nrf::ppi::AnyConfigurableChannel;
use embassy_nrf::spim::Error;
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_usb::driver::EndpointError;
use embedded_hal_bus::spi::DeviceError;
use nalgebra::Vector3;
use paj7025::Paj7025Error;

use crate::fodtrigger::FodTrigger;
use crate::image_mode::{ImageReceivers, ImageStart};
use crate::odyssey_protocol::control::device::TransportMode;
use crate::odyssey_protocol::image::{ImageCommand, ImageStatus, ImageStatusCode};
use crate::odyssey_protocol::{
    self, BatteryReport, ConfigKind, Packet, PacketData, PacketType, PropKind, Props,
    ReadRegisterResponse, wire,
};
use crate::platform::{Imu, ImuInterrupt, L2capChannels, L2capReceiver, L2capSender, Platform};
use crate::settings::{PajsSettings, Settings};
use crate::usb::{self, UsbConfigHandle};
use crate::utils::impl_from;
use crate::{Paj, PajGroup};

mod transport;

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;

/// Everything object_mode needs from the board.
///
/// Since: Odyssey Desktop 0.7.0.
pub struct ObjectModeContext<
    'd,
    P: Platform,
    C1: embassy_nrf::gpiote::Channel,
    C2: embassy_nrf::gpiote::Channel,
> {
    /// Since: Odyssey Desktop 0.7.0.
    pub paj7025r2_group: PajGroup<'d, C1>,
    /// Since: Odyssey Desktop 0.7.0.
    pub paj7025r3_group: PajGroup<'d, C2>,
    /// Since: Odyssey Desktop 0.7.0.
    pub fod_set_ch: embassy_nrf::Peri<'d, AnyConfigurableChannel>,
    /// Since: Odyssey Desktop 0.7.0.
    pub fod_clr_ch: embassy_nrf::Peri<'d, AnyConfigurableChannel>,
    /// Since: Odyssey Desktop 0.7.0.
    pub timer: embassy_nrf::Peri<'d, P::Timer>,
    /// Since: Odyssey Desktop 0.7.0.
    pub imu: P::Imu,
    /// Since: Odyssey Desktop 0.7.0.
    pub imu_int: P::ImuInterrupt,
    /// Since: Odyssey Desktop 0.7.0.
    pub usb_snd: Endpoint<'static, In>,
    /// Since: Odyssey Desktop 0.7.0.
    pub usb_rcv: Endpoint<'static, Out>,
    /// Since: Odyssey Desktop 0.7.0.
    pub usb_configured: UsbConfigHandle,
    /// Since: Odyssey Desktop 0.7.0.
    pub settings: &'d mut Settings,
    /// Since: Odyssey Desktop 0.7.0.
    pub l2cap_channels: P::L2capChannels,
    /// Transport that carries reports right now. Requests are accepted from every transport.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub data_mode: fn() -> TransportMode,
    /// Firmware version [major, minor, patch].
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub version: [u32; 3],
    /// Battery state-of-charge percentage (0–100), updated by power_state_task.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub battery_soc: &'static AtomicU8,
    /// Whether VBUS (USB power / charging) is present, updated by power_state_task.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub battery_charging: &'static AtomicBool,
    /// Since: Odyssey Desktop 0.7.0.
    pub image_receivers: ImageReceivers,
    pub ota: atslite_common::ota::OtaHandle,
}

/// Since: Odyssey Desktop 0.7.0.
pub async fn object_mode<
    'd,
    P: Platform,
    C1: embassy_nrf::gpiote::Channel,
    C2: embassy_nrf::gpiote::Channel,
>(
    pid: u16,
    mut ctx: ObjectModeContext<'d, P, C1, C2>,
) -> Result<(), Paj7025Error<DeviceError<Error, Infallible>>> {
    defmt::info!("Entering Object Mode on {}", P::NAME);
    let pkt_writer = PacketWriter {
        snd: &mut ctx.usb_snd,
    };
    let mut pkt_rcv = PacketReader::new(&mut ctx.usb_rcv);
    let pkt_channel = Channel::<Packet, 64>::new();
    let stream_infos = StreamInfos::new();
    let impact_settings = ImpactSettings::new(&ctx.settings.general);
    let near = ctx.paj7025r2_group.0;
    let wide = ctx.paj7025r3_group.0;

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
        ctx.ota,
        ctx.version,
        P::netcore_release,
        &impact_settings,
        P::round_accel_odr,
        P::set_ble_name,
        ctx.data_mode,
    );
    let b = usb_snd_loop(
        pkt_writer,
        pkt_channel.receiver(),
        ctx.usb_configured,
        &ctx.l2cap_channels,
        ctx.data_mode,
    );
    let c = imu_loop::<P>(
        &mut ctx.imu,
        &mut ctx.imu_int,
        pkt_channel.sender(),
        &stream_infos,
        &impact_settings,
    );
    let d = obj_loop(
        ctx.paj7025r2_group,
        ctx.paj7025r3_group,
        ctx.fod_set_ch,
        ctx.fod_clr_ch,
        ctx.timer,
        pkt_channel.sender(),
        &stream_infos,
    );
    let e = battery_loop(
        pkt_channel.sender(),
        &stream_infos,
        ctx.battery_soc,
        ctx.battery_charging,
    );
    // A sensor/control error must cancel the other loops, not wait forever for
    // the background reporting loops to finish.
    let start = match select3(a, d, join3(b, c, e)).await {
        Either3::First(result) => result?,
        Either3::Second(Err(error)) => return Err(error),
        Either3::Second(Ok(never)) => match never {},
        Either3::Third(_) => unreachable!("background reporting loops never return"),
    };
    // select3 has dropped tracking, register request handling and FOD before
    // the irreversible sensor transition. No sensor-register task survives it.
    crate::image_mode::run(
        near,
        wide,
        ctx.image_receivers,
        ctx.usb_snd,
        ctx.usb_rcv,
        ctx.usb_configured,
        start,
        &ctx.l2cap_channels,
        ctx.ota,
    )
    .await
}

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
    ota: atslite_common::ota::OtaHandle,
    version: [u32; 3],
    netcore_release: fn() -> u16,
    impact_settings: &ImpactSettings,
    round_accel_odr: fn(u16) -> u16,
    set_ble_name: fn(&str),
    data_mode: fn() -> TransportMode,
) -> Result<ImageStart, Paj7025Error<DeviceError<Error, Infallible>>> {
    loop {
        // Requests are taken from every transport whichever one carries reports, so neither is
        // parked on a transport choice that has gone stale.
        let (src, pkt) = match select3(
            async {
                loop {
                    match pkt_rcv.wait_for_packet().await {
                        Ok(pkt) => break pkt,
                        Err(WaitForPacketError::Usb(EndpointError::Disabled)) => {
                            defmt::debug!("USB endpoint disabled; waiting for a host...");
                            usb_configured.wait_until_active().await;
                            defmt::debug!("USB host attached; resuming packet handling");
                        }
                        Err(e) => {
                            defmt::error!("Failed to read USB packet: {}", e);
                            embassy_time::Timer::after_millis(100).await;
                        }
                    }
                }
            },
            l2cap_channels.control_rx().receive(),
            l2cap_channels.data_rx().receive(),
        )
        .await
        {
            Either3::First(pkt) => (PacketSource::Usb, pkt),
            Either3::Second(pkt) => (PacketSource::Ble, pkt),
            Either3::Third(pkt) => (PacketSource::Ble, pkt),
        };
        let ble_generation =
            matches!(src, PacketSource::Ble).then(|| l2cap_channels.session_generation());

        let mut reset_after_reply = false;
        let response = match pkt.data {
            PacketData::WriteRegister(register) => {
                let paj = match register.port {
                    odyssey_protocol::Port::Nf => paj7025r2,
                    odyssey_protocol::Port::Wf => paj7025r3,
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
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }
            PacketData::ReadRegister(register) => {
                defmt::debug!("[app] ReadRegister: acquiring paj lock");
                let paj = match register.port {
                    odyssey_protocol::Port::Nf => paj7025r2,
                    odyssey_protocol::Port::Wf => paj7025r3,
                };
                let mut paj = paj.lock().await;
                defmt::debug!(
                    "[app] ReadRegister: lock acquired, reading bank={}, addr={}",
                    register.bank,
                    register.address
                );
                let mut data: [u8; 1] = [0];
                paj.read_register(register.bank, register.address, &mut data)
                    .await?;
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
                let mut wire_config: wire::GeneralConfig = config.into();
                // Round accel_odr to the nearest rate this platform's sensor supports.
                if let wire::GeneralConfig::AccelConfig(ref mut ac) = wire_config {
                    ac.accel_odr = round_accel_odr(ac.accel_odr);
                }
                settings.general.set_general_config(&wire_config);
                // imu_loop reads the impact settings through atomics.
                match &wire_config {
                    wire::GeneralConfig::ImpactThreshold(v) => impact_settings.set_threshold(*v),
                    wire::GeneralConfig::SuppressMs(v) => impact_settings.set_suppress_ms(*v),
                    _ => {}
                }
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }
            PacketData::ReadConfig(kind) => {
                defmt::info!("[app] building ReadConfig response for {:?}", kind);
                let wire_config = match kind {
                    ConfigKind::ImpactThreshold => {
                        wire::GeneralConfig::ImpactThreshold(settings.general.impact_threshold)
                    }
                    ConfigKind::SuppressMs => {
                        wire::GeneralConfig::SuppressMs(settings.general.suppress_ms)
                    }
                    ConfigKind::AccelConfig => {
                        wire::GeneralConfig::AccelConfig(settings.general.accel_config.into())
                    }
                    ConfigKind::GyroConfig => {
                        wire::GeneralConfig::GyroConfig(settings.general.gyro_config.clone())
                    }
                    ConfigKind::CameraModelNf => {
                        wire::GeneralConfig::CameraModelNf(settings.general.camera_model_nf.clone())
                    }
                    ConfigKind::CameraModelWf => {
                        wire::GeneralConfig::CameraModelWf(settings.general.camera_model_wf.clone())
                    }
                    ConfigKind::StereoIso => {
                        wire::GeneralConfig::StereoIso(settings.general.stereo_iso.clone())
                    }
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadConfigResponse(wire_config.into()),
                })
            }
            PacketData::FlashSettings() => {
                defmt::info!(
                    "[app] FlashSettings received. Current impact_threshold={}, suppress_ms={}",
                    settings.general.impact_threshold,
                    settings.general.suppress_ms
                );
                settings.pajs = PajsSettings::read_from_pajs(
                    &mut *paj7025r2.lock().await,
                    &mut *paj7025r3.lock().await,
                )
                .await?;
                settings.write();
                defmt::info!("[app] FlashSettings: settings written to flash");
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }
            PacketData::ReadProp(kind) => {
                let prop = match kind {
                    PropKind::Uuid => Props::Uuid(device_id[..6].try_into().unwrap()),
                    PropKind::ProductId => Props::ProductId(pid),
                    PropKind::Name => Props::Name(settings.general.name.clone().into()),
                    PropKind::Version => Props::Version(odyssey_protocol::Version::new(version)),
                    PropKind::NetCoreRelease => Props::NetCoreRelease(netcore_release()),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ReadPropResponse(prop),
                })
            }
            PacketData::StreamUpdate(s) => {
                use odyssey_protocol::StreamUpdateAction as S;
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
            PacketData::LegacyWriteMode(odyssey_protocol::legacy::Mode::Object) => {
                settings.transient.mode = odyssey_protocol::legacy::Mode::Object;
                settings.transient_write();
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::Ack(),
                })
            }
            PacketData::LegacyWriteMode(odyssey_protocol::legacy::Mode::Image) => Some(Packet {
                id: pkt.id,
                data: PacketData::ImageStatus(ImageStatus {
                    generation: 0,
                    status: ImageStatusCode::Unsupported,
                }),
            }),
            PacketData::ImageCommand(command) => {
                let (generation, status) = match command {
                    ImageCommand::Renew { .. } => continue,
                    ImageCommand::Start { generation, config }
                        if !(1..=15).contains(&config.fps) =>
                    {
                        (generation, ImageStatusCode::CaptureError)
                    }
                    ImageCommand::Start { generation, config }
                        if matches!(src, PacketSource::Usb)
                            && data_mode() == TransportMode::Usb =>
                    {
                        return Ok(ImageStart {
                            id: pkt.id,
                            generation,
                            config,
                        });
                    }
                    ImageCommand::Start { generation, .. } => {
                        (generation, ImageStatusCode::UsbRequired)
                    }
                    ImageCommand::Stop { generation } => (
                        generation,
                        if matches!(src, PacketSource::Usb) {
                            ImageStatusCode::Stopped
                        } else {
                            ImageStatusCode::UsbRequired
                        },
                    ),
                };
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::ImageStatus(ImageStatus { generation, status }),
                })
            }
            PacketData::ReadVersion() => Some(Packet {
                id: pkt.id,
                data: PacketData::ReadVersionResponse(odyssey_protocol::Version::new(version)),
            }),
            PacketData::LegacyObjectReportRequest() => None,
            PacketData::Ack() => None,
            PacketData::ReadConfigResponse(_) => None,
            PacketData::ReadPropResponse(_) => None,
            PacketData::LegacyObjectReport(_) => None,
            PacketData::LegacyCombinedMarkersReport(_) => None,
            PacketData::MarkersReport(_) => None,
            PacketData::AccelReport(_) => None,
            PacketData::ImpactReport(_) => None,
            PacketData::Vendor(tag, data)
                if tag == odyssey_protocol::ota::VENDOR_TAG
                    && matches!(src, PacketSource::Ble) =>
            {
                let len = usize::from(data.len);
                let Some(payload) = data.data.get(..len) else {
                    continue;
                };
                if let Some(reply) = ota.request(payload).await {
                    reset_after_reply = reply.reset;
                    let mut body = odyssey_protocol::VendorData {
                        len: odyssey_protocol::ota::STATUS_LEN as u8,
                        data: [0; odyssey_protocol::VendorData::CAPACITY],
                    };
                    body.data[..odyssey_protocol::ota::STATUS_LEN]
                        .copy_from_slice(&reply.status);
                    Some(Packet {
                        id: pkt.id,
                        data: PacketData::Vendor(tag, body),
                    })
                } else {
                    None
                }
            }
            PacketData::Vendor(_, _) => None,
            PacketData::ReadRegisterResponse(_) => None,
            PacketData::ReadVersionResponse(_) => None,
            PacketData::BatteryReport(_) => None,
            PacketData::ImageStatus(_) => None,
            PacketData::ImageChunk(_) => None,
            PacketData::ImageObject(_) => None,
            PacketData::SetDeviceName(name) => {
                defmt::info!("[object_mode] SetDeviceName: {:?}", name.as_str());
                set_ble_name(name.as_str());
                settings.general.name = name.into();
                settings.write();
                Some(Packet {
                    id: pkt.id,
                    data: PacketData::SetDeviceNameResponse(
                        odyssey_protocol::SetDeviceNameResult::Ok(),
                    ),
                })
            }
            PacketData::SetDeviceNameResponse(_) => None,
        };

        if let Some(r) = response
            && r.id != 255
        {
            match src {
                PacketSource::Usb => {
                    pkt_snd.send(r).await;
                }
                PacketSource::Ble => {
                    // Sensor I/O can span a disconnect/reconnect. Never hand
                    // the new central a response belonging to the old session.
                    if ble_generation != Some(l2cap_channels.session_generation()) {
                        if reset_after_reply {
                            atslite_common::settings::settings_wait_settled(
                                embassy_time::Duration::from_millis(500),
                            )
                            .await;
                            cortex_m::peripheral::SCB::sys_reset();
                        }
                        continue;
                    }
                    if r.is_control() {
                        if reset_after_reply {
                            l2cap_channels.control_tx().send(r).await;
                        } else if l2cap_channels.control_tx().try_send(r).is_err() {
                            defmt::warn!("[app] Dropped control response - channel full");
                        }
                    } else if reset_after_reply {
                        l2cap_channels.data_tx().send(r).await;
                    } else if l2cap_channels.data_tx().try_send(r).is_err() {
                        defmt::warn!("[app] Dropped data response - channel full");
                    }
                }
            }
        }
        if reset_after_reply {
            embassy_time::Timer::after_millis(100).await;
            atslite_common::settings::settings_wait_settled(embassy_time::Duration::from_millis(500))
                .await;
            cortex_m::peripheral::SCB::sys_reset();
        }
    }
}

/// Sends reports over `data_mode()` and USB replies only to their original host.
/// A USB write in progress is abandoned when USB goes away, so reports move to
/// BLE without waiting for a stalled transfer.
async fn usb_snd_loop<L: L2capChannels>(
    mut pkt_writer: PacketWriter<'_>,
    pkt_rcv: Receiver<'_, Packet, 64>,
    usb_configured: UsbConfigHandle,
    l2cap_channels: &L,
    data_mode: fn() -> TransportMode,
) -> ! {
    loop {
        let pkt = pkt_rcv.receive().await;
        // BLE replies bypass this queue. All queued control packets belong to USB.
        let Some(transport) = transport::queued_transport(&pkt, data_mode(), usb::is_active())
        else {
            continue;
        };
        match transport {
            TransportMode::Usb => {
                let write = pkt_writer.write_packet(&pkt);
                match select(usb_configured.wait_until_inactive(), write).await {
                    Either::Second(Ok(())) => {}
                    Either::Second(Err(error)) => {
                        defmt::warn!("[usb_snd_loop] USB write failed: {:?}", error);
                    }
                    Either::First(()) => {
                        defmt::debug!("[usb_snd_loop] USB went away mid-write, packet dropped");
                    }
                }
            }
            TransportMode::Ble => {
                if pkt.is_control() {
                    let _ = l2cap_channels.control_tx().try_send(pkt);
                } else {
                    let _ = l2cap_channels.data_tx().try_send(pkt);
                }
            }
        }
    }
}

async fn battery_loop(
    pkt_snd: Sender<'_, Packet, 64>,
    stream_infos: &StreamInfos,
    battery_soc: &'static AtomicU8,
    battery_charging: &'static AtomicBool,
) -> ! {
    loop {
        embassy_time::Timer::after_secs(1).await;
        if stream_infos.battery.enabled() {
            let percent = battery_soc.load(Ordering::Relaxed);
            let charging = battery_charging.load(Ordering::Relaxed);
            let id = stream_infos.battery.req_id();
            let pkt = Packet {
                id,
                data: PacketData::BatteryReport(BatteryReport { percent, charging }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("[object_mode] Battery channel full, dropping packet");
            }
        }
    }
}

async fn imu_loop<P: Platform>(
    imu: &mut P::Imu,
    imu_int: &mut P::ImuInterrupt,
    pkt_snd: Sender<'_, Packet, 64>,
    stream_infos: &StreamInfos,
    impact_settings: &ImpactSettings,
) {
    const G: f32 = 9.806_65;
    const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

    // Last impact time, for the suppression window.
    let mut last_impact_ts_micros: Option<u32> = None;

    loop {
        imu_int.wait().await;

        let imu_data = match imu.read_data().await {
            Ok(data) => data,
            Err(_error) => {
                defmt::error!("Failed to read IMU data");
                continue;
            }
        };

        let acc = imu_data.accel;
        let gyr = imu_data.gyro;
        let ts_micros = imu_data.timestamp_micros;

        let acc_transformed = P::transform_accel(acc);
        let gyr_transformed = P::transform_gyro(gyr);

        let accel = Vector3::new(acc_transformed[0], acc_transformed[1], acc_transformed[2])
            .cast::<f32>()
            / P::ACC_LSB_PER_G
            * G;

        let gx = (gyr_transformed[0] as f32 / P::GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gy = (gyr_transformed[1] as f32 / P::GYRO_LSB_PER_DPS) * DEG_TO_RAD;
        let gz = (gyr_transformed[2] as f32 / P::GYRO_LSB_PER_DPS) * DEG_TO_RAD;

        if stream_infos.imu.enabled() {
            let id = stream_infos.imu.req_id();
            let pkt = Packet {
                id,
                data: PacketData::AccelReport(odyssey_protocol::AccelReport {
                    timestamp: ts_micros,
                    accel,
                    gyro: Vector3::new(gx, gy, gz),
                }),
            };
            if pkt_snd.try_send(pkt).is_err() {
                defmt::warn!("Failed to send IMU data due to full channel");
            }
        }

        // Impact threshold is in m/s^2.
        if stream_infos.impact.enabled() {
            let accel_magnitude_ms2 = accel.magnitude();

            let threshold_ms2 = impact_settings.threshold() as f32;
            if accel_magnitude_ms2 >= threshold_ms2 {
                let suppress_us = (impact_settings.suppress_ms() as u32) * 1000;
                let should_send = match last_impact_ts_micros {
                    Some(last_ts) => {
                        let elapsed = ts_micros.wrapping_sub(last_ts);
                        elapsed >= suppress_us
                    }
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
                        defmt::warn!("Failed to send impact report due to full channel");
                    }
                }
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

    // TODO: starting the trigger freezes everything when used with the raw USB driver.
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
                data: PacketData::LegacyCombinedMarkersReport(
                    odyssey_protocol::legacy::CombinedMarkersReport {
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
                    },
                ),
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

/// Since: Odyssey Desktop 0.7.0.
pub struct PacketReader<'a> {
    rcv: &'a mut Endpoint<'static, Out>,
    transfer: usb::TransferReader<1024>,
}

impl<'a> PacketReader<'a> {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn new(rcv: &'a mut Endpoint<'static, Out>) -> Self {
        Self {
            rcv,
            transfer: usb::TransferReader::new(),
        }
    }

    /// # Errors
    ///
    /// Returns USB endpoint errors or postcard decoding errors.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn wait_for_packet(&mut self) -> Result<Packet, WaitForPacketError> {
        defmt::debug!("Read transfer start");
        let data = self.transfer.read(self.rcv).await?;
        defmt::debug!("Read transfer done");
        Ok(postcard::from_bytes(data)?)
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub struct PacketWriter<'a> {
    pub(crate) snd: &'a mut Endpoint<'static, In>,
}

impl PacketWriter<'_> {
    /// # Errors
    ///
    /// Returns postcard encoding errors or USB endpoint errors.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn write_packet(&mut self, pkt: &Packet) -> Result<(), WaitForPacketError> {
        let payload = postcard::to_vec::<_, 1024>(pkt)?;
        defmt::debug!(
            "[USB] write_packet: payload_len={}, pkt_id={}",
            payload.len(),
            pkt.id
        );
        usb::write_transfer(self.snd, &payload, true).await?;
        Ok(())
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Format)]
pub enum WaitForPacketError {
    /// Since: Odyssey Desktop 0.7.0.
    Postcard(postcard::Error),
    /// Since: Odyssey Desktop 0.7.0.
    Packet(odyssey_protocol::Error),
    /// Since: Odyssey Desktop 0.7.0.
    Usb(EndpointError),
}

impl_from!(WaitForPacketError::Postcard(postcard::Error));
impl_from!(WaitForPacketError::Packet(odyssey_protocol::Error));
impl_from!(WaitForPacketError::Usb(EndpointError));

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
        defmt::info!("Enable {}, id = {=u8}", ty, req_id);
        let si = match ty {
            PacketType::AccelReport() => &self.imu,
            PacketType::ImpactReport() => &self.impact,
            PacketType::LegacyCombinedMarkersReport() => &self.marker,
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
            PacketType::LegacyCombinedMarkersReport() => &self.marker,
            PacketType::BatteryReport() => &self.battery,
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

struct ImpactSettings {
    threshold: AtomicU8,
    suppress_ms: AtomicU8,
}

impl ImpactSettings {
    fn new(settings: &crate::settings::GeneralSettings) -> Self {
        Self {
            threshold: AtomicU8::new(settings.impact_threshold),
            suppress_ms: AtomicU8::new(settings.suppress_ms),
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
