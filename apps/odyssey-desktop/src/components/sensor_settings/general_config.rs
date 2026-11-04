use dioxus::prelude::ReadableExt as _;
use odyssey_hub_common::device::Device;

use super::field_issue::{FieldIssue, parse_bounded};
use crate::hub::HubContext;

/// The fields that live in the device's general config, already validated.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct GeneralValues {
    pub(super) impact_threshold: u8,
    pub(super) suppress_ms: u8,
    pub(super) shot_delay: Option<u16>,
}

pub(super) fn general_validate(
    impact_threshold: &str,
    suppress_ms: &str,
    shot_delay: Option<&str>,
) -> Result<GeneralValues, Vec<FieldIssue>> {
    let mut issues = Vec::new();
    let mut take = |field, raw: &str, max| match parse_bounded(field, raw, max) {
        Ok(v) => Some(v),
        Err(e) => {
            issues.push(e);
            None
        }
    };

    let impact = take("Impact threshold", impact_threshold, u32::from(u8::MAX));
    let suppress = take("Suppress", suppress_ms, u32::from(u8::MAX));
    let delay = shot_delay.map(|d| take("Shot delay", d, u32::from(u16::MAX)));

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(GeneralValues {
        impact_threshold: impact.unwrap() as u8,
        suppress_ms: suppress.unwrap() as u8,
        shot_delay: delay.map(|d| d.unwrap() as u16),
    })
}

/// Write the general config. Callers validate first, so a typo in one field
/// cannot leave the device holding a half-applied set.
pub(super) async fn write_general_config(
    hub: &HubContext,
    device: &Device,
    values: GeneralValues,
) -> anyhow::Result<()> {
    let mut client = hub.client.peek().clone();
    client
        .write_config(device.clone(), 0, values.impact_threshold)
        .await?;
    client
        .write_config(device.clone(), 1, values.suppress_ms)
        .await?;
    if let Some(delay) = values.shot_delay {
        client.set_shot_delay(device.clone(), delay).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sensor_settings::field_issue::invalid_status;
    use crate::components::sensor_settings::field_issue::test_support::issues;
    use crate::components::sensor_settings::op_status::OpStatus;

    #[test]
    fn general_collects_every_issue() {
        let found = issues(general_validate("300", "", Some("x")));
        assert_eq!(found.len(), 3);
        match invalid_status(&found) {
            OpStatus::Failed { summary, detail } => {
                assert!(summary.contains("Impact threshold"), "{summary}");
                assert!(detail.is_none());
            }
            _ => panic!("expected failure"),
        }
    }
}
