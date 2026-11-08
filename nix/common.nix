{ pkgs, craneLib, src, privateSource }:
let
  inherit (pkgs) lib;
  source = lib.cleanSourceWith {
    src = src;
    filter = path: type:
      let
        name = baseNameOf path;
        relative = lib.removePrefix "${src}/" path;
        top = builtins.head (lib.splitString "/" relative);
        rustInput = path == toString src || builtins.elem top [
          "crates" "firmware" "sdk" "tools" ".cargo"
          "Cargo.toml" "Cargo.lock" "rust-toolchain.toml" "rustfmt.toml" "clippy.toml" "LICENSE"
        ];
      in rustInput && !(builtins.elem name [ ".git" ".jj" "target" "graphify-out" "result" ])
        && !(type == "directory" && builtins.elem name [ "build" "dist" "dist-bootloader" "dist-netcore" "target-bank-a" "target-bank-b" ]);
  };
  commonTypes = lib.cleanSource ../crates/odyssey-common;
  vendor = workspace: overrides: craneLib.vendorCargoDeps ({
    cargoLock = source + "/${workspace}/Cargo.lock";
    overrideVendorGitCheckout = packages: drv:
      if lib.any (package: lib.hasInfix "github.com/odysseyarm/odyssey-cv.git" package.source) packages
      then
        assert lib.assertMsg
          (!(privateSource ? rev) || lib.all (package: lib.hasSuffix "#${privateSource.rev}" package.source) packages)
          "odyssey-cv flake input must match its Cargo.lock revision";
        drv.overrideAttrs (old: {
          src = privateSource;
          postPatch = (old.postPatch or "") + ''
            # Upstream eskf names an absent README; Cargo packaging validates it.
            substituteInPlace third_party/eskf/Cargo.toml \
              --replace-fail 'readme = "README.md"' 'readme = false'
          '';
        })
      else drv;
  } // overrides);
  mkArgs = { workspace ? ".", pname, cargoExtraArgs ? "--locked --offline", vendorOverrides ? {}, ... }@args:
    (builtins.removeAttrs args [ "workspace" "vendorOverrides" ]) // {
      inherit pname cargoExtraArgs;
      version = "0.0.0";
      src = source;
      cargoLock = source + "/${workspace}/Cargo.lock";
      cargoVendorDir = vendor workspace vendorOverrides;
      extraDummyScript = lib.optionalString (workspace == ".") ''
        # Vendored odyssey-cv depends on this local crate's real tracking types.
        rm -rf "$out/crates/odyssey-common/src"
        cp -r ${commonTypes}/src "$out/crates/odyssey-common/src"
      '';
      postUnpack = lib.optionalString (workspace != ".") ''
        sourceRoot="$sourceRoot/${workspace}"
      '';
      strictDeps = true;
      nativeBuildInputs = [ pkgs.pkg-config pkgs.protobuf ];
      PROTOC = "${pkgs.protobuf}/bin/protoc";
    };
in {
  inherit source vendor mkArgs;
}
