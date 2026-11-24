self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.kaageModule;
  system = pkgs.stdenv.hostPlatform.system;
  defaultPackage = self.packages.${system}.kaaged;
  defaultUiPackage = self.packages.${system}.kaage-ui;
  effectiveDistDir =
    if cfg.distDir != null then
      toString cfg.distDir
    else
      toString cfg.uiPackage;
in
{
  options.kaageModule = {
    enable = lib.mkEnableOption "kaage BeOS-style service & terminal webtop (user service)";

    autoStart = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Start kaaged automatically via a systemd user service on login.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      description = "The kaaged daemon package.";
    };

    uiPackage = lib.mkOption {
      type = lib.types.package;
      default = defaultUiPackage;
      description = "The kaage-ui WASM static assets package.";
    };

    distDir = lib.mkOption {
      type = lib.types.nullOr (lib.types.either lib.types.path lib.types.str);
      default = null;
      description = "Optional local override directory for static UI assets (sets KAAGE_DIST_DIR).";
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Bind address for kaaged (served externally via Tailscale HTTPS).";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 9090;
      description = "TCP port for kaaged.";
    };

    allowedUser = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "pradeep@btbytes.com";
      description = "Expected Tailscale-User-Login header value.";
    };

    containerCli = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "podman";
      description = "Optional container CLI override (KAAGE_CONTAINER_CLI).";
    };

    appDataDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "~/appdata";
      description = "Optional base directory for kaage app data (sets KAAGE_APPDATA; defaults to ~/appdata).";
    };

    installNativeClient = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Also install the native desktop kaage-ui client into home.packages.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages =
      [ cfg.package ]
      ++ lib.optional cfg.installNativeClient self.packages.${system}.kaage-ui-native;

    systemd.user.services.kaaged = lib.mkIf cfg.autoStart {
      Unit = {
        Description = "kaage BeOS-style service & terminal webtop daemon";
        After = [ "network-online.target" ];
        Wants = [ "network-online.target" ];
      };

      Service = {
        Type = "simple";
        Environment =
          [
            "KAAGE_BIND=${cfg.host}:${toString cfg.port}"
            "KAAGE_DIST_DIR=${effectiveDistDir}"
            "RUST_LOG=kaaged=info,tower_http=info"
            "PATH=${config.home.homeDirectory}/.nix-profile/bin:/etc/profiles/per-user/${config.home.username}/bin:/run/current-system/sw/bin:/run/wrappers/bin:${config.home.homeDirectory}/.local/bin"
          ]
          ++ lib.optional (cfg.allowedUser != null) "KAAGE_ALLOWED_USER=${cfg.allowedUser}"
          ++ lib.optional (cfg.containerCli != null) "KAAGE_CONTAINER_CLI=${cfg.containerCli}"
          ++ lib.optional (cfg.appDataDir != null) "KAAGE_APPDATA=${cfg.appDataDir}";
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        RestartSec = 5;
      };

      Install = {
        WantedBy = [ "default.target" ];
      };
    };
  };
}
