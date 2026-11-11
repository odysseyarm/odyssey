//! What a dongle is, and which units it has bonded.

use odyssey_protocol::control::BondedDevice;
use odyssey_usb::device::MuxDevice;
use tracing::{info, warn};

use super::HubManager;
use crate::device_tasks::context::DeviceAddress;
use crate::device_tasks::messages::DongleInfo;
use crate::device_tasks::semver::semver_at_least;

/// First control protocol version that can list the dongle's bonds.
const LIST_BONDS_CONTROL_PROTOCOL: [u16; 3] = [0, 1, 1];

/// A dongle as it identifies itself.
pub(super) struct DongleIdentity {
    /// USB serial number
    pub(super) id: String,
    /// Mux endpoint protocol version
    protocol_version: [u16; 3],
    /// Control endpoint protocol version
    control_protocol_version: [u16; 3],
    firmware_version: [u32; 3],
}

impl DongleIdentity {
    /// Ask the dongle for its versions. A dongle that does not answer is registered with
    /// versions 0.0.0 rather than skipped.
    pub(super) async fn read(hub: &MuxDevice, serial: &str) -> Self {
        let id = serial.to_owned();
        let (protocol_version, firmware_version) = match hub.read_version().await {
            Ok(version) => {
                info!(
                    "Dongle protocol {}.{}.{} firmware {}.{}.{}",
                    version.protocol_semver[0],
                    version.protocol_semver[1],
                    version.protocol_semver[2],
                    version.firmware_semver[0],
                    version.firmware_semver[1],
                    version.firmware_semver[2]
                );
                (version.protocol_semver, version.firmware_semver)
            }
            Err(e) => {
                warn!("Failed to read dongle version: {}", e);
                ([0, 0, 0], [0, 0, 0])
            }
        };
        let control_protocol_version = match hub.read_ctrl_version().await {
            Ok(version) => {
                info!(
                    "Dongle control protocol {}.{}.{}",
                    version.protocol_semver[0],
                    version.protocol_semver[1],
                    version.protocol_semver[2]
                );
                version.protocol_semver
            }
            Err(e) => {
                warn!("Failed to read dongle control protocol version: {}", e);
                [0, 0, 0]
            }
        };
        Self {
            id,
            protocol_version,
            control_protocol_version,
            firmware_version,
        }
    }

    pub(super) fn supports_list_bonds(&self) -> bool {
        semver_at_least(self.control_protocol_version, LIST_BONDS_CONTROL_PROTOCOL)
    }

    pub(super) fn info(
        &self,
        connected_devices: Vec<DeviceAddress>,
        bonded_devices: Vec<DeviceAddress>,
    ) -> DongleInfo {
        DongleInfo {
            id: self.id.clone(),
            protocol_version: self.protocol_version,
            control_protocol_version: self.control_protocol_version,
            firmware_version: self.firmware_version,
            connected_devices,
            bonded_devices,
        }
    }
}

impl HubManager {
    /// List the dongle's bonds on startup and adopt their names, overwriting what is known.
    pub(super) async fn list_bonds_at_startup(&self) -> Vec<DeviceAddress> {
        if !self.dongle.supports_list_bonds() {
            return Vec::new();
        }
        let bonds = match self.hub.list_bonds().await {
            Ok(bonds) => bonds,
            Err(e) => {
                tracing::warn!("Failed to list dongle bonds: {}", e);
                return Vec::new();
            }
        };
        tracing::info!("list_bonds (startup): {} bonds returned", bonds.len());
        for bond in &bonds {
            tracing::info!(
                "  bond: uuid={:02x?} name={:?}",
                bond.uuid,
                bond.name.as_str()
            );
        }
        let mut bond_names = self.context.bond_names.lock().await;
        for bond in bonds.iter().filter(|bond| !bond.name.is_empty()) {
            bond_names.insert(bond.uuid, bond.name.to_string());
        }
        tracing::info!(
            "bond_names after startup list_bonds: {:?}",
            bond_names.keys().collect::<Vec<_>>()
        );
        bonds.iter().map(|bond| bond.uuid).collect()
    }

    /// List the dongle's bonds for a dongle-list update. Empty if the dongle cannot list
    /// them or fails to.
    pub(super) async fn refresh_bonded_devices(&self) -> Vec<DeviceAddress> {
        if !self.dongle.supports_list_bonds() {
            return Vec::new();
        }
        match self.hub.list_bonds().await {
            Ok(bonds) => {
                self.remember_missing_bond_names(&bonds).await;
                bonds.iter().map(|bond| bond.uuid).collect()
            }
            Err(e) => {
                tracing::warn!("Failed to refresh dongle bonds: {}", e);
                Vec::new()
            }
        }
    }

    /// Adopt the names the dongle has stored for units that have no name yet. Names set
    /// in this session (e.g. by a USB rename) win over the dongle's possibly stale flash
    /// copy.
    pub(super) async fn remember_missing_bond_names(&self, bonds: &[BondedDevice]) {
        let mut bond_names = self.context.bond_names.lock().await;
        for bond in bonds.iter().filter(|bond| !bond.name.is_empty()) {
            bond_names
                .entry(bond.uuid)
                .or_insert_with(|| bond.name.to_string());
        }
    }
}
