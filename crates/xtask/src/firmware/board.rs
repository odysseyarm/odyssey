use std::path::PathBuf;

use crate::cmd;

const APP_CORE_TARGET: &str = "thumbv8m.main-none-eabihf";
const NET_CORE_TARGET: &str = "thumbv8m.main-none-eabi";
const NRF52840_TARGET: &str = "thumbv7em-none-eabihf";
const NRF5340: &str = "nRF5340_xxAA";
const NRF52840: &str = "nRF52840_xxAA";

/// One firmware crate. cargo reads its `.cargo/config.toml` (runner, rustflags,
/// DEFMT_LOG) from the current directory, so every cargo call runs in `dir`.
#[derive(Clone, Copy)]
pub(super) struct Crate {
    /// Relative to the repository root.
    pub(super) dir: &'static str,
    pub(super) bin: &'static str,
    /// The triple the build lands under, `target/<triple>/release/<bin>`.
    pub(super) target: &'static str,
    /// Everything after `cargo build`.
    pub(super) build: &'static [&'static str],
}

impl Crate {
    pub(super) fn elf(&self) -> PathBuf {
        self.elf_in("target")
    }

    /// The ELF under another `--target-dir`, relative to `dir`. With
    /// `ODYSSEY_PREBUILT` set it is looked up there instead of in the checkout.
    pub(super) fn elf_in(&self, target_dir: &str) -> PathBuf {
        super::prebuilt()
            .unwrap_or_else(cmd::root)
            .join(self.dir)
            .join(target_dir)
            .join(self.target)
            .join("release")
            .join(self.bin)
    }

    /// The `main.rs` that declares the release counter.
    pub(super) fn main_rs(&self) -> String {
        format!("{}/src/main.rs", self.dir)
    }
}

pub(super) struct Board {
    pub(super) name: &'static str,
    /// Relative to the repository root; `build/`, `dist-bootloader/` and
    /// `dist-netcore/` are created inside it.
    pub(super) dir: &'static str,
    pub(super) chip: &'static str,
    pub(super) app: Crate,
    pub(super) bootloader: Crate,
    pub(super) cores: Cores,
    pub(super) flash: Flash,
    /// Crates `clean` empties, the board itself included.
    pub(super) clean: &'static [&'static Crate],
    /// Crates `check` type-checks; empty for a board without `check`.
    pub(super) check: &'static [&'static Crate],
}

pub(super) enum Cores {
    /// nRF528xx: one application behind one bootloader.
    Single,
    /// nRF5340: stage1 and stage2 bootloaders, two application banks, and a net
    /// core with its own bootloader.
    Dual(&'static DualCore),
}

pub(super) struct DualCore {
    pub(super) stage1: Crate,
    pub(super) netcore_bootloader: Crate,
    pub(super) net_core: Crate,
    /// The manifest device key, which also prefixes every release tag.
    pub(super) device: &'static str,
    /// The product name in release titles and notes.
    pub(super) product: &'static str,
}

/// How `flash` and its variants write the built images.
pub(super) enum Flash {
    /// Clear boot state pages and write the merged hex.
    MergedErasingPages,
    /// The merged hex alone.
    Merged,
}

impl Board {
    pub(super) fn path(&self, relative: &str) -> PathBuf {
        cmd::root().join(self.dir).join(relative)
    }

    pub(super) fn dual_core(&self) -> eros::Result<&DualCore> {
        match self.cores {
            Cores::Dual(dual) => Ok(dual),
            Cores::Single => eros::bail!(
                "{} is single-core: it has no stage1 bootloader, second app bank or net core",
                self.name
            ),
        }
    }
}

const fn app_core(dir: &'static str, bin: &'static str, build: &'static [&'static str]) -> Crate {
    Crate {
        dir,
        bin,
        target: APP_CORE_TARGET,
        build,
    }
}

const fn net_core(dir: &'static str, bin: &'static str, build: &'static [&'static str]) -> Crate {
    Crate {
        dir,
        bin,
        target: NET_CORE_TARGET,
        build,
    }
}

const APP_CORE_RELEASE: &[&str] = &["--release", "--target", APP_CORE_TARGET];
const APP_CORE_BOOTLOADER: &[&str] = &[
    "--release",
    "--target",
    APP_CORE_TARGET,
    "--features",
    "defmt",
];
const NET_CORE_RELEASE: &[&str] = &["--release", "--target", NET_CORE_TARGET];
/// The net core's `.cargo/config.toml` already names its target.
const CONFIGURED_RELEASE: &[&str] = &["--release"];

const LITE1_APP: Crate = app_core("firmware/lite1", "app", APP_CORE_RELEASE);
const LITE1_BOOTLOADER: Crate = app_core(
    "firmware/lite1-bootloader",
    "lite1-bootloader",
    APP_CORE_BOOTLOADER,
);
const LITE1_STAGE1: Crate = app_core(
    "firmware/lite1-stage1-bootloader",
    "lite1-stage1-bootloader",
    APP_CORE_RELEASE,
);
const LITE1_NETCORE_BOOTLOADER: Crate = net_core(
    "firmware/lite1-netcore-bootloader",
    "lite1-netcore-bootloader",
    NET_CORE_RELEASE,
);
const LITE1_NET_CORE: Crate = net_core(
    "firmware/lite1-net-core",
    "lite1-net-core",
    CONFIGURED_RELEASE,
);

pub(super) const LITE1: Board = Board {
    name: "lite1",
    dir: "firmware/lite1",
    chip: NRF5340,
    app: LITE1_APP,
    bootloader: LITE1_BOOTLOADER,
    cores: Cores::Dual(&DualCore {
        stage1: LITE1_STAGE1,
        netcore_bootloader: LITE1_NETCORE_BOOTLOADER,
        net_core: LITE1_NET_CORE,
        device: "atslite1",
        product: "ATS Lite 1",
    }),
    flash: Flash::MergedErasingPages,
    clean: &[
        &LITE1_APP,
        &LITE1_BOOTLOADER,
        &LITE1_STAGE1,
        &LITE1_NETCORE_BOOTLOADER,
        &LITE1_NET_CORE,
    ],
    check: &[
        &LITE1_APP,
        &LITE1_STAGE1,
        &LITE1_BOOTLOADER,
        &LITE1_NETCORE_BOOTLOADER,
        &LITE1_NET_CORE,
    ],
};

/// The standard library rebuilt for size (see firmware/README.md); the bank
/// boundaries are fixed.
const LEGACY_APP_RELEASE: &[&str] = &[
    "-Z",
    "build-std",
    "-Z",
    "build-std-features=optimize_for_size",
    "--release",
    "--target",
    APP_CORE_TARGET,
];

const LEGACY_ATSLITE_APP: Crate = app_core("firmware/legacy-atslite", "app", LEGACY_APP_RELEASE);
const LEGACY_ATSLITE_BOOTLOADER: Crate = app_core(
    "firmware/legacy-atslite-bootloader",
    "legacy-atslite-bootloader",
    APP_CORE_BOOTLOADER,
);

pub(super) const LEGACY_ATSLITE: Board = Board {
    name: "legacy-atslite",
    dir: "firmware/legacy-atslite",
    chip: NRF5340,
    app: LEGACY_ATSLITE_APP,
    bootloader: LEGACY_ATSLITE_BOOTLOADER,
    cores: Cores::Dual(&DualCore {
        stage1: app_core(
            "firmware/legacy-atslite-stage1-bootloader",
            "legacy-atslite-stage1-bootloader",
            APP_CORE_RELEASE,
        ),
        netcore_bootloader: net_core(
            "firmware/legacy-atslite-netcore-bootloader",
            "legacy-atslite-netcore-bootloader",
            NET_CORE_RELEASE,
        ),
        net_core: net_core(
            "firmware/legacy-atslite-net-core",
            "legacy-atslite-net-core",
            CONFIGURED_RELEASE,
        ),
        device: "legacy-atslite1",
        product: "Legacy ATS Lite",
    }),
    flash: Flash::MergedErasingPages,
    clean: &[&LEGACY_ATSLITE_APP, &LEGACY_ATSLITE_BOOTLOADER],
    check: &[],
};

const LEGACY_VM_RELEASE: &[&str] = &["--release", "--target", NRF52840_TARGET];
const LEGACY_VM_APP: Crate = Crate {
    dir: "firmware/legacy-vm",
    bin: "app",
    target: NRF52840_TARGET,
    build: LEGACY_VM_RELEASE,
};
const LEGACY_VM_BOOTLOADER: Crate = Crate {
    dir: "firmware/legacy-vm-bootloader",
    bin: "legacy-vm-bootloader",
    target: NRF52840_TARGET,
    build: LEGACY_VM_RELEASE,
};

pub(super) const LEGACY_VM: Board = Board {
    name: "legacy-vm",
    dir: "firmware/legacy-vm",
    chip: NRF52840,
    app: LEGACY_VM_APP,
    bootloader: LEGACY_VM_BOOTLOADER,
    cores: Cores::Single,
    flash: Flash::Merged,
    clean: &[&LEGACY_VM_APP, &LEGACY_VM_BOOTLOADER],
    check: &[],
};

const MARKER_APP: Crate = Crate {
    dir: "firmware/marker-control-board",
    bin: "app",
    target: NRF52840_TARGET,
    build: LEGACY_VM_RELEASE,
};
const MARKER_BOOTLOADER: Crate = Crate {
    dir: "firmware/marker-control-board-bootloader",
    bin: "marker-control-board-bootloader",
    target: NRF52840_TARGET,
    build: LEGACY_VM_RELEASE,
};

pub(super) const MARKER_CONTROL_BOARD: Board = Board {
    name: "marker-control-board",
    dir: "firmware/marker-control-board",
    chip: "nRF52833_xxAA",
    app: MARKER_APP,
    bootloader: MARKER_BOOTLOADER,
    cores: Cores::Single,
    flash: Flash::Merged,
    clean: &[&MARKER_APP, &MARKER_BOOTLOADER],
    check: &[&MARKER_APP, &MARKER_BOOTLOADER],
};
