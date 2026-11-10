//! The main window: sidebar, current page, and the app-wide services that live
//! as long as it does (hub server, hub connection, tray, UI-state writer).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled, Subscription, Window,
    WindowOptions, div, px, size,
};
use odyssey_hub_common::config::ConfigCorruptionEvent;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::components::ui::{self, ButtonKind, button};
use crate::config_corruption::ConfigCorruptionModal;
use crate::hub::{Hub, HubHandle};
use crate::server_status::{ServerExit, ShellMessage, handle_server_status};
use crate::theme::{self, ActivePalette, Palette, RADIUS};
use crate::ui_state::UiStateWriter;
use crate::views::calibrate::automatic::CalibrateAutomaticView;
use crate::views::calibrate::automatic::wizard::{Stage, Wizard};
use crate::workspace::{Workspace, WorkspaceHandle};
use crate::{cli, runtime, tray, views};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Home,
    Pairing,
    Accessories,
    Devices,
    Settings,
}

impl Page {
    /// Every page in the sidebar, in order.
    const ALL: [(&'static str, Self); 4] = [
        ("Setup", Self::Home),
        ("Devices", Self::Devices),
        ("Accessories", Self::Accessories),
        ("Settings", Self::Settings),
    ];
}

struct Shell {
    page: Page,
    view: AnyView,
    corruption: Entity<ConfigCorruptionModal>,
    wizard: Option<Entity<Wizard>>,
    calibration: Option<Entity<CalibrateAutomaticView>>,
    pending_page: Option<Page>,
    _appearance: Subscription,
    _context: Vec<Subscription>,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        apply_appearance(window, cx);
        let workspace = cx.global::<WorkspaceHandle>().0.clone();
        let monitors = workspace.read(cx).monitors.clone();
        let hub = cx.global::<HubHandle>().0.clone();
        let context = vec![
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.observe(&monitors, |_, _, cx| cx.notify()),
            cx.observe(&hub, |_, _, cx| cx.notify()),
        ];
        Self {
            page: Page::Home,
            view: Self::build(Page::Home, window, cx),
            corruption: cx.new(|_| ConfigCorruptionModal::new()),
            wizard: None,
            calibration: None,
            pending_page: None,
            _context: context,
            _appearance: window.observe_window_appearance(apply_appearance),
        }
    }

    /// Builds the page fresh on every visit: pages own background work (BLE
    /// scans, device streams) that has to stop when the user leaves.
    fn build(page: Page, window: &mut Window, cx: &mut Context<Self>) -> AnyView {
        match page {
            Page::Home | Page::Devices => {
                let home = cx.new(|cx| {
                    if page == Page::Devices {
                        views::HomeView::devices(window, cx)
                    } else {
                        views::HomeView::new(window, cx)
                    }
                });
                cx.subscribe_in(
                    &home,
                    window,
                    |this, _, event: &views::HomeEvent, window, cx| {
                        let page = match event {
                            views::HomeEvent::OpenPairing => Page::Pairing,
                            views::HomeEvent::OpenDevices => Page::Devices,
                            views::HomeEvent::BackToSetup => Page::Home,
                        };
                        this.navigate(page, window, cx);
                    },
                )
                .detach();
                home.into()
            }
            Page::Pairing => {
                let pairing = cx.new(|cx| views::PairingView::new(window, cx));
                cx.subscribe_in(
                    &pairing,
                    window,
                    |this, _, _: &views::pairing::PairingEvent, window, cx| {
                        this.navigate(Page::Home, window, cx);
                    },
                )
                .detach();
                pairing.into()
            }
            Page::Accessories => cx.new(|cx| views::AccessoriesView::new(window, cx)).into(),
            Page::Settings => cx.new(|cx| views::SettingsView::new(window, cx)).into(),
        }
    }

    fn navigate(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(wizard) = &self.wizard {
            let wizard = wizard.read(cx);
            if wizard.saving || !matches!(wizard.stage, Stage::Setup | Stage::Saved) {
                self.pending_page = Some(page);
                cx.notify();
                return;
            }
        }
        self.finish_calibration(cx);
        self.pending_page = None;
        if self.page == page {
            cx.notify();
            return;
        }
        self.page = page;
        self.view = Self::build(page, window, cx);
        cx.notify();
    }

    fn finish_calibration(&mut self, cx: &mut Context<Self>) {
        if let Some(wizard) = self.wizard.take() {
            wizard.update(cx, Wizard::back_to_setup);
            self.calibration = None;
            let workspace = cx.global::<WorkspaceHandle>().0.clone();
            workspace.update(cx, Workspace::refresh_calibrations);
        }
    }

    fn begin_calibration(
        &mut self,
        args: Option<cli::CalibrateArgs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.calibration.is_some() {
            return;
        }
        let wizard = cx.new(|cx| Wizard::new(args, window, cx));
        cx.observe(&wizard, |_, _, cx| cx.notify()).detach();
        self.calibration =
            Some(cx.new(|cx| CalibrateAutomaticView::new(wizard.clone(), window, cx)));
        self.wizard = Some(wizard);
        cx.notify();
    }

    fn leave_prompt(&self, p: Palette, cx: &mut Context<Self>) -> gpui::Div {
        let saving = self
            .wizard
            .as_ref()
            .is_some_and(|wizard| wizard.read(cx).saving);
        div().absolute().size_full().flex().items_center().justify_center().bg(gpui::black().opacity(0.55))
            .child(ui::card(p).w_full().max_w(px(480.0)).m_6().gap_4()
                .child(ui::card_title(p, if saving { "Saving calibration" } else { "Leave calibration?" }))
                .child(div().text_sm().child(if saving { "Wait for the save to finish before leaving. Your accepted result is being written." } else { "This stops the camera and discards the unfinished measurement. Saved calibrations stay unchanged." }))
                .child(div().flex().flex_wrap().gap_3()
                    .child(button("keep-calibrating", "Stay here", ButtonKind::Primary, p)
                        .on_click(cx.listener(|this, _, _, cx| { this.pending_page = None; cx.notify(); })))
                    .child(button("leave-calibration", "Discard and leave", ButtonKind::Danger, p).disabled(saving)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let page = this.pending_page.take();
                            this.finish_calibration(cx);
                            if let Some(page) = page { this.navigate(page, window, cx); }
                        })))))
    }
}

/// Points the app palette and the gpui-component widgets at the window's
/// appearance. Other windows read the same globals, so this runs from the main
/// window's observer rather than from any one render.
fn apply_appearance(window: &mut Window, cx: &mut App) {
    cx.set_global(ActivePalette(Palette::for_appearance(window.appearance())));
    gpui::component::Theme::sync_system_appearance(Some(window), cx);
    let input_border = theme::palette(cx).fg_muted.into();
    gpui::component::Theme::update(cx, |theme| theme.colors.input = input_border);
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = theme::palette(cx);
        let links = Page::ALL.into_iter().fold(
            div().flex().flex_col().gap_1().text_sm(),
            |links, (label, page)| {
                let active = self.page == page;
                links.child(
                    div()
                        .id(label)
                        .flex()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded(px(RADIUS))
                        .cursor_pointer()
                        .font_weight(if active {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::MEDIUM
                        })
                        .when(active, |link| {
                            link.bg(palette.accent_soft).text_color(palette.accent_ink)
                        })
                        .when(!active, |link| {
                            link.text_color(palette.fg_muted).hover(|link| {
                                link.bg(palette.surface_hover).text_color(palette.fg_strong)
                            })
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.navigate(page, window, cx);
                        }))
                        .child(label),
                )
            },
        );
        let content: AnyView = self
            .calibration
            .as_ref()
            .map_or_else(|| self.view.clone(), |view| view.clone().into());
        let leave_prompt = self.pending_page.map(|_| self.leave_prompt(palette, cx));
        let workspace = cx.global::<WorkspaceHandle>().0.read(cx);
        let monitors = workspace.monitors.read(cx);
        let display_label = monitors.selected().map_or_else(
            || "No display selected".to_owned(),
            |index| format!("Display {}", index + 1),
        );
        let hub = cx.global::<HubHandle>().0.read(cx);
        let tracker_label = workspace.selected_device.map_or_else(
            || "No tracker selected".to_owned(),
            |uuid| {
                hub.devices
                    .iter()
                    .find(|device| device.uuid == uuid)
                    .map_or_else(
                        || "Selected tracker disconnected".to_owned(),
                        views::device_label,
                    )
            },
        );
        let context_bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .border_b_1()
            .border_color(palette.line)
            .bg(palette.surface)
            .px_6()
            .py_3()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_sm()
                    .text_color(palette.fg_muted)
                    .child(format!("{display_label} · {tracker_label}")),
            )
            .child(
                button(
                    "change-context",
                    "Change selection",
                    ButtonKind::Ghost,
                    palette,
                )
                .small()
                .on_click(cx.listener(|this, _, window, cx| this.navigate(Page::Home, window, cx))),
            );

        div()
            .relative()
            .size_full()
            .flex()
            .bg(palette.canvas)
            .text_color(palette.fg)
            .child(
                div()
                    .w(px(144.0))
                    .h_full()
                    .flex_none()
                    .bg(palette.surface_alt)
                    .border_r_1()
                    .border_color(palette.line)
                    .px_3()
                    .py_4()
                    .child(
                        div()
                            .px_3()
                            .pb_4()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(palette.accent_ink)
                            .child("Odyssey"),
                    )
                    .child(links),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(context_bar)
                    .child(
                        div()
                            .id("page")
                            .flex_1()
                            .min_h_0()
                            .h_0()
                            .overflow_y_scroll()
                            .when(self.calibration.is_some(), Styled::overflow_hidden)
                            .child(content),
                    ),
            )
            .child(self.corruption.clone())
            .children(leave_prompt)
    }
}

struct MainWindow {
    window: gpui::AnyWindowHandle,
    shell: Entity<Shell>,
}

impl gpui::Global for MainWindow {}

/// Brings the main window forward on Home, where Zero lives.
pub(crate) fn show_home(cx: &mut App) {
    let main = cx.global::<MainWindow>();
    let (window, shell) = (main.window, main.shell.clone());
    cx.activate(true);
    window
        .update(cx, |_, window, cx| {
            shell.update(cx, |shell, cx| shell.navigate(Page::Home, window, cx));
            window.activate_window();
        })
        .ok();
}

pub(crate) fn activate_main(cx: &mut App) {
    let window = cx.global::<MainWindow>().window;
    cx.activate(true);
    window
        .update(cx, |_, window, _| window.activate_window())
        .ok();
}

pub(crate) fn show_calibration(args: Option<cli::CalibrateArgs>, cx: &mut App) {
    let main = cx.global::<MainWindow>();
    let (window, shell) = (main.window, main.shell.clone());
    cx.defer(move |cx| {
        window
            .update(cx, |_, window, cx| {
                shell.update(cx, |shell, cx| shell.begin_calibration(args, window, cx));
                window.activate_window();
            })
            .ok();
    });
}

pub(crate) fn finish_calibration(cx: &mut App) {
    let main = cx.global::<MainWindow>();
    let (window, shell) = (main.window, main.shell.clone());
    window
        .update(cx, |_, window, cx| {
            shell.update(cx, |shell, cx| shell.navigate(shell.page, window, cx));
            window.activate_window();
        })
        .ok();
}

/// Opens the main window and starts the services behind it.
pub(crate) fn launch(cx: &mut App) {
    gpui::init(cx);
    cx.set_global(ActivePalette(Palette::for_appearance(
        cx.window_appearance(),
    )));
    cx.set_global(UiStateWriter::start());

    let (server_ready_tx, server_ready) = watch::channel(false);
    let hub = cx.new(|cx| Hub::new(server_ready, cx));
    cx.set_global(HubHandle(hub));
    let workspace = cx.new(Workspace::new);
    cx.set_global(WorkspaceHandle(workspace));

    let options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Odyssey".into()),
            ..Default::default()
        }),
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            size(px(1100.0), px(760.0)),
            cx,
        ))),
        app_id: Some("odyssey-desktop".into()),
        ..Default::default()
    };
    let (window, shell) = match gpui::open_window(options, cx, |window, cx| {
        // Closing the window only hides it, so the tray is the way back.
        window.on_window_should_close(cx, |window, _| {
            window.minimize_window();
            false
        });
        cx.new(|cx| Shell::new(window, cx))
    }) {
        Ok(opened) => opened,
        Err(error) => {
            tracing::error!("Could not open the main window: {error:#}");
            cx.quit();
            return;
        }
    };

    cx.set_global(MainWindow {
        window,
        shell: shell.clone(),
    });
    let cancel = CancellationToken::new();
    let corruption = shell.read(cx).corruption.clone();
    tray::start(cancel.clone(), window, cx);
    start_server(window, corruption, cancel, server_ready_tx, cx);

    if cli::has_pending_calibrate() {
        views::calibrate::automatic::open(cli::take_pending_calibrate(), cx);
    }
}

/// Runs the hub server for the life of the app and applies its status to the
/// window. Another instance owning the hub closes this one.
fn start_server(
    window: gpui::AnyWindowHandle,
    corruption: Entity<ConfigCorruptionModal>,
    cancel: CancellationToken,
    server_ready: watch::Sender<bool>,
    cx: &mut App,
) {
    let (status_tx, status_rx) = mpsc::unbounded_channel();
    let (shell_tx, mut shell_rx) = mpsc::unbounded_channel::<ShellMessage>();

    // Cancelling must not detach a live hub server: the guard stops it when
    // the status task ends for any reason.
    let server_cancel = cancel.child_token();
    runtime::spawn(async move {
        let _stop_server = server_cancel.clone().drop_guard();
        let server = tokio::spawn(odyssey_hub_server::run_server(
            status_tx,
            server_cancel,
            cli::server_options(),
        ));
        let exit = handle_server_status(status_rx, shell_tx.clone(), server_ready).await;
        match server.await {
            Ok(Ok(())) => tracing::info!("Server task ended"),
            Ok(Err(error)) => tracing::error!("Hub server failed: {error}"),
            Err(error) => tracing::error!("Hub server task failed: {error}"),
        }
        let _ = shell_tx.send(ShellMessage::Exit(exit));
    });

    cx.spawn(async move |cx| {
        while let Some(message) = shell_rx.recv().await {
            match message {
                ShellMessage::ConfigCorruptions(corruptions) => {
                    show_corruptions(&corruption, corruptions, cx);
                }
                ShellMessage::BringToFront => {
                    cx.update(|cx| {
                        cx.activate(true);
                        window
                            .update(cx, |_, window, _| window.activate_window())
                            .ok();
                    });
                }
                ShellMessage::Exit(ServerExit::AnotherInstanceRunning) => {
                    tracing::info!("Closing app due to another instance running");
                    cx.update(|cx| cx.quit());
                    return;
                }
                ShellMessage::Exit(ServerExit::Stopped) => {}
            }
        }
    })
    .detach();
}

fn show_corruptions(
    corruption: &Entity<ConfigCorruptionModal>,
    corruptions: Vec<ConfigCorruptionEvent>,
    cx: &mut gpui::AsyncApp,
) {
    corruption.update(cx, |modal, cx| modal.show(corruptions, cx));
}
