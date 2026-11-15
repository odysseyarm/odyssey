# The repository's formatters. `nix fmt` rewrites files; `nix flake check`
# (checks.<system>.format, run by the Format workflow) fails if any tracked file
# would change. Add a formatter here, never as a separate CI step.
{ pkgs, ... }:
let
  # rustfmt.toml needs unstable options, so rustfmt must be the nightly pinned
  # in rust-toolchain.toml. The module wants a `mainProgram` to find the binary.
  toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
in
{
  projectRootFile = "flake.nix";

  programs = {
    rustfmt = {
      enable = true;
      package = toolchain.overrideAttrs (old: {
        meta = (old.meta or { }) // {
          mainProgram = "rustfmt";
        };
      });
    };
    nixfmt.enable = true;
    ruff-format.enable = true;
    shfmt.enable = true;
  };
}
