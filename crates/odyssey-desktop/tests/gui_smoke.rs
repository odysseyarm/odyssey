use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs, thread};

use gpui_goo::client::Client;
use gpui_goo::protocol::{Node, Selector};

struct Desktop {
    child: Child,
    client: Option<Client>,
    artifacts: PathBuf,
    app_log: PathBuf,
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if thread::panicking()
            && let Some(client) = &mut self.client
        {
            let _ = client.screenshot(&self.artifacts.join("failure.png"));
            if let Ok(nodes) = client.snapshot()
                && let Ok(json) = serde_json::to_vec_pretty(&nodes)
            {
                let _ = fs::write(self.artifacts.join("failure.json"), json);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.app_log.is_file() {
            let _ = fs::copy(&self.app_log, self.artifacts.join("latest.log"));
        }
    }
}

fn size_value(nodes: &[Node]) -> &str {
    let mut inputs = nodes.iter().filter(|node| {
        node.role.as_deref() == Some("TextInput")
            && node.path.iter().any(|id| id == "calibration-size")
    });
    let input = inputs
        .next()
        .expect("calibration size has an observed text input");
    assert!(
        inputs.next().is_none(),
        "calibration size input must be unique"
    );
    input
        .value
        .as_deref()
        .expect("size input exposes its real value")
}

#[test]
#[ignore = "requires a native display and no other Odyssey hub; run with ui-automation,sim"]
fn calibration_input_dialog_and_sidebar_use_real_widgets() {
    let runtime = tokio::runtime::Runtime::new().expect("test runtime starts");
    let existing = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(3),
            odyssey_hub_client::client::Client::connect(),
        )
        .await
    });
    assert!(
        existing.expect("hub collision check must finish").is_err(),
        "close existing Odyssey hub before running GUI smoke"
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock follows epoch")
        .as_nanos();
    let artifacts = std::env::var_os("ODYSSEY_GUI_ARTIFACTS")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/gui-smoke"),
            PathBuf::from,
        )
        .join(format!("{}-{nonce}", std::process::id()));
    fs::create_dir_all(&artifacts).expect("create artifact directory");
    // Unix socket paths must fit sockaddr_un even when the checkout path is long.
    let temporary = tempfile::Builder::new()
        .prefix("odyssey-gui-")
        .tempdir()
        .expect("create private short runtime directory");
    for directory in ["home", "config", "state", "appdata"] {
        let path = temporary.path().join(directory);
        fs::create_dir_all(&path).expect("create isolated app directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("make test directory private");
        }
    }
    eprintln!("Odyssey GUI artifacts: {}", artifacts.display());
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve automation port");
    let address = listener.local_addr().expect("bound listener has address");
    drop(listener);
    let token = hex::encode(rand::random::<[u8; 32]>());
    let executable = std::env::var_os("ODYSSEY_GUI_EXECUTABLE").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_odyssey-desktop")),
        PathBuf::from,
    );
    let child = Command::new(executable)
        .args([
            "--sim",
            "--no-discovery",
            "--temp-config",
            "calibrate",
            "--demo",
            "--diagonal",
            "55",
        ])
        .env("NATIVE_UI_ADDR", address.to_string())
        .env("NATIVE_UI_TOKEN", &token)
        .env("HOME", temporary.path().join("home"))
        .env("XDG_CONFIG_HOME", temporary.path().join("config"))
        .env("XDG_STATE_HOME", temporary.path().join("state"))
        .env("XDG_RUNTIME_DIR", temporary.path())
        .env("LOCALAPPDATA", temporary.path().join("appdata"))
        .env("ODYSSEY_OFFLINE", "1")
        .env(
            "ODYSSEY_FIRMWARE_PACK_DIR",
            temporary.path().join("empty-firmware-pack"),
        )
        .env("TMPDIR", temporary.path())
        .env("TMP", temporary.path())
        .env("TEMP", temporary.path())
        .stdin(Stdio::null())
        .stdout(fs::File::create(artifacts.join("stdout.log")).expect("create stdout artifact"))
        .stderr(fs::File::create(artifacts.join("stderr.log")).expect("create stderr artifact"))
        .spawn()
        .expect("start owned Odyssey process");
    let mut desktop = Desktop {
        child,
        client: None,
        artifacts,
        app_log: temporary
            .path()
            .join("appdata/Odyssey.Desktop/logs/latest.log"),
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(
            desktop.child.try_wait().expect("read app status").is_none(),
            "Odyssey exited; inspect app logs"
        );
        match Client::connect(address, token.clone()) {
            Ok(client) => {
                desktop.client = Some(client);
                break;
            }
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Err(error) => panic!("automation did not start: {error}"),
        }
    }
    let client = desktop
        .client
        .as_mut()
        .expect("connected automation client");
    client
        .wait_visible(
            &Selector::id("calibrate-automatic"),
            Duration::from_secs(15),
        )
        .expect("calibration viewport appears");
    for _ in 0..8 {
        let nodes = client.snapshot().expect("read scroll position");
        if nodes
            .iter()
            .any(|node| Selector::id("calibration-size").matches(node) && node.visible)
        {
            break;
        }
        client
            .scroll(Selector::id("calibrate-automatic"), [0.0, 300.0])
            .expect("scroll native calibration viewport");
    }
    client
        .wait_visible(&Selector::id("calibration-size"), Duration::from_secs(15))
        .expect("wizard size input appears");
    let before = size_value(&client.snapshot().expect("read initial size")).to_owned();
    client
        .click(Selector::id("calibration-size"))
        .expect("focus real size input");
    client
        .press(if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        })
        .expect("select prior size");
    client.input("42").expect("enter size");
    client
        .press("backspace")
        .expect("edit size through key dispatch");
    client.input("8").expect("complete edited size");
    client.press("tab").expect("leave size input");
    let after = size_value(&client.snapshot().expect("read edited size")).to_owned();
    assert_eq!(after, "48", "native input edits reach wizard state");
    client
        .screenshot(&desktop.artifacts.join("calibration-size.png"))
        .expect("capture edited calibration size");
    for _ in 0..8 {
        let nodes = client.snapshot().expect("read details position");
        if nodes
            .iter()
            .any(|node| Selector::id("scale-board-summary").matches(node) && node.visible)
        {
            break;
        }
        client
            .scroll(Selector::id("calibrate-automatic"), [0.0, 300.0])
            .expect("scroll to optional scale-board details");
    }
    client
        .click(Selector::id("scale-board-summary"))
        .expect("expand scale-board details");
    client
        .scroll(Selector::id("calibrate-automatic"), [0.0, 300.0])
        .expect("reveal expanded scale-board link");

    client
        .click(Selector::id("scale-board-open"))
        .expect("open actual scale-board dialog");
    let dialog = client
        .wait_visible(&Selector::id("scale-board-dialog"), Duration::from_secs(5))
        .expect("dialog appears");
    assert_eq!(dialog.role.as_deref(), Some("Dialog"));
    client
        .screenshot(&desktop.artifacts.join("scale-board.png"))
        .expect("capture actual dialog");
    client
        .press("escape")
        .expect("dismiss dialog using keyboard");
    assert!(
        client
            .snapshot()
            .expect("read closed dialog")
            .iter()
            .all(|node| !Selector::id("scale-board-dialog").matches(node))
    );
    client
        .click(Selector::id("scale-board-open"))
        .expect("reopen dialog");
    client
        .click(Selector::id("scale-board-close"))
        .expect("dismiss dialog through shared button");
    assert!(
        client
            .snapshot()
            .expect("read button dismissal")
            .iter()
            .all(|node| !Selector::id("scale-board-dialog").matches(node))
    );

    client
        .scroll(Selector::id("calibrate-automatic"), [0.0, -3000.0])
        .expect("return to calibration header");
    client
        .click(Selector::id("exit"))
        .expect("leave calibration before testing sidebar pages");
    let mut pages = Vec::new();
    for (navigation, page) in [
        ("Settings", "settings"),
        ("Devices", "devices"),
        ("Setup", "home"),
    ] {
        client
            .click(Selector::id(navigation))
            .expect("activate real sidebar link");
        let node = client
            .wait_visible(&Selector::id(page), Duration::from_secs(5))
            .expect("destination page appears");
        pages.push(node.path.last().expect("page has native identity").clone());
        client
            .screenshot(&desktop.artifacts.join(format!("{page}.png")))
            .expect("capture destination page");
        fs::write(
            desktop.artifacts.join(format!("{page}.json")),
            serde_json::to_vec_pretty(&client.snapshot().expect("read destination widgets"))
                .expect("serialize destination widgets"),
        )
        .expect("write destination widget snapshot");
    }
    assert_eq!(
        format!("size: {before} -> {after}\npages: {}", pages.join(" -> ")),
        "size: 55 -> 48\npages: settings -> devices -> home"
    );
    client
        .screenshot(&desktop.artifacts.join("setup.png"))
        .expect("capture actual setup page");
}
