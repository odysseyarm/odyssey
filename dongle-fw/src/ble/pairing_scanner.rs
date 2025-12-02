use defmt::info;
use embassy_time::Duration;
use trouble_host::Stack;
use trouble_host::prelude::{ScanConfig, Scanner};

use super::central::{PAIRING_FOUND_ADDR, reset_pairing_capture};

/// Pairing scanner task - scans for new devices during pairing
#[embassy_executor::task]
pub async fn pairing_scanner_task(
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    settings: &'static crate::storage::Settings,
) -> ! {
    info!("Pairing scanner task started");

    loop {
        // Wait until pairing is active
        while !crate::pairing::is_active() {
            embassy_time::Timer::after_millis(100).await;
        }

        info!("Pairing active, starting scan for new devices");
        reset_pairing_capture();

        // Build our own Central for scanning
        let host = stack.build();
        let central = host.central;
        let mut scanner = Scanner::new(central);

        let mut config = ScanConfig::default();
        config.active = true; // Active scan to get scan responses

        // Scan until device found or timeout
        match scanner.scan(&config).await {
            Ok(session) => {
                // Wait for device discovery (30 second timeout)
                let result =
                    embassy_time::with_timeout(Duration::from_secs(30), PAIRING_FOUND_ADDR.wait())
                        .await;

                drop(session); // Stop scanning immediately

                match result {
                    Ok(addr) => {
                        info!("Pairing found device: {:02x}", addr);

                        // Add to scan targets - BLE manager will spawn the device task
                        let _ = settings.add_scan_target(addr).await;

                        // Signal BLE manager to check for new targets immediately
                        crate::ble::central::RESTART_CENTRAL.signal(());

                        // Exit pairing mode
                        crate::pairing::cancel();
                        info!("Device added to targets, pairing cancelled");
                    }
                    Err(_) => {
                        info!("Pairing scan timeout - no device found in 30s");
                    }
                }
            }
            Err(e) => match &e {
                trouble_host::BleHostError::BleHost(kind) => {
                    defmt::error!("Scan start host error: {:?}", kind)
                }
                trouble_host::BleHostError::Controller(_) => {
                    defmt::error!("Scan start controller error")
                }
            },
        }

        // Small delay before potentially restarting
        embassy_time::Timer::after_millis(500).await;
    }
}
