use defmt::{error, info};
use trouble_host::Stack;
use trouble_host::prelude::ScanConfig;
use trouble_host::scan::Scanner;

use super::central::{
    CONNECT_PENDING, PAIRING_FOUND_ADDR, SCAN_ACTIVE, SCAN_RESUME, SCAN_STOPPED,
    reset_pairing_capture,
};

/// Background scanner — runs an active scan whenever no device task is connecting.
/// The on_adv_reports callback in HostEventHandler handles both name caching and
/// pairing device detection, so this task just needs to keep the scan running.
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
        // Wait until no device task is connecting
        while CONNECT_PENDING.load(core::sync::atomic::Ordering::Acquire) > 0 {
            SCAN_RESUME.wait().await;
        }

        SCAN_ACTIVE.store(true, core::sync::atomic::Ordering::Release);

        match scanner.scan(&config).await {
            Ok(session) => {
                // Keep scanning until a connect is initiated
                loop {
                    if CONNECT_PENDING.load(core::sync::atomic::Ordering::Acquire) > 0 {
                        break;
                    }
                    embassy_time::Timer::after_millis(20).await;
                }
                drop(session);
                // drop(session) calls scan_command_state.cancel(), which wakes host_ctrl_task.
                // Wait here so host_ctrl_task can run sdc_hci_cmd_le_set_scan_enable(false)
                // before we signal SCAN_STOPPED. Without this, LIFO scheduling causes the
                // device task to call central.connect() before the scan is disabled at the
                // hardware level, resulting in "Command Disallowed".
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

/// Pairing monitor — waits for a device found during pairing and adds it to scan targets.
/// The actual scanning is done by background_scanner_task; on_adv_reports fires
/// report_scan_address which signals PAIRING_FOUND_ADDR.
#[embassy_executor::task]
pub async fn pairing_monitor_task(settings: &'static crate::storage::Settings) -> ! {
    loop {
        let addr = PAIRING_FOUND_ADDR.wait().await;
        info!("Pairing: device found {:02x}, adding to scan targets", addr);
        let _ = settings.add_scan_target(addr).await;
        crate::ble::central::RESTART_CENTRAL.signal(());
        crate::pairing::cancel();
    }
}

/// Called when pairing mode starts to reset the pairing capture state.
pub fn on_pairing_start() {
    reset_pairing_capture();
}
