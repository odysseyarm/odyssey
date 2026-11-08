# Same public nixpkgs pin as Odyssey development environment; no external checkout needed.
{ nixpkgsPath ? builtins.fetchTree {
    type = "github";
    owner = "NixOS";
    repo = "nixpkgs";
    rev = "a7868a727837f3c09cee2ce0ca671c76b1589fed";
    narHash = "sha256-KgItSKML8Xvte0B7/uGnBDsYzSnnKHcOaiUbgWBXLXw=";
  }
}:
let
  pkgs = import nixpkgsPath {};
  python = pkgs.python3.withPackages (ps: [ ps.rerun-sdk ]);
in pkgs.mkShell {
  packages = [ pkgs.blender pkgs.rerun python ];
  shellHook = ''export PATH="${pkgs.rerun}/bin:$PATH"'';
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
    pkgs.stdenv.cc.cc.lib pkgs.zlib pkgs.vulkan-loader pkgs.libglvnd
    pkgs.wayland pkgs.libxkbcommon
  ] + ":/run/opengl-driver/lib";
}
