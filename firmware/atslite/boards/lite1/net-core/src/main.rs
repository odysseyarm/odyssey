#![no_std]
#![no_main]
#![recursion_limit = "256"]

mod cmd_dispatch;

use core::cell::RefCell;

use bt_hci::cmd::{self, Opcode, OpcodeGroup};
use bt_hci::data::{AclPacketHeader, IsoPacketHeader, SyncPacketHeader};
use bt_hci::event::EventPacketHeader;
use bt_hci::{FromHciBytes, FromHciBytesError, PacketKind, param};
use defmt::{Debug2Format, unwrap};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::interrupt::{InterruptExt as _, Priority};
use embassy_nrf::ipc::{self, Ipc, IpcChannel};
use embassy_nrf::mode::Async;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals::{self, RNG};
use embassy_nrf::rng::{self, Rng};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Delay;
use embedded_storage::nor_flash::NorFlash;
use icmsg::{IcMsg, Notifier, WaitForNotify};
use nrf_sdc::{self as sdc, mpsl};
use panic_probe as _;
use sdc::mpsl::MultiprotocolServiceLayer;
use static_cell::StaticCell;

use crate::cmd_dispatch::exec_cmd_by_opcode;
use crate::icmsg_config::ALIGN;

/// Release counter of this net-core image. Release tooling greps this exact line; keep it on one
/// line. 0 is reserved for "unknown" on the host side.
macro_rules! release {
    () => {
        1
    };
}

const RELEASE: u16 = release!();
const _: () = assert!(RELEASE != 0);

/// Vendor-specific HCI command (OGF 0x3F, OCF 0x3F0) that returns [`RELEASE`]. The net core answers
/// it itself; the SoftDevice Controller never sees it.
const OPCODE_READ_RELEASE: u16 = 0xFFF0;

/// H4 Command Complete event for [`OPCODE_READ_RELEASE`]: indicator 0x04, event 0x0E, plen 6,
/// one command credit, the opcode, status 0 and the release counter little-endian.
const fn read_release_reply(release: u16) -> [u8; 9] {
    let op = OPCODE_READ_RELEASE.to_le_bytes();
    let rel = release.to_le_bytes();
    [0x04, 0x0E, 0x06, 0x01, op[0], op[1], 0x00, rel[0], rel[1]]
}

embassy_nrf::bind_interrupts!(struct Irqs {
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
    RNG => rng::InterruptHandler<RNG>;
    EGU0 => mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => mpsl::ClockInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;
});

mod icmsg_config {
    unsafe extern "C" {
        static mut __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static mut __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    pub const ALIGN: usize = 4;
    pub fn get_icmsg_config() -> icmsg::MemoryConfig {
        unsafe {
            let send_buffer_len =
                (&raw const __icmsg_tx_end).byte_offset_from(&raw const __icmsg_tx_start) as u32
                    - size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
            let recv_buffer_len =
                (&raw const __icmsg_rx_end).byte_offset_from(&raw const __icmsg_rx_start) as u32
                    - size_of::<icmsg::transport::SharedMemoryRegionHeader<ALIGN>>() as u32;
            icmsg::MemoryConfig {
                send_region: (&raw mut __icmsg_tx_start).cast(),
                recv_region: (&raw mut __icmsg_rx_start).cast(),
                send_buffer_len,
                recv_buffer_len,
            }
        }
    }
}

/// How many outgoing L2CAP buffers per link
const L2CAP_TXQ: u8 = 6;

/// How many incoming L2CAP buffers per link
const L2CAP_RXQ: u8 = 6;

/// Size of L2CAP packets
const L2CAP_MTU: usize = 251;

fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut rng::Rng<Async>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_adv()?
        .support_peripheral()?
        .support_phy_update_peripheral()?
        .support_le_2m_phy()?
        .peripheral_count(2)? // Allow peripheral conn + transition overlap
        .buffer_cfg(L2CAP_MTU as u16, L2CAP_MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
        .build(p, rng, mpsl, mem)
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

const IRQS: &'static [embassy_nrf::interrupt::Interrupt] = {
    use embassy_nrf::interrupt::Interrupt::*;
    &[
        CLOCK_POWER,
        RADIO,
        RNG,
        GPIOTE,
        WDT,
        TIMER0,
        ECB,
        AAR_CCM,
        TEMP,
        RTC0,
        IPC,
        SERIAL0,
        EGU0,
        RTC1,
        TIMER1,
        TIMER2,
        SWI0,
        SWI1,
        SWI2,
        SWI3,
    ]
};

fn set_irq_prios(p: Priority) {
    for irq in IRQS {
        irq.set_priority(p);
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    set_irq_prios(Priority::P2);
    let mut config = embassy_nrf::config::Config::default();
    config.gpiote_interrupt_priority = Priority::P2;
    config.time_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(config);

    // Mark firmware as booted by erasing the state page.
    static NVMC: StaticCell<Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>> = StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    clear_boot_state(nvmc);

    let mut ipc = Ipc::new(p.IPC, Irqs);
    ipc.event0.configure_trigger([IpcChannel::Channel0]);
    ipc.event0.configure_wait([IpcChannel::Channel1]);

    let icmsg_config = icmsg_config::get_icmsg_config();
    defmt::info!("{:?}", Debug2Format(&icmsg_config));
    let icmsg = unsafe {
        IcMsg::<_, _, { icmsg_config::ALIGN }>::init(
            icmsg_config::get_icmsg_config(),
            IpcNotify {
                trigger: ipc.event0.trigger_handle(),
            },
            IpcWait { event: ipc.event0 },
            Delay,
        )
        .await
    };
    let icmsg = match icmsg {
        Err(e) => {
            defmt::error!("error: {:?}", Debug2Format(&e));
            return;
        }
        Ok(icmsg) => {
            defmt::info!("Connected!");
            icmsg
        }
    };

    // sdc nonsense
    let mpsl_p = mpsl::Peripherals::new(
        p.RTC0, p.TIMER0, p.TIMER1, p.TEMP, p.PPI_CH0, p.PPI_CH1, p.PPI_CH2,
    );
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(
        mpsl_p, Irqs, lfclk_cfg
    )));
    spawner.must_spawn(mpsl_task(&*mpsl));

    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH3, p.PPI_CH4, p.PPI_CH5, p.PPI_CH6, p.PPI_CH7, p.PPI_CH8, p.PPI_CH9, p.PPI_CH10,
        p.PPI_CH11, p.PPI_CH12,
    );

    static RNG_CELL: StaticCell<Rng<Async>> = StaticCell::new();
    let rng = RNG_CELL.init(Rng::new(p.RNG, Irqs));

    static SDC_MEM: StaticCell<sdc::Mem<11760>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::<11760>::new());
    static SDC_CELL: StaticCell<sdc::SoftdeviceController> = StaticCell::new();
    let sdc = SDC_CELL.init(unwrap!(build_sdc(sdc_p, rng, mpsl, sdc_mem)));

    let (send, recv) = icmsg.split();

    static SEND: StaticCell<
        Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    > = StaticCell::new();
    let send = SEND.init(Mutex::new(RefCell::new(send)));
    spawner.must_spawn(receive_task(send, recv, sdc));

    const OUT_MAX: usize = nrf_sdc::raw::HCI_MSG_BUFFER_MAX_SIZE as usize + 1;
    let mut buffer = [0u8; OUT_MAX];

    loop {
        let pkt_data = &mut buffer[1..];
        let kind = unwrap!(sdc.hci_get(pkt_data).await);
        let used = unwrap!(packet_len(kind, pkt_data));

        assert!(1 + used <= buffer.len());

        buffer[0] = kind as u8;
        let n = 1 + used;

        defmt::trace!("send {}: {=[u8]:x}", n, &buffer[..n]);
        send.lock(|x| x.borrow_mut().send(&buffer[..n]).unwrap());
    }
}

fn packet_len(kind: PacketKind, data: &[u8]) -> Result<usize, FromHciBytesError> {
    Ok(match kind {
        PacketKind::Cmd => {
            if data.len() < 3 {
                return Err(FromHciBytesError::InvalidSize);
            } else {
                data[2] as usize + 3
            }
        }
        PacketKind::AclData => AclPacketHeader::from_hci_bytes(data)?.0.data_len() + 4,
        PacketKind::SyncData => SyncPacketHeader::from_hci_bytes(data)?.0.data_len() + 3,
        PacketKind::Event => EventPacketHeader::from_hci_bytes(data)?.0.params_len as usize + 2,
        PacketKind::IsoData => IsoPacketHeader::from_hci_bytes(data)?.0.data_load_len() + 4,
    })
}

type CmdErr = cmd::Error<sdc::Error>;

fn parse_cmd_after_h4(rest: &[u8]) -> Result<(Opcode, &[u8]), CmdErr> {
    if rest.len() < 3 {
        return Err(cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS));
    }
    let raw = u16::from_le_bytes([rest[0], rest[1]]);
    let ogf = OpcodeGroup::new(((raw >> 10) & 0x3F) as u8);
    let ocf = raw & 0x03FF;
    let plen = rest[2] as usize;
    if 3 + plen > rest.len() {
        return Err(cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS));
    }
    Ok((Opcode::new(ogf, ocf), &rest[3..3 + plen]))
}

macro_rules! hci {
    ($e:expr) => {
        $e.map_err(|_| cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS))
    };
}
macro_rules! io {
    ($e:expr) => {
        $e.map_err(cmd::Error::Io)
    };
}

async fn exec_h4_to_sdc(
    send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    sdc: &sdc::SoftdeviceController<'static>,
    pkt: &[u8],
) -> Result<(), CmdErr> {
    let (kind, rest) = hci!(bt_hci::PacketKind::from_hci_bytes(pkt))?;
    match kind {
        bt_hci::PacketKind::Cmd => {
            let (opcode, payload) = parse_cmd_after_h4(rest)?;
            defmt::debug!("opcode {}", opcode);
            if opcode.to_raw() == OPCODE_READ_RELEASE {
                let reply = read_release_reply(RELEASE);
                return send
                    .lock(|x| x.borrow_mut().send(&reply))
                    .map_err(|_| cmd::Error::Io(sdc::Error::EIO));
            }
            exec_cmd_by_opcode::<sdc::Error>(send, sdc, opcode, payload).await
        }
        bt_hci::PacketKind::AclData => io!(sdc.hci_data_put(rest)),
        bt_hci::PacketKind::IsoData => io!(sdc.hci_iso_data_put(rest)),
        bt_hci::PacketKind::SyncData => Err(cmd::Error::Hci(param::Error::UNSUPPORTED)),
        bt_hci::PacketKind::Event => Err(cmd::Error::Hci(param::Error::UNSPECIFIED)),
    }
}

#[embassy_executor::task]
async fn receive_task(
    send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    mut recv: icmsg::Receiver<IpcWait<'static>, ALIGN>,
    sdc: &'static sdc::SoftdeviceController<'static>,
) {
    let mut buf = [0; nrf_sdc::raw::HCI_MSG_BUFFER_MAX_SIZE as usize];
    loop {
        let n = match recv.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                defmt::error!("Recv error: {:?}", defmt::Debug2Format(&e));
                return;
            }
        };
        defmt::info!("Received {} bytes: {=[u8]:x}", n, &buf[..n]);

        if let Err(e) = exec_h4_to_sdc(send, sdc, &buf[..n]).await {
            defmt::warn!("TX->SDC error: {:?}", e);
        }
    }
}

struct IpcNotify<'d> {
    trigger: ipc::EventTrigger<'d>,
}

struct IpcWait<'d> {
    event: ipc::Event<'d>,
}

impl Notifier for IpcNotify<'_> {
    fn notify(&mut self) {
        self.trigger.trigger();
    }
}

impl WaitForNotify for IpcWait<'_> {
    fn wait_for_notify(&mut self) -> impl Future<Output = ()> {
        self.event.wait()
    }
}

fn clear_boot_state(nvmc: &'static Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>) {
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }
    let state_base = unsafe { &__bootloader_state_start as *const u32 as u32 };
    let state_end = state_base + PAGE_SIZE as u32;
    nvmc.lock(|cell| {
        let mut nvmc = cell.borrow_mut();
        let _ = nvmc.erase(state_base, state_end);
    });
}
