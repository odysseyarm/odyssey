{ pkgs, craneLib, common, nativeInputs, nativeLibraries, nativeEnvironment, zigBackend }:
let
  inherit (pkgs) lib;

  manifest = builtins.fromTOML (builtins.readFile (common.source + "/crates/odyssey-desktop/Cargo.toml"));
  version = let buildVersion = builtins.getEnv "ODYSSEY_BUILD_VERSION"; in
    if buildVersion == "" then manifest.package.version else buildVersion;
  packager = manifest.package.metadata.packager;
  mainBinary = (lib.findFirst (b: b.main or false) (throw "odyssey-desktop packager metadata has no main binary") packager.binaries).path;
  iconPath = lib.findFirst (lib.hasSuffix ".png") (throw "odyssey-desktop packager metadata has no png icon") packager.icons;
  icon = common.source + "/crates/odyssey-desktop/${iconPath}";
  imageName = "Odyssey-${version}-x86_64.AppImage";

  # The desktop app only: no opencv (nothing in its dependency graph links it),
  # plus freetype, which the GPUI text stack links directly.
  libs = (lib.filter (p: lib.getName p != "opencv") nativeLibraries) ++ [ pkgs.freetype ];
  # zigbuild links against a glibc 2.39 baseline (zig's version stubs), not
  # nixpkgs' glibc, so the binary binds only symbols a 2.39 host exports.
  backend = zigBackend "2.39" "x86_64-unknown-linux-gnu";
  args = common.mkArgs {
    pname = "odyssey-desktop";
    cargoExtraArgs = "--package odyssey-desktop";
  } // nativeEnvironment // {
    inherit version;
    ODYSSEY_BUILD_VERSION = version;
    nativeBuildInputs = nativeInputs ++ backend.tools;
    buildInputs = libs;
    preBuild = backend.prelude + ''
      # zig's linker ignores NIX_LDFLAGS/LIBRARY_PATH, so name the native
      # library directories (xcb, xkbcommon, ...) to rustc directly.
      export RUSTFLAGS="$RUSTFLAGS ${lib.concatMapStringsSep " " (p: "-L native=${lib.getLib p}/lib") libs}"
      # Release binary: no symbol table (the zig prelude keeps one for UniFFI
      # metadata, which this app does not use).
      export CARGO_PROFILE_RELEASE_STRIP=symbols
    '';
    cargoBuildCommand = backend.build;
    doCheck = false;
  };
  cargoArtifacts = craneLib.buildDepsOnly (args // {
    buildPhaseCargoCommand = "${backend.build} ${args.cargoExtraArgs}";
  });
  desktop = craneLib.buildPackage (common.withPrivateSource args // { inherit cargoArtifacts; });

  # Portability contract, shared by the bundler and the verifier.
  #  * The loader, glibc and libgcc_s come from the host (PT_INTERP is the
  #    distro loader), so the host's graphics drivers load against the libc
  #    they were built for. maxVersions caps what the bundled objects may
  #    demand from that host. The zig baseline above keeps the binary under it.
  #  * Wayland/xkbcommon/Vulkan/EGL/GL come from the host, as
  #    package.metadata.packager.appimage.excludedLibs says. The other extras
  #    mirror what the 0.7.4 image also left to the host.
  policyData = {
    binary = mainBinary;
    desktopName = packager.productName;
    iconName = mainBinary;
    interpreter = "/lib64/ld-linux-x86-64.so.2";
    exeRunpath = "$ORIGIN/../lib";
    libRunpath = "$ORIGIN";
    maxVersions = { GLIBC = "2.39"; GCC = "12.0.0"; };
    hostLibs = packager.appimage.excludedLibs ++ [
      "ld-linux-x86-64.so.*" "libc.so.*" "libm.so.*" "libdl.so.*" "libpthread.so.*" "librt.so.*"
      "libutil.so.*" "libresolv.so.*" "libanl.so.*" "libmvec.so.*" "libnss_*" "libthread_db.so.*"
      "libBrokenLocale.so.*"
      "libgcc_s.so.*" "libxcb.so.*" "libX11.so.*" "libfreetype.so.*" "libfontconfig.so.*"
    ];
  };
  policy = pkgs.writeText "odyssey-appimage-policy.json" (builtins.toJSON policyData);

  desktopFile = pkgs.writeText "${mainBinary}.desktop" ''
    [Desktop Entry]
    Type=Application
    Name=${packager.productName}
    Comment=${packager.description}
    Exec=${mainBinary}
    Icon=${mainBinary}
    Terminal=false
    Categories=Utility;
    X-AppImage-Version=${version}
  '';
  appRun = pkgs.writeText "AppRun" ''
    #!/bin/sh
    here=$(dirname "$(readlink -f "$0")")
    exec "$here/usr/bin/${mainBinary}" "$@"
  '';

  # type2-runtime release 20251108: static, no libc, FUSE via the host's
  # fusermount. Pinned by hash, nothing is fetched while building.
  runtime = pkgs.fetchurl {
    url = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
    hash = "sha256-L8qLRDySUQ8Ug6iD9gBhrQm0a5eLJjHIB82HOkfsJg0=";
  };

  appimage = pkgs.stdenvNoCC.mkDerivation {
    pname = "odyssey-desktop-appimage";
    inherit version;
    dontUnpack = true;
    # The output is an ELF (the runtime) with a squashfs appended: stripping or
    # patching it would corrupt the image.
    dontFixup = true;
    allowedReferences = [];
    nativeBuildInputs = [ pkgs.patchelf pkgs.python3 pkgs.binutils pkgs.squashfsTools ];
    buildPhase = ''
      runHook preBuild
      python3 ${./appimage/bundle.py} --exe ${desktop}/bin/${mainBinary} --appdir AppDir \
        --policy ${policy} --search ${lib.makeLibraryPath libs}
      install -Dm755 ${appRun} AppDir/AppRun
      install -Dm644 ${desktopFile} AppDir/${mainBinary}.desktop
      install -Dm644 ${desktopFile} AppDir/usr/share/applications/${mainBinary}.desktop
      install -Dm644 ${icon} AppDir/usr/share/icons/hicolor/256x256/apps/${mainBinary}.png
      install -m644 ${icon} AppDir/.DirIcon
      ln -s usr/share/icons/hicolor/256x256/apps/${mainBinary}.png AppDir/${mainBinary}.png
      python3 ${./appimage/verify.py} AppDir --policy ${policy}
      # mksquashfs takes timestamps from the SOURCE_DATE_EPOCH stdenv exports.
      mksquashfs AppDir image.squashfs -noappend -no-progress -all-root -no-xattrs \
        -comp zstd -Xcompression-level 19
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cat ${runtime} image.squashfs > $out/${imageName}
      chmod 755 $out/${imageName}
      runHook postInstall
    '';
    passthru = { inherit imageName desktop policy; };
    meta.platforms = [ "x86_64-linux" ];
  };

  # Opens the finished file the way a user's machine would (the AppImage
  # runtime extracts its own payload), then holds it to the contract. The
  # mutations prove the verifier rejects the failures it exists to catch.
  contract = pkgs.runCommand "odyssey-desktop-appimage-contract" {
    nativeBuildInputs = [ pkgs.python3 pkgs.binutils pkgs.patchelf pkgs.desktop-file-utils ];
  } ''
    verify() { python3 ${./appimage/verify.py} "$1" --policy ${policy}; }
    mkdir work && cd work
    ${appimage}/${imageName} --appimage-extract > /dev/null
    python3 ${./appimage/verify.py} squashfs-root --policy ${policy} \
      --appimage ${appimage}/${imageName} --appimage-name ${imageName}
    desktop-file-validate squashfs-root/*.desktop squashfs-root/usr/share/applications/*.desktop

    # reject <name> <expected message> <mutation command...>, run in the mutant's root.
    reject() {
      name=$1 expected=$2
      shift 2
      cp -r squashfs-root "mutant-$name" && chmod -R u+w "mutant-$name"
      (cd "mutant-$name" && "$@")
      report=$(verify "mutant-$name" 2>&1 || true)
      case "$report" in
        *"$expected"*) ;;
        *) echo "verifier did not report '$expected' for mutation $name:" >&2; echo "$report" >&2; exit 1 ;;
      esac
    }
    reject interp "interpreter is" \
      patchelf --set-interpreter ${pkgs.glibc}/lib/ld-linux-x86-64.so.2 usr/bin/${mainBinary}
    reject rpath "RUNPATH is" \
      patchelf --set-rpath ${pkgs.glibc}/lib usr/bin/${mainBinary}
    reject host "host library must not be bundled" \
      cp ${pkgs.glibc}/lib/libc.so.6 usr/lib/libc.so.6
    touch $out
  '';
in {
  packages.desktop-appimage = appimage;
  checks.desktop-appimage-contract = contract;
}
