use dioxus::prelude::*;
#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::Foundation::GetLastError,
    Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, StartServiceW,
        SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS, SERVICE_START,
    },
};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplaySelection {
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

fn main() {
    #[cfg(windows)]
    {
        if let Err(e) = start_service("OdysseyService") {
            eprintln!("Service start error: {:?}", e);
        }
    }

    launch(app);
}

fn app() -> Element {
    let mut selected = use_signal(|| None);

    rsx! {
        div {
            style: "padding: 2rem; font-size: 1.2rem;",
            h1 { "Welcome!" }

            div {
                select {
                    onchange: move |evt| {
                        let value = evt.value().clone();
                        let sel = match value.as_str() {
                            "Display1" => Some(DisplaySelection::Display1),
                            "Display2" => Some(DisplaySelection::Display2),
                            "Display3" => Some(DisplaySelection::Display3),
                            _ => None,
                        };
                        selected.set(sel);
                    },
                    option { value: "", disabled: true, selected: selected().is_none(), "Select an option" }
                    option { value: "Display1", "Option 1" }
                    option { value: "Display2", "Option 2" }
                    option { value: "Display3", "Option 3" }
                }

                button {
                    style: "margin-left: 1rem;",
                    onclick: move |_| {
                        if let Some(sel) = selected() {
                            println!("Selected display: {:?}", sel);
                        } else {
                            println!("No display selected.");
                        }
                    },
                    "Go"
                }
            }
        }
    }
}

#[cfg(windows)]
fn to_pcwstr(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn start_service(service_name: &str) -> windows::core::Result<()> {
    use windows::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING;

    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)?;
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
