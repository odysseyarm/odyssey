{ inputs, system, src }:
let
  pkgs = import inputs.nixpkgs {
    inherit system;
    overlays = [ inputs.rust-overlay.overlays.default ];
    config = {
      allowUnfreePredicate = package: builtins.elem (pkgs.lib.getName package) [
        "orbbec-sdk" "nrf-command-line-tools" "segger-jlink" "segger-jlink-headless"
        "nrfutil" "nrfutil-completion" "nrfutil-nrf5sdk-tools"
      ];
      segger-jlink.acceptLicense = true;
      permittedInsecurePackages = [ "segger-jlink-qt4-952" ];
    };
  };
  inherit (pkgs) lib;
  toolchain = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  craneLib = (inputs.crane.mkLib pkgs).overrideToolchain toolchain;
  common = import ./common.nix {
    inherit pkgs craneLib src;
    privateSource = inputs.odyssey-cv;
  };
  orbbecSdk = pkgs.callPackage ./pkgs/orbbec-sdk.nix {
    pin = builtins.fromJSON (builtins.readFile ../crates/odyssey-desktop/orbbec-sdk.json);
  };
  nativeInputs = [ pkgs.pkg-config pkgs.protobuf pkgs.cmake pkgs.llvmPackages.libclang pkgs.makeWrapper ];
  nativeLibraries = [ pkgs.openssl pkgs.opencv4 ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
    pkgs.udev pkgs.dbus pkgs.libxkbcommon pkgs.libxcb pkgs.fontconfig
  ];
  runtimeLibraries = nativeLibraries ++ [ pkgs.stdenv.cc.cc.lib pkgs.zlib ]
    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
    pkgs.wayland pkgs.vulkan-loader pkgs.libglvnd pkgs.libx11 pkgs.libxcursor
    pkgs.libxi pkgs.libxrandr
  ];
  nativeEnvironment = {
    PROTOC = "${pkgs.protobuf}/bin/protoc";
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    LIBRARY_PATH = lib.makeLibraryPath runtimeLibraries;
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibraries + lib.optionalString pkgs.stdenv.hostPlatform.isLinux ":/run/opengl-driver/lib";
  } // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
    OPENCV_CLANG_ARGS = builtins.concatStringsSep " " [
      "-isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/${lib.versions.major pkgs.llvmPackages.libclang.version}/include"
      "-isystem ${pkgs.stdenv.cc.cc}/include/c++/${pkgs.stdenv.cc.cc.version}"
      "-isystem ${pkgs.stdenv.cc.cc}/include/c++/${pkgs.stdenv.cc.cc.version}/${pkgs.stdenv.hostPlatform.config}"
      "-isystem ${pkgs.glibc.dev}/include"
    ];
  };
  host = import ./host.nix {
    inherit pkgs craneLib common nativeInputs nativeLibraries nativeEnvironment;
  };
  sdk = import ./sdk.nix { inherit pkgs craneLib common toolchain; };
  firmware = import ./firmware.nix {
    inherit pkgs craneLib common toolchain;
    xtask = host.packages.xtask;
    gitRev = src.dirtyShortRev or src.shortRev or "unknown";
  };
  appimage = if system == "x86_64-linux" then import ./appimage.nix {
    inherit pkgs craneLib common nativeInputs nativeLibraries nativeEnvironment;
    zigBackend = sdk.backendWith;
  } else { packages = {}; checks = {}; };
  desktopNixos = if system == "x86_64-linux" then import ./desktop-nixos.nix {
    inherit pkgs craneLib common nativeInputs nativeLibraries nativeEnvironment;
  } else { packages = {}; checks = {}; };
  windows = if system == "x86_64-linux" then import ./desktop-windows.nix {
    inherit pkgs craneLib common;
  } else { packages = {}; checks = {}; };
  shell = pkgs.mkShell ({
    packages = [ toolchain pkgs.cargo-nextest pkgs.python3 pkgs.gh ]
      ++ nativeInputs ++ nativeLibraries ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
        (pkgs.dotnetCorePackages.combinePackages [ pkgs.dotnetCorePackages.sdk_10_0 pkgs.dotnetCorePackages.runtime_8_0 ])
      ];
    RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
    shellHook = ''export PATH="${toolchain}/bin:$PATH"'';
  } // nativeEnvironment // lib.optionalAttrs (system == "x86_64-linux") {
    ODYSSEY_ORBBEC_SDK_DIR = "${orbbecSdk}/lib";
  });
in {
  inherit pkgs;
  packages = host.packages // sdk.packages // firmware.packages // appimage.packages // desktopNixos.packages // windows.packages
    // lib.optionalAttrs (system == "x86_64-linux") { orbbec-sdk = orbbecSdk; };
  checks = host.checks // sdk.checks // firmware.checks // appimage.checks // desktopNixos.checks // windows.checks
    // { private-source-boundary = common.privateBoundaryCheck; };
  devShells = {
    default = shell;
    sdk = sdk.devShell;
    firmware = firmware.devShell;
    tracking = pkgs.mkShell {
      inputsFrom = [ shell ];
      packages = [ pkgs.blender pkgs.rerun (pkgs.python3.withPackages (ps: [ ps.rerun-sdk ])) ];
    };
  };
}
