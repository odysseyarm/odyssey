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
  # Private-source boundary. Every vendored copy of odyssey-cv (and the eskf it
  # carries) is a stub: the real manifests, so features and the dependency graph
  # are unchanged, with empty sources. Dependency layers (buildDepsOnly) compile
  # only that stub and are safe for public caches. Builds that need the real
  # code go through `withPrivateSource`: inside the sandbox it copies the stub's
  # vendored crates, overlays the flake input's sources, and repoints only that
  # one Cargo source at the copy. The real source is never a derivation output,
  # and every other vendored path stays byte-identical, so Cargo rebuilds only
  # odyssey-cv, eskf and their dependents on top of the cached layer.
  # The stub is addressed by manifest content alone, so dependency layers stay
  # cached across odyssey-cv changes that do not touch its manifests.
  privateManifests = builtins.path {
    name = "odyssey-cv-manifests";
    path = privateSource;
    filter = path: type:
      type == "directory" || builtins.elem (baseNameOf path) [ "Cargo.toml" "Cargo.lock" ];
  };
  privateCrates = { odyssey-cv = "crates/odyssey-cv"; eskf = "third_party/eskf"; };
  withPrivateSource = args: args // {
    postConfigure = (args.postConfigure or "") + ''
      stub=""
      for dir in $(sed -n 's/^directory = "\(.*\)"$/\1/p' "$CARGO_HOME/config.toml"); do
        for crate in "$dir"/odyssey-cv-*; do [ -e "$crate" ] && stub=$dir; done
      done
      if [ -z "$stub" ]; then echo "withPrivateSource: no vendored odyssey-cv" >&2; exit 1; fi
      private="$NIX_BUILD_TOP/private-vendor"
      cp -rL "$stub" "$private"
      chmod -R u+w "$private"
      ${lib.concatStrings (lib.mapAttrsToList (name: dir: ''
        crate=$(echo "$private"/${name}-*)
        cp -r --no-preserve=mode ${privateSource}/${dir}/. "$crate.real"
        cp "$crate/Cargo.toml" "$crate/.cargo-checksum.json" "$crate.real/"
        rm -rf "$crate"
        mv "$crate.real" "$crate"
      '') privateCrates)}
      sed -i "s|^directory = \"$stub\"$|directory = \"$private\"|" "$CARGO_HOME/config.toml"
    '';
  };
  vendor = workspace: overrides: craneLib.vendorCargoDeps ({
    cargoLock = source + "/${workspace}/Cargo.lock";
    overrideVendorGitCheckout = packages: drv:
      if lib.any (package: lib.hasInfix "github.com/odysseyarm/odyssey-cv.git" package.source) packages
      then
        assert lib.assertMsg
          (!(privateSource ? rev) || lib.all (package: lib.hasSuffix "#${privateSource.rev}" package.source) packages)
          "odyssey-cv flake input must match its Cargo.lock revision";
        drv.overrideAttrs (old: {
          src = privateManifests;
          postPatch = (old.postPatch or "") + ''
            # Upstream eskf names an absent README; Cargo packaging validates it.
            substituteInPlace third_party/eskf/Cargo.toml \
              --replace-fail 'readme = "README.md"' 'readme = false'
            find . -mindepth 2 -name Cargo.toml -execdir sh -c 'mkdir -p src && : > src/lib.rs' \;
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
      postUnpack = lib.optionalString (workspace != ".") ''
        sourceRoot="$sourceRoot/${workspace}"
      '';
      strictDeps = true;
      nativeBuildInputs = [ pkgs.pkg-config pkgs.protobuf ];
      PROTOC = "${pkgs.protobuf}/bin/protoc";
    } // lib.optionalAttrs (workspace == ".") {
      # Cargo artifact archives of the root workspace hold compiled private
      # code; only dependency layers (which force this on) may install one.
      doInstallCargoArtifacts = false;
      # Release binaries carry no symbol table. Profiles are part of every
      # fingerprint, so this applies to the dependency layer too.
      CARGO_PROFILE_RELEASE_STRIP = "symbols";
    };
  # Fails if any vendored copy of the private crates carries real source.
  privateBoundaryCheck = pkgs.runCommand "odyssey-private-source-boundary" { } ''
    found=0
    for crate in ${vendor "." { }}/*/odyssey-cv-* ${vendor "." { }}/*/eskf-*; do
      [ -e "$crate" ] || continue
      found=1
      real=$(find -L "$crate" -name '*.rs' -size +0)
      if [ -n "$real" ]; then echo "vendored stub has sources: $real" >&2; exit 1; fi
    done
    [ "$found" = 1 ] || { echo "no vendored private crates found" >&2; exit 1; }
    touch $out
  '';
in {
  inherit source vendor mkArgs withPrivateSource privateBoundaryCheck;
}
