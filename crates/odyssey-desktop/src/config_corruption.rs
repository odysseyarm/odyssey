use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use odyssey_hub_common::config::ConfigCorruptionEvent;

use crate::components::ui;
use crate::runtime;
use crate::theme::{self, RADIUS, RADIUS_XL};

/// Removes corrupted files and returns the entries that could not be deleted.
/// A file that is already gone needs no warning.
async fn delete_corrupted_files(
    corruptions: Vec<ConfigCorruptionEvent>,
) -> Vec<ConfigCorruptionEvent> {
    let mut remaining = Vec::new();
    for corruption in corruptions {
        match tokio::fs::remove_file(&corruption.file_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::error!(
                    "Could not delete corrupted config file {}: {error}",
                    corruption.file_path
                );
                remaining.push(corruption);
            }
        }
    }
    remaining
}

/// Tells the user which config files were unreadable at startup and offers to
/// delete them or carry on with defaults.
pub(crate) struct ConfigCorruptionModal {
    corruptions: Vec<ConfigCorruptionEvent>,
    deleting: bool,
    delete_failed: bool,
}

impl ConfigCorruptionModal {
    pub(crate) fn new() -> Self {
        Self {
            corruptions: Vec::new(),
            deleting: false,
            delete_failed: false,
        }
    }

    pub(crate) fn show(&mut self, corruptions: Vec<ConfigCorruptionEvent>, cx: &mut Context<Self>) {
        self.corruptions = corruptions;
        self.delete_failed = false;
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        self.corruptions.clear();
        cx.notify();
    }

    fn delete_files(&mut self, cx: &mut Context<Self>) {
        if self.deleting {
            return;
        }
        self.deleting = true;
        let reported = self.corruptions.clone();
        let unchanged = reported.clone();
        let deletion = runtime::spawn(delete_corrupted_files(reported));
        cx.spawn(async move |this, cx| {
            let remaining = deletion.await.unwrap_or(unchanged);
            this.update(cx, |this, cx| {
                this.delete_failed = !remaining.is_empty();
                this.corruptions = remaining;
                this.deleting = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for ConfigCorruptionModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.corruptions.is_empty() {
            return div().into_any_element();
        }
        let palette = theme::palette(cx);
        let count = self.corruptions.len();
        let intro = if count == 1 {
            "The following configuration file was corrupted and could not be loaded. Default values are being used instead:"
        } else {
            "The following configuration files were corrupted and could not be loaded. Default values are being used instead:"
        };
        let deleting = self.deleting;

        let list =
            self.corruptions
                .iter()
                .fold(div().flex().flex_col().gap_3(), |list, corruption| {
                    list.child(
                        div()
                            .bg(palette.surface_alt)
                            .rounded(px(RADIUS))
                            .p_4()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(palette.fg_strong)
                                    .child(format!("{:?}", corruption.file_type)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(palette.fg_muted)
                                    .child(format!("Path: {}", corruption.file_path)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(palette.danger_ink)
                                    .child(format!("Error: {}", corruption.error_message)),
                            ),
                    )
                });

        let mut card = div()
            .id("config-corruption-card")
            .occlude()
            .bg(palette.surface)
            .rounded(px(RADIUS_XL))
            .shadow_lg()
            .max_w(px(672.0))
            .w_full()
            .max_h(px(560.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div().bg(palette.warning).px_6().py_4().child(
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(palette.on_warning)
                        .child("⚠️ Configuration File Warning"),
                ),
            )
            .child(
                div()
                    .id("config-corruption-body")
                    .p_6()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_color(palette.fg)
                    .child(div().mb_4().child(intro))
                    .child(list)
                    .child(ui::alert(
                        ui::AlertKind::Info,
                        palette,
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_sm()
                            .child("You can continue using the application with default settings. To fix this issue, you can either:")
                            .child("• Delete the corrupted file(s) to reset to defaults")
                            .child("• Manually fix the JSON syntax errors in the file(s)")
                            .child("• Restore from a backup if available"),
                    )),
            );
        if self.delete_failed {
            card = card.child(
                div()
                    .px_6()
                    .py_3()
                    .text_sm()
                    .text_color(palette.danger_ink)
                    .child("Some files could not be deleted. Check their permissions and try again, or continue with defaults."),
            );
        }
        let card = card.child(
            div()
                .bg(palette.surface_alt)
                .px_6()
                .py_4()
                .flex()
                .justify_end()
                .gap_3()
                .child(
                    ui::button(
                        "delete-corrupted",
                        if deleting {
                            "Deleting…"
                        } else {
                            "Delete Files"
                        },
                        ui::ButtonKind::Danger,
                        palette,
                    )
                    .disabled(deleting)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_files(cx))),
                )
                .child(
                    ui::button(
                        "continue-defaults",
                        "Continue with Defaults",
                        ui::ButtonKind::Primary,
                        palette,
                    )
                    .disabled(deleting)
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                ),
        );

        div()
            .id("config-corruption-backdrop")
            .absolute()
            .inset_0()
            .bg(gpui::black().opacity(0.6))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .occlude()
            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .child(card)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::config::ConfigFileType;

    use super::*;

    #[tokio::test]
    async fn deletion_keeps_only_failed_files_for_retry() {
        let root =
            std::env::temp_dir().join(format!("odyssey-corruption-delete-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("scratch directory");
        let removable = root.join("corrupt.json");
        std::fs::write(&removable, "{").expect("corrupt file");
        let missing = root.join("already-removed.json");
        let blocked = root.join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("directory cannot be removed as a file");
        let reports = [&removable, &missing, &blocked]
            .into_iter()
            .map(|path| ConfigCorruptionEvent {
                file_type: ConfigFileType::DeviceOffsets,
                file_path: path.to_string_lossy().into_owned(),
                error_message: "invalid JSON".into(),
                using_default: true,
            })
            .collect();

        let remaining = delete_corrupted_files(reports).await;

        assert_eq!(
            remaining.len(),
            1,
            "only the undeletable path remains for retry"
        );
        assert!(!removable.exists(), "the corrupted file is removed");
        assert_eq!(remaining[0].error_message, "invalid JSON");
        std::fs::remove_dir_all(root).expect("clean scratch directory");
    }
}
