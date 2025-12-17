{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    rustup = {
      url = "github:yshui/rustup.nix";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rustup, ... } @ inputs:
    let
      load = p: with builtins; let
        specs = fromTOML (readFile "${p}/F.toml");
        locks = fromTOML (readFile "${p}/F.lock");
        locked = mapAttrs (n: v: v // (locks.${n} or {})) specs;
        fetchF = v:
        let
          spec = split ":" v.spec;
          scheme = elemAt spec 0;
          url = if scheme == "github" then
                  "https://github.com/${elemAt spec 2}/archive/${v.rev}.tar.gz"
                else if scheme == "http" || scheme == "https" then
                  v.spec
                else assert false; "";
          fetcher = if v.unpack then builtins.fetchTarball else builtins.fetchurl;
        in
          fetcher {
            inherit url;
            sha256 = v.hash;
          };
        srcs = mapAttrs (n: v: v // { outPath = fetchF v; }) locked;
      in
        srcs;

      srcs = load ./.;
      g = pkgs: let
        rust-toolchain = (pkgs.rustToolchainFromManifestFile srcs.rust-manifest).minimal;
        rustPlatform = (pkgs.makeRustPlatform {
          cargo = rust-toolchain;
          rustc = rust-toolchain;
        });

        inherit (rustPlatform) buildRustPackage bindgenHook;

      in {
        devShell = pkgs.mkShell {
          nativeBuildInputs = [ (rust-toolchain.override { extensions = [ "clippy" ]; }) ];
        };
        packages.default = buildRustPackage {
          name = "ff";
          cargoLock.lockFile = ./Cargo.lock;
          src = ./.;
        };
      };
    in
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system}.extend rustup.overlays.default;
      in (g pkgs)) // {
        overlays.default = final: prev: {
          ff = (g (final.extend rustup.overlays.default)).packages.default;
        };
      }
    ) // {
      lib.load = load;
    };
}
