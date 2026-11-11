//! The size question, which depends on what is being calibrated.

use gpui::{AnyElement, App, Entity, IntoElement as _};

use super::flat_size::flat_size;
use super::projector_size::projector_size;
use super::size_box::SizeBox;
use crate::theme::Palette;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::wizard::Wizard;

pub(super) fn size_section(
    p: &Palette,
    wizard: &Entity<Wizard>,
    size_box: &Entity<SizeBox>,
    details_open: &Entity<bool>,
    cx: &App,
) -> AnyElement {
    match wizard.read(cx).kind() {
        DisplayKind::Flat => flat_size(p, wizard, size_box, details_open, cx).into_any_element(),
        DisplayKind::Projector => projector_size(p, wizard, size_box, cx).into_any_element(),
    }
}
