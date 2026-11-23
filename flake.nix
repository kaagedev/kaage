{
  description = "kaage — BeOS-style service & terminal webtop over Tailscale";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            };
            rustToolchain = pkgs.rust-bin.stable.latest.default.override {
              targets = [ "wasm32-unknown-unknown" ];
            };
            kaagePkgs = import ./nix/packages.nix {
              inherit pkgs rustToolchain;
              lib = pkgs.lib;
            };
          in
          f {
            inherit
              system
              pkgs
              rustToolchain
              kaagePkgs
              ;
          }
        );
    in
    {
      packages = forAllSystems (
        { kaagePkgs, ... }:
        {
          inherit (kaagePkgs)
            kaaged
            kaage-ui
            kaage-ui-native
            kaage
            default
            ;
        }
      );

      devShells = forAllSystems (
        {
          pkgs,
          rustToolchain,
          kaagePkgs,
          ...
        }:
        {
          default = pkgs.mkShell {
            packages = [
              rustToolchain
              kaagePkgs.wasmBindgenCli
              pkgs.binaryen
              pkgs.trunk
              pkgs.zig
              pkgs.pkg-config
              pkgs.rust-analyzer
              pkgs.bottom
            ];
          };
        }
      );

      overlays.default = final: prev:
        let
          system = prev.stdenv.hostPlatform.system;
        in
        {
          inherit (self.packages.${system})
            kaage
            kaaged
            kaage-ui
            kaage-ui-native
            ;
        };

      nixosModules = {
        kaage = import ./nix/nixos-module.nix self;
        default = self.nixosModules.kaage;
      };

      homeManagerModules = {
        kaage = import ./nix/home-manager-module.nix self;
        default = self.homeManagerModules.kaage;
      };
    };
}
