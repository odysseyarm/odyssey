{ lib, stdenv, fetchzip, autoPatchelfHook, makeWrapper, toolchain }:
# Verus's release bundle: rust_verify links against the librustc_driver of the
# stable toolchain named in the bundle's version.json, so `toolchain` has to be
# that release. The launcher looks for rust_verify, z3 and vstd beside itself.
stdenv.mkDerivation rec {
  pname = "verus";
  version = "0.2026.11.02.8bf68ee";
  src = fetchzip {
    url = "https://github.com/verus-lang/verus/releases/download/release/${version}/verus-${version}-x86-linux.zip";
    hash = "sha256-m7u6bXGFeis39gpBlD5o3z4Btl1/CpbYuog03tzmKQA=";
  };
  nativeBuildInputs = [ autoPatchelfHook makeWrapper ];
  buildInputs = [ stdenv.cc.cc.lib toolchain ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    mkdir -p "$out/libexec" "$out/bin"
    cp -a . "$out/libexec/verus"
    chmod -R u+w "$out/libexec/verus"
    # Without rustup the launcher runs rust_verify directly, which then needs
    # the toolchain's shared libraries and its rustc on PATH.
    makeWrapper "$out/libexec/verus/verus" "$out/bin/verus" \
      --set VERUS_USE_RUSTUP 0 \
      --prefix PATH : "${toolchain}/bin" \
      --prefix LD_LIBRARY_PATH : "${toolchain}/lib"
  '';
  meta = {
    description = "Verus verifier for Rust, from its release bundle";
    homepage = "https://github.com/verus-lang/verus";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
  };
}
