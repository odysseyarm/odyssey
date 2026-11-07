//! Step 1: what is being calibrated, which screen, which camera, and how big
//! it is (known from the display, typed in, or left to the camera).

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};

use super::camera_picker::camera_picker;
use super::kind_chooser::kind_chooser;
use super::screen_number_picker::screen_number_picker;
use super::size_box::SizeBox;
use super::size_section::size_section;
use crate::components::screen_picker::ScreenPicker;
use crate::components::ui::{ButtonKind, button, card};
use crate::theme::{Palette, palette};
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::reference_helper::ReferenceHelper;
use crate::views::calibrate::automatic::scale_board_info::ScaleBoardInfo;
use crate::views::calibrate::automatic::sdk_download::{
    DownloadSdk, DownloadedSdkRow, SdkVersionNote,
};
use crate::views::calibrate::automatic::ui::{act, amber_alert, heading, plain_details, step_page};
use crate::views::calibrate::automatic::wizard::Wizard;

/// Open/closed state of the collapsible parts of the page.
struct Opens {
    camera: Entity<bool>,
    size: Entity<bool>,
    scale_board: Entity<bool>,
}

pub(in crate::views::calibrate::automatic) struct SetupStep {
    wizard: Entity<Wizard>,
    screen_picker: Entity<ScreenPicker>,
    size_box: Entity<SizeBox>,
    helper: Entity<ReferenceHelper>,
    scale_board: Entity<ScaleBoardInfo>,
    sdk_note: Entity<SdkVersionNote>,
    sdk_row: Entity<DownloadedSdkRow>,
    sdk_download: Entity<DownloadSdk>,
    kind_focus: [FocusHandle; 2],
    opens: Opens,
    _subscriptions: Vec<Subscription>,
}

impl SetupStep {
    pub(in crate::views::calibrate::automatic) fn new(
        wizard: Entity<Wizard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let monitors = wizard.read(cx).monitors.clone();
        Self {
            screen_picker: cx.new(|cx| ScreenPicker::new(monitors, window, cx)),
            size_box: cx.new(|cx| SizeBox::new(wizard.clone(), window, cx)),
            helper: cx.new(|cx| ReferenceHelper::new(wizard.clone(), window, cx)),
            scale_board: cx.new(ScaleBoardInfo::new),
            sdk_note: cx.new(|cx| SdkVersionNote::new(wizard.clone(), cx)),
            sdk_row: cx.new(|cx| DownloadedSdkRow::new(wizard.clone(), cx)),
            sdk_download: cx.new(|cx| DownloadSdk::new(wizard.clone(), cx)),
            kind_focus: [cx.focus_handle(), cx.focus_handle()],
            opens: Opens {
                camera: cx.new(|_| false),
                size: cx.new(|_| false),
                scale_board: cx.new(|_| false),
            },
            _subscriptions: vec![cx.observe(&wizard, |_, _, cx| cx.notify())],
            wizard,
        }
    }

    fn section(p: Palette, title: impl Into<gpui::SharedString>, gap: bool) -> gpui::Div {
        let section = card(p);
        let section = if gap {
            section.gap_4()
        } else {
            section.gap_3()
        };
        section.child(heading(p, title))
    }
}

impl Render for SetupStep {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let wizard = self.wizard.clone();
        let state = wizard.read(cx);
        let can_continue = state.can_continue();

        let size_title = match state.kind() {
            DisplayKind::Flat => "4. How big is the screen?",
            DisplayKind::Projector => "4. How big is the picture? (optional)",
        };

        let mut content: Vec<AnyElement> = state
            .notes
            .iter()
            .map(|note| amber_alert(p, note.clone()))
            .collect();
        content.push(
            Self::section(p, "1. What are you calibrating?", true)
                .child(kind_chooser(p, &wizard, &self.kind_focus, cx))
                .into_any_element(),
        );
        content.push(
            Self::section(p, "2. Which screen?", true)
                .child(self.screen_picker.clone())
                .child(screen_number_picker(p, &wizard, cx))
                .into_any_element(),
        );
        content.push(
            Self::section(p, "3. Which camera?", false)
                .child(camera_picker(
                    p,
                    &wizard,
                    &self.sdk_download,
                    &self.opens.camera,
                    cx,
                ))
                .child(self.sdk_note.clone())
                .child(self.sdk_row.clone())
                .into_any_element(),
        );
        content.push(
            Self::section(p, size_title, false)
                .child(size_section(p, &wizard, &self.size_box, &self.opens.size, cx))
                .child(self.helper.clone())
                .child(plain_details(
                    "scale-board",
                    p,
                    "Scale board (optional, prototype)",
                    vec![
                        div()
                            .child("Print files for the scale board. It does not change your calibration yet.")
                            .into_any_element(),
                        self.scale_board.clone().into_any_element(),
                    ],
                    &self.opens.scale_board,
                    cx,
                ))
                .into_any_element(),
        );

        step_page(
            p,
            "Let's calibrate a screen",
            Some("A few quick questions, then we'll point the camera at the screen.".into()),
            content,
            vec![
                div().into_any_element(),
                button("continue", "Continue", ButtonKind::Primary, p)
                    .disabled(!can_continue)
                    .on_click(act(&wizard, |w, cx| w.begin_position(cx)))
                    .into_any_element(),
            ],
        )
    }
}
