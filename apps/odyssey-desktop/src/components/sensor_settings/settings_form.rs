use std::fmt::Display;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::field_issue::{FieldIssue, invalid_status};
use super::general_config::{
    IMPACT_THRESHOLD_CONFIG, SUPPRESS_CONFIG, general_validate, write_general_config,
};
use super::op_status::{OpStatus, rpc_failure};
use super::pag_state::{
    PagState, PagValues, pag_apply_to_device, pag_load_defaults, pag_load_from_device, pag_validate,
};
use super::paj_state::{
    PajState, PajValues, paj_apply_to_device, paj_load_defaults, paj_load_from_device, paj_validate,
};
use super::product_ids::SensorFamily;
use super::registers::{NEAR_FIELD_PORT, WIDE_FIELD_PORT};
use crate::hub::HubContext;

/// Factory value the form is filled with for both general config fields.
const DEFAULT_GENERAL_VALUE: &str = "25";

/// What a load or write needs to know about the device being edited: the device
/// as it is now and whether its sensors can be tuned, which depends on the
/// events link being up.
#[derive(Clone, PartialEq)]
pub(super) struct LoadTarget {
    pub(super) device: Device,
    pub(super) can_tune_sensors: bool,
}

impl LoadTarget {
    /// The sensor family to edit, if register access is available at all.
    pub(super) fn tunable_sensors(&self) -> Option<SensorFamily> {
        SensorFamily::for_product(self.device.product_id).filter(|_| self.can_tune_sensors)
    }
}

/// A complete sensor snapshot, validated before the first device write. Reading
/// signals between RPCs would mix later edits into an in-flight save, or apply
/// general/near-field settings before discovering an invalid wide-field value.
enum SensorValues {
    Paj {
        near_field: PajValues,
        wide_field: PajValues,
    },
    Pag(PagValues),
}

impl SensorValues {
    fn validate(
        sensors: Option<SensorFamily>,
        near_field: &PajState,
        wide_field: &PajState,
        pag: &PagState,
    ) -> Result<Option<Self>, Vec<FieldIssue>> {
        match sensors {
            None => Ok(None),
            Some(SensorFamily::Paj) => Ok(Some(Self::Paj {
                near_field: paj_validate(near_field)?,
                wide_field: paj_validate(wide_field)?,
            })),
            Some(SensorFamily::Pag) => Ok(Some(Self::Pag(pag_validate(pag)?))),
        }
    }
}

/// Every signal behind the settings form. Signals are cheap `Copy` handles, so
/// the form can be moved into coroutines and callbacks freely.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SettingsForm {
    pub(super) impact_threshold: Signal<String>,
    pub(super) suppress_ms: Signal<String>,
    /// Stored on the PC, not on the device.
    pub(super) shot_delay: Signal<String>,
    pub(super) paj_near_field: Signal<PajState>,
    pub(super) paj_wide_field: Signal<PajState>,
    pub(super) pag: Signal<PagState>,
    pub(super) status: Signal<OpStatus>,
    pub(super) loading: Signal<bool>,
    /// Empty fields before the first read are "not loaded yet", not "invalid";
    /// marking them red on first paint is noise the user cannot act on.
    pub(super) hydrated: Signal<bool>,
}

pub(super) fn use_settings_form() -> SettingsForm {
    SettingsForm {
        impact_threshold: use_signal(String::new),
        suppress_ms: use_signal(String::new),
        shot_delay: use_signal(String::new),
        paj_near_field: use_signal(PajState::default),
        paj_wide_field: use_signal(PajState::default),
        pag: use_signal(PagState::default),
        status: use_signal(OpStatus::default),
        loading: use_signal(|| false),
        hydrated: use_signal(|| false),
    }
}

/// Keeps the first failure of a load. A later read succeeding does not make the
/// form trustworthy, and reporting "Loaded" over a failed read is how stale
/// values end up being flashed back onto the device.
fn record_failure(
    failure: &mut Option<OpStatus>,
    log_context: &str,
    action: &str,
    error: &dyn Display,
) {
    tracing::error!("{log_context}: {error:#}");
    failure.get_or_insert(rpc_failure(action, error));
}

fn write_failure(log_context: &str, action: &str, error: &dyn Display) -> OpStatus {
    tracing::error!("{log_context}: {error:#}");
    rpc_failure(action, error)
}

impl SettingsForm {
    /// The issues in the general fields, empty when they are all valid.
    pub(super) fn general_issues(&self) -> Vec<FieldIssue> {
        general_validate(
            &self.impact_threshold.read(),
            &self.suppress_ms.read(),
            &self.shot_delay.read(),
        )
        .err()
        .unwrap_or_default()
    }

    /// Whether any field the device would receive is invalid.
    pub(super) fn has_invalid_input(&self, sensors: Option<SensorFamily>) -> bool {
        let sensor_issue_count = match sensors {
            None => 0,
            Some(SensorFamily::Paj) => {
                paj_validate(&self.paj_near_field.read())
                    .err()
                    .map_or(0, |issues| issues.len())
                    + paj_validate(&self.paj_wide_field.read())
                        .err()
                        .map_or(0, |issues| issues.len())
            }
            Some(SensorFamily::Pag) => pag_validate(&self.pag.read())
                .err()
                .map_or(0, |issues| issues.len()),
        };
        !self.general_issues().is_empty() || sensor_issue_count > 0
    }

    /// Fills the form with factory values; nothing reaches the device.
    pub(super) fn restore_defaults(mut self, sensors: Option<SensorFamily>) {
        if *self.loading.peek() {
            return;
        }
        self.impact_threshold.set(DEFAULT_GENERAL_VALUE.into());
        self.suppress_ms.set(DEFAULT_GENERAL_VALUE.into());
        match sensors {
            None => {}
            Some(SensorFamily::Paj) => {
                self.paj_near_field.set(paj_load_defaults(NEAR_FIELD_PORT));
                self.paj_wide_field.set(paj_load_defaults(WIDE_FIELD_PORT));
            }
            Some(SensorFamily::Pag) => self.pag.set(pag_load_defaults()),
        }
        self.hydrated.set(true);
        self.status.set(OpStatus::Done(
            "Defaults filled in. Nothing is written until you apply or save.".into(),
        ));
    }

    /// Reads every setting from the device into the form.
    pub(super) async fn load(
        mut self,
        hub: Signal<HubContext>,
        target: &LoadTarget,
        sensors_only: bool,
    ) {
        if *self.loading.peek() {
            return;
        }
        self.loading.set(true);
        self.status
            .set(OpStatus::Busy("Reading from device...".into()));
        let hub_ctx = hub.peek().clone();
        let device = target.device.clone();
        let mut failure: Option<OpStatus> = None;

        if !sensors_only {
            self.load_general(&hub_ctx, &device, &mut failure).await;
        }
        if let Some(sensors) = target.tunable_sensors()
            && let Err(error) = self.load_sensors(&hub_ctx, &device, sensors).await
        {
            // A failed reload must not leave an apparently valid stale sensor
            // snapshot available for the next save.
            match sensors {
                SensorFamily::Paj => {
                    self.paj_near_field.set(PajState::default());
                    self.paj_wide_field.set(PajState::default());
                }
                SensorFamily::Pag => self.pag.set(PagState::default()),
            }
            record_failure(
                &mut failure,
                "reading sensor settings",
                "read the sensor settings",
                &error,
            );
        }

        self.hydrated.set(true);
        self.status
            .set(failure.unwrap_or(OpStatus::Done("Loaded from device".into())));
        self.loading.set(false);
    }

    async fn load_general(
        &mut self,
        hub_ctx: &HubContext,
        device: &Device,
        failure: &mut Option<OpStatus>,
    ) {
        match hub_ctx
            .client
            .peek()
            .clone()
            .read_config(device.clone(), IMPACT_THRESHOLD_CONFIG)
            .await
        {
            Ok(value) => self.impact_threshold.set(value.to_string()),
            Err(error) => {
                self.impact_threshold.set(String::new());
                record_failure(
                    failure,
                    "reading impact threshold",
                    "read the impact threshold",
                    &error,
                );
            }
        }
        match hub_ctx
            .client
            .peek()
            .clone()
            .read_config(device.clone(), SUPPRESS_CONFIG)
            .await
        {
            Ok(value) => self.suppress_ms.set(value.to_string()),
            Err(error) => {
                self.suppress_ms.set(String::new());
                record_failure(
                    failure,
                    "reading suppress time",
                    "read the suppress time",
                    &error,
                );
            }
        }
        match hub_ctx
            .client
            .peek()
            .clone()
            .get_shot_delay(device.clone())
            .await
        {
            Ok(value) => self.shot_delay.set(value.to_string()),
            Err(error) => {
                self.shot_delay.set(String::new());
                record_failure(failure, "reading shot delay", "read the shot delay", &error);
            }
        }
    }

    async fn load_sensors(
        &mut self,
        hub_ctx: &HubContext,
        device: &Device,
        sensors: SensorFamily,
    ) -> anyhow::Result<()> {
        match sensors {
            SensorFamily::Paj => {
                let near_field = paj_load_from_device(hub_ctx, device, NEAR_FIELD_PORT).await;
                let wide_field = paj_load_from_device(hub_ctx, device, WIDE_FIELD_PORT).await;
                match (near_field, wide_field) {
                    (Ok(near_field), Ok(wide_field)) => {
                        self.paj_near_field.set(near_field);
                        self.paj_wide_field.set(wide_field);
                        Ok(())
                    }
                    (Err(error), _) | (_, Err(error)) => Err(error),
                }
            }
            SensorFamily::Pag => {
                let state = pag_load_from_device(hub_ctx, device).await?;
                self.pag.set(state);
                Ok(())
            }
        }
    }

    /// Validates the whole form and writes it to the device. `persist` adds the
    /// flash step, so "Save" and "Try without saving" cannot drift apart in what
    /// they send to the device.
    pub(super) async fn write_all(
        mut self,
        hub: Signal<HubContext>,
        target: &LoadTarget,
        persist: bool,
    ) {
        if *self.loading.peek() {
            return;
        }
        self.loading.set(true);
        self.status.set(OpStatus::Busy(
            if persist {
                "Saving to device..."
            } else {
                "Applying..."
            }
            .into(),
        ));
        let hub_ctx = hub.peek().clone();

        // Both outcomes are a status for the user; the error side is just the
        // early-exit path.
        let outcome = self.try_write_all(&hub_ctx, target, persist).await;
        self.status.set(outcome.unwrap_or_else(|failed| failed));
        self.loading.set(false);
    }

    async fn try_write_all(
        &self,
        hub_ctx: &HubContext,
        target: &LoadTarget,
        persist: bool,
    ) -> Result<OpStatus, OpStatus> {
        let device = &target.device;
        let general = general_validate(
            &self.impact_threshold.peek(),
            &self.suppress_ms.peek(),
            &self.shot_delay.peek(),
        )
        .map_err(|issues| invalid_status(&issues))?;
        let sensors = SensorValues::validate(
            target.tunable_sensors(),
            &self.paj_near_field.peek(),
            &self.paj_wide_field.peek(),
            &self.pag.peek(),
        )
        .map_err(|issues| invalid_status(&issues))?;
        write_general_config(hub_ctx, device, general)
            .await
            .map_err(|error| {
                write_failure(
                    "writing general config",
                    "write the general settings",
                    &error,
                )
            })?;

        self.write_sensors(hub_ctx, device, sensors).await?;

        // Flash last: it persists whatever the device currently holds, so it
        // must only run once every write above has succeeded.
        if !persist {
            return Ok(OpStatus::Done(
                "Applied for this session. Power cycling restores the saved values.".into(),
            ));
        }
        hub_ctx
            .client
            .peek()
            .clone()
            .flash_settings(device.clone())
            .await
            .map_err(|error| {
                write_failure(
                    "flashing settings",
                    "save the settings to the device",
                    &error,
                )
            })?;
        Ok(OpStatus::Done("Saved to the device".into()))
    }

    async fn write_sensors(
        &self,
        hub_ctx: &HubContext,
        device: &Device,
        sensors: Option<SensorValues>,
    ) -> Result<(), OpStatus> {
        let sensor_failure = |error: anyhow::Error| {
            write_failure(
                "writing sensor settings",
                "write the sensor settings",
                &error,
            )
        };
        match sensors {
            None => {}
            Some(SensorValues::Paj {
                near_field,
                wide_field,
            }) => {
                for (port, values) in [(NEAR_FIELD_PORT, near_field), (WIDE_FIELD_PORT, wide_field)]
                {
                    paj_apply_to_device(hub_ctx, device, port, &values)
                        .await
                        .map_err(sensor_failure)?;
                }
            }
            Some(SensorValues::Pag(values)) => {
                pag_apply_to_device(hub_ctx, device, &values)
                    .await
                    .map_err(sensor_failure)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::field_issue::test_support::issues;
    use super::super::paj_state::PAJ_EXPOSURE;
    use super::*;

    #[test]
    fn invalid_wide_field_rejects_the_complete_sensor_snapshot() {
        let near_field = paj_load_defaults(NEAR_FIELD_PORT);
        let mut wide_field = paj_load_defaults(WIDE_FIELD_PORT);
        wide_field.exposure_time = "65536".into();
        let found = issues(SensorValues::validate(
            Some(SensorFamily::Paj),
            &near_field,
            &wide_field,
            &PagState::default(),
        ));
        assert_eq!(
            found.iter().map(|issue| issue.field).collect::<Vec<_>>(),
            [PAJ_EXPOSURE]
        );
    }

    #[test]
    fn only_active_sensor_family_is_validated() {
        assert!(matches!(
            SensorValues::validate(
                None,
                &PajState::default(),
                &PajState::default(),
                &PagState::default()
            ),
            Ok(None)
        ));
        let pag = pag_load_defaults();
        assert!(matches!(
            SensorValues::validate(
                Some(SensorFamily::Pag),
                &PajState::default(),
                &PajState::default(),
                &pag,
            ),
            Ok(Some(SensorValues::Pag(_)))
        ));
        let mut invalid_pag = pag;
        invalid_pag.exposure_us = "12701".into();
        assert!(
            SensorValues::validate(
                Some(SensorFamily::Pag),
                &PajState::default(),
                &PajState::default(),
                &invalid_pag,
            )
            .is_err()
        );
    }
}
