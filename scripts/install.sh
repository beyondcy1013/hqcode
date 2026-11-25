#!/usr/bin/env bash
set -euo pipefail

REPO="${HQCODE_REPO:-beyondcy1013/hqcode}"
INSTALL_DIR="${HQCODE_INSTALL_DIR:-$HOME/.local/bin}"
DATA_DIR="${HQCODE_DATA_DIR:-$HOME/.hqcode}"
VERSION=""
LOCAL_ARCHIVE=""
NO_START=0

usage() {
  cat <<'EOF'
HQCode Installer

Usage:
  install.sh [options]

Options:
  --version <tag>          Install a specific release tag (e.g. v0.1.0)
  --local-archive <path>   Install offline from a local release .tar.gz archive
  --install-dir <dir>      Target binary directory (default: ~/.local/bin)
  --data-dir <dir>         HQCode data directory (default: ~/.hqcode)
  --no-start               Install binary without printing/running startup
  -h, --help               Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --local-archive)
      LOCAL_ARCHIVE="${2:-}"
      shift 2
      ;;
    --install-dir)
      INSTALL_DIR="${2:-}"
      shift 2
      ;;
    --data-dir)
      DATA_DIR="${2:-}"
      shift 2
      ;;
    --no-start)
      NO_START=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

detect_platform() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os" in
    linux) os="linux" ;;
    darwin) os="macos" ;;
    *)
      echo "Unsupported OS: $os" >&2
      exit 1
      ;;
  esac
  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
      echo "Unsupported architecture: $arch" >&2
      exit 1
      ;;
  esac
  echo "${os}-${arch}"
}

sha256_verify() {
  local file="$1"
  local expected="$2"
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    echo "Warning: sha256sum/shasum not found; skipping checksum check." >&2
    return 0
  fi
  if [[ "$actual" != "$expected" ]]; then
    echo "Checksum mismatch for $file (expected $expected, got $actual)" >&2
    exit 1
  fi
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE_PATH=""
if [[ -n "$LOCAL_ARCHIVE" ]]; then
  if [[ ! -f "$LOCAL_ARCHIVE" ]]; then
    echo "Local archive not found: $LOCAL_ARCHIVE" >&2
    exit 1
  fi
  ARCHIVE_PATH="$LOCAL_ARCHIVE"
  if [[ -f "${LOCAL_ARCHIVE}.sha256" ]]; then
    EXPECTED_SHA="$(awk '{print $1}' "${LOCAL_ARCHIVE}.sha256")"
    sha256_verify "$ARCHIVE_PATH" "$EXPECTED_SHA"
  fi
else
  PLATFORM="$(detect_platform)"
  if [[ -z "$VERSION" ]]; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
    if [[ -z "$VERSION" ]]; then
      echo "Failed to resolve latest release tag for ${REPO}." >&2
      exit 1
    fi
  fi
  ASSET_NAME="hqcode-${VERSION}-${PLATFORM}.tar.gz"
  BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
  ARCHIVE_PATH="${TMP_DIR}/${ASSET_NAME}"
  echo "Downloading ${BASE_URL}/${ASSET_NAME}..."
  curl -fsSL "${BASE_URL}/${ASSET_NAME}" -o "$ARCHIVE_PATH"
  if curl -fsSL "${BASE_URL}/${ASSET_NAME}.sha256" -o "${ARCHIVE_PATH}.sha256" 2>/dev/null; then
    EXPECTED_SHA="$(awk '{print $1}' "${ARCHIVE_PATH}.sha256")"
    sha256_verify "$ARCHIVE_PATH" "$EXPECTED_SHA"
  fi
fi

mkdir -p "${TMP_DIR}/extract"
tar -xzf "$ARCHIVE_PATH" -C "${TMP_DIR}/extract"

BIN_SRC="$(find "${TMP_DIR}/extract" -type f -name "hqcode" | head -n1)"
if [[ -z "$BIN_SRC" ]]; then
  echo "hqcode binary not found inside archive." >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR" "$DATA_DIR"
install -m 0755 "$BIN_SRC" "${INSTALL_DIR}/hqcode"
echo "Installed HQCode to ${INSTALL_DIR}/hqcode"

if ! command -v tmux >/dev/null 2>&1; then
  echo "Note: 'tmux' was not found in PATH. Install tmux before starting terminal sessions." >&2
fi

if [[ "$NO_START" -eq 0 ]]; then
  echo "Start HQCode with:"
  echo "  ${INSTALL_DIR}/hqcode serve --addr 127.0.0.1:9900 --workspace \"\$(pwd)\""
fi
