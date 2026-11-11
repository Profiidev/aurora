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

    systemd.target = lib.mkOption {
      type = lib.types.str;
      default = "graphical-session.target";
      description = "The systemd target to start the Aurora daemon on.";
    };

    recommendedServices.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to enable services the daemon needs (bluetooth, notifications).";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.systemPackages = [ cfg.package ];

        systemd.user.services.aurorad = {
          description = "Aurora daemon";
          partOf = [ cfg.systemd.target ];
          after = [ cfg.systemd.target ];
          wantedBy = [ cfg.systemd.target ];
          restartTriggers = [ cfg.package ];

          serviceConfig = {
            ExecStart = lib.getExe' cfg.package "aurorad";
            Restart = "on-failure";
          };
        };
      }

      (lib.mkIf cfg.recommendedServices.enable {
        hardware.bluetooth.enable = lib.mkDefault true;
      })
    ]
  );
}
