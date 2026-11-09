{ pkgs, craneLib, common, nativeInputs, nativeLibraries, nativeEnvironment }:
let
  inherit (pkgs) lib;

  manifest = builtins.fromTOML (builtins.readFile (common.source + "/crates/odyssey-desktop/Cargo.toml"));
  inherit (manifest.package) version;
  packager = manifest.package.metadata.packager;
  mainBinary = (lib.findFirst (b: b.main or false) (throw "odyssey-desktop packager metadata has no main binary") packager.binaries).path;
  iconPath = lib.findFirst (lib.hasSuffix ".png") (throw "odyssey-desktop packager metadata has no png icon") packager.icons;
  icon = common.source + "/crates/odyssey-desktop/${iconPath}";
  runtimeLibraries =
    (lib.filter (p: lib.getName p != "opencv") nativeLibraries)
    ++ [ pkgs.freetype pkgs.stdenv.cc.cc.lib pkgs.zlib
      pkgs.wayland pkgs.vulkan-loader pkgs.libglvnd pkgs.libx11
      pkgs.libxcursor pkgs.libxi pkgs.libxrandr
    ];
  runtimeLibraryPath = lib.makeLibraryPath runtimeLibraries + ":/run/opengl-driver/lib";
  args = common.mkArgs {
    pname = "odyssey-desktop-nixos";
    cargoExtraArgs = "--locked --offline --package odyssey-desktop";
  } // nativeEnvironment // {
    inherit version;
    nativeBuildInputs = nativeInputs;
    buildInputs = (lib.filter (p: lib.getName p != "opencv") nativeLibraries) ++ [ pkgs.freetype ];
    doCheck = false;
  };
  cargoArtifacts = craneLib.buildDepsOnly args;
  desktop = craneLib.buildPackage (common.withPrivateSource args // {
    inherit cargoArtifacts;
    meta.mainProgram = mainBinary;
    postInstall = ''
      wrapProgram "$out/bin/${mainBinary}" \
        --prefix LD_LIBRARY_PATH : "${runtimeLibraryPath}"
      install -Dm644 ${icon} "$out/share/icons/hicolor/256x256/apps/${mainBinary}.png"
      mkdir -p "$out/share/applications"
      cat > "$out/share/applications/${mainBinary}.desktop" <<EOF
      [Desktop Entry]
      Type=Application
      Name=${packager.productName}
      Comment=${packager.description}
      Exec=$out/bin/${mainBinary}
      Icon=${mainBinary}
      Terminal=false
      Categories=Utility;
      EOF
    '';
  });
in {
  packages.desktop-nixos = desktop;
  checks.desktop-nixos-glibc = pkgs.runCommand "odyssey-desktop-nixos-check" {
    nativeBuildInputs = [ pkgs.patchelf ];
  } ''
    exe=${desktop}/bin/.${mainBinary}-wrapped
    test "$(patchelf --print-interpreter "$exe")" = "${pkgs.glibc}/lib/ld-linux-x86-64.so.2"
    touch $out
  '';
}
