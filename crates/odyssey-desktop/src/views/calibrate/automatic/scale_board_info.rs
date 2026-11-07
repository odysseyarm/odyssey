//! The "scale board" explanation: what the printed board is for, which one to
//! print and how, and the PDFs to print. Opened from Setup as a window over the
//! page; nothing in the calibration depends on it.

use std::path::PathBuf;

use gpui::{
    Context, ElementId, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, anchored, deferred, div, point, px,
};

use super::ui::link_button;
use crate::components::ui::{ButtonKind, button};
use crate::scale_board::{self, Board};
use crate::theme::{Palette, RADIUS_XL, palette};

fn board_label(board: Board) -> &'static str {
    match board {
        Board::Letter => "US Letter (8.5 \u{d7} 11 in)",
        Board::A4 => "A4",
        Board::Rigid => "12 \u{d7} 8.5 in, for a print shop",
        Board::RigidLarge => "24 \u{d7} 17 in, for a print shop",
    }
}

/// What the last button press did.
enum Done {
    Opened(PathBuf),
    Saved(PathBuf),
}

/// A small link under the size question that opens the explanation.
pub(super) struct ScaleBoardInfo {
    open: bool,
    /// What happened to the last PDF the tech asked for.
    status: Option<Result<Done, String>>,
    focus: FocusHandle,
}

impl ScaleBoardInfo {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            open: false,
            status: None,
            focus: cx.focus_handle(),
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn save_copy(board: Board, cx: &mut Context<Self>) {
        let saving = crate::runtime::spawn(scale_board::save_copy(board));
        cx.spawn(async move |this, cx| {
            let result = match saving.await {
                Ok(result) => result,
                Err(error) => Err(format!("Couldn't save the file: {error}.")),
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Some(path)) => this.status = Some(Ok(Done::Saved(path))),
                    // Cancelled: leave whatever was shown.
                    Ok(None) => return,
                    Err(message) => this.status = Some(Err(message)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn body(p: Palette, text: &'static str) -> gpui::Div {
    div().text_base().text_color(p.fg).child(text)
}

fn heading(p: Palette, text: &'static str) -> gpui::Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.fg_strong)
        .child(text)
}

fn bullet(p: Palette, text: &'static str) -> gpui::Div {
    div()
        .flex()
        .gap_2()
        .text_base()
        .text_color(p.fg)
        .child(div().child("\u{2022}"))
        .child(div().flex_1().child(text))
}

impl Render for ScaleBoardInfo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let link = link_button(
            "scale-board-open",
            p,
            "About the scale board (experimental)",
            cx.listener(|this, _, window, cx| {
                this.open = true;
                window.focus(&this.focus, cx);
                cx.notify();
            }),
        );
        let root = div().child(div().child(link));
        if !self.open {
            return root;
        }

        let viewport = window.viewport_size();
        let status = match &self.status {
            Some(Ok(Done::Opened(path))) => {
                Some(div().text_sm().text_color(p.fg_muted).child(format!(
                    "Opened {}. Print or save it from your PDF viewer.",
                    path.display()
                )))
            }
            Some(Ok(Done::Saved(path))) => Some(
                div()
                    .text_sm()
                    .text_color(p.fg_muted)
                    .child(format!("Saved {}.", path.display())),
            ),
            Some(Err(message)) => Some(
                div()
                    .text_sm()
                    .text_color(p.danger_ink)
                    .child(SharedString::from(message.clone())),
            ),
            None => None,
        };

        let mut boards = div().flex().flex_col().gap_2();
        for (i, board) in Board::ALL.into_iter().enumerate() {
            boards = boards.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .min_w(px(256.0))
                            .text_base()
                            .text_color(p.fg)
                            .child(board_label(board)),
                    )
                    .child(
                        button(
                            ElementId::from(("scale-board-print", i)),
                            "Open to print",
                            ButtonKind::Secondary,
                            p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.status = Some(scale_board::save_and_open(board).map(Done::Opened));
                            cx.notify();
                        })),
                    )
                    .child(
                        button(
                            ElementId::from(("scale-board-save", i)),
                            "Save a copy\u{2026}",
                            ButtonKind::Secondary,
                            p,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| Self::save_copy(board, cx))),
                    ),
            );
        }

        let dialog = div()
            .id("scale-board-dialog")
            .occlude()
            .flex()
            .flex_col()
            .gap_5()
            .w_full()
            .max_w(px(768.0))
            .max_h(viewport.height - px(32.0))
            .overflow_y_scroll()
            .rounded(px(RADIUS_XL))
            .bg(p.surface)
            .p_6()
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.fg_strong)
                            .child("The scale board (experimental)"),
                    )
                    .child(
                        button("scale-board-close", "Close", ButtonKind::Secondary, p)
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(body(p, "A printed sheet of black dots, designed to a known size (you check your print against its scale bar, below). It is meant to give the wizard an independent scale for sizing the screen, one that does not depend on the depth sensor or the display's reported size. That step is not built yet."))
                    .child(body(p, "What works today is a check of the camera's depth scale: stood next to the screen where the camera can see it, the camera can tell how far away the sheet is from how big it looks, and compare that with what its depth sensor says. Odyssey support does this with a separate test tool. The board is optional and does not change your calibration.")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(heading(p, "1. Pick a board and open its PDF"))
                    .child(body(p, "Start with US Letter or A4: print it on paper and glue it flat on foam board. For the best result, have a print shop print the 12 \u{d7} 8.5 in file onto rigid board. The 24 \u{d7} 17 in one is only for when the test tool says the dots look too small at your distance."))
                    .child(boards)
                    .children(status),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(p, "2. Print it"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(bullet(p, "Set the scale to Actual size or 100%. Never Fit, Shrink to printable area or Fit to page: that changes the size of everything."))
                            .child(bullet(p, "Landscape, on the paper size the file is for. Black and white is fine; use the best quality."))
                            .child(bullet(p, "Plain white matte paper. No gloss and no lamination: glare washes the dots out."))
                            .child(bullet(p, "At a print shop, ask for one copy at 100%, no scaling, and ask whether they mount prints on foam board."))
                            .child(bullet(p, "Glue it flat on 3/16 in foam board with spray adhesive or full-sheet double-sided tape, working from the middle out. Leave it flat under a weight for an hour. A wavy sheet spoils the measurement.")),
                    )
                    .child(body(p, "For a rigid print, a shop such as foamcoreprint.com (Gator Board Printing) can print the 12 \u{d7} 8.5 in file directly: custom size, 3/16 in thick, one side, no holes. Check their proof shows exactly that size, black dots on white, nothing cropped or scaled.")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(heading(p, "3. Check the scale of your print"))
                    .child(body(p, "With a ruler or tape, measure the printed bar from the middle of its left end tick to the middle of its right end tick. It should be 200 mm (7.87 in); on the 24 \u{d7} 17 in board, 400 mm. If it isn't, don't fix the printer: the test tool takes the distance between dots, not the bar length, so work it out:"))
                    .child(
                        div()
                            .font_family("monospace")
                            .text_base()
                            .text_color(p.fg_strong)
                            .child("pitch = 40 \u{d7} measured bar \u{f7} 200   (24 \u{d7} 17 in board: 80 \u{d7} measured bar \u{f7} 400)"),
                    )
                    .child(body(p, "For example, a bar that measures 201.4 mm means a pitch of 40.28 mm.")),
            );

        // A window-sized layer drawn above the page: `anchored` positions in
        // window coordinates, wherever this view sits.
        let overlay = div()
            .id("scale-board-backdrop")
            .track_focus(&self.focus)
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .w(viewport.width)
            .h(viewport.height)
            .p_4()
            .bg(gpui::black().opacity(0.6))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close(cx);
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)))
            .child(dialog);

        root.child(
            deferred(anchored().position(point(px(0.0), px(0.0))).child(overlay)).with_priority(1),
        )
    }
}
