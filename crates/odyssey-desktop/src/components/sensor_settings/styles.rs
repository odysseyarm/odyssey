//! Tailwind class strings shared by the settings panels.

/// Text inputs and dropdowns share one look, tighter than a default field so
/// the two-column grid stays compact.
pub(super) const FIELD_CLASS: &str = "field block w-full px-2 py-1.5";

pub(super) const INVALID_FIELD_CLASS: &str = "field field-invalid block w-full px-2 py-1.5";

pub(super) const LABEL_CLASS: &str = "label whitespace-nowrap";

/// Label column plus input column of a settings form.
pub(super) const FIELD_GRID_CLASS: &str =
    "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center";

pub(super) const SECTION_HEADING_CLASS: &str = "text-sm font-semibold text-fg-strong";

pub(super) const DISCLOSURE_SUMMARY_CLASS: &str =
    "text-sm font-medium text-fg cursor-pointer hover:text-accent-ink select-none";
