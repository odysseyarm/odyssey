{
  description = "Odyssey development environments and Cargo builds";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/a7868a727837f3c09cee2ce0ca671c76b1589fed";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/42076d1c6a0fb405ed04f9a674e4a8f69de62ba3";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane/79fd63528501446749c09f08cad486a58c7d173e";
    flake-compat = {
      url = "github:NixOS/flake-compat/5edf11c44bc78a0d334f6334cdaf7d60d732daab";
      flake = false;
    };
    odyssey-cv = {
      url = "git+ssh://git@github.com/odysseyarm/odyssey-cv.git?rev=9a60fa4b19c24f13c461d6ea65ac734acdca82ca";
      flake = false;
    };
  };

  outputs = inputs@{ self, nixpkgs, rust-overlay, crane, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      projects = forAllSystems (system: import ./nix {
        inherit inputs system;
        src = self;
      });
    in {
      devShells = forAllSystems (system: projects.${system}.devShells);
      packages = forAllSystems (system: projects.${system}.packages);
      checks = forAllSystems (system: projects.${system}.checks);
      formatter = forAllSystems (system: projects.${system}.pkgs.nixfmt);
    };
}
