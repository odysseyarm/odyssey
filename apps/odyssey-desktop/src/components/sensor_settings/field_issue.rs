use super::op_status::OpStatus;

/// A value the user typed that cannot be sent to the device.
#[derive(Clone, PartialEq)]
pub(super) struct FieldIssue {
    pub(super) field: &'static str,
    pub(super) message: String,
}

/// Parse an unsigned field and bound it, so a typo is reported instead of
/// silently turning into a default that gets written to the sensor.
pub(super) fn parse_bounded(field: &'static str, raw: &str, max: u32) -> Result<u32, FieldIssue> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(FieldIssue {
            field,
            message: "Required".into(),
        });
    }
    match raw.parse::<u32>() {
        Ok(v) if v <= max => Ok(v),
        Ok(_) => Err(FieldIssue {
            field,
            message: format!("Must be 0-{max}"),
        }),
        Err(_) => Err(FieldIssue {
            field,
            message: format!("Whole number 0-{max}"),
        }),
    }
}

pub(super) fn issue_for<'a>(issues: &'a [FieldIssue], field: &str) -> Option<&'a str> {
    issues
        .iter()
        .find(|i| i.field == field)
        .map(|i| i.message.as_str())
}

/// Summarise invalid input without repeating every message; the offending
/// fields are already marked in place.
pub(super) fn invalid_status(issues: &[FieldIssue]) -> OpStatus {
    let fields: Vec<&str> = issues.iter().map(|i| i.field).collect();
    let noun = if fields.len() == 1 { "field" } else { "fields" };
    OpStatus::Failed {
        summary: format!("Check the highlighted {noun}: {}", fields.join(", ")),
        detail: None,
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::FieldIssue;

    pub(in super::super) fn ok<T>(result: Result<T, Vec<FieldIssue>>) -> T {
        match result {
            Ok(value) => value,
            Err(issues) => panic!(
                "unexpected issues: {:?}",
                issues.iter().map(|issue| issue.field).collect::<Vec<_>>()
            ),
        }
    }

    pub(in super::super) fn issues<T>(result: Result<T, Vec<FieldIssue>>) -> Vec<FieldIssue> {
        match result {
            Ok(_) => panic!("expected issues"),
            Err(issues) => issues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<u32, FieldIssue>) -> String {
        match result {
            Ok(value) => panic!("expected issue, got {value}"),
            Err(issue) => issue.message,
        }
    }

    #[test]
    fn bounds_and_messages() {
        assert_eq!(message(parse_bounded("f", "", 255)), "Required");
        assert_eq!(
            message(parse_bounded("f", "abc", 255)),
            "Whole number 0-255"
        );
        assert_eq!(message(parse_bounded("f", "256", 255)), "Must be 0-255");
        assert!(matches!(parse_bounded("f", " 40 ", 255), Ok(40)));
    }
}
