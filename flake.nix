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
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
      in
      {
        packages = rec {
          default = aurora;
          aurora = pkgs.callPackage ./platform/packaging/package.nix { };
        };
      }
    );
}
