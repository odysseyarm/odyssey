{ pkgs, craneLib, common, nativeInputs, nativeLibraries, nativeEnvironment }:
let
  hostArgs = common.mkArgs {
    pname = "odyssey-host";
    cargoExtraArgs = "--locked --offline --workspace --features odyssey-hub-server/tracking-regression,odyssey-desktop/sim,odyssey-vision-tool/sim";
  } // nativeEnvironment // {
    nativeBuildInputs = nativeInputs;
    buildInputs = nativeLibraries;
  };
  hostArtifacts = craneLib.buildDepsOnly (hostArgs // { doCheck = false; });
  host = craneLib.buildPackage (common.withPrivateSource hostArgs // {
    cargoArtifacts = hostArtifacts;
    doCheck = false;
    meta.mainProgram = "odyssey-desktop";
    postFixup = ''
      for program in odyssey-desktop odyssey-vision-tool; do
        wrapProgram "$out/bin/$program" \
          --prefix LD_LIBRARY_PATH : "${nativeEnvironment.LD_LIBRARY_PATH}"
      done
    '';
  });
  trackingArgs = hostArgs // {
    pname = "odyssey-tracking-regression";
    cargoExtraArgs = "--locked --offline -p odyssey-hub-server --features tracking-regression --bin tracking-regression";
  };
  # The host layer already compiles every dependency of this selection.
  trackingArtifacts = hostArtifacts;
  tracking = craneLib.buildPackage (common.withPrivateSource trackingArgs // {
    cargoArtifacts = trackingArtifacts;
    doCheck = false;
  });
  xtaskArgs = common.mkArgs {
    workspace = "crates/xtask";
    pname = "odyssey-xtask";
  };
  xtaskArtifacts = craneLib.buildDepsOnly (xtaskArgs // { doCheck = false; });
  xtaskBinary = craneLib.buildPackage (xtaskArgs // {
    cargoArtifacts = xtaskArtifacts;
    doCheck = false;
  });
  xtask = pkgs.writeShellApplication {
    name = "xtask";
    runtimeInputs = [ pkgs.coreutils ];
    text = ''
      if [ -z "''${ODYSSEY_REPO:-}" ]; then
        root="$PWD"
        while [ "$root" != / ] && [ ! -f "$root/rust-toolchain.toml" ]; do
          root="$(dirname "$root")"
        done
        if [ ! -f "$root/crates/xtask/Cargo.toml" ]; then
          echo "Run xtask inside an Odyssey checkout or set ODYSSEY_REPO" >&2
          exit 1
        fi
        export ODYSSEY_REPO="$root"
      fi
      exec ${xtaskBinary}/bin/xtask "$@"
    '';
  };
in {
  # Host dependency layer, shared with other native-glibc builds of this workspace.
  artifacts = hostArtifacts;
  packages = {
    default = host;
    inherit host tracking xtask;
  };
  checks = {
    tracking-tests = craneLib.cargoTest (common.withPrivateSource trackingArgs // {
      cargoArtifacts = trackingArtifacts;
      cargoExtraArgs = "--locked --offline -p odyssey-sim -p odyssey-hub-server --features odyssey-hub-server/tracking-regression";
    });
    host-tests = craneLib.cargoTest (common.withPrivateSource hostArgs // { cargoArtifacts = hostArtifacts; });
    postcard-compatibility = craneLib.mkCargoDerivation ((common.mkArgs {
      pname = "odyssey-postcard-compatibility";
    }) // {
      cargoArtifacts = null;
      buildPhaseCargoCommand = ''
        cargo test --locked --offline -p odyssey-protocol -p odyssey-protocol-macros \
          --features odyssey-protocol/schema,odyssey-protocol/minicbor --lib --tests
        cargo test --locked --offline -p odyssey-protocol --no-default-features \
          --features schema,serde-no-std,device,minicbor --lib --tests
        cargo clippy --locked --offline -p odyssey-protocol -p odyssey-protocol-macros \
          --features odyssey-protocol/schema,odyssey-protocol/minicbor \
          --all-targets -- -D warnings -A dead_code_pub_in_binary
      '';
      doCheck = false;
      installPhaseCommand = "touch $out";
    });
    host-lint = craneLib.cargoClippy (common.withPrivateSource hostArgs // {
      cargoArtifacts = hostArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings -A dead_code_pub_in_binary";
    });
    host-lint-bins = craneLib.cargoClippy (common.withPrivateSource hostArgs // {
      cargoArtifacts = hostArtifacts;
      cargoClippyExtraArgs = "--bins -- -D warnings";
    });
    xtask-tests = craneLib.cargoTest (xtaskArgs // { cargoArtifacts = xtaskArtifacts; });
    xtask-lint = craneLib.cargoClippy (xtaskArgs // {
      cargoArtifacts = xtaskArtifacts;
      cargoClippyExtraArgs = "--all-targets -- -D warnings -A dead_code_pub_in_binary";
    });
    xtask-lint-bins = craneLib.cargoClippy (xtaskArgs // {
      cargoArtifacts = xtaskArtifacts;
      cargoClippyExtraArgs = "--bins -- -D warnings";
    });
    format = pkgs.runCommand "odyssey-format" {
      nativeBuildInputs = [ craneLib.rustfmt ];
    } ''
      find ${common.source} -type f -name '*.rs' -print0 | xargs -0 rustfmt --check
      touch "$out"
    '';
  };
}
