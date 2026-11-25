#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

SKIP_BUILD=0
TARGET_TRIPLE="${HQCODE_TARGET:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --target)
      TARGET_TRIPLE="${2:-}"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# Sanitization gate: ensure public package contains no private skills or internal hostnames.
if find "$ROOT_DIR" -maxdepth 4 \( -name "SKILL.md" -o -path "*/.codex/skills*" \) | grep -q .; then
  echo "Sanitization gate failed: found SKILL.md or .codex/skills inside repository!" >&2
  exit 1
fi
if grep -rn "fpsq\.xyz" "$ROOT_DIR/src" "$ROOT_DIR/crates" "$ROOT_DIR/static" "$ROOT_DIR/scripts" 2>/dev/null; then
  echo "Sanitization gate failed: found private hostname reference!" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n1)"
if [[ -z "$VERSION" ]]; then
  echo "Failed to read package version from Cargo.toml" >&2
  exit 1
fi

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$OS" in
  linux) OS="linux" ;;
  darwin) OS="macos" ;;
esac
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
esac

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  if [[ -n "$TARGET_TRIPLE" ]]; then
    cargo build --release --target "$TARGET_TRIPLE"
  else
    cargo build --release
  fi
fi

if [[ -n "$TARGET_TRIPLE" ]]; then
  BIN_PATH="$ROOT_DIR/target/${TARGET_TRIPLE}/release/hqcode"
else
  BIN_PATH="$ROOT_DIR/target/release/hqcode"
fi
if [[ ! -f "$BIN_PATH" ]]; then
  echo "Release binary missing: $BIN_PATH" >&2
  exit 1
fi

STAGE_NAME="hqcode-v${VERSION}-${OS}-${ARCH}"
STAGE_DIR="$ROOT_DIR/dist/${STAGE_NAME}"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"
cp "$BIN_PATH" "$STAGE_DIR/hqcode"
chmod 0755 "$STAGE_DIR/hqcode"
cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$STAGE_DIR/"
if [[ -f "$ROOT_DIR/README.zh-CN.md" ]]; then
  cp "$ROOT_DIR/README.zh-CN.md" "$STAGE_DIR/"
fi

ARCHIVE="$ROOT_DIR/dist/${STAGE_NAME}.tar.gz"
tar -C "$ROOT_DIR/dist" -czf "$ARCHIVE" "$STAGE_NAME"
rm -rf "$STAGE_DIR"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$ROOT_DIR/dist" && sha256sum "${STAGE_NAME}.tar.gz" > "${STAGE_NAME}.tar.gz.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$ROOT_DIR/dist" && shasum -a 256 "${STAGE_NAME}.tar.gz" > "${STAGE_NAME}.tar.gz.sha256")
fi

# Verify archive contents pass sanitization checks.
if tar -tzf "$ARCHIVE" | grep -qE '(\.codex/skills|SKILL\.md|\.hqcode-)'; then
  echo "Archive sanitization gate failed!" >&2
  exit 1
fi

echo "Packaged: $ARCHIVE"
cat "${ARCHIVE}.sha256" 2>/dev/null || true
