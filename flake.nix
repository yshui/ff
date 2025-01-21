{
  inputs = {
    nixpkgs.url = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:nix-community/fenix";
    };
    rust-manifest = {
      flake = false;
      url = "https://static.rust-lang.org/dist/2025-01-08/channel-rust-nightly.toml";
    };
  };

  outputs = { self, nixpkgs, flake-utils, fenix, ... } @ inputs:
    let
      g = pkgs: let
        rust-toolchain = pkgs.fenix.fromManifestFile inputs.rust-manifest;
        rust = pkgs.fenix.combine (with rust-toolchain; [
          rustc cargo rust-src rustfmt clippy
        ]);
        rustPlatform = (pkgs.makeRustPlatform {
          cargo = rust-toolchain.cargo;
          rustc = rust-toolchain.rustc;
        });

        inherit (rustPlatform) buildRustPackage bindgenHook;

      in {
        devShell = pkgs.mkShell {
          nativeBuildInputs = [ rust ];
        };
        packages.default = buildRustPackage {
          name = "ff";
          cargoLock.lockFile = ./Cargo.lock;
          src = ./.;
        };
      };
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system}.extend fenix.overlays.default;
      in (g pkgs)) // {
      overlays.default = final: prev: {
        ff = (g (final.extend fenix.overlays.default)).packages.default;
      };
    };
}
