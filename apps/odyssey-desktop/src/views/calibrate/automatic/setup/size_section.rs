//! The size question, which depends on what is being calibrated.

use dioxus::prelude::*;

use super::flat_size::FlatSize;
use super::projector_size::ProjectorSize;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::wizard::Wizard;

#[component]
pub(super) fn SizeSection(wizard: Wizard) -> Element {
    match wizard.kind() {
        DisplayKind::Flat => rsx! { FlatSize { wizard } },
        DisplayKind::Projector => rsx! { ProjectorSize { wizard } },
    }
}
