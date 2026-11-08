{ system ? builtins.currentSystem }:
(import ./nix/compat.nix).outputs.packages.${system}
