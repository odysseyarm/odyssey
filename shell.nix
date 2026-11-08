{ system ? builtins.currentSystem, shell ? "default" }:
(import ./nix/compat.nix).outputs.devShells.${system}.${shell}
