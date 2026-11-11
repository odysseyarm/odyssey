use defmt::{error, info};
use trouble_host::Stack;
use trouble_host::prelude::ScanConfig;
use trouble_host::scan::Scanner;

use super::central::{
    CONNECT_PENDING, PAIRING_FOUND_ADDR, SCAN_ACTIVE, SCAN_RESUME, SCAN_STOPPED,
    reset_pairing_capture,
};

/// Runs an active scan whenever no device task is connecting. Name caching and
/// pairing detection both happen in the HostEventHandler's `on_adv_reports`
/// callback. This task only keeps the scan running.
#[embassy_executor::task]
pub async fn background_scanner_task(
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
) -> ! {
    info!("Background scanner task started");

    let host = stack.build();
    let central = host.central;
    let mut scanner = Scanner::new(central);

    let config = ScanConfig {
        active: true,
        ..Default::default()
    };

    loop {
        while CONNECT_PENDING.load(core::sync::atomic::Ordering::Acquire) > 0 {
            SCAN_RESUME.wait().await;
        }

        SCAN_ACTIVE.store(true, core::sync::atomic::Ordering::Release);

        match scanner.scan(&config).await {
            Ok(session) => {
                loop {
                    if CONNECT_PENDING.load(core::sync::atomic::Ordering::Acquire) > 0 {
                        break;
                    }
                    embassy_time::Timer::after_millis(20).await;
                }
                drop(session);
                // drop(session) calls scan_command_state.cancel(), which wakes host_ctrl_task.
                // Yield so host_ctrl_task can run sdc_hci_cmd_le_set_scan_enable(false)
                // before SCAN_STOPPED is signalled. Without this, LIFO scheduling lets the
                // device task call central.connect() before the controller has disabled the
                // scan, and the connect fails with "Command Disallowed".
                embassy_time::Timer::after_millis(5).await;
            }
            Err(e) => {
                error!("Background scan error: {:?}", defmt::Debug2Format(&e));
                embassy_time::Timer::after_millis(500).await;
            }
        }

        SCAN_ACTIVE.store(false, core::sync::atomic::Ordering::Release);
        SCAN_STOPPED.signal(());
    }
}

/// Adds the device found during pairing to the scan targets and ends pairing.
/// Scanning itself happens in [`background_scanner_task`]. `on_adv_reports`
/// calls `report_scan_address`, which signals `PAIRING_FOUND_ADDR`.
#[embassy_executor::task]
pub async fn pairing_monitor_task(settings: &'static crate::storage::Settings) -> ! {
    loop {
        let addr = PAIRING_FOUND_ADDR.wait().await;
        // A cancellation may have happened after the advertisement callback
        // queued this address but before this task was polled.
        if !crate::pairing::is_active() {
            continue;
        }
        info!("Pairing: device found {:02x}, adding to scan targets", addr);
        let _ = settings.add_scan_target(addr).await;
        crate::ble::central::RESTART_CENTRAL.signal(());
        crate::pairing::cancel();
    }
}

/// Clears the pairing capture state. Call when pairing mode starts.
pub fn on_pairing_start() {
    reset_pairing_capture();
}
