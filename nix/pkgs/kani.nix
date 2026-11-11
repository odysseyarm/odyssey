{ lib, stdenv, fetchurl, autoPatchelfHook, makeWrapper, gcc, toolchain }:
# Kani's release bundle: kani-compiler links against the rustc it was built
# with, so `toolchain` has to be the nightly named in the bundle's
# rust-toolchain-version, with rustc-dev. The driver looks for it at
# <bundle>/toolchain and runs that cargo, never the repository's pin.
stdenv.mkDerivation rec {
  pname = "kani";
  version = "0.68.0";
  src = fetchurl {
    url = "https://github.com/model-checking/kani/releases/download/kani-${version}/kani-${version}-x86_64-unknown-linux-gnu.tar.gz";
    hash = "sha256-MuK0hNc+3gu/ZKLPCHnEJZQiSX2K7E7mfUSN6a54Q9M=";
  };
  nativeBuildInputs = [ autoPatchelfHook makeWrapper ];
  buildInputs = [ stdenv.cc.cc.lib toolchain ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    mkdir -p "$out/libexec" "$out/bin"
    cp -a . "$out/libexec/kani"
    ln -s ${toolchain} "$out/libexec/kani/toolchain"
    # The driver picks cargo or standalone mode from argv[0] and finds cbmc on
    # PATH; goto-cc preprocesses Kani's C stubs with gcc.
    for name in cargo-kani kani; do
      makeWrapper "$out/libexec/kani/bin/kani-driver" "$out/bin/$name" \
        --argv0 "$name" --prefix PATH : "$out/libexec/kani/bin:${gcc}/bin"
    done
  '';
  meta = {
    description = "Kani Rust model checker, from its release bundle";
    homepage = "https://github.com/model-checking/kani";
    license = with lib.licenses; [ asl20 mit ];
    platforms = [ "x86_64-linux" ];
  };
}
