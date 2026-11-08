# Pinned Windows (MSVC CRT + SDK) sysroot for cross-compiling *-pc-windows-msvc
# from Linux/macOS inside the Nix sandbox. Shared by the SDK (nix/sdk.nix) and
# any other crate that cross-builds for Windows.
#
# Nothing here downloads at build time of a Rust derivation:
#  * channelManifest is the Visual Studio 17 channel manifest, fetched by hash
#    (the URL is content-addressed on Microsoft's CDN and the hash is pinned).
#  * sysroot is a fixed-output derivation: `xwin splat` against that manifest.
#    Every payload is sha256-checked against the manifest by xwin, and the
#    whole tree is pinned by outputHash. Bump both together to move the SDK.
#
# Consumers add `tools` to nativeBuildInputs and run `setup` before invoking
# `cargo xwin ...` or `maturin build --target x86_64-pc-windows-msvc`:
# cargo-xwin (and maturin's built-in xwin backend) find the pre-populated
# cache through XWIN_CACHE_DIR and never try to download.
{ pkgs }:
let
  inherit (pkgs) lib;
  version = "17.14.44";
  channelManifest = pkgs.fetchurl {
    url = "https://download.visualstudio.microsoft.com/download/pr/6bd63d7f-1818-4e03-a552-9feb11496c19/8d9e516e27b5de40b0b3c5f042ea975a63c1744999a72125df4fbfc136b4ae9d/VisualStudio.17.Release.chman";
    hash = "sha256-swC1r2sgVrZIYcO7Ayn+M1biTNLEtF6deKmn7BI7HtY=";
  };
  sysroot = pkgs.stdenvNoCC.mkDerivation {
    pname = "xwin-msvc-sysroot";
    inherit version;
    nativeBuildInputs = [ pkgs.xwin ];
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-UFQjsFVBwcF/9e9tVFoG0Z1JySxyTnFqoaRwr/tUWzA=";
    buildCommand = ''
      export HOME=$TMPDIR
      xwin --accept-license --http-retry 5 \
        --manifest ${channelManifest} \
        --cache-dir "$TMPDIR/xwin-cache" \
        --arch x86_64 \
        splat --output "$TMPDIR/splat"
      # splat renames across the build dir; $out may sit on another mount.
      cp -a "$TMPDIR/splat" "$out"
    '';
  };
  # cc-rs drives the MSVC target through clang-cl, and cargo-xwin would
  # otherwise symlink the *wrapped* clang as clang-cl (the wrapper execs
  # `clang`, so the driver never enters cl mode). The unwrapped driver needs the
  # compiler's own builtin headers named explicitly.
  clang = pkgs.llvmPackages.clang-unwrapped;
  clangCl = pkgs.runCommand "clang-cl-for-xwin" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
    makeWrapper ${clang}/bin/clang-cl $out/bin/clang-cl \
      --add-flags "-resource-dir ${clang.lib}/lib/clang/${lib.versions.major clang.version}"
  '';
in rec {
  inherit sysroot channelManifest clangCl;
  target = "x86_64-pc-windows-msvc";
  # llvm provides llvm-lib/llvm-dlltool (cc-rs archives with llvm-lib); lld
  # provides lld-link.
  tools = [ clangCl pkgs.llvmPackages.lld pkgs.llvmPackages.llvm pkgs.cargo-xwin ];
  # Shell snippet: build a writable cargo-xwin cache at `dir` around the
  # read-only sysroot. DONE tells cargo-xwin the requested arch is already
  # splatted, so it never downloads.
  setupAt = dir: ''
    export XWIN_CACHE_DIR="${dir}"
    export XWIN_ARCH=x86_64
    mkdir -p "$XWIN_CACHE_DIR/xwin"
    ln -sfn ${sysroot}/crt "$XWIN_CACHE_DIR/xwin/crt"
    ln -sfn ${sysroot}/sdk "$XWIN_CACHE_DIR/xwin/sdk"
    echo x86_64 > "$XWIN_CACHE_DIR/xwin/DONE"
  '';
  setup = setupAt "$TMPDIR/cargo-xwin";
}
