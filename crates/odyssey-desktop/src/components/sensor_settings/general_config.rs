use odyssey_hub_client::client::DeviceClient;

use super::field_issue::{FieldChecker, FieldIssue};

/// Config `kind` ids understood by the hub's read/write config calls.
pub(super) const IMPACT_THRESHOLD_CONFIG: u8 = 0;
pub(super) const SUPPRESS_CONFIG: u8 = 1;

// Field names are shared between the validator and the form so an issue can be
// routed back to the input that produced it.
pub(super) const IMPACT_THRESHOLD_FIELD: &str = "Impact threshold";
pub(super) const SUPPRESS_FIELD: &str = "Suppress";
pub(super) const SHOT_DELAY_FIELD: &str = "Shot delay";

/// The fields that live in the device's general config, already validated.
/// The shot delay rides along because the form edits it together with them,
/// although the hub stores it on this computer rather than in the device.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct GeneralValues {
    pub(super) impact_threshold: u8,
    pub(super) suppress_ms: u8,
    pub(super) shot_delay: u16,
}

pub(super) fn general_validate(
    impact_threshold: &str,
    suppress_ms: &str,
    shot_delay: &str,
) -> Result<GeneralValues, Vec<FieldIssue>> {
    let mut check = FieldChecker::default();
    let impact_threshold = check.check_u8(IMPACT_THRESHOLD_FIELD, impact_threshold);
    let suppress_ms = check.check_u8(SUPPRESS_FIELD, suppress_ms);
    let shot_delay = check.check_u16(SHOT_DELAY_FIELD, shot_delay);

    let (Some(impact_threshold), Some(suppress_ms), Some(shot_delay)) =
        (impact_threshold, suppress_ms, shot_delay)
    else {
        return Err(check.into_issues());
    };
    Ok(GeneralValues {
        impact_threshold,
        suppress_ms,
        shot_delay,
    })
}

/// Write the general config. Callers validate first, so a typo in one field
/// cannot leave the device holding a half-applied set.
pub(super) async fn write_general_config(
    client: &DeviceClient,
    values: GeneralValues,
) -> eros::Result<()> {
    client
        .write_config(IMPACT_THRESHOLD_CONFIG, values.impact_threshold)
        .await?;
    client
        .write_config(SUPPRESS_CONFIG, values.suppress_ms)
        .await?;
    client.set_shot_delay(values.shot_delay).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sensor_settings::field_issue::test_support::issues;

    #[test]
    fn general_collects_every_issue() {
        let found = issues(general_validate("300", "", "x"));
        assert_eq!(
            found.iter().map(|issue| issue.field).collect::<Vec<_>>(),
            [IMPACT_THRESHOLD_FIELD, SUPPRESS_FIELD, SHOT_DELAY_FIELD]
        );
    }
}
