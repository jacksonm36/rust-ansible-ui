#!/usr/bin/env bash
# Install build deps, build a single static-ish ansible-server binary (UI embedded),
# install under /usr/local/bin, create ansible-ui user + data dir, enable systemd.
# Optionally install nginx or lighttpd as reverse proxy on :80 -> 127.0.0.1:14300 (LAN access).
#
# Usage (as root):
#   curl -fsSL ... | sudo bash
#   OR: sudo bash scripts/install-linux.sh
#
# Optional env:
#   SKIP_BUILD=1              — skip cargo build (use existing target/release/ansible-server in repo)
#   REPO_DIR=/path/to/repo    — git clone root (default: script dir parent)
#   INSTALL_WEB_PROXY=nginx   — nginx (default) | lighttpd | none
#   OPEN_FIREWALL_HTTP=1      — try to open port 80 in firewalld/ufw (default: 1 when proxy installed)

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { echo -e "${GREEN}[install]${NC} $*"; }
warn()  { echo -e "${YELLOW}[install]${NC} $*"; }
die()   { echo -e "${RED}[install] ERROR:${NC} $*" >&2; exit 1; }

[[ "$(id -u)" -eq 0 ]] || die "Run as root (sudo)."

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="${REPO_DIR:-$(cd "$SCRIPT_DIR/.." && pwd)}"
SERVICE_SRC="$REPO_DIR/deploy/ansible-ui.service"
NGINX_CONF_SRC="$REPO_DIR/deploy/nginx/ansible-ui.conf"
LIGHTTPD_CONF_SRC="$REPO_DIR/deploy/lighttpd/90-ansible-ui.conf"
BIN_NAME="ansible-server"
INSTALL_BIN="/usr/local/bin/$BIN_NAME"
DATA_DIR="/var/lib/ansible-ui"
RUN_USER="ansible-ui"

INSTALL_WEB_PROXY="${INSTALL_WEB_PROXY:-nginx}"
INSTALL_WEB_PROXY="${INSTALL_WEB_PROXY,,}"
OPEN_FIREWALL_HTTP="${OPEN_FIREWALL_HTTP:-1}"

info "Repository directory: $REPO_DIR"
info "INSTALL_WEB_PROXY=$INSTALL_WEB_PROXY"

# --- Detect package manager and install OS dependencies ---
if command -v apt-get >/dev/null 2>&1; then
  info "Installing packages (apt)…"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq build-essential pkg-config libssl-dev curl git ansible-core || \
    apt-get install -y -qq build-essential pkg-config libssl-dev curl git ansible || true
elif command -v dnf >/dev/null 2>&1; then
  info "Installing packages (dnf)…"
  dnf install -y gcc gcc-c++ make pkg-config openssl-devel curl git ansible || true
elif command -v pacman >/dev/null 2>&1; then
  info "Installing packages (pacman)…"
  pacman -Sy --noconfirm base-devel pkgconf openssl curl git ansible || true
else
  warn "Unknown package manager; install manually: gcc, make, pkg-config, openssl dev, curl, git, ansible"
fi

# --- Rust toolchain ---
if ! command -v cargo >/dev/null 2>&1; then
  info "Installing Rust (rustup)…"
  export RUSTUP_INIT_SKIP_PATH_CHECK=yes
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

command -v cargo >/dev/null 2>&1 || die "cargo not found after rustup install"

# --- Build single binary with embedded UI ---
TAURI_DIR="$REPO_DIR/src-tauri"
[[ -d "$TAURI_DIR" ]] || die "Missing $TAURI_DIR (wrong REPO_DIR?)"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  info "Building $BIN_NAME (release, embedded static)…"
  ( cd "$TAURI_DIR" && cargo build --release \
      --bin "$BIN_NAME" \
      --no-default-features \
      --features "server-only,embedded-static" )
else
  warn "SKIP_BUILD=1 — not running cargo"
fi

BUILT="$TAURI_DIR/target/release/$BIN_NAME"
[[ -x "$BUILT" ]] || die "Binary not found: $BUILT"

install -m 0755 "$BUILT" "$INSTALL_BIN"
info "Installed $INSTALL_BIN"

# --- Service user and data directory ---
if ! id "$RUN_USER" &>/dev/null; then
  useradd --system --home-dir "$DATA_DIR" --shell /usr/sbin/nologin "$RUN_USER" || true
fi
mkdir -p "$DATA_DIR"
chown -R "$RUN_USER:$RUN_USER" "$DATA_DIR"

# --- systemd unit ---
[[ -f "$SERVICE_SRC" ]] || die "Missing unit file: $SERVICE_SRC"
install -m 0644 "$SERVICE_SRC" /etc/systemd/system/ansible-ui.service

apply_proxy_dropin() {
  mkdir -p /etc/systemd/system/ansible-ui.service.d
  cat >/etc/systemd/system/ansible-ui.service.d/50-proxy.conf <<'EOF'
# Managed by install-linux.sh — reverse proxy on port 80
[Service]
Environment=ANSIBLE_UI_BIND=127.0.0.1:14300
# Allow browsers on other PCs to call the API (use firewall; do not expose to the public Internet)
Environment=ANSIBLE_UI_RELAX_CORS=1
EOF
  info "Wrote ansible-ui.service.d/50-proxy.conf (backend 127.0.0.1:14300 + relaxed CORS)."
}

remove_proxy_dropin() {
  rm -f /etc/systemd/system/ansible-ui.service.d/50-proxy.conf
}

selinux_allow_nginx_proxy() {
  if command -v getenforce >/dev/null 2>&1 && [[ "$(getenforce 2>/dev/null)" == "Enforcing" ]]; then
    if command -v setsebool >/dev/null 2>&1; then
      setsebool -P httpd_can_network_connect 1 2>/dev/null && info "SELinux: set httpd_can_network_connect=1" || true
    fi
  fi
}

open_firewall_http() {
  [[ "${OPEN_FIREWALL_HTTP}" == "1" ]] || return 0
  if command -v firewall-cmd >/dev/null 2>&1 && systemctl is-active --quiet firewalld 2>/dev/null; then
    firewall-cmd --permanent --add-service=http 2>/dev/null && firewall-cmd --reload 2>/dev/null && \
      info "firewalld: opened http (port 80)." || warn "firewalld: could not open http."
  fi
  if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi "Status: active"; then
    ufw allow 80/tcp comment 'ansible-ui' 2>/dev/null || true
    info "ufw: allowed 80/tcp (run 'ufw reload' if needed)."
  fi
}

install_nginx_proxy() {
  [[ -f "$NGINX_CONF_SRC" ]] || die "Missing $NGINX_CONF_SRC"
  if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nginx
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y nginx
  elif command -v pacman >/dev/null 2>&1; then
    pacman -Sy --noconfirm nginx
  else
    die "nginx: install nginx manually, then copy deploy/nginx/ansible-ui.conf to your nginx conf.d/"
  fi

  install -m 0644 "$NGINX_CONF_SRC" /etc/nginx/conf.d/ansible-ui.conf

  # Debian/Ubuntu default site often owns :80
  if [[ -e /etc/nginx/sites-enabled/default ]]; then
    rm -f /etc/nginx/sites-enabled/default
    info "Removed /etc/nginx/sites-enabled/default so ansible-ui can use :80."
  fi

  selinux_allow_nginx_proxy
  nginx -t
  systemctl enable nginx.service
  systemctl restart nginx.service
  info "nginx reverse proxy: http://<this-server>/ -> 127.0.0.1:14300"
}

install_lighttpd_proxy() {
  [[ -f "$LIGHTTPD_CONF_SRC" ]] || die "Missing $LIGHTTPD_CONF_SRC"

  if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq lighttpd
    lighttpd-enable-mod proxy 2>/dev/null || true
    install -m 0644 "$LIGHTTPD_CONF_SRC" /etc/lighttpd/conf-available/90-ansible-ui.conf
    ln -sf ../conf-available/90-ansible-ui.conf /etc/lighttpd/conf-enabled/99-ansible-ui.conf
    rm -f /etc/lighttpd/conf-enabled/90-debian-doc.conf 2>/dev/null || true
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y lighttpd
    # Fedora: module config directory
    if [[ -d /etc/lighttpd/conf.d ]]; then
      install -m 0644 "$LIGHTTPD_CONF_SRC" /etc/lighttpd/conf.d/99-ansible-ui.conf
      # Ensure mod_proxy is loaded (Fedora may use different layout)
      if ! grep -q 'server.modules.*mod_proxy' /etc/lighttpd/lighttpd.conf 2>/dev/null; then
        warn "Add mod_proxy to lighttpd (see /etc/lighttpd/lighttpd.conf) if lighttpd -t fails."
      fi
    else
      die "lighttpd on this distro: configure mod_proxy manually (see deploy/lighttpd/)."
    fi
  else
    die "lighttpd auto-setup supports apt (Debian/Ubuntu) or dnf with /etc/lighttpd/conf.d. Install manually otherwise."
  fi

  lighttpd -t -f /etc/lighttpd/lighttpd.conf 2>/dev/null || lighttpd -t 2>/dev/null || true
  systemctl enable lighttpd.service
  systemctl restart lighttpd.service
  info "lighttpd reverse proxy: http://<this-server>/ -> 127.0.0.1:14300"
}

case "$INSTALL_WEB_PROXY" in
  nginx)
    apply_proxy_dropin
    install_nginx_proxy
    open_firewall_http
    ;;
  lighttpd)
    apply_proxy_dropin
    install_lighttpd_proxy
    open_firewall_http
    ;;
  none)
    remove_proxy_dropin 2>/dev/null || true
    info "Skipping web proxy (INSTALL_WEB_PROXY=none). ansible-ui listens per unit file (0.0.0.0:14300)."
    ;;
  *)
    warn "Unknown INSTALL_WEB_PROXY=$INSTALL_WEB_PROXY — expected nginx, lighttpd, or none. Skipping proxy."
    ;;
esac

systemctl daemon-reload
systemctl enable ansible-ui.service
systemctl restart ansible-ui.service || systemctl start ansible-ui.service

info "Done."
echo ""
echo "  Service: systemctl status ansible-ui"
echo "  Logs:    journalctl -u ansible-ui -f"
PRIMARY_IP="$(hostname -I 2>/dev/null | awk '{print $1}' || true)"
if [[ "$INSTALL_WEB_PROXY" == "nginx" || "$INSTALL_WEB_PROXY" == "lighttpd" ]]; then
  echo "  UI (LAN):  http://${PRIMARY_IP:-this-host}/   (port 80 via ${INSTALL_WEB_PROXY})"
  echo "  Backend:   http://127.0.0.1:14300 (not reachable from other PCs)"
else
  echo "  UI (LAN):  http://${PRIMARY_IP:-this-host}:14300"
fi
echo ""
warn "Use a firewall. ANSIBLE_UI_RELAX_CORS=1 is enabled with the proxy — do not expose port 80 to the Internet without TLS + auth."
warn "Set ANSIBLE_UI_SECRET_KEY via systemctl edit ansible-ui if you do not use the auto keyfile."
