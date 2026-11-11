# Windows desktop release artifacts, cross-compiled from Linux in the Nix sandbox.
#
# What the Windows runner used to compile (the app twice, then cargo-packager and
# the xtask) is built here, with the same Crane graph and the same hash-pinned MSVC
# CRT + SDK as the SDK libraries (./xwin.nix). Only WiX itself still needs Windows,
# so the release job downloads the `desktop-windows` output and runs
#
#   tools/xtask.exe desktop bundle <online|full> --prebuilt <that directory>
#
# `desktop-windows` layout (crates/xtask/src/desktop/prebuilt.rs reads exactly this):
#
#   online/odyssey-desktop.exe   default features: the app downloads the Orbbec SDK
#   full/odyssey-desktop.exe     --no-default-features: no SDK downloader
#   tools/cargo-packager.exe     version and source hash pinned from Cargo.toml
#   tools/xtask.exe              crates/xtask
#   packager.json                [package.metadata.packager] of crates/odyssey-desktop
#   prebuilt.json                versions and sha256 of every file above
#
# Parity with the native MSVC build this replaces: the same rustc, `--release`, the
# default x86-64 CPU baseline, the dynamic CRT (cargo-xwin keeps rustc's default
# unless RUSTFLAGS asks for +crt-static, which nothing here does).
#
# gpui compiles its HLSL shaders with Microsoft's fxc.exe at build time. fxc.exe
# is the very binary from the pinned Windows SDK, run under headless Wine (a
# throwaway prefix inside the build directory). Nothing of Wine or fxc ends up in
# the outputs, only the DXBC it produced.
{ pkgs, craneLib, common, ... }:
let
  inherit (pkgs) lib;
  xwin = import ./xwin.nix { inherit pkgs; };
  inherit (xwin) target;

  pin = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain;
  crane = craneLib.overrideToolchain (pkgs.rust-bin.fromRustupToolchain {
    inherit (pin) channel profile;
    components = [ ];
    targets = [ target ];
  });

  # A sandbox HOME and the cargo-xwin cache around the pinned sysroot.
  sandbox = ''
    export HOME="$TMPDIR/home"
    export XDG_CACHE_HOME="$TMPDIR/xdg-cache"
    export CARGO_NET_OFFLINE=true
    mkdir -p "$HOME" "$XDG_CACHE_HOME"
  '' + xwin.setup;

  cargoXwin = "cargo xwin build --release --locked --offline --target ${target}";

  # Strip toolchain and vendored-source store paths (panic locations) from the
  # binaries so the outputs do not drag the toolchain and every crate along.
  scrub = [ crane.removeReferencesToRustToolchainHook crane.removeReferencesToVendoredSourcesHook ];

  # ---- Microsoft fxc.exe -------------------------------------------------------
  # `Windows SDK for Windows Store Apps Tools` of Windows SDK 10.0.26100 (the SDK
  # in ./xwin.nix's manifest). The cab is what that MSI's File table points at; the
  # two members are the x64 fxc.exe and the x64 d3dcompiler_47.dll it loads.
  fxcCab = pkgs.fetchurl {
    url = "https://download.visualstudio.microsoft.com/download/pr/6452c1f1-dc1e-413c-8b19-991b61870a8b/944d105a33e16da614cbf2a17da22954/69661e20556b3ca9456b946c2c881ddd.cab";
    hash = "sha256-vvWwOPziiPodsjS9YcPkUKvswC9aOGJIYlBzqDAQ9h4=";
  };
  fxc = pkgs.runCommand "microsoft-fxc-10.0.26100" { nativeBuildInputs = [ pkgs.cabextract ]; } ''
    cabextract -q -d members \
      -F fil6dfaf6bbf56939fbab75b569ffa0fd41 \
      -F fil0d3f984977487cd87127d614f654195b ${fxcCab}
    mkdir -p $out/lib
    cp members/fil6dfaf6bbf56939fbab75b569ffa0fd41 $out/lib/fxc.exe
    cp members/fil0d3f984977487cd87127d614f654195b $out/lib/d3dcompiler_47.dll
    cd $out/lib
    sha256sum --check <<'EOF'
    31fa10504b1551337de4d06ec2194143885a76a8b5a81177999e2faa34729183  fxc.exe
    2b23efdc8b45adc4b79c1c0b940a906336628cd35ed80defd489d70d22ff6d74  d3dcompiler_47.dll
    EOF
  '';

  wine = pkgs.wine64;
  # Console fxc needs no display: the graphics drivers stay off, and Wine's
  # built-in d3dcompiler (vkd3d) must never stand in for Microsoft's.
  wineEnv = ''
    export WINEDEBUG=-all
    export WINEARCH=win64
    export WINEDLLOVERRIDES="mscoree,mshtml,winex11.drv,winewayland.drv=d;d3dcompiler_47=n"
    unset DISPLAY WAYLAND_DISPLAY XDG_RUNTIME_DIR
  '';
  # gpui's build script runs this as `fxc /T … /E … /Fh <out> /Vn … /O3 <hlsl>`.
  # Absolute Unix paths become Z: paths; switches (/T, /Fh, /O3, …) have no second slash.
  fxcUnderWine = pkgs.writeShellScript "fxc-under-wine" ''
    : "''${WINEPREFIX:?fxcPrefix has not run}"
    ${wineEnv}
    args=()
    for arg in "$@"; do
      case "$arg" in
        /*/*) arg="Z:''${arg//\//\\}" ;;
      esac
      args+=("$arg")
    done
    exec ${wine}/bin/wine ${fxc}/lib/fxc.exe "''${args[@]}"
  '';
  fxcPrefix = ''
    export WINEPREFIX="$TMPDIR/wine-prefix"
    (
      ${wineEnv}
      ${wine}/bin/wineboot --init
      ${wine}/bin/wineserver --wait
    )
  '';

  # gpui-pre-windows 0.3.11 only compiles its shaders when the *host* is
  # Windows. The patch moves that decision to the target and keeps the rest of
  # upstream's build.rs; it is tied to this version and refuses any other.
  gpuiCrossBuild = { name, version, ... }: drv:
    if name != "gpui-pre-windows" then drv
    else
      assert lib.assertMsg (version == "0.3.11")
        "nix/patches/gpui-pre-windows-0.3.11-cross-build.patch is for gpui-pre-windows 0.3.11, not ${version}: port it";
      drv.overrideAttrs (old: {
        patches = (old.patches or [ ]) ++ [ ./patches/gpui-pre-windows-0.3.11-cross-build.patch ];
      });

  # ---- the app -------------------------------------------------------------------
  appCommon = common.mkArgs {
    pname = "odyssey-desktop-windows";
    ODYSSEY_BUILD_VERSION = desktopVersion;
    vendorOverrides.overrideVendorCargoPackage = gpuiCrossBuild;
  };
  appBase = appCommon // {
    nativeBuildInputs = appCommon.nativeBuildInputs ++ xwin.tools;
    GPUI_FXC_PATH = "${fxcUnderWine}";
    preBuild = sandbox + fxcPrefix;
    doInstallCargoArtifacts = false;
    dontFixup = true;
  };
  # Both variants share one dependency build; cargo recompiles only what the
  # SDK-downloader feature (reqwest, sha2) changes.
  appDeps = crane.buildDepsOnly (appBase // {
    doCheck = false;
    doInstallCargoArtifacts = true;
    buildPhaseCargoCommand = "${cargoXwin} -p odyssey-desktop";
  });
  app = { variant, flags }: crane.mkCargoDerivation (common.withPrivateSource appBase // {
    pnameSuffix = "-${variant}";
    cargoArtifacts = appDeps;
    nativeBuildInputs = appBase.nativeBuildInputs ++ scrub;
    buildPhaseCargoCommand = "${cargoXwin} -p odyssey-desktop ${flags}";
    installPhaseCommand = ''
      mkdir -p $out/bin
      cp target/${target}/release/odyssey-desktop.exe $out/bin/
    '';
  });
  online = app { variant = "online"; flags = ""; };
  # The full installer carries the SDK and never downloads it.
  full = app { variant = "full"; flags = "--no-default-features"; };

  # ---- xtask (its own workspace and lockfile) ---------------------------------------
  xtaskCommon = common.mkArgs { workspace = "crates/xtask"; pname = "odyssey-xtask-windows"; };
  xtaskBase = xtaskCommon // {
    nativeBuildInputs = xtaskCommon.nativeBuildInputs ++ xwin.tools;
    preBuild = sandbox;
    doInstallCargoArtifacts = false;
    dontFixup = true;
  };
  xtaskDeps = crane.buildDepsOnly (xtaskBase // {
    doCheck = false;
    doInstallCargoArtifacts = true;
    buildPhaseCargoCommand = cargoXwin;
  });
  xtask = crane.mkCargoDerivation (xtaskBase // {
    cargoArtifacts = xtaskDeps;
    nativeBuildInputs = xtaskBase.nativeBuildInputs ++ scrub;
    buildPhaseCargoCommand = cargoXwin;
    installPhaseCommand = ''
      mkdir -p $out/bin
      cp target/${target}/release/xtask.exe $out/bin/
    '';
  });

  # ---- cargo-packager ------------------------------------------------------------------
  # The version Cargo.toml pins for `cargo bin` (workspace.metadata.bin) is the
  # only version here; its source hash is pinned next to it below and a new
  # version has no hash until someone adds one. The crate's own Cargo.lock
  # (`locked = true`) fixes every dependency.
  root = builtins.fromTOML (builtins.readFile (common.source + "/Cargo.toml"));
  packagerPin = root.workspace.metadata.bin.cargo-packager;
  packagerSourceHashes = {
    "0.11.8" = "sha256-DjqrsomwtM5JzGrBIjfREZ15pUijza+/p+3CwXe+dSY=";
  };
  packagerSource = pkgs.fetchCrate {
    pname = "cargo-packager";
    inherit (packagerPin) version;
    hash = packagerSourceHashes.${packagerPin.version}
      or (throw "cargo-packager ${packagerPin.version} is pinned in Cargo.toml but has no source hash in nix/desktop-windows.nix");
  };
  packagerBase = {
    pname = "cargo-packager";
    inherit (packagerPin) version;
    src = packagerSource;
    cargoVendorDir = crane.vendorCargoDeps { src = packagerSource; };
    strictDeps = true;
    nativeBuildInputs = xwin.tools;
    preBuild = sandbox;
    doInstallCargoArtifacts = false;
    dontFixup = true;
  };
  packagerBuild = "${cargoXwin} --bin cargo-packager";
  packagerDeps = crane.buildDepsOnly (packagerBase // {
    doCheck = false;
    doInstallCargoArtifacts = true;
    buildPhaseCargoCommand = packagerBuild;
  });
  cargoPackager = assert lib.assertMsg (packagerPin.locked or false)
    "cargo-packager must stay `locked = true` in [workspace.metadata.bin]";
    crane.mkCargoDerivation (packagerBase // {
      cargoArtifacts = packagerDeps;
      nativeBuildInputs = packagerBase.nativeBuildInputs ++ scrub;
      buildPhaseCargoCommand = packagerBuild;
      installPhaseCommand = ''
        mkdir -p $out/bin
        cp target/${target}/release/cargo-packager.exe $out/bin/
      '';
    });

  # ---- the bundle the Windows job downloads ---------------------------------------------
  desktopManifest = builtins.fromTOML (builtins.readFile (common.source + "/crates/odyssey-desktop/Cargo.toml"));
  desktopVersion = let buildVersion = builtins.getEnv "ODYSSEY_BUILD_VERSION"; in
    if buildVersion == "" then desktopManifest.package.version else buildVersion;
  msiVersionParts = builtins.match "([0-9]+\\.[0-9]+\\.[0-9]+)(-.*|\\+.*)?" desktopVersion;
  msiCore = if msiVersionParts == null then
    throw "desktop version ${desktopVersion} has no numeric MSI version"
  else builtins.elemAt msiVersionParts 0;
  msiFields = map builtins.fromJSON (lib.splitString "." msiCore);
  msiVersion = if builtins.elemAt msiFields 0 > 255 || builtins.elemAt msiFields 1 > 255
    || builtins.elemAt msiFields 2 > 65535 then
    throw "desktop version ${desktopVersion} exceeds Windows Installer version limits"
  else msiCore;
  packagerConfig = pkgs.writeText "packager.json" (builtins.toJSON (
    desktopManifest.package.metadata.packager // {
      name = desktopManifest.package.name;
      version = msiVersion;
    }
  ));

  desktopWindows = pkgs.runCommand "odyssey-desktop-windows-${desktopVersion}" {
    nativeBuildInputs = [ pkgs.jq pkgs.llvmPackages.llvm ];
    allowedReferences = [];
    meta.platforms = [ "x86_64-linux" ];
  } ''
    mkdir -p $out/online $out/full $out/tools
    cp ${online}/bin/odyssey-desktop.exe $out/online/
    cp ${full}/bin/odyssey-desktop.exe $out/full/
    cp ${cargoPackager}/bin/cargo-packager.exe ${xtask}/bin/xtask.exe $out/tools/
    cp ${packagerConfig} $out/packager.json

    # Each is a 64-bit Windows binary; the apps carry their icon and manifest.
    for exe in $out/online/odyssey-desktop.exe $out/full/odyssey-desktop.exe \
               $out/tools/cargo-packager.exe $out/tools/xtask.exe; do
      llvm-readobj --file-headers "$exe" | grep -q IMAGE_FILE_MACHINE_AMD64 \
        || { echo "$exe is not an x86-64 PE file" >&2; exit 1; }
    done
    for exe in $out/online/odyssey-desktop.exe $out/full/odyssey-desktop.exe; do
      llvm-objdump -h "$exe" | grep -q ' \.rsrc ' \
        || { echo "$exe has no resource section (icon, manifest)" >&2; exit 1; }
    done

    cd $out
    sha256sum online/odyssey-desktop.exe full/odyssey-desktop.exe \
      tools/cargo-packager.exe tools/xtask.exe packager.json \
      | jq -Rn --arg version ${lib.escapeShellArg desktopVersion} --arg packager ${lib.escapeShellArg packagerPin.version} '{
          version: $version,
          cargoPackager: $packager,
          sha256: ([inputs | capture("^(?<hash>[0-9a-f]{64})  (?<file>.+)$") | {key: .file, value: .hash}] | from_entries)
        }' > prebuilt.json
  '';
in {
  packages = {
    desktop-windows = desktopWindows;
    desktop-windows-online = online;
    desktop-windows-full = full;
    xtask-windows = xtask;
    cargo-packager-windows = cargoPackager;
  };
  checks = { };
}
