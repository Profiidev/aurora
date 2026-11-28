{
  description = "Aurora";

  nixConfig = {
    extra-substituters = [
      "https://projects.cache.profidev.io"
    ];

    extra-trusted-public-keys = [
      "profidev.cachix.org:tg4xEn64UMdvA5jJYT8omo/CQHk8+spLyeGT2YAku70="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    (flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        pkg = pkgs.callPackage ./nix/package.nix { };
      in
      {
        packages = {
          default = pkg;
          aurora = pkg;
          # PR build check: no fat LTO and no optimization, it only proves the package builds
          ci = pkg.overrideAttrs (old: {
            env = (old.env or { }) // {
              CARGO_PROFILE_RELEASE_LTO = "off";
              CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
              CARGO_PROFILE_RELEASE_OPT_LEVEL = "0";
            };
          });
        };
      }
    ))
    // {
      overlays.default = final: _: {
        aurora = final.callPackage ./nix/package.nix { };
      };

      nixosModules.default = import ./nix/nixos-module.nix self;
      homeModules.default = import ./nix/home-module.nix self;
    };
}
