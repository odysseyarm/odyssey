#![no_main]
#![no_std]

mod ble;
mod control;
#[cfg(not(feature = "rtt"))]
mod defmt_serial;
mod pairing;
mod storage;
mod usb;

// Initialize RTT logging and panic handler when rtt feature is enabled
#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

use cortex_m_rt::{ExceptionFrame, exception};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::{bind_interrupts, peripherals, usb as nrf_usb};
use embassy_nrf::{peripherals::RNG as NRF_RNG, rng};
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use embassy_usb::driver::{EndpointError, EndpointOut};
// Using trouble-host + nrf-sdc
use protodongers::hub::{DevicePacket, HubMsg, MAX_DEVICES, PairingError, Version};

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
#[allow(dead_code)]
const L2CAP_PSM: u16 = 0x0080; // Dynamic PSM in valid range (0x0080-0x00FF)
use static_cell::StaticCell;

use nrf_sdc::{self as sdc, mpsl};
use rand_core::{CryptoRng, RngCore};
use trouble_host::PacketPool;
use trouble_host::prelude::DefaultPacketPool;

struct SeedRng {
    buf: [u8; 32],
    off: usize,
}
impl SeedRng {
    fn new(buf: [u8; 32]) -> Self {
        Self { buf, off: 0 }
    }
}
impl RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        0
    }
    fn next_u64(&mut self) -> u64 {
        0
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let n = core::cmp::min(dest.len(), 32 - self.off);
        dest[..n].copy_from_slice(&self.buf[self.off..self.off + n]);
        self.off += n;
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl CryptoRng for SeedRng {}

// Type aliases used across tasks
type Controller = nrf_sdc::SoftdeviceController<'static>;
type Pool = DefaultPacketPool;

use crate::ble::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager, DevicePacketChannel};
use crate::storage::Settings;

use core::pin::pin;
use embassy_futures::select::{Either, select};
// Capture scan reports during pairing using a host event handler
struct HostEventHandler;
impl trouble_host::prelude::EventHandler for HostEventHandler {
    fn on_adv_reports(&self, reports: trouble_host::scan::LeAdvReportsIter) {
        if crate::pairing::is_active() {
            for r in reports {
                if let Ok(rep) = r {
                    // Only consider connectable legacy adverts (ADV_IND or ADV_DIRECT_IND)
                    match rep.event_kind {
                        bt_hci::param::LeAdvEventKind::AdvInd
                        | bt_hci::param::LeAdvEventKind::AdvDirectInd => {}
                        _ => continue,
                    }
                    if crate::ble::central::adv_matches_pairing(rep.data) {
                        let raw = rep.addr.raw();
                        let mut addr = [0u8; 6];
                        addr.copy_from_slice(raw);
                        defmt::info!("pairing: adv match (LEGACY) {:02x}", addr);
                        crate::ble::central::report_scan_address(addr);
                        break;
                    }
                }
            }
        }
    }
    fn on_ext_adv_reports(&self, reports: trouble_host::scan::LeExtAdvReportsIter) {
        if crate::pairing::is_active() {
            for r in reports {
                if let Ok(rep) = r {
                    // Only consider connectable extended adverts
                    if !rep.event_kind.connectable() {
                        continue;
                    }
                    if crate::ble::central::adv_matches_pairing(rep.data) {
                        let raw = rep.addr.raw();
                        let mut addr = [0u8; 6];
                        addr.copy_from_slice(raw);
                        defmt::info!("pairing: adv match (EXT) {:02x}", addr);
                        crate::ble::central::report_scan_address(addr);
                        break;
                    }
                }
            }
        }
    }
}
static HOST_HANDLER: HostEventHandler = HostEventHandler;

bind_interrupts!(pub struct Irqs {
    // CLOCK_POWER for both USB VBUS detect and MPSL
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler, mpsl::ClockInterruptHandler;

    // MPSL interrupt handlers
    EGU0_SWI0 => mpsl::LowPrioInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;

    // RNG and USBD
    RNG => rng::InterruptHandler<NRF_RNG>;
    USBD => nrf_usb::InterruptHandler<peripherals::USBD>;
});

// Hardware VBUS detect is used; no SoftwareVbusDetect needed.

const VERSION: Version = Version {
    protocol_semver: [0, 1, 0],
    firmware_semver: [0, 1, 0],
};

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    defmt::info!("Booted");
    let p = embassy_nrf::init(Default::default());

    embassy_time::Timer::after_millis(2000).await;

    // nRF52840-DK LEDs: P0.13, P0.14, P0.15, P0.16 (all white)
    // Using P0.13 as "red" (status), P0.14 as "blue" (pairing)
    let red = p.P0_08;

    let mut red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );

    red.set_low();

    // Default interrupt priorities are fine; no overrides needed.

    // Flash blue LED while pairing is active

    // Initialize USB device and logging
    #[cfg(feature = "rtt")]
    let (ep_in, ep_out, usb_signal) = {
        use static_cell::StaticCell;

        // When using RTT, build USB device without serial logger
        let (builder, ep_in, ep_out, usb_signal) = usb::usb_device(p.USBD);
        let usb_device = builder.build();

        static USB_DEVICE: StaticCell<embassy_usb::UsbDevice<'static, usb::UsbDriver>> =
            StaticCell::new();
        let usb = USB_DEVICE.init(usb_device);

        // Spawn USB device task
        spawner.must_spawn(usb_device_task(usb));

        defmt::info!("RTT logger ready");
        (ep_in, ep_out, usb_signal)
    };

    #[cfg(not(feature = "rtt"))]
    let (ep_in, ep_out, usb_signal) = {
        let (builder, ep_in, ep_out, usb_signal) = usb::usb_device(p.USBD);
        // Initialize USB serial logger (consumes builder and spawns USB device task)
        defmt_serial::init(&mut spawner, builder);
        defmt::info!("USB serial logger initialized");
        (ep_in, ep_out, usb_signal)
    };

    for _ in 0..3 {
        defmt::info!("Hello, World!");
        Timer::after_secs(1).await;
    }

    // Initialize nrf-mpsl (required even without BLE for flash access coordination)
    info!("MPSL initialization");

    // MPSL init
    let mpsl_p =
        mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    // Initialize MPSL with timeslots so nrf_mpsl::Flash can schedule NVMC operations safely.
    static MPSL: StaticCell<mpsl::MultiprotocolServiceLayer> = StaticCell::new();
    static MPSL_SESSION: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
    let session_mem = MPSL_SESSION.init(mpsl::SessionMem::<1>::new());
    let mpsl = MPSL.init(
        mpsl::MultiprotocolServiceLayer::with_timeslots::<
            embassy_nrf::interrupt::typelevel::EGU0_SWI0,
            Irqs,
            1,
        >(mpsl_p, Irqs, lfclk_cfg, session_mem)
        .unwrap(),
    );
    spawner.must_spawn(mpsl_task(mpsl));

    // SDC init
    info!("SDC initialization");
    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
        p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    // Match example scanner minimal memory; adjust if needed by features
    static SDC_MEM: StaticCell<sdc::Mem<5408>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::new());
    use embassy_nrf::mode::Async as RngAsync;
    static RNG: StaticCell<rng::Rng<'static, RngAsync>> = StaticCell::new();
    let rng: &'static mut rng::Rng<'static, RngAsync> = RNG.init(rng::Rng::new(p.RNG, Irqs));
    let mut seed = [0u8; 32];
    rng.fill_bytes(&mut seed).await;
    let sdc = sdc::Builder::new()
        .unwrap()
        .support_scan()
        .unwrap()
        .support_ext_scan()
        .unwrap()
        .support_central()
        .unwrap()
        .support_ext_central()
        .unwrap()
        .support_phy_update_central()
        .unwrap()
        .support_le_2m_phy()
        .unwrap()
        .central_count(1)
        .unwrap()
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            3,
            3,
        )
        .unwrap()
        .build(sdc_p, rng, mpsl, &mut *sdc_mem)
        .unwrap();

    // Host resources and stack
    info!("trouble-host initialization");
    static RESOURCES: StaticCell<trouble_host::HostResources<Pool, { MAX_DEVICES }, 8>> =
        StaticCell::new();
    let resources = RESOURCES.init(trouble_host::HostResources::new());
    static STACK_CELL: StaticCell<trouble_host::Stack<'static, Controller, Pool>> =
        StaticCell::new();
    let mut host_seed_rng = SeedRng::new(seed);
    // Set a static random address like the examples; replace with a stable ID if desired.
    let address = trouble_host::Address::random([0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03]);
    let stack = STACK_CELL.init(
        trouble_host::new(sdc, resources)
            .set_random_address(address)
            .set_random_generator_seed(&mut host_seed_rng),
    );

    // Defer stack.build() until after settings are initialized so we can preload bonds.

    red.set_high();

    // Start pairing LED task (flash "blue" while pairing) - using LED2 (P0.14)
    let blue = embassy_nrf::gpio::Output::new(
        p.P0_14,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    spawner.must_spawn(pairing::pairing_led_task(blue));

    // Initialize settings using MPSL-coordinated flash access
    let flash = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    let settings = storage::init_settings(&spawner, flash).await;

    // Initialize active connections tracker
    let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());

    // Load bonds into cache for reconnection
    ble::security::load_bond_cache(settings).await;

    // Restore stored bonds into the host before building the stack
    {
        let bonds = settings.get_all_bonds().await;
        for opt in bonds.slots.iter() {
            if let Some(bd) = opt {
                let info = ble::security::info_from_bonddata(bd);
                let _ = stack.add_bond_information(info);
            }
        }
    }

    // Now build the host and start its tasks
    let host = stack.build();
    let (rx, control, tx) = host.runner.split();
    spawner.must_spawn(host_rx_task(rx));
    spawner.must_spawn(host_ctrl_task(control));
    spawner.must_spawn(host_tx_task(tx));

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
    // Start BLE manager (central)
    info!("Starting BLE manager task");
    spawner.must_spawn(ble_task(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        host.central,
    ));
    spawner.must_spawn(bond_storage_task(settings));
}

// Hardware VBUS detect is used; no SoftDevice VBUS forwarding needed.

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

                // Add to runtime scan targets so reconnection works without reboot
                let _ = settings.add_scan_target(bond.bd_addr).await;

                // Update the in-memory cache for fast lookup on reconnection
                ble::security::update_bond_cache(bond).await;

                // Send pairing success to USB host
                control::try_send(HubMsg::PairingResult(Ok(bond.bd_addr)));

                // Exit pairing mode now that we've successfully bonded
                pairing::cancel();
                info!("Pairing mode exited after successful bonding");
            }
            Err(()) => {
                defmt::error!("Failed to store bond for device {:02x}", bond.bd_addr);
            }
        }
    }
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static mpsl::MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[cfg(feature = "rtt")]
#[embassy_executor::task]
async fn usb_device_task(usb: &'static mut embassy_usb::UsbDevice<'static, usb::UsbDriver>) -> ! {
    usb.run().await
}

// Type aliases used across tasks

#[embassy_executor::task]
async fn host_rx_task(mut rx: trouble_host::prelude::RxRunner<'static, Controller, Pool>) -> ! {
    defmt::info!("host_rx_task: starting");
    loop {
        match rx.run_with_handler(&HOST_HANDLER).await {
            Ok(_) => defmt::warn!("host_rx_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_rx_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn host_ctrl_task(
    mut ctrl: trouble_host::prelude::ControlRunner<'static, Controller, Pool>,
) -> ! {
    defmt::info!("host_ctrl_task: starting");
    loop {
        match ctrl.run().await {
            Ok(_) => defmt::warn!("host_ctrl_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_ctrl_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn host_tx_task(mut tx: trouble_host::prelude::TxRunner<'static, Controller, Pool>) -> ! {
    defmt::info!("host_tx_task: starting");
    loop {
        match tx.run().await {
            Ok(_) => defmt::warn!("host_tx_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_tx_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn ble_task(
    device_packets: &'static DevicePacketChannel,
    device_queues: &'static ble::central::DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    stack: &'static trouble_host::Stack<'static, Controller, Pool>,
    central: trouble_host::prelude::Central<'static, Controller, Pool>,
) -> ! {
    defmt::info!("ble_task: Creating BleManager");
    let mut ble_manager = BleManager::<Controller, Pool>::new(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        central,
    );
    defmt::info!("ble_task: Starting BleManager run loop");
    ble_manager.run().await
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

        let mut read_buf = [0u8; 512];
        let mut write_buf = [0u8; 256];

        info!("USB task entering main loop");
        loop {
            // Concurrently wait for either a host OUT packet, a device->host packet, or a control event.
            let res = {
                let out_read = pin!(EndpointOut::read_transfer(&mut ep_out, &mut read_buf));
                let dev_recv = pin!(device_packets.receive());
                let ctrl_recv = pin!(control_ch.receive());
                select(out_read, select(dev_recv, ctrl_recv)).await
            };
            match res {
                Either::First(res) => match res {
                    Ok(n) => {
                        info!("USB task: received {} bytes", n);
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
                            error!("Error handling USB RX ({} bytes)", n);
                        }
                    }
                    Err(EndpointError::Disabled) => {
                        info!("USB disabled, waiting for reconnection");
                        break;
                    }
                    Err(_e) => {
                        warn!("USB read error");
                    }
                },
                Either::Second(branch) => match branch {
                    Either::First(pkt) => {
                        if let Err(_e) =
                            send_hub_msg(&HubMsg::DevicePacket(pkt), &mut write_buf, &mut ep_in)
                                .await
                        {
                            warn!("Error sending device packet");
                        }
                    }
                    Either::Second(evt) => {
                        if let Err(_e) = send_hub_msg(&evt, &mut write_buf, &mut ep_in).await {
                            warn!("Error sending control event");
                        }
                    }
                },
            }

            if pairing::take_timeout() {
                let _ = send_hub_msg(
                    &HubMsg::PairingResult(Err(PairingError::Timeout)),
                    &mut write_buf,
                    &mut ep_in,
                )
                .await;
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
    let msg: HubMsg = postcard::from_bytes(data).map_err(|e| {
        error!(
            "Failed to deserialize USB packet ({} bytes): {:?}",
            data.len(),
            defmt::Debug2Format(&e)
        );
        ()
    })?;

    match msg {
        HubMsg::RequestDevices => {
            let devices = active_connections.get_all().await;
            let response = HubMsg::DevicesSnapshot(devices);
            send_hub_msg(&response, write_buf, ep_in).await?;
        }
        HubMsg::SendTo(send_to_msg) => {
            // Route message to the specific device's queue
            let device_uuid = &send_to_msg.dev;
            info!("SendTo message for device {:02x}", device_uuid);
            if let Some(queue) = device_queues.get_queue(device_uuid).await {
                let device_pkt = DevicePacket {
                    dev: send_to_msg.dev,
                    pkt: send_to_msg.pkt,
                };
                queue.send(device_pkt).await;
                info!("Packet queued successfully for device {:02x}", device_uuid);
            } else {
                error!(
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
            send_hub_msg(&HubMsg::StartPairingResponse, write_buf, ep_in).await?;
        }
        HubMsg::CancelPairing => {
            info!("USB CancelPairing received");
            pairing::cancel();
            send_hub_msg(
                &HubMsg::PairingResult(Err(PairingError::Cancelled)),
                write_buf,
                ep_in,
            )
            .await?;
        }
        HubMsg::ClearBonds => {
            info!("USB ClearBonds received");

            // Clear bonds (scan targets will be empty on next boot since they're derived from bonds)
            settings.bonds_clear().await;

            // Also clear runtime scan targets immediately
            settings.scan_list_clear().await;

            // Clear bond cache so they're not used until reboot
            ble::security::clear_bond_cache().await;

            send_hub_msg(&HubMsg::ClearBondsResponse(Ok(())), write_buf, ep_in).await?;
        }
        _ => {
            error!(
                "Unexpected message from host: {:?}",
                defmt::Debug2Format(&msg)
            );
        }
    }

    Ok(())
}

async fn send_hub_msg(
    msg: &HubMsg,
    write_buf: &mut [u8],
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    use embassy_usb::driver::EndpointIn;

    // Encode using postcard (standard for USB in ats_usb)
    let data = postcard::to_slice(msg, write_buf).map_err(|_| ())?;

    // Use write_transfer to handle large packets automatically
    EndpointIn::write_transfer(ep_in, data, true)
        .await
        .map_err(|_| ())?;
    Ok(())
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    // nRF52840-DK: Turn on LED1 (P0.13) to indicate hardfault
    let red = unsafe { embassy_nrf::peripherals::P0_08::steal() };
    let blue = unsafe { embassy_nrf::peripherals::P0_12::steal() };
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
