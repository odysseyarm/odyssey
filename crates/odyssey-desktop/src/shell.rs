//! The main window: sidebar, current page, and the app-wide services that live
//! as long as it does (hub server, hub connection, tray, UI-state writer).

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    WindowOptions, div, px, size,
};
use odyssey_hub_common::config::ConfigCorruptionEvent;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::config_corruption::ConfigCorruptionModal;
use crate::hub::{Hub, HubHandle};
use crate::server_status::{ServerExit, ShellMessage, handle_server_status};
use crate::theme::{self, ActivePalette, Palette, RADIUS};
use crate::ui_state::UiStateWriter;
use crate::{cli, runtime, tray, views};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Home,
    Pairing,
    Accessories,
    Calibrate,
    Settings,
}

impl Page {
    /// Every page in the sidebar, in order.
    const ALL: [(&'static str, Self); 5] = [
        ("Home", Self::Home),
        ("Pairing", Self::Pairing),
        ("Accessories", Self::Accessories),
        ("Calibrate", Self::Calibrate),
        ("Settings", Self::Settings),
    ];
}

struct Shell {
    page: Page,
    view: AnyView,
    corruption: Entity<ConfigCorruptionModal>,
    _appearance: Subscription,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        apply_appearance(window, cx);
        Self {
            page: Page::Home,
            view: Self::build(Page::Home, window, cx),
            corruption: cx.new(|_| ConfigCorruptionModal::new()),
            _appearance: window
                .observe_window_appearance(|window, cx| apply_appearance(window, cx)),
        }
    }

    /// Builds the page fresh on every visit: pages own background work (BLE
    /// scans, device streams) that has to stop when the user leaves.
    fn build(page: Page, window: &mut Window, cx: &mut Context<Self>) -> AnyView {
        match page {
            Page::Home => {
                let home = cx.new(|cx| views::HomeView::new(window, cx));
                cx.subscribe_in(
                    &home,
                    window,
                    |this, _, event: &views::HomeEvent, window, cx| {
                        let page = match event {
                            views::HomeEvent::OpenPairing => Page::Pairing,
                            views::HomeEvent::OpenCalibrate => Page::Calibrate,
                        };
                        this.navigate(page, window, cx);
                    },
                )
                .detach();
                home.into()
            }
            Page::Pairing => cx.new(|cx| views::PairingView::new(window, cx)).into(),
            Page::Accessories => cx.new(|cx| views::AccessoriesView::new(window, cx)).into(),
            Page::Calibrate => cx.new(|cx| views::CalibrateView::new(window, cx)).into(),
            Page::Settings => cx.new(|cx| views::SettingsView::new(window, cx)).into(),
        }
    }

    fn navigate(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == page {
            return;
        }
        self.page = page;
        self.view = Self::build(page, window, cx);
        cx.notify();
    }
}

/// Points the app palette and the gpui-component widgets at the window's
/// appearance. Other windows read the same globals, so this runs from the main
/// window's observer rather than from any one render.
fn apply_appearance(window: &mut Window, cx: &mut App) {
    cx.set_global(ActivePalette(Palette::for_appearance(window.appearance())));
    gpui::component::Theme::sync_system_appearance(Some(window), cx);
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
                    .id("page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(self.view.clone()),
            )
            .child(self.corruption.clone())
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
        let server = tokio::spawn(odyssey_hub_server::run_server(status_tx, server_cancel));
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
