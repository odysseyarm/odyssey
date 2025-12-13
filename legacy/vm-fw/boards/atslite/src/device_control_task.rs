//! Device control task - handles USB control endpoint commands
//! Replaces triple-press bond clearing with CLI-based control

use defmt::info;
use protodongers::control::device::{ClearBondsError, PairingError, DeviceMsg, Version};

const FIRMWARE_VERSION: [u16; 3] = [0, 1, 0]; // TODO: Get from build

#[embassy_executor::task]
pub async fn device_control_task() {
    info!("Device control task started");
    
    loop {
        let cmd = common::device_control::recv_cmd().await;
        info!("Received control command: {:?}", cmd);
        
        match cmd {
            DeviceMsg::ReadVersion() => {
                let version = Version::new(FIRMWARE_VERSION);
                common::device_control::try_send_event(
                    DeviceMsg::ReadVersionResponse(version)
                );
            }
            
            DeviceMsg::ClearBond => {
                info!("Clearing bond...");
                // Clear the bond
                unsafe { common::settings::get_settings() }.ble_bond_clear();
                
                // Request disconnect if currently connected
                crate::ble::peripheral::request_disconnect();
                
                info!("Bond cleared successfully");
                common::device_control::try_send_event(
                    DeviceMsg::ClearBondResponse(Ok(()))
                );
            }
            
            DeviceMsg::StartPairing(params) => {
                info!("Starting pairing mode for {} ms", params.timeout_ms);
                // Enter pairing mode
                crate::ble::peripheral::enter_pairing_mode();
                
                // Clear any existing bond so pairing is fresh
                unsafe { common::settings::get_settings() }.ble_bond_clear();
                crate::ble::peripheral::request_disconnect();
                
                common::device_control::try_send_event(
                    DeviceMsg::StartPairingResponse
                );
                
                // TODO: Handle timeout and send PairingResult
                // For now, just wait for the timeout
                embassy_time::Timer::after_millis(params.timeout_ms as u64).await;
                
                if !crate::ble::peripheral::is_pairing_active() {
                    // Pairing succeeded
                    // Get the bonded device address
                    let addr = [0u8; 6]; // TODO: Get actual address from BLE stack
                    common::device_control::try_send_event(
                        DeviceMsg::PairingResult(Ok(addr))
                    );
                } else {
                    // Pairing timed out
                    crate::ble::peripheral::cancel_pairing_mode();
                    common::device_control::try_send_event(
                        DeviceMsg::PairingResult(Err(PairingError::Timeout))
                    );
                }
            }
            
            DeviceMsg::CancelPairing => {
                info!("Cancelling pairing mode");
                crate::ble::peripheral::cancel_pairing_mode();
                common::device_control::try_send_event(
                    DeviceMsg::PairingResult(Err(PairingError::Cancelled))
                );
            }
            
            _ => {
                // Ignore response messages received as commands
            }
        }
    }
}
