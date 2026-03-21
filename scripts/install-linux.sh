#!/usr/bin/env bash
# Install build deps, build a single static-ish ansible-server binary (UI embedded),
# install under /usr/local/bin, create ansible-ui user + data dir, enable systemd.
#
# Usage (as root):
#   curl -fsSL ... | sudo bash
#   OR: sudo bash scripts/install-linux.sh
#
# Optional env:
#   SKIP_BUILD=1          — skip cargo build (use existing target/release/ansible-server in repo)
#   REPO_DIR=/path/to/tauri_ansible_rust — where the git clone lives (default: script dir parent)

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { echo -e "${GREEN}[install]${NC} $*"; }
warn()  { echo -e "${YELLOW}[install]${NC} $*"; }
die()   { echo -e "${RED}[install] ERROR:${NC} $*" >&2; exit 1; }

[[ "$(id -u)" -eq 0 ]] || die "Run as root (sudo)."

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="${REPO_DIR:-$(cd "$SCRIPT_DIR/.." && pwd)}"
SERVICE_SRC="$REPO_DIR/deploy/ansible-ui.service"
BIN_NAME="ansible-server"
INSTALL_BIN="/usr/local/bin/$BIN_NAME"
DATA_DIR="/var/lib/ansible-ui"
RUN_USER="ansible-ui"

info "Repository directory: $REPO_DIR"

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

# --- systemd ---
[[ -f "$SERVICE_SRC" ]] || die "Missing unit file: $SERVICE_SRC"
install -m 0644 "$SERVICE_SRC" /etc/systemd/system/ansible-ui.service
systemctl daemon-reload
systemctl enable ansible-ui.service
systemctl restart ansible-ui.service || systemctl start ansible-ui.service

info "Done."
echo ""
echo "  Service: systemctl status ansible-ui"
echo "  Logs:    journalctl -u ansible-ui -f"
echo "  URL:     http://$(hostname -I 2>/dev/null | awk '{print $1}' || echo THIS_HOST):14300"
echo ""
warn "Set ANSIBLE_UI_SECRET_KEY in /etc/systemd/system/ansible-ui.service.d/override.conf (32+ chars) for production."
