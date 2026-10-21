{
  description = "vcs-objectfs, a FUSE mount making VCS object files immutable";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      # The crate links no FUSE library: `fuser` uses its pure-Rust mount and
      # execs `fusermount3` at runtime, so only Linux is packaged.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      perSystem =
        { pkgs, ... }:
        let
          vcs-objectfs = pkgs.callPackage ./nix/package.nix { };
        in
        {
          packages = {
            inherit vcs-objectfs;
            default = vcs-objectfs;
          };

          devShells.default = pkgs.mkShell {
            inputsFrom = [ vcs-objectfs ];
            packages = [
              pkgs.clippy
              pkgs.rustfmt
              pkgs.fuse3
            ];
          };

          formatter = pkgs.nixfmt;
        };
    };
}
