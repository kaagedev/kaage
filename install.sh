#!/usr/bin/env bash
set -euo pipefail

# kaage (ಕಾಗೆ / 影) — Bare NixOS & Linux Installer
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/kaagedev/kaage/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/kaagedev/kaage/main/install.sh | bash -s -- --mode nixos

MODE="user"
FLAKE_REF="github:kaagedev/kaage"
PORT="9090"
BIND=""
APPDATA_DIR="${HOME}/appdata"
ALLOWED_USER=""
TAILSCALE_SERVE="auto"
WRITE_MODULE=""
UNINSTALL="false"

usage() {
  cat <<'EOF'
kaage installer — install or run kaage on a bare NixOS machine

Usage:
  install.sh [options]

Options:
  --mode <user|nixos|run>     Install mode (default: user)
                                user  : nix profile install + systemd --user service (instant)
                                nixos : print (or write) declarative NixOS flake/overlay module
                                run   : run ephemerally via `nix run` (no permanent changes)
  --flake <ref>               Flake reference (default: github:kaagedev/kaage)
  --port <port>               HTTP port (default: 9090)
  --bind <host:port>          Bind address (default: 127.0.0.1:<port>)
  --appdata <dir>             KAAGE_APPDATA directory (default: ~/appdata)
  --allowed-user <login>      Optional Tailscale-User-Login auth header value
  --tailscale-serve           Configure `tailscale serve` HTTPS proxy (default: auto if running)
  --no-tailscale-serve        Skip `tailscale serve` configuration
  --write-module <path>       In --mode nixos, write the generated module to <path>
  --uninstall                 Stop/disable user systemd service and remove kaage from nix profile
  -h, --help                  Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) MODE="${2:?--mode requires a value}"; shift 2 ;;
    --flake) FLAKE_REF="${2:?--flake requires a value}"; shift 2 ;;
    --port) PORT="${2:?--port requires a value}"; shift 2 ;;
    --bind) BIND="${2:?--bind requires a value}"; shift 2 ;;
    --appdata) APPDATA_DIR="${2:?--appdata requires a value}"; shift 2 ;;
    --allowed-user) ALLOWED_USER="${2:?--allowed-user requires a value}"; shift 2 ;;
    --tailscale-serve) TAILSCALE_SERVE="yes"; shift ;;
    --no-tailscale-serve) TAILSCALE_SERVE="no"; shift ;;
    --write-module) WRITE_MODULE="${2:?--write-module requires a path}"; shift 2 ;;
    --uninstall) UNINSTALL="true"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ -z "${BIND}" ]]; then
  BIND="127.0.0.1:${PORT}"
fi

# Expand leading ~/ in APPDATA_DIR
APPDATA_DIR="${APPDATA_DIR/#\~/${HOME}}"

NIX_FLAGS=(--extra-experimental-features "nix-command flakes")

require_nix() {
  if ! command -v nix >/dev/null 2>&1; then
    echo "Error: 'nix' was not found on PATH." >&2
    echo "Install NixOS or Nix first: https://nixos.org/download" >&2
    exit 1
  fi
}

do_uninstall() {
  echo "==> Uninstalling kaage user service..."
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now kaaged.service 2>/dev/null || true
    rm -f "${HOME}/.config/systemd/user/kaaged.service"
    systemctl --user daemon-reload 2>/dev/null || true
  fi
  if command -v nix >/dev/null 2>&1; then
    nix "${NIX_FLAGS[@]}" profile remove '.*kaage.*' 2>/dev/null || true
  fi
  echo "==> Done. Your app data in ${APPDATA_DIR} was preserved."
}

if [[ "${UNINSTALL}" == "true" ]]; then
  do_uninstall
  exit 0
fi

case "${MODE}" in
  run)
    require_nix
    mkdir -p "${APPDATA_DIR}"
    echo "==> Running kaage ephemerally from ${FLAKE_REF}#kaage on http://${BIND}"
    echo "    KAAGE_APPDATA=${APPDATA_DIR}"
    export KAAGE_BIND="${BIND}"
    export KAAGE_APPDATA="${APPDATA_DIR}"
    if [[ -n "${ALLOWED_USER}" ]]; then
      export KAAGE_ALLOWED_USER="${ALLOWED_USER}"
    fi
    exec nix "${NIX_FLAGS[@]}" run "${FLAKE_REF}#kaage"
    ;;

  nixos)
    USER_NAME="${USER:-$(id -un)}"
    ALLOWED_LINE=""
    if [[ -n "${ALLOWED_USER}" ]]; then
      ALLOWED_LINE="    allowedUser = \"${ALLOWED_USER}\";"
    else
      ALLOWED_LINE="    # allowedUser = \"you@example.com\"; # uncomment to enforce Tailscale-User-Login"
    fi
    SNIPPET=$(cat <<EOF
# 1) Add kaage to your flake.nix inputs:
#   inputs.kaage = {
#     url = "${FLAKE_REF}";
#     inputs.nixpkgs.follows = "nixpkgs";
#   };
#
# 2) Add the overlay and NixOS module in your nixosSystem modules:
#   { nixpkgs.overlays = [ inputs.kaage.overlays.default ]; }
#   inputs.kaage.nixosModules.default
#
# 3) Enable services.kaage in configuration.nix (or import this file):
{ pkgs, ... }:
{
  services.kaage = {
    enable = true;
    package = pkgs.kaage;
    user = "${USER_NAME}";
    port = ${PORT};
    appDataDir = "${APPDATA_DIR}";
${ALLOWED_LINE}
    tailscaleServe = {
      enable = true;
      httpsPort = ${PORT};
    };
  };
}
EOF
)
    if [[ -n "${WRITE_MODULE}" ]]; then
      printf '%s\n' "${SNIPPET}" > "${WRITE_MODULE}"
      echo "==> Wrote NixOS module snippet to ${WRITE_MODULE}"
    else
      printf '%s\n' "${SNIPPET}"
    fi
    ;;

  user)
    require_nix
    echo "==> Installing kaage from ${FLAKE_REF}#kaage..."
    nix "${NIX_FLAGS[@]}" profile remove '.*kaage.*' 2>/dev/null || true
    nix "${NIX_FLAGS[@]}" profile install "${FLAKE_REF}#kaage"

    KAAGED_BIN="$(command -v kaaged || echo "${HOME}/.nix-profile/bin/kaaged")"
    mkdir -p "${APPDATA_DIR}"
    mkdir -p "${HOME}/.config/systemd/user"

    UNIT_FILE="${HOME}/.config/systemd/user/kaaged.service"
    {
      echo "[Unit]"
      echo "Description=kaage Mission-Control Webtop Daemon"
      echo "After=network-online.target"
      echo "Wants=network-online.target"
      echo ""
      echo "[Service]"
      echo "Type=simple"
      echo "Environment=KAAGE_BIND=${BIND}"
      echo "Environment=KAAGE_APPDATA=${APPDATA_DIR}"
      echo "Environment=RUST_LOG=kaaged=info,tower_http=info"
      if [[ -n "${ALLOWED_USER}" ]]; then
        echo "Environment=KAAGE_ALLOWED_USER=${ALLOWED_USER}"
      fi
      echo "ExecStart=${KAAGED_BIN}"
      echo "Restart=on-failure"
      echo "RestartSec=5"
      echo ""
      echo "[Install]"
      echo "WantedBy=default.target"
    } > "${UNIT_FILE}"

    if command -v systemctl >/dev/null 2>&1; then
      systemctl --user daemon-reload
      systemctl --user enable --now kaaged.service
      echo "==> Started systemd user service: kaaged.service"
    else
      echo "==> systemctl not found; start manually with: KAAGE_BIND=${BIND} KAAGE_APPDATA=${APPDATA_DIR} ${KAAGED_BIN}"
    fi

    if [[ "${TAILSCALE_SERVE}" != "no" ]] && command -v tailscale >/dev/null 2>&1; then
      if tailscale status >/dev/null 2>&1; then
        echo "==> Exposing kaage via Tailscale Serve on HTTPS :${PORT}..."
        if tailscale serve --bg --https="${PORT}" "http://${BIND}" >/dev/null 2>&1; then
          echo "==> Tailscale Serve configured for https://<your-node>.ts.net:${PORT}"
        else
          echo "    (Skipping tailscale serve — run 'sudo tailscale serve --bg --https=${PORT} http://${BIND}' if desired)"
        fi
      fi
    fi

    echo ""
    echo "kaage is installed and running!"
    echo "  Local URL     : http://${BIND}"
    echo "  KAAGE_APPDATA : ${APPDATA_DIR}"
    echo "  Service logs  : journalctl --user -u kaaged.service -f"
    ;;

  *)
    echo "Invalid --mode '${MODE}'. Expected: user, nixos, or run." >&2
    exit 1
    ;;
esac
