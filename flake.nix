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
    treefmt-nix = {
      url = "github:numtide/treefmt-nix/0d6d739a833ba9a9c93034862e4a7b061a062a1c";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    odyssey-cv = {
      url = "git+ssh://git@github.com/odysseyarm/odyssey-cv.git?rev=0726310b937f2dea019923336f80a4e633b91f5a";
      flake = false;
    };
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      rust-overlay,
      crane,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      treefmt = forAllSystems (
        system: inputs.treefmt-nix.lib.evalModule projects.${system}.pkgs ./treefmt.nix
      );
      projects = forAllSystems (
        system:
        import ./nix {
          inherit inputs system;
          src = self;
        }
      );
    in
    {
      devShells = forAllSystems (system: projects.${system}.devShells);
      packages = forAllSystems (system: projects.${system}.packages);
      checks = forAllSystems (
        system:
        projects.${system}.checks
        // {
          format = treefmt.${system}.config.build.check self;
        }
      );
      legacyPackages = forAllSystems (system: {
        inherit (projects.${system}) publicCache;
      });
      formatter = forAllSystems (system: treefmt.${system}.config.build.wrapper);
    };
}
