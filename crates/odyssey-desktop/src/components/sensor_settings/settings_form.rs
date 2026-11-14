use std::fmt::Display;

use odyssey_hub_client::client::{Client, DeviceClient};
use odyssey_hub_common::device::Device;

use super::field_issue::{FieldIssue, invalid_status};
use super::general_config::{
    GeneralValues, IMPACT_THRESHOLD_CONFIG, SUPPRESS_CONFIG, general_validate, write_general_config,
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
/// the form between RPCs would mix later edits into an in-flight save, or apply
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

/// Everything behind the settings form, as the text the user sees and edits.
#[derive(Default)]
pub(super) struct SettingsForm {
    pub(super) impact_threshold: String,
    pub(super) suppress_ms: String,
    /// Stored on the PC, not on the device.
    pub(super) shot_delay: String,
    pub(super) paj_near_field: PajState,
    pub(super) paj_wide_field: PajState,
    pub(super) pag: PagState,
    pub(super) status: OpStatus,
    pub(super) loading: bool,
    /// Empty fields before the first read are "not loaded yet", not "invalid".
    /// Marking them red on first paint is noise the user cannot act on.
    pub(super) hydrated: bool,
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

/// The device settings a load read, as form text. Empty where the read failed.
struct GeneralFields {
    impact_threshold: String,
    suppress_ms: String,
    shot_delay: String,
}

enum SensorFields {
    Paj {
        near_field: Box<PajState>,
        wide_field: Box<PajState>,
    },
    Pag(PagState),
}

/// A load that is ready to run off the UI thread.
pub(super) struct LoadJob {
    client: Option<Client>,
    device: Device,
    sensors_only: bool,
    sensors: Option<SensorFamily>,
}

/// What a load found, to be applied to the form on the UI thread.
pub(super) struct LoadOutcome {
    general: Option<GeneralFields>,
    sensors: Option<Result<SensorFields, ()>>,
    sensors_family: Option<SensorFamily>,
    failure: Option<OpStatus>,
}

/// A validated write that is ready to run off the UI thread.
pub(super) struct WriteJob {
    client: Option<Client>,
    device: Device,
    general: GeneralValues,
    sensors: Option<SensorValues>,
    persist: bool,
}

impl SettingsForm {
    /// The issues in the general fields, empty when they are all valid.
    pub(super) fn general_issues(&self) -> Vec<FieldIssue> {
        general_validate(&self.impact_threshold, &self.suppress_ms, &self.shot_delay)
            .err()
            .unwrap_or_default()
    }

    /// Whether any field the device would receive is invalid.
    pub(super) fn has_invalid_input(&self, sensors: Option<SensorFamily>) -> bool {
        let sensor_issue_count = match sensors {
            None => 0,
            Some(SensorFamily::Paj) => {
                paj_validate(&self.paj_near_field)
                    .err()
                    .map_or(0, |issues| issues.len())
                    + paj_validate(&self.paj_wide_field)
                        .err()
                        .map_or(0, |issues| issues.len())
            }
            Some(SensorFamily::Pag) => pag_validate(&self.pag)
                .err()
                .map_or(0, |issues| issues.len()),
        };
        !self.general_issues().is_empty() || sensor_issue_count > 0
    }

    /// Fills the form with factory values. Nothing reaches the device.
    pub(super) fn restore_defaults(&mut self, sensors: Option<SensorFamily>) {
        if self.loading {
            return;
        }
        self.impact_threshold = DEFAULT_GENERAL_VALUE.into();
        self.suppress_ms = DEFAULT_GENERAL_VALUE.into();
        match sensors {
            None => {}
            Some(SensorFamily::Paj) => {
                self.paj_near_field = paj_load_defaults(NEAR_FIELD_PORT);
                self.paj_wide_field = paj_load_defaults(WIDE_FIELD_PORT);
            }
            Some(SensorFamily::Pag) => self.pag = pag_load_defaults(),
        }
        self.hydrated = true;
        self.status = OpStatus::Done(
            "Defaults filled in. Nothing is written until you apply or save.".into(),
        );
    }

    /// Marks the form busy and describes what to read from the device.
    pub(super) fn start_load(
        &mut self,
        target: &LoadTarget,
        client: Option<Client>,
        sensors_only: bool,
    ) -> LoadJob {
        self.loading = true;
        self.status = OpStatus::Busy("Reading from device...".into());
        LoadJob {
            client,
            device: target.device.clone(),
            sensors_only,
            sensors: target.tunable_sensors(),
        }
    }

    /// Applies what a load found.
    pub(super) fn finish_load(&mut self, outcome: LoadOutcome) {
        if let Some(general) = outcome.general {
            self.impact_threshold = general.impact_threshold;
            self.suppress_ms = general.suppress_ms;
            self.shot_delay = general.shot_delay;
        }
        match outcome.sensors {
            Some(Ok(SensorFields::Paj {
                near_field,
                wide_field,
            })) => {
                self.paj_near_field = *near_field;
                self.paj_wide_field = *wide_field;
            }
            Some(Ok(SensorFields::Pag(state))) => self.pag = state,
            // A failed reload must not leave an apparently valid stale sensor
            // snapshot available for the next save.
            Some(Err(())) => match outcome.sensors_family {
                Some(SensorFamily::Paj) => {
                    self.paj_near_field = PajState::default();
                    self.paj_wide_field = PajState::default();
                }
                Some(SensorFamily::Pag) => self.pag = PagState::default(),
                None => {}
            },
            None => {}
        }
        self.hydrated = true;
        self.status = outcome
            .failure
            .unwrap_or(OpStatus::Done("Loaded from device".into()));
        self.loading = false;
    }

    /// Validates the whole form into one snapshot, so later edits cannot reach
    /// an in-flight save. `persist` adds the flash step, so "Save" and "Try
    /// without saving" cannot drift apart in what they send to the device.
    /// `None` when the form is invalid, and the status then says so.
    pub(super) fn start_write(
        &mut self,
        target: &LoadTarget,
        client: Option<Client>,
        persist: bool,
    ) -> Option<WriteJob> {
        self.loading = true;
        self.status = OpStatus::Busy(
            if persist {
                "Saving to device..."
            } else {
                "Applying..."
            }
            .into(),
        );
        let snapshot =
            general_validate(&self.impact_threshold, &self.suppress_ms, &self.shot_delay).and_then(
                |general| {
                    SensorValues::validate(
                        target.tunable_sensors(),
                        &self.paj_near_field,
                        &self.paj_wide_field,
                        &self.pag,
                    )
                    .map(|sensors| (general, sensors))
                },
            );
        match snapshot {
            Ok((general, sensors)) => Some(WriteJob {
                client,
                device: target.device.clone(),
                general,
                sensors,
                persist,
            }),
            Err(issues) => {
                self.status = invalid_status(&issues);
                self.loading = false;
                None
            }
        }
    }

    pub(super) fn finish_write(&mut self, status: OpStatus) {
        self.status = status;
        self.loading = false;
    }
}

/// Reads every requested setting from the device.
pub(super) async fn read_from_device(job: LoadJob) -> LoadOutcome {
    let mut failure = None;
    let client = job.client.map(|client| client.device(job.device));
    let general = if job.sensors_only {
        None
    } else {
        Some(read_general(client.as_ref(), &mut failure).await)
    };
    let sensors = match job.sensors {
        None => None,
        Some(family) => Some(match read_sensors(client.as_ref(), family).await {
            Ok(fields) => Ok(fields),
            Err(error) => {
                record_failure(
                    &mut failure,
                    "reading sensor settings",
                    "read the sensor settings",
                    &error,
                );
                Err(())
            }
        }),
    };
    LoadOutcome {
        general,
        sensors,
        sensors_family: job.sensors,
        failure,
    }
}

async fn read_general(
    client: Option<&DeviceClient>,
    failure: &mut Option<OpStatus>,
) -> GeneralFields {
    let mut fields = GeneralFields {
        impact_threshold: String::new(),
        suppress_ms: String::new(),
        shot_delay: String::new(),
    };
    let Some(client) = client else {
        record_failure(
            failure,
            "reading impact threshold",
            "read the impact threshold",
            &"not connected to the hub",
        );
        return fields;
    };
    match client.read_config(IMPACT_THRESHOLD_CONFIG).await {
        Ok(value) => fields.impact_threshold = value.to_string(),
        Err(error) => record_failure(
            failure,
            "reading impact threshold",
            "read the impact threshold",
            &error,
        ),
    }
    match client.read_config(SUPPRESS_CONFIG).await {
        Ok(value) => fields.suppress_ms = value.to_string(),
        Err(error) => record_failure(
            failure,
            "reading suppress time",
            "read the suppress time",
            &error,
        ),
    }
    match client.get_shot_delay().await {
        Ok(value) => fields.shot_delay = value.to_string(),
        Err(error) => record_failure(failure, "reading shot delay", "read the shot delay", &error),
    }
    fields
}

async fn read_sensors(
    client: Option<&DeviceClient>,
    sensors: SensorFamily,
) -> eros::Result<SensorFields> {
    let client = client.ok_or_else(|| eros::error!("not connected to the hub"))?;
    match sensors {
        SensorFamily::Paj => {
            let near_field = paj_load_from_device(client, NEAR_FIELD_PORT).await;
            let wide_field = paj_load_from_device(client, WIDE_FIELD_PORT).await;
            match (near_field, wide_field) {
                (Ok(near_field), Ok(wide_field)) => Ok(SensorFields::Paj {
                    near_field: Box::new(near_field),
                    wide_field: Box::new(wide_field),
                }),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        SensorFamily::Pag => Ok(SensorFields::Pag(pag_load_from_device(client).await?)),
    }
}

/// Writes a validated snapshot to the device. Both outcomes are a status for
/// the user, and the error side is the early-exit path.
pub(super) async fn write_to_device(job: WriteJob) -> OpStatus {
    try_write(job).await.unwrap_or_else(|failed| failed)
}

async fn try_write(job: WriteJob) -> Result<OpStatus, OpStatus> {
    let not_connected =
        |log_context, action| write_failure(log_context, action, &"not connected to the hub");
    let client = job
        .client
        .ok_or_else(|| not_connected("writing general config", "write the general settings"))?
        .device(job.device);
    write_general_config(&client, job.general)
        .await
        .map_err(|error| {
            write_failure(
                "writing general config",
                "write the general settings",
                &error,
            )
        })?;

    write_sensors(&client, job.sensors).await?;

    // Flash last: it persists whatever the device currently holds, so it
    // must only run once every write above has succeeded.
    if !job.persist {
        return Ok(OpStatus::Done(
            "Applied for this session. Power cycling restores the saved values.".into(),
        ));
    }
    client.flash_settings().await.map_err(|error| {
        write_failure(
            "flashing settings",
            "save the settings to the device",
            &error,
        )
    })?;
    Ok(OpStatus::Done("Saved to the device".into()))
}

async fn write_sensors(
    client: &DeviceClient,
    sensors: Option<SensorValues>,
) -> Result<(), OpStatus> {
    let sensor_failure = |error: eros::ErrorUnion| {
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
            for (port, values) in [(NEAR_FIELD_PORT, near_field), (WIDE_FIELD_PORT, wide_field)] {
                paj_apply_to_device(client, port, &values)
                    .await
                    .map_err(sensor_failure)?;
            }
        }
        Some(SensorValues::Pag(values)) => {
            pag_apply_to_device(client, &values)
                .await
                .map_err(sensor_failure)?;
        }
    }
    Ok(())
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
