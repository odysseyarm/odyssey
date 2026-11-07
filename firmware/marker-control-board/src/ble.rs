use core::cell::RefCell;

use embassy_futures::select::{Either, select};
use embassy_nrf::gpio::Input;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embedded_storage_async::nor_flash::NorFlash;
use marker_control_board::security::{BondStore, Enrollment, OwnerBond};
use marker_control_board::{ButtonAction, Led, OutputState, PowerButton};
use odyssey_protocol::marker_control::{
    DEMAND_UUID, MODE_UUID, Mode, PAIRING_STATE_VERSION, SERVICE_UUID, WriteError, parse_demand,
};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use trouble_host::att::{AttClient, AttReq};
use trouble_host::prelude::{
    AdStructure, Address, Advertisement, AttErrorCode, BR_EDR_NOT_SUPPORTED, BleHostError,
    BondInformation, Controller, DefaultPacketPool, Error, FromGatt, GapConfig, GattConnection,
    GattConnectionEvent, GattEvent, Host, HostResources, Identity, IdentityResolvingKey,
    IoCapabilities, LE_GENERAL_DISCOVERABLE, LongTermKey, Peripheral, PeripheralConfig,
    SecurityLevel, appearance, gatt_server, gatt_service,
};

use crate::hardware::{Markers, reset_safe};

static STATE: Mutex<CriticalSectionRawMutex, RefCell<OutputState>> =
    Mutex::new(RefCell::new(OutputState::new()));
pub static CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static ENROLLMENT: Mutex<CriticalSectionRawMutex, RefCell<Enrollment>> =
    Mutex::new(RefCell::new(Enrollment::new()));
static ENROLLMENT_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn enrollment_active() -> bool {
    ENROLLMENT.lock(|window| window.borrow().active(Instant::now().as_millis()))
}

fn close_enrollment() {
    ENROLLMENT.lock(|window| window.borrow_mut().close());
    ENROLLMENT_CHANGED.signal(());
    CHANGED.signal(());
}

fn enrollment_deadline(now: u64) -> Option<u64> {
    ENROLLMENT.lock(|window| {
        window
            .borrow()
            .deadline()
            .filter(|deadline| now < *deadline)
    })
}

fn stored_bond(bond: &BondInformation) -> Option<OwnerBond> {
    if !bond.is_bonded || !bond.security_level.encrypted() {
        return None;
    }
    Some(OwnerBond {
        address: bond.identity.bd_addr.raw().try_into().ok()?,
        ltk: bond.ltk.to_le_bytes(),
        irk: bond.identity.irk.map(|irk| irk.to_le_bytes()),
        security_level: match bond.security_level {
            SecurityLevel::NoEncryption => return None,
            SecurityLevel::Encrypted => 1,
            SecurityLevel::EncryptedAuthenticated => 2,
        },
    })
}

fn bond_information(bond: OwnerBond) -> BondInformation {
    BondInformation {
        identity: Identity {
            bd_addr: trouble_host::prelude::BdAddr::new(bond.address),
            irk: bond.irk.map(IdentityResolvingKey::from_le_bytes),
        },
        ltk: LongTermKey::from_le_bytes(bond.ltk),
        is_bonded: true,
        security_level: match bond.security_level {
            2 => SecurityLevel::EncryptedAuthenticated,
            _ => SecurityLevel::Encrypted,
        },
    }
}

#[gatt_server]
struct Server {
    markers: MarkerService,
}

#[gatt_service(uuid = u128::from_be_bytes(SERVICE_UUID))]
struct MarkerService {
    #[characteristic(uuid = u128::from_be_bytes(DEMAND_UUID), read, write, value = 0)]
    demand: u8,
    #[characteristic(uuid = u128::from_be_bytes(MODE_UUID), read, write, value = 0)]
    mode: u8,
}

fn state<R>(f: impl FnOnce(&mut OutputState) -> R) -> R {
    STATE.lock(|state| {
        let mut state = state.borrow_mut();
        state.expire(Instant::now().as_millis());
        f(&mut state)
    })
}

pub async fn outputs(mut markers: Markers<'_>) {
    let mut previous = false;
    let mut previous_led = Led::Off;
    loop {
        let now = Instant::now().as_millis();
        let pairing = enrollment_active();
        let dfu = crate::DFU_PENDING.load(core::sync::atomic::Ordering::Relaxed);
        let (on, lease, led) = state(|s| {
            (
                s.on() && !dfu,
                s.deadline(),
                if dfu { Led::Off } else { s.led(pairing) },
            )
        });
        if on != previous {
            if markers.set(on).is_err() {
                reset_safe();
            }
            previous = on;
        }
        // A failed LED write only loses the indicator; it must not reset the outputs.
        if led != previous_led && markers.set_led(led.bits()).is_ok() {
            previous_led = led;
        }
        let deadline = [lease, enrollment_deadline(now)]
            .into_iter()
            .flatten()
            .min();
        let expiry = async {
            match deadline {
                Some(deadline) => Timer::at(Instant::from_millis(deadline)).await,
                None => core::future::pending::<()>().await,
            }
        };
        select(CHANGED.wait(), expiry).await;
    }
}

pub async fn button(button: Input<'_>) {
    let mut clicks = PowerButton::default();
    loop {
        let low = button.is_low();
        Timer::after_millis(30).await;
        if low != button.is_low() {
            continue;
        }
        match clicks.observe(low, Instant::now().as_millis()) {
            Some(ButtonAction::Click) => {
                state(OutputState::click);
                CHANGED.signal(());
            }
            Some(ButtonAction::Enroll) => {
                ENROLLMENT.lock(|window| window.borrow_mut().open(Instant::now().as_millis()));
                state(OutputState::disconnect);
                CHANGED.signal(());
                ENROLLMENT_CHANGED.signal(());
            }
            None => {}
        }
    }
}

pub async fn run<C: Controller, F: NorFlash>(controller: C, flash: F, seed: [u8; 32]) {
    let low = embassy_nrf::pac::FICR.deviceid(0).read().to_le_bytes();
    let high = embassy_nrf::pac::FICR.deviceid(1).read().to_le_bytes();
    let address = Address::random([low[0], low[1], low[2], low[3], high[0], high[1] | 0xc0]);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let mut seed_rng = ChaCha20Rng::from_seed(seed);
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .set_io_capabilities(IoCapabilities::NoInputNoOutput)
        .set_random_generator_seed(&mut seed_rng);
    let Ok(mut bonds) = BondStore::load(flash, 0).await else {
        reset_safe();
    };
    if let Some(owner) = bonds.owner()
        && stack.add_bond_information(bond_information(owner)).is_err()
    {
        reset_safe();
    }
    let Host {
        mut peripheral,
        mut runner,
        ..
    } = stack.build();
    let Ok(server) = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Odyssey Markers",
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    })) else {
        reset_safe();
    };
    embassy_futures::join::join(
        async {
            let _ = runner.run().await;
            reset_safe();
        },
        async {
            let stack = &stack;
            loop {
                let conn = match advertise(&mut peripheral, &server).await {
                    Ok(conn) => conn,
                    Err(_) => reset_safe(),
                };
                if conn.raw().set_bondable(enrollment_active()).is_ok() {
                    let _ = conn.raw().request_security();
                    let _ = session(&server, &conn, &mut bonds, || {
                        stack
                            .get_bond_information()
                            .iter()
                            .find(|bond| bond.identity.match_identity(&conn.raw().peer_identity()))
                            .and_then(stored_bond)
                    })
                    .await;
                }
                state(OutputState::disconnect);
                CHANGED.signal(());
                conn.raw().disconnect();
                for bond in stack.get_bond_information() {
                    if stack.remove_bond_information(bond.identity).is_err() {
                        reset_safe();
                    }
                }
                if let Some(owner) = bonds.owner()
                    && stack.add_bond_information(bond_information(owner)).is_err()
                {
                    reset_safe();
                }
            }
        },
    )
    .await;
}

async fn advertise<'values, 'server, C: Controller>(
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut service_data = [0; 18];
    service_data[..16].copy_from_slice(&SERVICE_UUID);
    service_data[..16].reverse();
    service_data[16] = PAIRING_STATE_VERSION;
    let mut scan_data = [0; 31];
    let scan_len = AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(b"Odyssey Markers")],
        &mut scan_data,
    )?;
    loop {
        ENROLLMENT_CHANGED.reset();
        let (remaining, next_change) = ENROLLMENT.lock(|window| {
            let window = window.borrow();
            let now = Instant::now().as_millis();
            (window.remaining_seconds(now), window.next_change_at(now))
        });
        service_data[17] = remaining;
        let mut data = [0; 31];
        let len = AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                // Pinned trouble-host represents ServiceData128 as raw AD type 0x21.
                AdStructure::Unknown {
                    ty: 0x21,
                    data: &service_data,
                },
            ],
            &mut data,
        )?;
        let connection = async {
            Ok(peripheral
                .advertise(
                    &Default::default(),
                    Advertisement::ConnectableScannableUndirected {
                        adv_data: &data[..len],
                        scan_data: &scan_data[..scan_len],
                    },
                )
                .await?
                .accept()
                .await?
                .with_attribute_server(server)?)
        };
        let expiry = async {
            match next_change {
                Some(deadline) => Timer::at(Instant::from_millis(deadline)).await,
                None => core::future::pending::<()>().await,
            }
        };
        // Dropping Advertiser/advertise cancels through trouble-host's command state.
        match select(connection, select(ENROLLMENT_CHANGED.wait(), expiry)).await {
            Either::First(connection) => return connection,
            Either::Second(Either::First(())) => {}
            Either::Second(Either::Second(())) => {
                if !enrollment_active() {
                    close_enrollment();
                }
            }
        }
    }
}

async fn session<F: NorFlash>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    bonds: &mut BondStore<F>,
    current_bond: impl Fn() -> Option<OwnerBond>,
) -> Result<(), Error> {
    loop {
        let event = match select(conn.next(), ENROLLMENT_CHANGED.wait()).await {
            Either::First(event) => event,
            Either::Second(()) => {
                if enrollment_active() {
                    return Ok(());
                }
                conn.raw().set_bondable(false)?;
                continue;
            }
        };
        match event {
            GattConnectionEvent::Disconnected { .. } => return Ok(()),
            GattConnectionEvent::PairingComplete {
                security_level,
                bond: Some(bond),
            } => {
                let candidate = stored_bond(&bond).filter(|_| security_level.encrypted());
                match candidate {
                    Some(candidate) if bonds.owner() == Some(candidate) => {}
                    Some(candidate) if enrollment_active() => {
                        if bonds.persist(candidate, enrollment_active).await.is_err() {
                            return Ok(());
                        }
                        close_enrollment();
                    }
                    _ => return Ok(()),
                }
            }
            GattConnectionEvent::PairingFailed(_) => return Ok(()),
            GattConnectionEvent::Gatt { event } => {
                let mut update = None;
                let error = match &event {
                    GattEvent::Write(write) => {
                        let encrypted = conn.raw().security_level()?.encrypted();
                        let current = current_bond();
                        if !encrypted {
                            Some(AttErrorCode::INSUFFICIENT_ENCRYPTION)
                        } else if !bonds
                            .owner()
                            .is_some_and(|owner| owner.permits(encrypted, current.as_ref()))
                        {
                            Some(AttErrorCode::INSUFFICIENT_AUTHORISATION)
                        } else if !matches!(
                            write.payload().incoming(),
                            AttClient::Request(AttReq::Write { .. })
                        ) || (write.handle() != server.markers.demand.handle
                            && write.handle() != server.markers.mode.handle)
                        {
                            Some(AttErrorCode::WRITE_NOT_PERMITTED)
                        } else {
                            let parsed = if write.handle() == server.markers.demand.handle {
                                parse_demand(write.data()).map(|active| (false, u8::from(active)))
                            } else {
                                Mode::parse(write.data()).map(|mode| (true, mode as u8))
                            };
                            match parsed {
                                Ok(command) => {
                                    update = Some(command);
                                    None
                                }
                                Err(WriteError::Length) => {
                                    Some(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)
                                }
                                Err(WriteError::Value) => Some(AttErrorCode::VALUE_NOT_ALLOWED),
                            }
                        }
                    }
                    GattEvent::Other(other)
                        if matches!(
                            other.payload().incoming(),
                            AttClient::Request(
                                AttReq::PrepareWrite { .. } | AttReq::ExecuteWrite { .. }
                            )
                        ) =>
                    {
                        Some(AttErrorCode::REQUEST_NOT_SUPPORTED)
                    }
                    _ => None,
                };
                let (mode, demand) = state(|s| (s.mode() as u8, u8::from(s.deadline().is_some())));
                server.set(&server.markers.mode, &mode)?;
                server.set(&server.markers.demand, &demand)?;
                let reply = if let Some(error) = error {
                    event.reject(error)?
                } else {
                    event.accept()?
                };
                if let Some((mode, value)) = update {
                    state(|s| {
                        if mode {
                            s.write_mode(&[value])
                                .expect("GATT mode command passed validation");
                        } else {
                            s.write_demand(&[value], Instant::now().as_millis())
                                .expect("GATT demand command passed validation");
                        }
                    });
                    CHANGED.signal(());
                }
                reply.send().await;
            }
            _ => {}
        }
    }
}
