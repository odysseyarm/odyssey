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

/// The message to show next to `field`, or `None` while issues are hidden
/// (before the first load, empty fields are "not loaded yet", not "invalid").
pub(super) fn visible_issue(
    issues: &[FieldIssue],
    field: &str,
    show_issues: bool,
) -> Option<String> {
    if !show_issues {
        return None;
    }
    issue_for(issues, field).map(str::to_string)
}

/// Validates several fields in one pass, remembering every failure so the form
/// can mark all bad fields at once instead of one per attempt.
#[derive(Default)]
pub(super) struct FieldChecker {
    issues: Vec<FieldIssue>,
}

impl FieldChecker {
    /// `None` means the field was invalid and an issue was recorded.
    pub(super) fn check(&mut self, field: &'static str, raw: &str, max: u32) -> Option<u32> {
        match parse_bounded(field, raw, max) {
            Ok(value) => Some(value),
            Err(issue) => {
                self.issues.push(issue);
                None
            }
        }
    }

    pub(super) fn check_u8(&mut self, field: &'static str, raw: &str) -> Option<u8> {
        self.check_u8_up_to(field, raw, u8::MAX)
    }

    pub(super) fn check_u8_up_to(&mut self, field: &'static str, raw: &str, max: u8) -> Option<u8> {
        // `check` bounds the value by `max`, so the conversion cannot fail.
        u8::try_from(self.check(field, raw, u32::from(max))?).ok()
    }

    pub(super) fn check_u16(&mut self, field: &'static str, raw: &str) -> Option<u16> {
        self.check_u16_up_to(field, raw, u16::MAX)
    }

    pub(super) fn check_u16_up_to(
        &mut self,
        field: &'static str,
        raw: &str,
        max: u16,
    ) -> Option<u16> {
        u16::try_from(self.check(field, raw, u32::from(max))?).ok()
    }

    /// The recorded issues. Call after a failed `let ... else` on the checked
    /// values, which implies at least one was recorded.
    pub(super) fn into_issues(self) -> Vec<FieldIssue> {
        self.issues
    }
}

/// Summarise invalid input without repeating every message. The offending
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

    #[test]
    fn unsigned_fields_accept_boundaries_and_reject_invalid_input() {
        assert!(matches!(parse_bounded("f", "0", 255), Ok(0)));
        assert!(matches!(parse_bounded("f", "255", 255), Ok(255)));
        assert!(matches!(parse_bounded("f", " 40 ", 255), Ok(40)));
        for raw in ["", " ", "abc", "-1", "1.5", "256", "4294967296"] {
            match parse_bounded("value", raw, 255) {
                Ok(value) => panic!("accepted {raw:?} as {value}"),
                Err(issue) => assert_eq!(issue.field, "value"),
            }
        }
    }
}
