//! Choosing the camera, and what to show when there is none.

use gpui::{
    AnyElement, App, Div, Entity, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::components::ui::{ButtonKind, button};
use crate::components::wizard::{Icon, icon};
use crate::sdk_install::{self, Delivery};
use crate::theme::{Palette, RADIUS, RADIUS_LG};
use crate::views::calibrate::automatic::camera_support::{self, Notice, SDK_DOWNLOAD_URL};
use crate::views::calibrate::automatic::sdk_download::DownloadSdk;
use crate::views::calibrate::automatic::ui::{act, amber_alert, plain_details, working_row};
use crate::views::calibrate::automatic::wizard::{CameraList, Wizard};

pub(super) fn camera_picker(
    p: &Palette,
    wizard: &Entity<Wizard>,
    download: &Entity<DownloadSdk>,
    details_open: &Entity<bool>,
    cx: &App,
) -> AnyElement {
    let state = wizard.read(cx);
    let chosen = state.camera();

    match &state.cameras {
        CameraList::Searching => working_row(p, "Looking for cameras…").into_any_element(),
        CameraList::Failed(error) => no_camera(
            p,
            "Couldn't look for cameras".into(),
            "Unplug the camera and plug it back in. If this keeps happening, restart the app."
                .into(),
            vec![error.clone()],
            None,
            details_open,
            cx,
        ),
        CameraList::Unavailable(status) => match camera_support::notice(status) {
            Some(notice) => camera_support_notice(p, wizard, notice, download, details_open, cx),
            None => div().into_any_element(),
        },
        CameraList::Found(list) if list.is_empty() => no_camera(
            p,
            "No camera found".into(),
            "Plug in the Orbbec camera with its USB cable. This page notices by itself.".into(),
            Vec::new(),
            None,
            details_open,
            cx,
        ),
        CameraList::Found(list) => {
            let mut col = div().flex().flex_col().gap_2();
            for (index, camera) in list.iter().enumerate() {
                let accent_ink = p.accent_ink;
                let selected = chosen.as_ref().is_some_and(|c| c.id == camera.id);
                let (border, bg) = if selected {
                    (p.accent_ink, p.accent_soft)
                } else {
                    (p.line, p.surface)
                };
                let id = camera.id.clone();
                let wizard = wizard.clone();
                let mut info = div().flex().flex_col().min_w_0().child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.fg_strong)
                        .child(camera.name.clone()),
                );
                if !camera.serial.is_empty() {
                    info = info.child(
                        div()
                            .text_sm()
                            .text_color(p.fg_muted)
                            .child(format!("Serial {}", camera.serial)),
                    );
                }
                let mut row = div()
                    .id(("camera", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(RADIUS))
                    .border_2()
                    .border_color(border)
                    .bg(bg)
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .child(icon(Icon::Camera, 28.0, p.accent_ink))
                    .child(info)
                    .on_click(move |_, _, cx| {
                        let id = id.clone();
                        wizard.update(cx, |w, cx| {
                            w.camera_choice = Some(id);
                            cx.notify();
                        });
                    });
                if !selected {
                    row = row.hover(move |s| s.border_color(accent_ink));
                }
                if selected {
                    row = row.child(div().ml_auto().child(icon(Icon::Check, 24.0, p.accent_ink)));
                }
                col = col.child(row);
                if let Some(warning) = camera.link.warning() {
                    col = col.child(amber_alert(p, warning));
                }
            }
            col.into_any_element()
        }
    }
}

fn no_camera(
    p: &Palette,
    title: String,
    detail: String,
    technical: Vec<String>,
    extra: Option<AnyElement>,
    details_open: &Entity<bool>,
    cx: &App,
) -> AnyElement {
    let mut col: Div = div()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .rounded(px(RADIUS_LG))
        .border_2()
        .border_dashed()
        .border_color(p.line)
        .px_4()
        .py_6()
        .text_center()
        .child(icon(Icon::Camera, 40.0, p.fg_faint))
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.fg_strong)
                .child(title),
        )
        .child(div().text_base().text_color(p.fg_muted).child(detail));
    if let Some(extra) = extra {
        col = col.child(extra);
    }
    if !technical.is_empty() {
        let lines = technical
            .into_iter()
            .map(|line| div().child(line).into_any_element())
            .collect();
        col = col.child(div().w_full().text_left().child(plain_details(
            "camera-details",
            p,
            "Details",
            lines,
            details_open,
            cx,
        )));
    }
    col.into_any_element()
}

/// The camera library is missing, broken or the wrong version. The page keeps
/// checking by itself. The button only saves the wait.
fn camera_support_notice(
    p: &Palette,
    wizard: &Entity<Wizard>,
    notice: Notice,
    download: &Entity<DownloadSdk>,
    details_open: &Entity<bool>,
    cx: &App,
) -> AnyElement {
    let mut actions = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap_3()
        .pt_2();
    if notice.offer_download {
        actions = match sdk_install::delivery() {
            Delivery::Link => actions.child(
                button("get-sdk", "Get the Orbbec SDK", ButtonKind::Primary, p)
                    .on_click(|_, _, cx| cx.open_url(SDK_DOWNLOAD_URL)),
            ),
            Delivery::Download => actions.child(download.clone()),
            Delivery::Installer => actions.child(
                div().max_w(px(576.0)).text_sm().text_color(p.fg_muted).child(
                    "This copy of Odyssey doesn't download the camera software. To add Orbbec camera support, run the Odyssey installer again, choose Change, and tick \"Orbbec camera support\".",
                ),
            ),
            Delivery::Offline => actions.child(
                div().max_w(px(576.0)).text_sm().text_color(p.fg_muted).child(
                    "Offline mode is on, so Odyssey won't download the camera software. Install it from the full Odyssey installer (the \"Orbbec camera support\" option), or ask your administrator.",
                ),
            ),
        };
    }
    actions = actions.child(
        button("check-again", "Check again", ButtonKind::Secondary, p)
            .on_click(act(wizard, Wizard::request_camera_list)),
    );
    no_camera(
        p,
        notice.title,
        notice.detail,
        notice.details,
        Some(actions.into_any_element()),
        details_open,
        cx,
    )
}
