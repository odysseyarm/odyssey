{
  pkgs,
  craneLib,
  src,
  privateSource,
}:
let
  inherit (pkgs) lib;
  source = lib.cleanSourceWith {
    src = src;
    filter =
      path: type:
      let
        name = baseNameOf path;
        relative = lib.removePrefix "${src}/" path;
        top = builtins.head (lib.splitString "/" relative);
        rustInput =
          path == toString src
          || builtins.elem relative [
            ".config"
            ".config/nextest.toml"
          ]
          || builtins.elem top [
            "crates"
            "firmware"
            "sdk"
            "tools"
            ".cargo"
            "Cargo.toml"
            "Cargo.lock"
            "rust-toolchain.toml"
            "rustfmt.toml"
            "clippy.toml"
            "LICENSE"
            "THIRD_PARTY_NOTICES.txt"
          ];
      in
      rustInput
      && !(builtins.elem name [
        ".git"
        ".jj"
        "target"
        "graphify-out"
        "result"
      ])
      && !(
        type == "directory"
        && builtins.elem name [
          "build"
          "dist"
          "dist-bootloader"
          "dist-netcore"
          "target-bank-a"
          "target-bank-b"
        ]
      );
  };
  # Private-source boundary
  #
  # odyssey-cv (a private git dependency) and the eskf crate it carries must
  # never reach the public binary cache. Three pieces enforce that:
  #
  # 1. Stub vendoring (`privateCheckout`, via `vendor`). Every vendored copy of
  #    the private crates keeps the real Cargo.toml and Cargo.lock, so features
  #    and the dependency graph are unchanged, but its sources are empty. The
  #    stub is addressed by manifest content alone, so cached dependency layers
  #    survive odyssey-cv changes that leave its manifests alone.
  # 2. Overlay in the final build (`withPrivateSource`). Dependency layers
  #    (buildDepsOnly) compile only the stub and are safe to publish. The final
  #    build, and any check that needs the real code, wraps its args in
  #    `withPrivateSource`. Inside the sandbox it copies the stub's vendored
  #    crates, overlays the flake input's sources, and repoints only that one
  #    Cargo source at the copy. The real source is never a derivation output
  #    and every other vendored path stays byte-identical, so Cargo rebuilds
  #    only odyssey-cv, eskf and their dependents on top of the cached layer.
  # 3. Publication policy. Outputs built from the real source are listed so the
  #    cache push skips them or publishes only the finished output: see
  #    `publicCache` in default.nix and `privatePackages` in firmware.nix.
  #    `privateBoundaryCheck` verifies piece 1 and runs as the
  #    `private-source-boundary` check.
  #
  # Callers: host.nix, appimage.nix, desktop-*.nix and firmware.nix apply
  # `withPrivateSource`; `vendor` here and `vendorArgs` in firmware.nix apply
  # `privateCheckout`.
  #
  # Manifests only (plus directories, so the tree layout is kept).
  privateManifests = builtins.path {
    name = "odyssey-cv-manifests";
    path = privateSource;
    filter =
      path: type:
      type == "directory"
      || builtins.elem (baseNameOf path) [
        "Cargo.toml"
        "Cargo.lock"
      ];
  };
  privateCrates = {
    odyssey-cv = "crates/odyssey-cv";
    eskf = "third_party/eskf";
  };
  # Wraps crane args so the build overlays the real odyssey-cv and eskf sources
  # over the stub vendor directory (piece 2 above). Fails if the vendor
  # directory holds no odyssey-cv. Pass only final builds, never buildDepsOnly.
  withPrivateSource =
    args:
    args
    // {
      postConfigure = (args.postConfigure or "") + ''
        stub=""
        for dir in $(sed -n 's/^directory = "\(.*\)"$/\1/p' "$CARGO_HOME/config.toml"); do
          for crate in "$dir"/odyssey-cv-*; do [ -e "$crate" ] && stub=$dir; done
        done
        if [ -z "$stub" ]; then echo "withPrivateSource: no vendored odyssey-cv" >&2; exit 1; fi
        private="$NIX_BUILD_TOP/private-vendor"
        cp -rL "$stub" "$private"
        chmod -R u+w "$private"
        ${lib.concatStrings (
          lib.mapAttrsToList (name: dir: ''
            for crate in "$private"/${name}-*; do
              [ -e "$crate" ] || continue
              cp -r --no-preserve=mode ${privateSource}/${dir}/. "$crate.real"
              cp "$crate/Cargo.toml" "$crate/.cargo-checksum.json" "$crate.real/"
              rm -rf "$crate"
              mv "$crate.real" "$crate"
            done
          '') privateCrates
        )}
        sed -i "s|^directory = \"$stub\"$|directory = \"$private\"|" "$CARGO_HOME/config.toml"
      '';
    };
  # `overrideVendorGitCheckout` hook: replaces a checkout of the odyssey-cv git
  # repository with the manifest-only stub (piece 1 above) and leaves every other
  # git dependency alone. Also checks that the flake input is the revision
  # Cargo.lock pins, since the overlay trusts it.
  privateCheckout =
    packages: drv:
    if
      lib.any (package: lib.hasInfix "github.com/odysseyarm/odyssey-cv.git" package.source) packages
    then
      assert lib.assertMsg (
        !(privateSource ? rev)
        || lib.all (package: lib.hasSuffix "#${privateSource.rev}" package.source) packages
      ) "odyssey-cv flake input must match its Cargo.lock revision";
      drv.overrideAttrs (old: {
        src = privateManifests;
        postPatch = (old.postPatch or "") + ''
          # Upstream eskf names a README that does not exist, and Cargo packaging validates it.
          substituteInPlace third_party/eskf/Cargo.toml \
            --replace-fail 'readme = "README.md"' 'readme = false'
          # no_std so the stub also builds for firmware targets.
          find . -mindepth 2 -name Cargo.toml -execdir sh -c 'mkdir -p src && echo "#![no_std]" > src/lib.rs' \;
        '';
      })
    else
      drv;
  vendor =
    workspace: overrides:
    craneLib.vendorCargoDeps (
      {
        cargoLock = source + "/${workspace}/Cargo.lock";
        overrideVendorGitCheckout = privateCheckout;
      }
      // overrides
    );
  mkArgs =
    {
      workspace ? ".",
      pname,
      cargoExtraArgs ? "--locked --offline",
      vendorOverrides ? { },
      ...
    }@args:
    (builtins.removeAttrs args [
      "workspace"
      "vendorOverrides"
    ])
    // {
      inherit pname cargoExtraArgs;
      version = "0.0.0";
      src = source;
      cargoLock = source + "/${workspace}/Cargo.lock";
      cargoVendorDir = vendor workspace vendorOverrides;
      postUnpack = lib.optionalString (workspace != ".") ''
        sourceRoot="$sourceRoot/${workspace}"
      '';
      strictDeps = true;
      nativeBuildInputs = [
        pkgs.pkg-config
        pkgs.protobuf
      ];
      PROTOC = "${pkgs.protobuf}/bin/protoc";
    }
    // lib.optionalAttrs (workspace == ".") {
      # Cargo artifact archives of the root workspace hold compiled private
      # code, so only dependency layers (which force this on) may install one.
      doInstallCargoArtifacts = false;
      # Release binaries carry no symbol table. Profiles are part of every
      # fingerprint, so this applies to the dependency layer too.
      CARGO_PROFILE_RELEASE_STRIP = "symbols";
    };
  # The `private-source-boundary` check: fails if any vendored copy of the
  # private crates under `vendors` has a source file other than the stub's
  # `#![no_std]`, or if a vendor directory holds no private crate (which would
  # make the check pass vacuously).
  privateBoundaryCheck =
    name: vendors:
    pkgs.runCommand name { } ''
      for vendor in ${lib.escapeShellArgs vendors}; do
        found=0
        for crate in "$vendor"/*/odyssey-cv-* "$vendor"/*/eskf-*; do
          [ -e "$crate" ] || continue
          found=1
          real=$(find -L "$crate" -name '*.rs' ! -empty -exec sh -c 'echo "#![no_std]" | cmp -s - "$1" || echo "$1"' _ {} \;)
          if [ -n "$real" ]; then echo "vendored stub has sources: $real" >&2; exit 1; fi
        done
        [ "$found" = 1 ] || { echo "no vendored private crates in $vendor" >&2; exit 1; }
      done
      touch $out
    '';
in
{
  inherit
    source
    vendor
    mkArgs
    withPrivateSource
    privateCheckout
    privateBoundaryCheck
    ;
}
