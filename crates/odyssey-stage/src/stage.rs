use serde::{Deserialize, Serialize};

use crate::StageError;
use crate::display::Display;
use crate::light::Lighting;
use crate::pattern::{Handling, Movement, Pattern, Press};
use crate::radio::Radio;

/// Since: Odyssey Desktop 0.7.5.
pub const SCHEMA: u32 = 1;
const MAX_DEVICES: usize = 32;
const MAX_HUB_DONGLES: usize = 8;
const MAX_DURATION_S: f64 = 600.0;

/// Which software owns a device's behavior.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    /// Exists only in the stage. Drives the previews and exports.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    #[default]
    Stage,
    /// One of the simulated devices of a running hub started with `--sim`:
    /// trackers become its units and dongles its dongles, in stage order.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    HubSim,
    /// A physical device the hub reports. The stage shows it, never drives it.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Real {
        #[doc = "Since: Odyssey Desktop 0.7.5."]
        uuid: String,
    },
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tracker {
    /// Since: Odyssey Desktop 0.7.5.
    pub pattern: Pattern,
    /// Since: Odyssey Desktop 0.7.5.
    pub handling: Handling,
    /// Since: Odyssey Desktop 0.7.5.
    pub movement: Movement,
    /// Since: Odyssey Desktop 0.7.5.
    pub presses: Vec<Press>,
    /// Since: Odyssey Desktop 0.7.5.
    pub roll_deg: f64,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            pattern: Pattern::Hold { at: [0.5, 0.5] },
            handling: Handling::default(),
            movement: Movement::default(),
            presses: Vec::new(),
            roll_deg: 0.0,
        }
    }
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceKind {
    /// Since: Odyssey Desktop 0.7.5.
    Tracker(Tracker),
    /// Since: Odyssey Desktop 0.7.5.
    Dongle,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Since: Odyssey Desktop 0.7.5.
    pub id: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub name: String,
    /// Meters from the display's top-left corner: +X right, +Y down, +Z away
    /// from the viewer, so a viewer standing in front has negative Z.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub position_m: [f64; 3],
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(default)]
    pub binding: Binding,
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(flatten)]
    pub kind: DeviceKind,
}

impl Device {
    /// Since: Odyssey Desktop 0.7.5.
    pub fn tracker(&self) -> Option<&Tracker> {
        match &self.kind {
            DeviceKind::Tracker(tracker) => Some(tracker),
            DeviceKind::Dongle => None,
        }
    }
}

/// An authored scene: the display, the devices around it, the light and radio
/// environment, and how long a run lasts.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stage {
    /// Since: Odyssey Desktop 0.7.5.
    pub schema: u32,
    /// Since: Odyssey Desktop 0.7.5.
    pub name: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub display: Display,
    /// Since: Odyssey Desktop 0.7.5.
    pub devices: Vec<Device>,
    /// Since: Odyssey Desktop 0.7.5.
    pub lighting: Lighting,
    /// Since: Odyssey Desktop 0.7.5.
    pub radio: Radio,
    /// Since: Odyssey Desktop 0.7.5.
    pub duration_s: f64,
    /// Whether a hub this stage calibrates is told the display's measured
    /// surface, or only the flat fit an uncalibrated tracker would assume.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub tracker_knows_surface: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            name: "Untitled".to_owned(),
            display: Display::default(),
            devices: vec![
                Device {
                    id: "tracker-1".to_owned(),
                    name: "Tracker".to_owned(),
                    position_m: [0.6, 0.3, -2.0],
                    binding: Binding::Stage,
                    kind: DeviceKind::Tracker(Tracker {
                        pattern: Pattern::named("circle")
                            .unwrap_or(Pattern::Hold { at: [0.5, 0.5] }),
                        ..Tracker::default()
                    }),
                },
                Device {
                    id: "dongle-1".to_owned(),
                    name: "Dongle".to_owned(),
                    position_m: [0.2, 0.3, -2.3],
                    binding: Binding::Stage,
                    kind: DeviceKind::Dongle,
                },
            ],
            lighting: Lighting::default(),
            radio: Radio::default(),
            duration_s: 20.0,
            tracker_knows_surface: true,
        }
    }
}

impl Stage {
    /// # Errors
    ///
    /// Returns an error if no device has the requested identifier.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn device(&self, id: &str) -> Result<&Device, StageError> {
        self.devices
            .iter()
            .find(|device| device.id == id)
            .ok_or_else(|| StageError::UnknownDevice(id.to_owned()))
    }

    /// The devices bound to a hub's simulator with the address each has there:
    /// trackers are its units and dongles its dongles, numbered in stage order.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn hub_devices(&self) -> impl Iterator<Item = (odyssey_sim::Uuid, &Device)> {
        let (mut units, mut dongles) = (0_u8, 0_u8);
        self.devices
            .iter()
            .filter(|device| device.binding == Binding::HubSim)
            .map(move |device| {
                let (count, address): (_, fn(u8) -> odyssey_sim::Uuid) = match device.kind {
                    DeviceKind::Tracker(_) => (&mut units, odyssey_sim::unit_uuid),
                    DeviceKind::Dongle => (&mut dongles, odyssey_sim::dongle_uuid),
                };
                let index = *count;
                *count = count.saturating_add(1);
                (address(index), device)
            })
    }

    /// # Errors
    ///
    /// Returns an error if any stage setting or device is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn validate(&self) -> Result<(), StageError> {
        if self.schema != SCHEMA {
            return Err(StageError::invalid(format!(
                "stage schema {} is not supported (expected {SCHEMA})",
                self.schema
            )));
        }
        if !self.duration_s.is_finite() || !(0.1..=MAX_DURATION_S).contains(&self.duration_s) {
            return Err(StageError::invalid(format!(
                "duration must be 0.1 to {MAX_DURATION_S} s"
            )));
        }
        if self.devices.len() > MAX_DEVICES {
            return Err(StageError::invalid(format!(
                "at most {MAX_DEVICES} devices"
            )));
        }
        self.display.validate()?;
        self.lighting.validate()?;
        self.radio.validate()?;
        for (index, device) in self.devices.iter().enumerate() {
            if device.id.trim().is_empty()
                || device.id.len() > 64
                || self.devices[..index]
                    .iter()
                    .any(|other| other.id == device.id)
            {
                return Err(StageError::invalid(
                    "device ids must be unique, non-empty and at most 64 characters",
                ));
            }
            if !device.position_m.iter().all(|value| value.is_finite()) {
                return Err(StageError::invalid("device positions must be finite"));
            }
            if let DeviceKind::Tracker(tracker) = &device.kind {
                tracker.pattern.validate()?;
                tracker.handling.validate()?;
                tracker.movement.validate()?;
                if !tracker.roll_deg.is_finite()
                    || tracker.presses.len() > 4096
                    || tracker
                        .presses
                        .iter()
                        .any(|press| !press.at_s.is_finite() || press.at_s < 0.0)
                {
                    return Err(StageError::invalid(
                        "invalid tracker roll or button presses",
                    ));
                }
            }
        }
        let hub_dongles = self
            .devices
            .iter()
            .filter(|device| {
                device.binding == Binding::HubSim && matches!(device.kind, DeviceKind::Dongle)
            })
            .count();
        if hub_dongles > MAX_HUB_DONGLES {
            return Err(StageError::invalid(format!(
                "the hub simulator has at most {MAX_HUB_DONGLES} dongles"
            )));
        }
        Ok(())
    }
}

/// Since: Odyssey Desktop 0.7.5.
pub const DISPLAY_PRESET_NAMES: [&str; 5] = [
    "monitor",
    "tv-55",
    "projector-wall",
    "pull-down-screen",
    "tarp-on-a-wall",
];

/// Starting points for the display, named for what people actually have.
///
/// Since: Odyssey Desktop 0.7.5.
pub fn display_preset(name: &str) -> Option<Display> {
    use crate::display::{Keystone, Waviness};
    Some(match name {
        "monitor" => Display::default(),
        "tv-55" => Display {
            width_m: 1.22,
            height_m: 0.69,
            ..Display::default()
        },
        "projector-wall" => Display {
            width_m: 2.0,
            height_m: 1.125,
            keystone: Keystone {
                horizontal: 0.06,
                vertical: 0.02,
            },
            barrel: 0.04,
            ..Display::default()
        },
        "pull-down-screen" => Display {
            width_m: 2.0,
            height_m: 1.125,
            waviness: Waviness {
                amplitude_m: 0.025,
                waves_u: 2.5,
                waves_v: 1.0,
                phase_deg: 20.0,
            },
            keystone: Keystone {
                horizontal: 0.04,
                vertical: 0.0,
            },
            ..Display::default()
        },
        "tarp-on-a-wall" => Display {
            width_m: 2.4,
            height_m: 1.35,
            waviness: Waviness {
                amplitude_m: 0.06,
                waves_u: 3.0,
                waves_v: 2.0,
                phase_deg: 60.0,
            },
            keystone: Keystone {
                horizontal: 0.08,
                vertical: -0.03,
            },
            barrel: 0.08,
            imported: None,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stage_is_valid_and_round_trips_through_json() {
        let stage = Stage::default();
        stage.validate().unwrap();
        let json = serde_json::to_string(&stage).unwrap();
        assert_eq!(serde_json::from_str::<Stage>(&json).unwrap(), stage);
    }

    #[test]
    fn a_stage_file_may_omit_everything_it_does_not_change() {
        let stage: Stage = serde_json::from_str(r#"{"name":"bare"}"#).unwrap();
        assert_eq!(stage.name, "bare");
        stage.validate().unwrap();
    }

    #[test]
    fn presets_all_build_a_valid_display() {
        for name in [
            "monitor",
            "tv-55",
            "projector-wall",
            "pull-down-screen",
            "tarp-on-a-wall",
        ] {
            display_preset(name)
                .unwrap()
                .screen()
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn duplicate_ids_are_rejected_and_several_trackers_may_drive_the_hub() {
        let mut stage = Stage::default();
        stage.devices[1].id = stage.devices[0].id.clone();
        assert!(stage.validate().is_err());

        let mut stage = Stage::default();
        let mut second = stage.devices[0].clone();
        second.id = "tracker-2".to_owned();
        stage.devices.push(second);
        for device in &mut stage.devices {
            device.binding = Binding::HubSim;
        }
        stage.validate().unwrap();
    }
}
