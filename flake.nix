{
  description = "Compio dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    nixpkgs,
    rust-overlay,
    flake-utils,
    crane,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain (p: rust);
        buildInputs = with pkgs; [
          openssl
          pkg-config
          rust
        ];

        realmkbot = craneLib.buildPackage {
          src = ./.;
          pname = "realmkbot";
          strictDeps = true;
          doCheck = false;
          nativeBuildInputs = buildInputs;
        };

        docker = pkgs.dockerTools.buildLayeredImage {
          name = "realmkbot";
          created = "now";
          tag = "latest";
          config = {
            Cmd = ["${realmkbot}/bin/realmkbot"];
          };
        };
      in
        with pkgs; {
          packages = {
            inherit docker realmkbot;
            default = realmkbot;
          };

          apps.default = flake-utils.lib.mkApp {
            drv = realmkbot;
          };

          devShells.default = mkShell {
            buildInputs = with pkgs;
              [
                infisical
              ]
              ++ buildInputs;
          };
        }
    );
}
