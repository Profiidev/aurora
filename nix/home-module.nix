self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (pkgs.stdenv.hostPlatform) system;
  cfg = config.programs.aurora;
in
{
  options.programs.aurora = {
    enable = lib.mkEnableOption "Enable Aurora (desktop app, CLI and daemon)";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.aurora;
      description = "The package to use for Aurora.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    systemd.user.services.aurorad = {
      Unit = {
        Description = "Aurora daemon";
        PartOf = [ config.wayland.systemd.target ];
        After = [ config.wayland.systemd.target ];
        X-RestartTriggers = [ "${cfg.package}" ];
      };

      Service = {
        ExecStart = lib.getExe' cfg.package "aurorad";
        Restart = "on-failure";
      };

      Install.WantedBy = [ config.wayland.systemd.target ];
    };
  };
}
