# SDK builds: the C/C++ FFI library (sdk/odyssey-hub-client-ffi) and the UniFFI
# library (sdk/odyssey-hub-client-uniffi) with its Python wheels, C# natives and
# C# bindings.
#
# Every row of .github/workflows/ffi-build.yml and uniffi-build.yml is a Nix
# output here, built with Crane so each (crate, target) dependency graph is its
# own cached derivation:
#
#   linux gnu/musl   cargo-zigbuild. Zig supplies the target libc: the glibc
#                    rows link against a glibc 2.17 baseline (manylinux2014),
#                    NOT nixpkgs' glibc, and the musl rows against zig's musl.
#   windows msvc     cargo-xwin against the hash-pinned sysroot in ./xwin.nix.
#   macos            native cargo on a darwin builder (the x86_64 target is
#                    cross-built on aarch64 by the same SDK).
#
# Python wheels come from maturin itself (same tool, wheel metadata, platform
# tags and manylinux/musllinux audit as maturin-action), driven against the
# vendored crates inside the sandbox.
{ pkgs, craneLib, common, toolchain }:
let
  inherit (pkgs) lib stdenv;
  xwin = import ./xwin.nix { inherit pkgs; };

  # The repo's one toolchain pin (channel only) plus exactly the std targets a
  # build needs. Components stay empty: SDK builds need no clippy/rustfmt.
  pin = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain;
  toolchainFor = targets: pkgs.rust-bin.fromRustupToolchain {
    inherit (pin) channel profile;
    components = [ ];
    targets = lib.unique targets;
  };
  craneFor = targets: craneLib.overrideToolchain (toolchainFor targets);

  ffi = { dir = "sdk/odyssey-hub-client-ffi"; package = "odyssey-hub-client-ffi"; };
  uniffi = { dir = "sdk/odyssey-hub-client-uniffi"; package = "odyssey-hub-client-uniffi"; };
  argsFor = ws: suffix: common.mkArgs {
    workspace = ws.dir;
    pname = "${ws.package}${suffix}";
  };

  gnu = {
    x86_64 = "x86_64-unknown-linux-gnu";
    x86 = "i686-unknown-linux-gnu";
    aarch64 = "aarch64-unknown-linux-gnu";
    armv7 = "armv7-unknown-linux-gnueabihf";
    ppc64le = "powerpc64le-unknown-linux-gnu";
  };
  musl = {
    x86_64 = "x86_64-unknown-linux-musl";
    x86 = "i686-unknown-linux-musl";
    aarch64 = "aarch64-unknown-linux-musl";
    armv7 = "armv7-unknown-linux-musleabihf";
  };
  windows = xwin.target;
  macos = {
    x64 = "x86_64-apple-darwin";
    arm64 = "aarch64-apple-darwin";
  };

  # manylinux2014's glibc. The released cross images were newer (2.23+), so
  # everything that loaded the old libs loads these. zig ships no ppc64le stubs
  # below 2.19 and auditwheel defines no manylinux_2_19/2_20-2_23 policy, so the
  # ppc64le row is manylinux_2_24.
  glibcFloor = triple: if lib.hasPrefix "powerpc64le" triple then "2.24" else "2.17";

  # Writable HOME/cache dirs: the sandbox HOME does not exist and zig, maturin
  # and cargo-xwin all want a cache directory.
  sandbox = ''
    export HOME="$TMPDIR/home"
    export XDG_CACHE_HOME="$TMPDIR/xdg-cache"
    export CARGO_NET_OFFLINE=true
    mkdir -p "$HOME" "$XDG_CACHE_HOME"
  '';

  # How one target is compiled against a given glibc floor (gnu rows only).
  # `build` is the cargo front end (without -p); `maturin` the extra maturin
  # flags that select the same cross backend.
  backendWith = glibc: triple:
    if lib.hasInfix "windows" triple then {
      tools = xwin.tools;
      prelude = sandbox + xwin.setup;
      build = "cargo xwin build --release --locked --offline --target ${triple}";
      maturin = "";
    } else if lib.hasInfix "apple-darwin" triple then {
      tools = [ ];
      # nixpkgs' darwin stdenv exports its own (newer) deployment target, which
      # would change the wheel tags and the libs' minimum macOS; keep rustc's.
      prelude = sandbox + ''
        export MACOSX_DEPLOYMENT_TARGET=${if lib.hasPrefix "aarch64" triple then "11.0" else "10.12"}
      '';
      build = "cargo build --release --locked --offline --target ${triple}";
      maturin = "";
    } else {
      tools = [ pkgs.cargo-zigbuild pkgs.zig ];
      prelude = sandbox + ''
        export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-global-cache"
        export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
        # Zig links the target only. Build scripts and proc macros keep the Nix
        # linker: a zig-linked executable asks for /lib64/ld-linux-*, which the
        # sandbox lacks (matters when the target triple equals the host's).
        export CARGO_UNSTABLE_HOST_CONFIG=true
        export CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST=true
        export CARGO_TARGET_APPLIES_TO_HOST=false
        export CARGO_HOST_LINKER=cc
        # rustc's default release `strip = "debuginfo"` reaches zig as a full
        # strip, dropping .symtab; uniffi-bindgen reads UniFFI metadata from it.
        export CARGO_PROFILE_RELEASE_STRIP=none
      '' + lib.optionalString (lib.hasInfix "musl" triple) ''
        # A cdylib needs a dynamic musl (musllinux wheels load into CPython).
        export RUSTFLAGS="-C target-feature=-crt-static"
      '';
      build = "cargo zigbuild --release --locked --offline --target ${
        if lib.hasInfix "-gnu" triple then "${triple}.${glibc}" else triple
      }";
      maturin = "--zig --compatibility ${
        if lib.hasInfix "musl" triple then "musllinux_1_2"
        else if glibc == "2.17" then "manylinux2014"
        else "manylinux_${lib.replaceStrings [ "." ] [ "_" ] glibc}"
      }";
    };
  backendFor = triple: backendWith (glibcFloor triple) triple;

  # (workspace, target) -> the cached dependency build and a derivation helper
  # that reuses it.
  mkTarget = ws: triple:
    let
      craneL = craneFor [ triple ];
      backend = backendFor triple;
      base = argsFor ws "-${triple}" // {
        nativeBuildInputs = (argsFor ws "").nativeBuildInputs ++ backend.tools;
        preBuild = backend.prelude;
        doInstallCargoArtifacts = false;
        # Keep the artifacts byte-for-byte: no strip/patchelf on foreign ELF.
        dontFixup = true;
      };
      cargoBuild = "${backend.build} -p ${ws.package} --lib";
      deps = craneL.buildDepsOnly (base // {
        doCheck = false;
        doInstallCargoArtifacts = true;
        # maturin runs the workspace's own `cargo run --bin uniffi-bindgen` on the
        # host (dev profile); build its dependencies here so they are cached too.
        buildPhaseCargoCommand = cargoBuild + lib.optionalString (ws == uniffi) ''

          cargo build --locked --offline -p odyssey-uniffi-bindgen
        '';
      });
      derive = args: craneL.mkCargoDerivation (base // { cargoArtifacts = deps; } // args);
      # Raw native artifacts: scrub toolchain/vendor store paths out of the
      # binaries so the closure stays small (crane's own hooks, byte-preserving).
      scrub = [ craneL.removeReferencesToRustToolchainHook craneL.removeReferencesToVendoredSourcesHook ];
    in {
      inherit deps;
      native = { suffix, install }: derive {
        pnameSuffix = suffix;
        nativeBuildInputs = base.nativeBuildInputs ++ scrub;
        buildPhaseCargoCommand = cargoBuild;
        installPhaseCommand = install;
      };
      wheel = derive {
        pnameSuffix = "-wheel";
        nativeBuildInputs = base.nativeBuildInputs ++ [ pkgs.maturin pkgs.python3 ];
        # nixpkgs' python hooks export _PYTHON_HOST_PLATFORM, which maturin
        # honors over its own manylinux/macOS platform tag detection.
        buildPhaseCargoCommand = ''
          unset _PYTHON_HOST_PLATFORM
          maturin build --release --locked --offline --out dist --target ${triple} ${backend.maturin}
        '';
        installPhaseCommand = ''
          mkdir -p $out/dist
          cp dist/*.whl $out/dist/
        '';
      };
    };

  # Install `from` (relative to target/<triple>/release) as `to` under $out/<dir>.
  installFiles = triple: dir: files: ''
    mkdir -p $out/${dir}
    ${lib.concatStringsSep "\n" (lib.mapAttrsToList (to: from: "cp target/${triple}/release/${from} $out/${dir}/${to}") files)}
  '';

  # ---- C/C++ FFI (CMake bundle inputs) ----------------------------------------
  # Output layout: libs/<final bundle file names>, include/ (cbindgen headers,
  # identical for every target; build.rs writes them next to the crate).
  ffiHeaders = "mkdir -p $out/include && cp -r generated/include/. $out/include/";
  ffiRow = triplet: triple: files:
    (mkTarget ffi triple).native {
      suffix = "-libs-${triplet}";
      install = installFiles triple "libs" files + "\n" + ffiHeaders;
    };
  ffiLinux = triplet: triple: ffiRow triplet triple {
    "libohc__${triplet}.a" = "libohc.a";
    "libohc__${triplet}.so" = "libohc.so";
  };
  ffiRows = lib.optionalAttrs stdenv.hostPlatform.isLinux {
    "sdk-ffi-linux_x64" = ffiLinux "linux_x64" gnu.x86_64;
    "sdk-ffi-linux_arm64" = ffiLinux "linux_arm64" gnu.aarch64;
    "sdk-ffi-win_x64" = ffiRow "win_x64" windows {
      "ohc__win_x64.lib" = "ohc.lib";
      "ohc__win_x64.dll" = "ohc.dll";
      "ohc__win_x64.dll.lib" = "ohc.dll.lib";
    };
  } // lib.optionalAttrs stdenv.hostPlatform.isDarwin (lib.mapAttrs' (name: triple:
    lib.nameValuePair "sdk-ffi-macos_${name}" (ffiRow "macos_${name}" triple {
      "libohc__macos_${name}.a" = "libohc.a";
      "libohc__macos_${name}.dylib" = "libohc.dylib";
    })) macos);

  # ---- UniFFI: Python wheels ---------------------------------------------------
  # Artifact names keep the CI scheme python-<os>-<manylinux>-<target>.
  wheelRows =
    lib.optionalAttrs stdenv.hostPlatform.isLinux (
      lib.mapAttrs' (arch: triple: lib.nameValuePair "sdk-python-linux-auto-${arch}" (mkTarget uniffi triple).wheel) gnu
      // lib.mapAttrs' (arch: triple: lib.nameValuePair "sdk-python-linux-musllinux_1_2-${arch}" (mkTarget uniffi triple).wheel) musl
      // { "sdk-python-windows-${windows}" = (mkTarget uniffi windows).wheel; }
    ) // lib.optionalAttrs stdenv.hostPlatform.isDarwin (
      lib.mapAttrs' (name: triple: lib.nameValuePair "sdk-python-macos-${if name == "x64" then "x86_64" else "aarch64"}" (mkTarget uniffi triple).wheel) macos
    );

  # ---- UniFFI: C# natives -------------------------------------------------------
  # One file per triple, named as the NuGet project expects it under
  # target/<triple>/release/.
  dotnetNative = triple: file:
    (mkTarget uniffi triple).native {
      suffix = "-native";
      install = installFiles triple "" { ${file} = file; };
    };
  dotnetRows =
    lib.optionalAttrs stdenv.hostPlatform.isLinux {
      "sdk-dotnet-native-${gnu.x86_64}" = dotnetNative gnu.x86_64 "libohc_uniffi.so";
      "sdk-dotnet-native-${gnu.aarch64}" = dotnetNative gnu.aarch64 "libohc_uniffi.so";
      "sdk-dotnet-native-${windows}" = dotnetNative windows "ohc_uniffi.dll";
    } // lib.optionalAttrs stdenv.hostPlatform.isDarwin (lib.mapAttrs' (_: triple:
      lib.nameValuePair "sdk-dotnet-native-${triple}" (dotnetNative triple "libohc_uniffi.dylib")) macos);

  # ---- UniFFI: C# bindings generator ---------------------------------------------
  # Pinned to the tag the C# project and xtask name (v<tool>+v<uniffi>, must
  # match uniffi in sdk/odyssey-hub-client-uniffi/Cargo.toml; also
  # UNIFFI_BINDGEN_CS_TAG in crates/xtask/src/sdk.rs).
  uniffiBindgenCs =
    let
      args = {
        pname = "uniffi-bindgen-cs";
        version = "0.11.0+v0.31.0";
        src = pkgs.fetchFromGitHub {
          owner = "NordSecurity";
          repo = "uniffi-bindgen-cs";
          rev = "e10ce410eb3a10cc19c7928b93ea8d84e038c034"; # tag v0.11.0+v0.31.0
          hash = "sha256-vrT6t5YV4jH+434so/4bL5sTyZatCm1XvzZXExhAuXU=";
        };
        strictDeps = true;
        cargoExtraArgs = "--locked -p uniffi-bindgen-cs";
        # Third-party generator: its own tests need a .NET fixture toolchain
        # and are not part of this repo's gate.
        doCheck = false;
      };
    in craneLib.buildPackage (args // { cargoArtifacts = craneLib.buildDepsOnly args; });

  # Host builds of the UniFFI workspace: the repo's cs_bindgen post-processor and
  # the crate's unit tests share one dependency build.
  uniffiHostArgs = argsFor uniffi "-host" // {
    cargoExtraArgs = "--locked --offline";
  };
  uniffiHostDeps = craneLib.buildDepsOnly uniffiHostArgs;
  csBindgen = craneLib.buildPackage (uniffiHostArgs // {
    cargoArtifacts = uniffiHostDeps;
    cargoExtraArgs = "--locked --offline --bin cs_bindgen";
    doCheck = false; # tests run in checks.sdk-uniffi-tests
  });

  # C# bindings: uniffi-bindgen-cs reads the UniFFI metadata of the x86_64
  # linux native (as CI always did) inside the crate's workspace, then
  # cs_bindgen applies the visibility patches.
  csBindings = craneLib.mkCargoDerivation (argsFor uniffi "-csharp-bindings" // {
    cargoArtifacts = null;
    doInstallCargoArtifacts = false;
    nativeBuildInputs = (argsFor uniffi "").nativeBuildInputs ++ [ csBindgen uniffiBindgenCs ];
    preBuild = sandbox;
    buildPhaseCargoCommand = ''
      cd dotnet/OdysseyHubClient
      cs_bindgen ${dotnetRows."sdk-dotnet-native-${gnu.x86_64}"}/libohc_uniffi.so OdysseyHubClient/generated
      cd ../..
    '';
    installPhaseCommand = ''
      mkdir -p $out
      cp -r dotnet/OdysseyHubClient/OdysseyHubClient/generated/. $out/
    '';
  });

  uniffiTests = craneLib.cargoTest (uniffiHostArgs // { cargoArtifacts = uniffiHostDeps; });

  # `pip install <wheel>; import ohc_uniffi` for the wheel this builder can run.
  wheelImport = name: wheel: pkgs.runCommand name {
    nativeBuildInputs = [ (pkgs.python3.withPackages (ps: [ ps.pip ])) ];
    LD_LIBRARY_PATH = lib.optionalString stdenv.hostPlatform.isLinux "${stdenv.cc.cc.lib}/lib";
  } ''
    export HOME=$TMPDIR
    pip install --no-index --no-deps --target "$TMPDIR/site" ${wheel}/dist/*.whl
    PYTHONPATH="$TMPDIR/site" python -c "import ohc_uniffi"
    touch $out
  '';
  nativeWheel =
    if stdenv.hostPlatform.isLinux then wheelRows."sdk-python-linux-auto-${if stdenv.hostPlatform.isAarch64 then "aarch64" else "x86_64"}"
    else wheelRows."sdk-python-macos-${if stdenv.hostPlatform.isAarch64 then "aarch64" else "x86_64"}";

  # ---- NuGet package ----------------------------------------------------------
  # The pack step is host-native (below) because the macOS natives come from
  # other runners; everything it consumes is pinned here: the .NET SDK and the
  # restored NuGet packages (a fixed-output `dotnet restore`).
  dotnetSdk = pkgs.dotnetCorePackages.combinePackages [
    pkgs.dotnetCorePackages.sdk_10_0
    pkgs.dotnetCorePackages.runtime_8_0
  ];
  dotnetEnv = ''
    export HOME="$TMPDIR/home"
    export DOTNET_CLI_HOME="$HOME"
    export DOTNET_NOLOGO=1
    export DOTNET_CLI_TELEMETRY_OPTOUT=1
    export DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
    export MSBUILDDISABLENODEREUSE=1
    mkdir -p "$HOME"
  '';
  dotnetProjectDir = "sdk/odyssey-hub-client-uniffi/dotnet/OdysseyHubClient";
  nugetDeps = pkgs.stdenvNoCC.mkDerivation {
    pname = "odyssey-hub-client-nuget-deps";
    version = "0.0.0";
    src = lib.cleanSourceWith {
      src = common.source + "/${dotnetProjectDir}";
      filter = path: _: !(builtins.elem (baseNameOf path) [ "bin" "obj" "generated" "nugets" "output" ]);
    };
    nativeBuildInputs = [ dotnetSdk ];
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-CkHz6LTm/RAs/sf8A1Vvi1UqPtDlFjQwUYcOlrP4Awc=";
    dontFixup = true;
    buildPhase = dotnetEnv + ''
      dotnet restore OdysseyHubClient/OdysseyArm.HubClient.csproj --source https://api.nuget.org/v3/index.json --packages "$out" -p:NuGetAudit=false
    '';
    installPhase = "true";
  };
  # nix run .#sdk-dotnet-pack -- [version] from the repository root, after the
  # natives are in sdk/odyssey-hub-client-uniffi/target/<triple>/release and the
  # bindings in OdysseyHubClient/generated: restores from the pinned packages
  # only, builds and packs OdysseyArm.HubClient into nugets/.
  dotnetPack = pkgs.writeShellApplication {
    name = "odyssey-dotnet-pack";
    runtimeInputs = [ dotnetSdk ];
    text = ''
      version="''${1:-}"
      export DOTNET_NOLOGO=1 DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
      cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)/${dotnetProjectDir}"
      version_props=()
      if [ -n "$version" ]; then
        version_props=(-p:Version="$version")
      fi
      dotnet restore OdysseyHubClient --source ${nugetDeps} -p:NuGetAudit=false
      # Explicit Release build first: GeneratePackageOnBuild makes `dotnet pack`
      # skip its implicit build, and pack looks in bin/Release.
      dotnet build OdysseyHubClient -c Release --no-restore "''${version_props[@]}"
      dotnet pack OdysseyHubClient -o nugets --no-restore "''${version_props[@]}"
    '';
  };

  allTargets = lib.optionals stdenv.hostPlatform.isLinux (lib.attrValues gnu ++ lib.attrValues musl ++ [ windows ])
    ++ lib.optionals stdenv.hostPlatform.isDarwin (lib.attrValues macos);
in {
  packages = ffiRows // wheelRows // dotnetRows // {
    "uniffi-bindgen-cs" = uniffiBindgenCs;
    "sdk-dotnet-sdk" = dotnetSdk;
    "sdk-dotnet-nuget-deps" = nugetDeps;
    "sdk-dotnet-pack" = dotnetPack;
  } // lib.optionalAttrs stdenv.hostPlatform.isLinux {
    "sdk-dotnet-csharp-bindings" = csBindings;
    "sdk-xwin-sysroot" = xwin.sysroot;
  };

  checks = {
    sdk-uniffi-tests = uniffiTests;
    sdk-python-wheel-import = wheelImport "sdk-python-wheel-import" nativeWheel;
  };

  devShell = pkgs.mkShell {
    packages = [ (toolchainFor allTargets) pkgs.protobuf pkgs.pkg-config pkgs.python3 pkgs.maturin uniffiBindgenCs ]
      ++ lib.optionals stdenv.hostPlatform.isLinux ([ pkgs.cargo-zigbuild pkgs.zig ] ++ xwin.tools)
      ++ [ dotnetSdk ];
    PROTOC = "${pkgs.protobuf}/bin/protoc";
    shellHook = lib.optionalString stdenv.hostPlatform.isLinux (xwin.setupAt "\${XDG_CACHE_HOME:-$HOME/.cache}/odyssey-xwin");
  };

  inherit xwin backendWith;
}
