{
  pkgs,
  lib,
  config,
  ...
}:

let
  # gpui desktop build deps (same as proton)
  buildDeps = with pkgs; [
    pkg-config
    fontconfig
    freetype
    libxkbcommon
    libxcb
    # bluer talks to BlueZ over D-Bus
    dbus
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
  # dev daemon, CLI and GUI talk over their own socket, not an installed aurorad's
  env.AURORA_SOCKET = "${config.devenv.runtime}/aurora.sock";

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
