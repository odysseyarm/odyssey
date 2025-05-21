use iced::{widget::{button, column, pick_list, row, text}, Element, Task};

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::Foundation::GetLastError,
    Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, StartServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
        SERVICE_START,
    },
};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[derive(Default)]
struct Odyssey {
    selected_display: Option<DisplaySelection>,
}

#[derive(Debug, Clone)]
enum Message {
    GoButtonPressed,
    DisplayOptionSelected(DisplaySelection),
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum DisplaySelection {
    #[default]
    Display1,
    Display2,
    Display3,
}

impl std::fmt::Display for DisplaySelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisplaySelection::Display1 => write!(f, "Option 1"),
            DisplaySelection::Display2 => write!(f, "Option 2"),
            DisplaySelection::Display3 => write!(f, "Option 3"),
        }
    }
}

fn main() -> iced::Result {
    start_service("OdysseyService").unwrap();

    iced::application("Odyssey", update, view)
        .window_size(iced::Size::new(800.0, 600.0))
        .decorations(true)
        .run_with(|| (Odyssey::default(), Task::none()))
}

fn update(state: &mut Odyssey, message: Message) -> Task<Message> {
    match message {
        Message::DisplayOptionSelected(display) => {
            state.selected_display = Some(display);
        },
        Message::GoButtonPressed => {
            if let Some(display) = state.selected_display {
                println!("Selected display: {:?}", display);
            } else {
                println!("No display selected.");
            }
        },
    }
    Task::none()
}

fn view(state: &Odyssey) -> Element<Message> {
    column![
        text("Welcome!"),
        row![
            pick_list(
                vec![DisplaySelection::Display1, DisplaySelection::Display2, DisplaySelection::Display3],
                state.selected_display,
                Message::DisplayOptionSelected,
            ).placeholder("Select an option"),
            button("Go").on_press_maybe(state.selected_display.map(|_| Message::GoButtonPressed)),
        ].spacing(20),
    ].spacing(20).padding(20).into()
}

#[cfg(windows)]
fn to_pcwstr(s: &str) -> Vec<u16> {
    let wide: Vec<u16> = OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect();
    wide
}

#[cfg(windows)]
fn start_service(service_name: &str) -> windows::core::Result<()> {
    use windows::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING;

    unsafe {
        let scm = OpenSCManagerW(
            None,
            None,
            SC_MANAGER_CONNECT,
        )?;

        let service_name_w = to_pcwstr(service_name);
        let service = OpenServiceW(
            scm,
            PCWSTR(service_name_w.as_ptr()),
            SERVICE_START | SERVICE_QUERY_STATUS,
        )?;

        let result = StartServiceW(service, Some(&[]));

        if !result.is_ok() {
            let error = GetLastError();
            if error == ERROR_SERVICE_ALREADY_RUNNING {
                println!("Service is already running.");
            } else {
                println!("StartService failed with error: {}", error.0);
            }
        } else {
            println!("Service started successfully.");
        }

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);
    }

    Ok(())
}
