#!/bin/sh
# Copyright 2026 JJy <jjyruby@gmail.com>
# SPDX-License-Identifier: Apache-2.0
#
# Canban installer — https://canban.dev/install.sh

set -eu

REPO="${CANBAN_INSTALL_REPO:-jjyr/canban}"
DEST="${CANBAN_INSTALL_DEST:-/Applications/Canban.app}"

info() {
  printf '\033[0;36m==>\033[0m %s\n' "$*"
}

warn() {
  printf '\033[0;33mwarn:\033[0m %s\n' "$*" >&2
}

fail() {
  printf '\033[0;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin) PLATFORM_OS="macos" ;;
  Linux|MINGW*|MSYS*|CYGWIN*)
    fail "Canban currently ships prebuilt releases for macOS first; Linux & Windows support is on the roadmap. Follow https://github.com/${REPO} for updates."
    ;;
  *)
    fail "Unsupported operating system: ${OS}"
    ;;
esac

case "$ARCH" in
  arm64|aarch64) PLATFORM_ARCH="aarch64" ;;
  x86_64) PLATFORM_ARCH="x86_64" ;;
  *) fail "Unsupported CPU architecture: ${ARCH}" ;;
esac

PLATFORM="${PLATFORM_OS}-${PLATFORM_ARCH}"
ASSET="Canban-${PLATFORM}.tar.zst"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

info "Detecting platform: ${PLATFORM}"

if ! command -v curl >/dev/null 2>&1; then
  fail "curl is required to download Canban."
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/canban-install.XXXXXXXX")"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

AUTH_HEADER=""
if [ -n "${GH_TOKEN:-}" ]; then
  AUTH_HEADER="Authorization: Bearer ${GH_TOKEN}"
elif [ -n "${GITHUB_TOKEN:-}" ]; then
  AUTH_HEADER="Authorization: Bearer ${GITHUB_TOKEN}"
fi

fetch_release_json() {
  if [ -n "$AUTH_HEADER" ]; then
    curl -fsSL -H "Accept: application/vnd.github+json" -H "$AUTH_HEADER" "$API_URL" -o "$TMP_DIR/release.json"
  else
    curl -fsSL -H "Accept: application/vnd.github+json" "$API_URL" -o "$TMP_DIR/release.json"
  fi
}

if ! fetch_release_json 2>/dev/null; then
  if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    GH_RESOLVED="$(gh auth token 2>/dev/null || true)"
    if [ -n "$GH_RESOLVED" ]; then
      AUTH_HEADER="Authorization: Bearer ${GH_RESOLVED}"
      fetch_release_json 2>/dev/null || true
    fi
  fi
fi

if [ ! -s "$TMP_DIR/release.json" ]; then
  warn "No public binary release was reachable at ${API_URL}."
  printf '\nTo build and install Canban from source with mise:\n' >&2
  printf '  git clone https://github.com/%s.git\n' "$REPO" >&2
  printf '  cd canban && mise run install-app\n\n' >&2
  exit 1
fi

if ! command -v zstd >/dev/null 2>&1; then
  fail "zstd is required to unpack ${ASSET} (install with: brew install zstd)."
fi

PARSE_OUT="$(/usr/bin/python3 - "$TMP_DIR/release.json" "$ASSET" "$PLATFORM" <<'PY'
import json, sys
release_path, asset_name, platform = sys.argv[1:4]
with open(release_path, "r", encoding="utf-8") as fh:
    data = json.load(fh)
tag = data.get("tag_name", "")
assets = data.get("assets") or []
binary = next((a for a in assets if a.get("name") == asset_name), None)
manifest = next((a for a in assets if a.get("name") == f"manifest-{platform}.json"), None)
if not binary:
    sys.exit(2)
print(tag)
print(binary.get("url", ""))
print(binary.get("browser_download_url", ""))
print(manifest.get("url", "") if manifest else "")
print(manifest.get("browser_download_url", "") if manifest else "")
PY
)" || fail "No prebuilt asset '${ASSET}' found in the latest release."

TAG="$(printf '%s\n' "$PARSE_OUT" | sed -n '1p')"
ASSET_API_URL="$(printf '%s\n' "$PARSE_OUT" | sed -n '2p')"
ASSET_WEB_URL="$(printf '%s\n' "$PARSE_OUT" | sed -n '3p')"
MANIFEST_API_URL="$(printf '%s\n' "$PARSE_OUT" | sed -n '4p')"
MANIFEST_WEB_URL="$(printf '%s\n' "$PARSE_OUT" | sed -n '5p')"

download_asset() {
  api_u="$1"
  web_u="$2"
  out="$3"
  if [ -n "$AUTH_HEADER" ] && [ -n "$api_u" ]; then
    curl -fsSL -L -H "Accept: application/octet-stream" -H "$AUTH_HEADER" "$api_u" -o "$out"
  else
    curl -fsSL -L "$web_u" -o "$out"
  fi
}

info "Downloading Canban ${TAG} (${ASSET})..."
download_asset "$ASSET_API_URL" "$ASSET_WEB_URL" "$TMP_DIR/$ASSET"

if [ -n "$MANIFEST_API_URL" ] || [ -n "$MANIFEST_WEB_URL" ]; then
  download_asset "$MANIFEST_API_URL" "$MANIFEST_WEB_URL" "$TMP_DIR/manifest.json"
  EXPECTED_SHA="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sha256"].lower())' "$TMP_DIR/manifest.json")"
  ACTUAL_SHA="$(/usr/bin/shasum -a 256 "$TMP_DIR/$ASSET" | awk '{print tolower($1)}')"
  [ "$EXPECTED_SHA" = "$ACTUAL_SHA" ] || fail "SHA-256 checksum mismatch for ${ASSET}"
  info "Verified SHA-256 checksum."
fi

if ! command -v hc >/dev/null 2>&1; then
  info "Installing Herdr Core (hc)..."
  curl -fsSL https://releases.herdr.dev/core/install.sh | sh
fi

info "Extracting Canban.app..."
zstd -dc "$TMP_DIR/$ASSET" | tar -xf - -C "$TMP_DIR"
[ -d "$TMP_DIR/Canban.app" ] || fail "Extracted archive did not contain Canban.app"

/usr/bin/xattr -dr com.apple.quarantine "$TMP_DIR/Canban.app" 2>/dev/null || true

DEST_PARENT="$(dirname "$DEST")"
mkdir -p "$DEST_PARENT"
STAGE_DEST="${DEST_PARENT}/.Canban-stage-$$.app"
rm -rf "$STAGE_DEST"
cp -R "$TMP_DIR/Canban.app" "$STAGE_DEST"
rm -rf "$DEST"
mv "$STAGE_DEST" "$DEST"

LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [ -x "$LSREGISTER" ]; then
  "$LSREGISTER" -f "$DEST" >/dev/null 2>&1 || true
fi

info "Installed Canban ${TAG} to ${DEST}"
info "Open Canban from Spotlight or run: open -a '${DEST}'"
