//! Tailwind class strings shared by the settings panels.

/// Text inputs and dropdowns share one look.
pub(super) const FIELD_CLASS: &str = "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg \
    focus:ring-blue-500 focus:border-blue-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-gray-600 dark:text-white \
    dark:focus:ring-blue-500 dark:focus:border-blue-500";

pub(super) const INVALID_FIELD_CLASS: &str = "bg-gray-50 border border-red-500 text-gray-900 text-sm rounded-lg \
    focus:ring-red-500 focus:border-red-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-red-500 dark:text-white \
    dark:focus:ring-red-500 dark:focus:border-red-500";

pub(super) const LABEL_CLASS: &str = "text-sm text-gray-700 dark:text-gray-300 whitespace-nowrap";

/// Label column plus input column of a settings form.
pub(super) const FIELD_GRID_CLASS: &str =
    "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center";

pub(super) const SECTION_HEADING_CLASS: &str =
    "text-sm font-semibold text-gray-800 dark:text-gray-200";

pub(super) const HELP_TEXT_CLASS: &str = "text-xs text-gray-500 dark:text-gray-400";

pub(super) const DISCLOSURE_SUMMARY_CLASS: &str = "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
    hover:text-blue-600 dark:hover:text-blue-400 select-none";

// Disabled styling matters here: the write buttons go dead whenever a field is
// invalid, and a button that looks live but ignores clicks reads as a bug.
pub(super) const ACTION_PRIMARY_CLASS: &str =
    "btn-primary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";

pub(super) const ACTION_SECONDARY_CLASS: &str =
    "btn-secondary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";
