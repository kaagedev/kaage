self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.kaage;
  system = pkgs.stdenv.hostPlatform.system;
  defaultPackage = self.packages.${system}.kaaged;
  defaultUiPackage = self.packages.${system}.kaage-ui;
  effectiveDistDir =
    if cfg.distDir != null then
      toString cfg.distDir
    else
      toString cfg.uiPackage;

  extraBinPath = lib.makeBinPath (
    [
      pkgs.coreutils
      pkgs.systemd
      pkgs.bashInteractive
      pkgs.iproute2
      pkgs.tailscale
      pkgs.bottom
    ]
    ++ cfg.extraPath
  );

  startScript = pkgs.writeShellScript "kaaged-start" ''
    set -euo pipefail
    USER_UID="$(${pkgs.coreutils}/bin/id -u)"
    if [ -z "''${XDG_RUNTIME_DIR:-}" ] && [ -d "/run/user/$USER_UID" ]; then
      export XDG_RUNTIME_DIR="/run/user/$USER_UID"
    fi
    if [ -z "''${DBUS_SESSION_BUS_ADDRESS:-}" ] && [ -S "/run/user/$USER_UID/bus" ]; then
      export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$USER_UID/bus"
    fi
    export PATH="/etc/profiles/per-user/${cfg.user}/bin:/run/current-system/sw/bin:/run/wrappers/bin:${extraBinPath}''${PATH:+:$PATH}"
    exec ${lib.getExe cfg.package}
  '';
in
{
  options.services.kaage = {
    enable = lib.mkEnableOption "kaage BeOS-style service & terminal webtop";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      description = "The kaaged host daemon package.";
    };

    uiPackage = lib.mkOption {
      type = lib.types.package;
      default = defaultUiPackage;
      description = "The kaage-ui WASM static assets package.";
    };

    distDir = lib.mkOption {
      type = lib.types.nullOr (lib.types.either lib.types.path lib.types.str);
      default = null;
      description = ''
        Optional override directory for static UI assets (sets KAAGE_DIST_DIR).
        Useful during local development (e.g. pointing at a local trunk dist/).
        When null, serves the Nix-built uiPackage.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "kaage";
      description = ''
        User account under which kaaged runs (and under which PTY shells spawn).
        On single-user NixOS machines (such as aihole), set this to your main
        username (e.g. my.username) so terminals and user systemd units work
        as your user.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "users";
      description = "Primary group under which kaaged runs.";
    };

    host = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Bind host address for kaaged.";
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
      description = ''
        Expected Tailscale-User-Login header value enforced on /ws and /api/v1/*.
        Leave null only for local testing without Tailscale Serve identity headers.
      '';
    };

    containerCli = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "podman";
      description = "Optional container CLI override (KAAGE_CONTAINER_CLI) for container shell exec.";
    };

    allowSystemdControl = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Install a polkit rule allowing cfg.user to manage systemd system units
        via D-Bus without interactive authentication prompts.
      '';
    };

    extraPath = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      description = "Additional packages to place on kaaged's PATH (available inside spawned PTYs).";
    };

    tailscaleServe = {
      enable = lib.mkEnableOption "Expose kaage over Tailscale HTTPS via tailscale serve";

      httpsPort = lib.mkOption {
        type = lib.types.port;
        default = cfg.port;
        description = ''
          HTTPS port on the tailnet node to expose kaage on via `tailscale serve`.
          Matches the convention in aihole's tailscale-serve-local service (e.g. :9090).
          Set to 443 to serve on the default HTTPS port.
        '';
      };

      openFirewall = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Open tailscaleServe.httpsPort on the tailscale0 firewall interface.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    users.users = lib.mkIf (cfg.user == "kaage") {
      kaage = {
        isSystemUser = true;
        group = cfg.group;
        home = "/var/lib/kaage";
        createHome = true;
        shell = pkgs.bashInteractive;
      };
    };

    users.groups = lib.mkIf (cfg.group == "kaage") {
      kaage = { };
    };

    security.polkit = lib.mkIf cfg.allowSystemdControl {
      enable = true;
      extraConfig = ''
        polkit.addRule(function(action, subject) {
          if (action.id == "org.freedesktop.systemd1.manage-units" &&
              subject.user == "${cfg.user}") {
            return polkit.Result.YES;
          }
        });
      '';
    };

    systemd.services.kaaged = {
      description = "kaage BeOS-style service & terminal webtop daemon";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      environment =
        {
          KAAGE_BIND = "${cfg.host}:${toString cfg.port}";
          KAAGE_DIST_DIR = effectiveDistDir;
          RUST_LOG = "kaaged=info,tower_http=info";
        }
        // lib.optionalAttrs (cfg.allowedUser != null) {
          KAAGE_ALLOWED_USER = cfg.allowedUser;
        }
        // lib.optionalAttrs (cfg.containerCli != null) {
          KAAGE_CONTAINER_CLI = cfg.containerCli;
        };

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        ExecStart = startScript;
        Restart = "on-failure";
        RestartSec = 5;
      };
    };

    systemd.services.tailscale-serve-kaage = lib.mkIf cfg.tailscaleServe.enable {
      description = "Configure Tailscale HTTPS serve for kaage (:${toString cfg.tailscaleServe.httpsPort})";
      after = [
        "network-online.target"
        "tailscaled.service"
        "tailscaled-set.service"
        "kaaged.service"
      ];
      wants = [
        "network-online.target"
        "tailscaled.service"
      ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        Restart = "on-failure";
        RestartSec = "5s";
      };
      script = ''
        for _ in $(seq 1 30); do
          if ${pkgs.tailscale}/bin/tailscale status >/dev/null 2>&1; then
            break
          fi
          sleep 1
        done
        ${pkgs.tailscale}/bin/tailscale serve --bg --https=${toString cfg.tailscaleServe.httpsPort} http://${cfg.host}:${toString cfg.port}
      '';
    };

    networking.firewall.interfaces."tailscale0".allowedTCPPorts =
      lib.mkIf (cfg.tailscaleServe.enable && cfg.tailscaleServe.openFirewall) [
        cfg.tailscaleServe.httpsPort
      ];
  };
}
