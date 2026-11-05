use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedSender;
use gpui::{AnyWindowHandle, App, AppContext as _};
use tokio_util::sync::CancellationToken;

/// What the tray asks of the app. Tray callbacks run on foreign threads, so
/// they only send these; the GPUI side acts on them.
#[derive(Clone, Copy, Debug)]
enum TrayCommand {
    Show,
    Quit,
}

type Commands = UnboundedSender<TrayCommand>;

/// Decoded tray icon, in whatever form the platform backend wants.
struct TrayImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn tray_image() -> TrayImage {
    let bytes: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/images/tray-icon.png"
    ));
    let image = image::load_from_memory(bytes)
        .expect("the bundled tray icon is a valid image")
        .into_rgba8();
    let (width, height) = image.dimensions();
    TrayImage {
        width,
        height,
        rgba: image.into_raw(),
    }
}

/// Builds the tray icon and its menu and acts on its clicks for as long as the
/// app runs. Closing the window only minimizes it, so the tray is the way back.
/// Quitting from the tray cancels `cancellation_token` and ends the app.
pub(crate) fn start(cancellation_token: CancellationToken, window: AnyWindowHandle, cx: &mut App) {
    let (commands, mut received) = futures::channel::mpsc::unbounded();
    platform::start(cancellation_token.clone(), commands, cx);

    cx.spawn(async move |cx| {
        while let Some(command) = received.next().await {
            match command {
                TrayCommand::Show => {
                    let shown = cx.update_window(window, |_, window, _| window.activate_window());
                    if shown.is_err() {
                        tracing::warn!("The tray could not show the window: it is closed");
                    }
                }
                TrayCommand::Quit => {
                    cancellation_token.cancel();
                    cx.update(|cx| cx.quit());
                }
            }
        }
    })
    .detach();
}

/// StatusNotifier hosts only offer a left click to items that implement
/// Activate. tray-icon's Linux backend is libappindicator, which exports a menu
/// and nothing else, so a click there can only ever raise the menu. Talking to
/// the host directly is what buys the conventional behaviour: left click opens
/// the window, right click opens the menu.
#[cfg(target_os = "linux")]
mod platform {
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem, Tray, TrayMethods as _};
    use tokio_util::sync::CancellationToken;

    use super::{Commands, TrayCommand, TrayImage, tray_image};
    use crate::runtime;

    struct OdysseyTray {
        icon: Icon,
        commands: Commands,
    }

    impl OdysseyTray {
        fn send(&self, command: TrayCommand) {
            // The receiver only closes when the app is already shutting down.
            let _ = self.commands.unbounded_send(command);
        }
    }

    impl Tray for OdysseyTray {
        fn id(&self) -> String {
            "odyssey-desktop".into()
        }

        fn title(&self) -> String {
            "Odyssey".into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            vec![self.icon.clone()]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayCommand::Show);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Open".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::Show)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(TrayCommand::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// The wire format is ARGB32 in network byte order, not the RGBA the
    /// decoder hands back.
    fn icon_from(image: TrayImage) -> Icon {
        let mut data = Vec::with_capacity(image.rgba.len());
        for pixel in image.rgba.as_chunks::<4>().0 {
            data.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        Icon {
            width: image.width as i32,
            height: image.height as i32,
            data,
        }
    }

    pub(super) fn start(
        cancellation_token: CancellationToken,
        commands: Commands,
        _cx: &mut gpui::App,
    ) {
        let tray = OdysseyTray {
            icon: icon_from(tray_image()),
            commands,
        };
        runtime::spawn(async move {
            // The handle has to outlive the wait below: dropping it
            // unregisters the item and the icon disappears.
            let _handle = match tray.spawn().await {
                Ok(handle) => handle,
                Err(e) => {
                    tracing::error!("failed to register the tray icon: {e}");
                    return;
                }
            };
            cancellation_token.cancelled().await;
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use gpui::Global;
    use tokio_util::sync::CancellationToken;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{Commands, TrayCommand, tray_image};

    const OPEN_ID: &str = "open";
    const QUIT_ID: &str = "quit";

    /// Dropping the icon unregisters it, so it lives as long as the app.
    struct TrayHolder(#[expect(dead_code, reason = "held only to keep the icon registered")] TrayIcon);

    impl Global for TrayHolder {}

    pub(super) fn start(
        _cancellation_token: CancellationToken,
        commands: Commands,
        cx: &mut gpui::App,
    ) {
        let image = tray_image();
        let icon = match Icon::from_rgba(image.rgba, image.width, image.height) {
            Ok(icon) => icon,
            Err(e) => {
                tracing::error!("failed to build the tray icon: {e}");
                return;
            }
        };

        let menu = Menu::new();
        let open_item = MenuItem::with_id(OPEN_ID, "Open", true, None);
        let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        if let Err(e) = menu.append_items(&[&open_item, &quit_item]) {
            tracing::error!("failed to build the tray menu: {e}");
        }

        let tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(icon)
            .build()
        {
            Ok(tray) => tray,
            Err(e) => {
                tracing::error!("failed to register the tray icon: {e}");
                return;
            }
        };

        // The handlers run on whichever thread the OS delivers the event to.
        let menu_commands = commands.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.0.as_str() {
                OPEN_ID => TrayCommand::Show,
                QUIT_ID => TrayCommand::Quit,
                _ => return,
            };
            let _ = menu_commands.unbounded_send(command);
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = commands.unbounded_send(TrayCommand::Show);
            }
        }));
        cx.set_global(TrayHolder(tray));
    }
}
