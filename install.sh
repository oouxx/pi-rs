#!/usr/bin/env bash
#
# pi-rs installer — 自动探测操作系统/架构，从 GitHub Releases 安装、更新、卸载。
#
# 用法:
#   ./install.sh install     # 安装（默认）
#   ./install.sh update      # 更新到最新版
#   ./install.sh uninstall   # 卸载
#   ./install.sh --version   # 显示版本
#
# 环境变量:
#   PI_RS_VERSION      要安装的版本标签（默认 latest）
#   PI_RS_INSTALL_DIR  安装目录（默认 ~/.local/bin）

set -euo pipefail

REPO="oouxx/pi-rs"
VERSION="${PI_RS_VERSION:-latest}"
INSTALL_DIR="${PI_RS_INSTALL_DIR:-$HOME/.local/bin}"

# ── 探测 ────────────────────────────────────────────────────────────────

detect_os() {
  case "$(uname -s)" in
    Linux*) echo "linux" ;;
    Darwin*) echo "macos" ;;
    MINGW* | MSYS* | CYGWIN*) echo "windows" ;;
    *) echo "unsupported" ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) echo "x86_64" ;;
    aarch64 | arm64) echo "aarch64" ;;
    *) echo "unsupported" ;;
  esac
}

# ── 下载 ────────────────────────────────────────────────────────────────

download_url() {
  local os="$1" arch="$2"
  if [ "$VERSION" = "latest" ]; then
    echo "https://github.com/$REPO/releases/latest/download/pi-rs-$os-$arch"
  else
    echo "https://github.com/$REPO/releases/download/$VERSION/pi-rs-$os-$arch"
  fi
}

download() {
  local url="$1" dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$dest" "$url"
  else
    echo "Error: neither curl nor wget found" >&2
    exit 1
  fi
}

# ── 安装 / 更新 ────────────────────────────────────────────────────────

install() {
  local os arch url dest
  os="$(detect_os)"
  arch="$(detect_arch)"

  if [ "$os" = "unsupported" ]; then
    echo "Error: unsupported OS ($(uname -s))" >&2
    exit 1
  fi
  if [ "$arch" = "unsupported" ]; then
    echo "Error: unsupported architecture ($(uname -m))" >&2
    exit 1
  fi

  url="$(download_url "$os" "$arch")"
  mkdir -p "$INSTALL_DIR"

  if [ "$os" = "windows" ]; then
    dest="$INSTALL_DIR/pi-rs.exe"
  else
    dest="$INSTALL_DIR/pi-rs"
  fi

  echo "Detected: $os/$arch"
  echo "Downloading $url"
  download "$url" "$dest.tmp"
  chmod +x "$dest.tmp"
  mv -f "$dest.tmp" "$dest"

  echo
  echo "Installed pi-rs to $dest"
  echo "Run 'pi-rs --version' to verify."
  if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo
    echo "Note: $INSTALL_DIR is not on your PATH. Add it with:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
  fi
}

# ── 卸载 ────────────────────────────────────────────────────────────────

uninstall() {
  local dest
  if [ "$(detect_os)" = "windows" ]; then
    dest="$INSTALL_DIR/pi-rs.exe"
  else
    dest="$INSTALL_DIR/pi-rs"
  fi

  if [ -f "$dest" ]; then
    rm -f "$dest"
    echo "Removed $dest"
  else
    echo "pi-rs not found at $dest (nothing to uninstall)"
  fi
}

# ── 入口 ────────────────────────────────────────────────────────────────

usage() {
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
}

case "${1:-install}" in
  install | update)
    install
    ;;
  uninstall)
    uninstall
    ;;
  --version | -v)
    echo "pi-rs installer (repo: $REPO, version: $VERSION)"
    ;;
  --help | -h | help)
    usage
    ;;
  *)
    echo "Error: unknown command '$1'" >&2
    usage >&2
    exit 1
    ;;
esac
