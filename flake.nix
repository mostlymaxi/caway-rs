{
  description = "caway: low-resource MPRIS now-playing waybar module with a cava equalizer";

  # Only nixpkgs, so consumers can `inputs.nixpkgs.follows = "nixpkgs"`.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        caway = final.callPackage ./package.nix { };
      };

      packages = forAllSystems (pkgs: rec {
        caway = pkgs.callPackage ./package.nix { };
        default = caway;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.caway ];
          packages = with pkgs; [
            rustfmt
            clippy
            pre-commit
            cava
          ];
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
