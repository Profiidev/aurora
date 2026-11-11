{ pkgs, lib, ... }:

let
  # gpui desktop build deps (same as proton)
  buildDeps = with pkgs; [
    pkg-config
    fontconfig
    freetype
    libxkbcommon
    libxcb
  ];

  # gpui dlopens these
  gpuiLibs = with pkgs; [
    wayland
    vulkan-loader
    libxkbcommon
  ];
in
{
  packages =
    with pkgs;
    [
      cargo-ndk
      just
      # oxfmt: json/md/yaml formatting, same as CI
      oxfmt
      # `just icon`
      imagemagick
    ]
    ++ buildDeps
    ++ gpuiLibs;

  env.LD_LIBRARY_PATH = lib.makeLibraryPath gpuiLibs;

  languages = {
    java.jdk.package = pkgs.jdk17;
  };

  android = {
    enable = true;
    platforms.version = [ "34" ];
    buildTools.version = [ "36.0.0" ];
    ndk.enable = true;
  };
}
