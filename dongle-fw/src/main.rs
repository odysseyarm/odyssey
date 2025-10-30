#![no_main]
#![no_std]

mod ble;
mod control;
mod defmt_serial;
mod pairing;
mod storage;
mod usb;

use cortex_m_rt::{ExceptionFrame, exception};
use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt::InterruptExt as _;
use embassy_nrf::usb::vbus_detect::{HardwareVbusDetect, SoftwareVbusDetect};
use embassy_nrf::{bind_interrupts, interrupt, peripherals, usb as nrf_usb};
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use embassy_usb::driver::{EndpointError, EndpointIn, EndpointOut};
use nrf_softdevice::{self as sd, SocEvent, Softdevice};
use protodongers::hub::{HubMsg, MAX_DEVICES, Version};

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
#[allow(dead_code)]
const L2CAP_PSM: u16 = 0x0080; // Dynamic PSM in valid range (0x0080-0x00FF)
use static_cell::StaticCell;

use crate::ble::{
    ACTIVE_CONNECTIONS, ActiveConnections, BleManager, DevicePacketChannel, DongleSecurityHandler,
    L2capPacket,
};
use crate::storage::{Settings, softdevice_ready};
use nrf_softdevice::ble::l2cap;

bind_interrupts!(struct Irqs {
    USBD => nrf_usb::InterruptHandler<peripherals::USBD>;
});

static VBUS_DETECT: StaticCell<SoftwareVbusDetect> = StaticCell::new();

const VERSION: Version = Version {
    protocol_semver: [0, 1, 0],
    firmware_semver: [0, 1, 0],
};

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    let x = VBUS_DETECT.init_with(|| SoftwareVbusDetect::new(true, true));

    defmt::info!("Booted");
    let mut c = embassy_nrf::config::Config::default();
    c.gpiote_interrupt_priority = interrupt::Priority::P2;
    c.time_interrupt_priority = interrupt::Priority::P2;
    let p = embassy_nrf::init(c);

    // let red = p.P0_08;
    let red = p.P0_06;

    let mut red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );

    red.set_low();

    interrupt::USBD.set_priority(interrupt::Priority::P2);
    interrupt::GPIOTE.set_priority(interrupt::Priority::P2);

    // Flash blue LED while pairing is active

    // Initialize USB
    let (builder, ep_in, ep_out, usb_signal) = usb::usb_device(p.USBD, x);

    // Initialize global logger
    defmt_serial::init(&mut spawner, builder);
    defmt::info!("Successfully initialized");
    for _ in 0..3 {
        defmt::info!("Hello, World!");
        Timer::after_secs(1).await;
    }

    let config = nrf_softdevice::Config {
        clock: Some(sd::raw::nrf_clock_lf_cfg_t {
            source: sd::raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: sd::raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        conn_gap: Some(sd::raw::ble_gap_conn_cfg_t {
            conn_count: 7,
            event_length: 24,
        }),
        conn_gatt: Some(sd::raw::ble_gatt_conn_cfg_t { att_mtu: 256 }),
        gatts_attr_tab_size: Some(sd::raw::ble_gatts_cfg_attr_tab_size_t {
            attr_tab_size: sd::raw::BLE_GATTS_ATTR_TAB_SIZE_DEFAULT,
        }),
        gap_role_count: Some(sd::raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: 0,
            central_role_count: 7,
            // Allow up to 7 concurrent secured central links
            central_sec_count: 7,
            _bitfield_1: sd::raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        gap_device_name: Some(sd::raw::ble_gap_cfg_device_name_t {
            p_value: b"HelloRust" as *const u8 as _,
            current_len: 9,
            max_len: 9,
            write_perm: unsafe { core::mem::zeroed() },
            _bitfield_1: sd::raw::ble_gap_cfg_device_name_t::new_bitfield_1(
                sd::raw::BLE_GATTS_VLOC_STACK as u8,
            ),
        }),
        ..Default::default()
    };

    let sd = Softdevice::enable(&config);
    spawner.must_spawn(softdevice_task(sd, x));

    red.set_high();

    // Start pairing LED task (flashing blue while pairing)

    // Initialize L2CAP driver
    static L2CAP: StaticCell<l2cap::L2cap<L2capPacket>> = StaticCell::new();
    let l2cap = L2CAP.init(l2cap::L2cap::init(sd));

    // Get peripherals after SoftDevice is enabled
    let _p = unsafe { embassy_nrf::Peripherals::steal() };

    // Initialize settings with NVMC flash
    let flash_dev = sd::Flash::take(sd);
    let settings = storage::init_settings(&spawner, flash_dev).await;

    // Initialize active connections tracker
    let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());

    // Load bonds into cache for reconnection
    ble::security::load_bond_cache(settings).await;

    // Initialize security handler
    static SECURITY_HANDLER: StaticCell<DongleSecurityHandler> = StaticCell::new();
    let security_handler = SECURITY_HANDLER.init(DongleSecurityHandler::new(settings));

    // Initialize channels for BLE <-> USB communication
    static DEVICE_PACKETS: StaticCell<DevicePacketChannel> = StaticCell::new();
    let device_packets = DEVICE_PACKETS.init(Channel::new());
    let control_ch = control::init();
    control::register(control_ch);

    // Initialize per-device queue registry
    use ble::central::DEVICE_QUEUES;
    let device_queues = DEVICE_QUEUES.init(ble::central::DeviceQueues::new());

    spawner.must_spawn(usb_task(
        ep_in,
        ep_out,
        usb_signal,
        device_packets,
        control_ch,
        device_queues,
        active_connections,
        settings,
    ));
    spawner.must_spawn(ble_task(
        sd,
        device_packets,
        device_queues,
        settings,
        active_connections,
        security_handler,
        l2cap,
    ));
    spawner.must_spawn(bond_storage_task(settings));
}

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice, vbus_detect: &'static SoftwareVbusDetect) {
    info!("SoftDevice task started");
    // Signal to storage worker that SoftDevice is ready; flash ops are safe
    softdevice_ready();

    unsafe {
        sd::raw::sd_power_usbremoved_enable(1);
        sd::raw::sd_power_usbdetected_enable(1);
        sd::raw::sd_power_usbpwrrdy_enable(1);
    }

    sd.run_with_callback(|event: SocEvent| {
        info!("SoftDevice event: {:?}", event);

        match event {
            SocEvent::PowerUsbRemoved => vbus_detect.detected(false),
            SocEvent::PowerUsbDetected => vbus_detect.detected(true),
            SocEvent::PowerUsbPowerReady => vbus_detect.ready(),
            _ => {}
        };
    })
    .await;
}

#[embassy_executor::task]
async fn bond_storage_task(settings: &'static Settings) -> ! {
    use ble::security::BOND_TO_STORE;
    use defmt::info;

    loop {
        // Wait for a bond to be stored
        let bond = BOND_TO_STORE.wait().await;

        info!("Storing bond for device {:02x}...", bond.bd_addr);

        // Store the bond to flash
        match settings.add_bond(bond).await {
            Ok(()) => {
                info!("Bond stored successfully for device {:02x}", bond.bd_addr);

                // Update the in-memory cache for fast lookup on reconnection
                ble::security::update_bond_cache(bond).await;
            }
            Err(()) => {
                defmt::error!("Failed to store bond for device {:02x}", bond.bd_addr);
            }
        }
    }
}

#[embassy_executor::task]
async fn ble_task(
    sd: &'static Softdevice,
    device_packets: &'static DevicePacketChannel,
    device_queues: &'static ble::central::DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    security_handler: &'static DongleSecurityHandler,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
) -> ! {
    let mut ble_manager = BleManager::new(
        device_packets,
        device_queues,
        settings,
        active_connections,
        security_handler,
        l2cap,
    );
    ble_manager.run(sd).await
}

#[embassy_executor::task]
async fn usb_task(
    mut ep_in: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
    mut ep_out: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::Out>,
    usb_signal: &'static embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::ThreadModeRawMutex,
        bool,
    >,
    device_packets: &'static DevicePacketChannel,
    control_ch: &'static embassy_sync::channel::Channel<
        embassy_sync::blocking_mutex::raw::ThreadModeRawMutex,
        HubMsg,
        4,
    >,
    device_queues: &'static ble::central::DeviceQueues,
    active_connections: &'static ActiveConnections,
    settings: &'static Settings,
) {
    loop {
        // Wait for USB to be configured
        usb_signal.wait().await;
        info!("USB configured");

        let mut read_buf = [0u8; 256];
        let mut write_buf = [0u8; 256];

        loop {
            // Handle incoming USB data (host -> dongle -> devices)
            match EndpointOut::read(&mut ep_out, &mut read_buf).await {
                Ok(n) => {
                    if let Err(_e) = handle_usb_rx(
                        &read_buf[..n],
                        &mut write_buf,
                        device_queues,
                        active_connections,
                        settings,
                        &mut ep_in,
                    )
                    .await
                    {
                        warn!("Error handling USB RX");
                    }
                }
                Err(EndpointError::Disabled) => {
                    info!("USB disabled, waiting for reconnection");
                    break;
                }
                Err(_e) => {
                    warn!("USB read error");
                }
            }

            // Handle outgoing device data (devices -> dongle -> host)
            if let Ok(pkt) = device_packets.try_receive() {
                if let Err(_e) =
                    send_hub_msg(&HubMsg::DevicePacket(pkt), &mut write_buf, &mut ep_in).await
                {
                    warn!("Error sending device packet");
                }
            }
            // Handle control events
            if let Ok(evt) = control_ch.try_receive() {
                if let Err(_e) = send_hub_msg(&evt, &mut write_buf, &mut ep_in).await {
                    warn!("Error sending control event");
                }
            }
            // Emit timeout events
            if pairing::take_timeout() {
                let _ = send_hub_msg(&HubMsg::PairingTimeout, &mut write_buf, &mut ep_in).await;
            }
        }
    }
}

async fn handle_usb_rx(
    data: &[u8],
    write_buf: &mut [u8],
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
    settings: &Settings,
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    // Decode using postcard (standard for USB in ats_usb)
    let msg: HubMsg = postcard::from_bytes(data).map_err(|_| ())?;

    match msg {
        HubMsg::RequestDevices => {
            let devices = active_connections.get_all().await;
            let response = HubMsg::DevicesSnapshot(devices);
            send_hub_msg(&response, write_buf, ep_in).await?;
        }
        HubMsg::SendTo(send_to_msg) => {
            // Route message to the specific device's queue
            let device_uuid = &send_to_msg.dev;
            if let Some(queue) = device_queues.get_queue(device_uuid).await {
                queue.send(send_to_msg.pkt).await;
            } else {
                warn!(
                    "No queue found for device {:02x} - device not connected",
                    device_uuid
                );
            }
        }
        HubMsg::ReadVersion() => {
            let response = HubMsg::ReadVersionResponse(VERSION);
            send_hub_msg(&response, write_buf, ep_in).await?;
        }
        HubMsg::StartPairing(start) => {
            info!("USB StartPairing received, timeout_ms={}", start.timeout_ms);
            let timeout = embassy_time::Duration::from_millis(start.timeout_ms as u64);
            pairing::enter_with_timeout(timeout);
            send_hub_msg(&HubMsg::PairingStarted, write_buf, ep_in).await?;
        }
        HubMsg::CancelPairing => {
            info!("USB CancelPairing received");
            pairing::cancel();
            send_hub_msg(&HubMsg::PairingCancelled, write_buf, ep_in).await?;
        }
        _ => {
            warn!("Unexpected message from host");
        }
    }

    Ok(())
}

async fn send_hub_msg(
    msg: &HubMsg,
    write_buf: &mut [u8],
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    // Encode using postcard (standard for USB in ats_usb)
    let data = postcard::to_slice(msg, write_buf).map_err(|_| ())?;
    EndpointIn::write(ep_in, data).await.map_err(|_| ())?;
    Ok(())
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    // let red = unsafe { embassy_nrf::peripherals::P0_08::steal() };
    let red = unsafe { embassy_nrf::peripherals::P0_06::steal() };
    // let blue = unsafe { embassy_nrf::peripherals::P0_12::steal() };
    let blue = unsafe { embassy_nrf::peripherals::P0_08::steal() };
    let _blue = embassy_nrf::gpio::Output::new(
        blue,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    let _red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::Low,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    loop {}
}
