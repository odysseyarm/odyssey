{ lib, stdenv, fetchurl, autoPatchelfHook, libglvnd, pin }:
stdenv.mkDerivation {
  pname = "orbbec-sdk";
  version = pin.version;
  src = fetchurl {
    url = pin.linux_x64.url;
    sha256 = pin.linux_x64.sha256;
  };
  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [ stdenv.cc.cc.lib libglvnd ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    mkdir -p "$out/lib" "$out/share/orbbec-sdk"
    cp -a lib/libOrbbecSDK.so* lib/extensions "$out/lib/"
    cp shared/OrbbecSDKConfig.xml "$out/lib/"
    install -Dm644 shared/99-obsensor-libusb.rules "$out/lib/udev/rules.d/99-obsensor-libusb.rules"
    cp -r licenses LICENSE.txt "$out/share/orbbec-sdk/"
  '';
  meta = {
    description = "Orbbec camera SDK with adjacent extensions for ODYSSEY_ORBBEC_SDK_DIR";
    homepage = "https://github.com/orbbec/OrbbecSDK_v2";
    license = lib.licenses.unfree;
    platforms = [ "x86_64-linux" ];
  };
}
