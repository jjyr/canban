#!/bin/bash
if [ -z "${BASH_VERSION:-}" ]; then
  printf 'canban install error: bash is required (run: curl -fsSL https://canban.dev/install.sh | bash)\n' >&2
  exit 1
fi
set -euo pipefail

# Canban cross-platform installer and upgrader (macOS Canban.app, Linux desktop bundle, and Linux headless canband).
# Safe to run on fresh Macs (uses JXA without requiring Xcode Command Line Tools) and Linux desktops/servers.

DEFAULT_REPO="jjyr/canban"
DEFAULT_API="https://api.github.com"
DEFAULT_HOME="${HOME}/.canban"

REPO="${CANBAN_INSTALL_REPO:-$DEFAULT_REPO}"
API="${CANBAN_INSTALL_API:-$DEFAULT_API}"
HOME_DIR="${CANBAN_HOME:-$DEFAULT_HOME}"
ENV_DEST="${CANBAN_INSTALL_DEST:-${CANBAN_INSTALL_DIR:-}}"
DEST="$ENV_DEST"
CUSTOM_DEST=0
[ -n "$ENV_DEST" ] && CUSTOM_DEST=1

BIN_DIR="${CANBAN_BIN_DIR:-}"
CUSTOM_BIN_DIR=0
[ -n "$BIN_DIR" ] && CUSTOM_BIN_DIR=1

XDG_DATA_DIR="${CANBAN_XDG_DATA_HOME:-}"
CUSTOM_XDG_DATA=0
[ -n "$XDG_DATA_DIR" ] && CUSTOM_XDG_DATA=1

RAW_MODE="${CANBAN_INSTALL_MODE:-desktop}"
TARGET_VERSION=""
FORCE=0
NO_RESTART=0

TMP_DIR=""
STAGE_DIR=""

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Install or upgrade Canban on macOS (Apple Silicon) or Linux (x86_64 / aarch64).

Options:
  --version <YYYYMMDD-N>   Install a specific release tag (default: latest release)
  --dest <path>            Destination bundle path (macOS: /Applications/Canban.app, Linux: ~/.local/opt/canban)
  --bin-dir <path>         Directory for CLI/UI symlinks or headless binary on Linux (default: ~/.local/bin)
  --xdg-data-home <path>   XDG data home for .desktop and icons on Linux (default: $XDG_DATA_HOME or ~/.local/share)
  --home <path>            Canban data home directory (default: $CANBAN_HOME or ~/.canban)
  --feed <url-or-dir>      Release API base URL or local feed directory (default: https://api.github.com)
  --headless               Linux only: install standalone headless `canban` CLI/daemon without GTK4/desktop files
  --desktop                Install full desktop application bundle (default)
  --force                  Reinstall even if the same version (or a newer version) is installed
  --no-restart             Do not restart Canban UI or AppServer after upgrading
  -h, --help               Show this help message

Environment overrides:
  CANBAN_INSTALL_REPO      GitHub owner/repo (default: jjyr/canban)
  CANBAN_INSTALL_API       GitHub API base URL or file:// feed
  CANBAN_INSTALL_DEST      Default destination bundle path (alias: CANBAN_INSTALL_DIR)
  CANBAN_BIN_DIR           Default Linux binary/symlink directory
  CANBAN_XDG_DATA_HOME     Default Linux XDG data directory
  CANBAN_INSTALL_MODE      `desktop` (default) or `headless`
  CANBAN_HOME              Default Canban home directory
EOF
}

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'canban install error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$STAGE_DIR" ] && [ -d "$STAGE_DIR" ] && [ -w "$(dirname -- "$STAGE_DIR")" ]; then
    rm -rf -- "$STAGE_DIR" 2>/dev/null || true
  fi
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    rm -rf -- "$TMP_DIR" 2>/dev/null || true
  fi
}
trap cleanup EXIT

canon_path() {
  local p="$1"
  if [ -d "$p" ]; then
    (cd -P -- "$p" 2>/dev/null && pwd -P) || printf '%s\n' "$p"
  else
    local parent
    parent="$(dirname -- "$p")"
    if [ -d "$parent" ]; then
      printf '%s/%s\n' "$(cd -P -- "$parent" 2>/dev/null && pwd -P)" "$(basename -- "$p")"
    else
      printf '%s\n' "$p"
    fi
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || fail "--version requires an argument"
      TARGET_VERSION="$2"
      shift 2
      ;;
    --dest)
      [ $# -ge 2 ] || fail "--dest requires an argument"
      DEST="$2"
      CUSTOM_DEST=1
      shift 2
      ;;
    --bin-dir)
      [ $# -ge 2 ] || fail "--bin-dir requires an argument"
      BIN_DIR="$2"
      CUSTOM_BIN_DIR=1
      shift 2
      ;;
    --xdg-data-home)
      [ $# -ge 2 ] || fail "--xdg-data-home requires an argument"
      XDG_DATA_DIR="$2"
      CUSTOM_XDG_DATA=1
      shift 2
      ;;
    --home)
      [ $# -ge 2 ] || fail "--home requires an argument"
      HOME_DIR="$2"
      shift 2
      ;;
    --feed)
      [ $# -ge 2 ] || fail "--feed requires an argument"
      API="$2"
      shift 2
      ;;
    --headless)
      RAW_MODE="headless"
      shift
      ;;
    --desktop)
      RAW_MODE="desktop"
      shift
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --no-restart)
      NO_RESTART=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "Unknown option: $1"
      ;;
  esac
done

case "$RAW_MODE" in
  desktop)
    INSTALL_MODE="desktop"
    ;;
  headless|cli)
    INSTALL_MODE="headless"
    ;;
  *)
    fail "Invalid install mode '$RAW_MODE' (expected 'desktop' or 'headless')"
    ;;
esac

UNAME_S="${CANBAN_INSTALL_OS:-$(uname -s)}"
UNAME_M="${CANBAN_INSTALL_ARCH:-$(uname -m)}"

case "$UNAME_S" in
  Darwin)
    OS_KIND="darwin"
    [ "$UNAME_M" = "arm64" ] || fail "Canban on macOS currently supports Apple Silicon (arm64) only (got $UNAME_M)"
    PLATFORM="macos-aarch64"
    if [ "$INSTALL_MODE" = "headless" ]; then
      fail "--headless mode is supported on Linux; macOS installs Canban.app"
    fi
    if [ -z "$DEST" ]; then
      DEST="/Applications/Canban.app"
    fi
    ;;
  Linux)
    OS_KIND="linux"
    case "$UNAME_M" in
      x86_64|amd64)
        PLATFORM="linux-x86_64"
        ;;
      aarch64|arm64)
        PLATFORM="linux-aarch64"
        ;;
      *)
        fail "Unsupported Linux architecture: $UNAME_M (expected x86_64 or aarch64)"
        ;;
    esac
    if [ -z "$DEST" ]; then
      DEST="${HOME}/.local/opt/canban"
    fi
    if [ "$CUSTOM_BIN_DIR" -eq 0 ]; then
      if [ "$CUSTOM_DEST" -eq 1 ]; then
        if [ "$(basename -- "$DEST")" = "bin" ]; then
          BIN_DIR="$DEST"
        elif [ "$(basename -- "$(dirname -- "$DEST")")" = "opt" ]; then
          BIN_DIR="$(dirname -- "$(dirname -- "$DEST")")/bin"
        else
          BIN_DIR="$(dirname -- "$DEST")/bin"
        fi
      else
        BIN_DIR="${HOME}/.local/bin"
      fi
    fi
    if [ "$CUSTOM_XDG_DATA" -eq 0 ]; then
      if [ "$CUSTOM_DEST" -eq 1 ]; then
        if [ "$(basename -- "$(dirname -- "$DEST")")" = "opt" ]; then
          XDG_DATA_DIR="$(dirname -- "$(dirname -- "$DEST")")/share"
        else
          XDG_DATA_DIR="$(dirname -- "$DEST")/share"
        fi
      else
        XDG_DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}"
      fi
    fi
    ;;
  *)
    fail "Canban supports macOS and Linux only (got $UNAME_S)"
    ;;
esac

if [[ "$API" == /* || "$API" == ./* || "$API" == ../* ]]; then
  API="file://$(canon_path "$API")"
fi
API="${API%/}"

USE_JXA=0
PYTHON3_BIN=""
if [ "$OS_KIND" = "darwin" ] && [ -x /usr/bin/osascript ] && [ -z "${CANBAN_INSTALL_FORCE_PYTHON:-}" ]; then
  USE_JXA=1
else
  for py_candidate in "${CANBAN_PYTHON3_BIN:-}" "$(command -v python3 2>/dev/null || true)" /usr/bin/python3 /usr/local/bin/python3; do
    if [ -n "$py_candidate" ] && [ -x "$py_candidate" ] && "$py_candidate" -c 'import json, sys' >/dev/null 2>&1; then
      PYTHON3_BIN="$py_candidate"
      break
    fi
  done
  [ -n "$PYTHON3_BIN" ] || fail "Working python3 is required on Linux (install via apt install python3 / pacman -S python / dnf install python3)"
fi

run_jxa() {
  /usr/bin/osascript -l JavaScript -e "$1" -- "${@:2}"
}

run_py() {
  "$PYTHON3_BIN" -c "$1" "${@:2}"
}

compute_sha256() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print tolower($1)}'
  elif [ -x /usr/bin/shasum ]; then
    /usr/bin/shasum -a 256 "$file" | awk '{print tolower($1)}'
  else
    run_py 'import hashlib, pathlib, sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' "$file"
  fi
}

# Coexistence guard on Linux: if `--headless` is requested where a full Canban desktop bundle
# is already installed, upgrade the full desktop bundle so `canban` and `canban-ui` never drift.
if [ "$OS_KIND" = "linux" ] && [ "$INSTALL_MODE" = "headless" ] && [ -z "${CANBAN_INSTALL_FORCE_HEADLESS:-}" ]; then
  if [ -L "$BIN_DIR/canban" ]; then
    LINK_TARGET="$(run_py 'import os, pathlib, sys
p = pathlib.Path(sys.argv[1])
try:
    r = p.resolve()
    b = r.parent.parent
    if r.parent.name == "bin" and (b / "bin/canban-ui").is_file() and (b / "bundle.json").is_file():
        print(str(b))
except OSError:
    pass
' "$BIN_DIR/canban")"
    if [ -n "$LINK_TARGET" ]; then
      if [ "$CUSTOM_DEST" -eq 0 ]; then
        DEST="$LINK_TARGET"
      fi
      log "Detected existing Canban desktop bundle at $DEST; upgrading full desktop bundle to keep CLI and UI in sync..."
      INSTALL_MODE="desktop"
    fi
  fi
  if [ "$INSTALL_MODE" = "headless" ] && [ -f "$DEST/bundle.json" ] && [ -f "$DEST/bin/canban-ui" ]; then
    log "Detected existing Canban desktop bundle at $DEST; upgrading full desktop bundle to keep CLI and UI in sync..."
    INSTALL_MODE="desktop"
  fi
fi

LIVE_HOME="${CANBAN_HOME:-$DEFAULT_HOME}"
CANON_TARGET_HOME="$(canon_path "$HOME_DIR")"
CANON_LIVE_HOME="$(canon_path "$LIVE_HOME")"
if [ -n "${CANBAN_CARD:-}" ] && [ -z "${CANBAN_INSTALL_ALLOW_AGENT:-}" ] && [ "$CANON_TARGET_HOME" = "$CANON_LIVE_HOME" ]; then
  fail "Refusing to replace Canban from inside a Card Agent session targeting the live home ($CANON_TARGET_HOME)"
fi

if [ "$OS_KIND" = "darwin" ] && [ -e "$DEST/Contents/Resources/dev-instance.json" ]; then
  fail "Refusing to overwrite a branch-bound development bundle at $DEST"
fi
if [ "$OS_KIND" = "linux" ] && [ "$INSTALL_MODE" = "desktop" ] && [ -e "$DEST/share/dev-instance.json" ]; then
  fail "Refusing to overwrite a branch-bound development bundle at $DEST"
fi

json_journal_phase() {
  local journal="$1"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      ObjC.import("Foundation");
      function run(argv) {
        var data = $.NSData.dataWithContentsOfFile(argv[0]);
        if (!data) return "UNREADABLE";
        var raw = $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding);
        if (!raw) return "UNREADABLE";
        try {
          var j = JSON.parse(raw.js);
          return typeof j.phase === "string" ? j.phase : "MISSING_PHASE";
        } catch (e) {
          return "CORRUPT";
        }
      }
    ' "$journal"
  else
    run_py '
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
try:
    raw = p.read_text(encoding="utf-8")
except Exception:
    print("UNREADABLE")
    raise SystemExit(0)
try:
    j = json.loads(raw)
    print(j["phase"] if isinstance(j, dict) and isinstance(j.get("phase"), str) else "MISSING_PHASE")
except Exception:
    print("CORRUPT")
' "$journal"
  fi
}

check_install_journal() {
  local journal="$HOME_DIR/updates/install.json"
  if [ ! -e "$journal" ] && [ ! -L "$journal" ]; then
    return 0
  fi
  local status
  status="$(json_journal_phase "$journal")" || status="ERROR"

  case "$status" in
    complete|rolled_back)
      return 0
      ;;
    *)
      fail "An in-app Canban update is pending or needs recovery (journal: $journal, phase: $status). Finish or run canban-updater --recover first."
      ;;
  esac
}

read_db_schema() {
  local db="$1"
  if [ ! -f "$db" ]; then
    printf '0\n'
    return 0
  fi
  local sz
  sz="$(wc -c < "$db" | tr -d '[:space:]')"
  if [ "${sz:-0}" -lt 64 ]; then
    printf '0\n'
    return 0
  fi
  od -An -tu1 -j 60 -N 4 "$db" | awk 'NF >= 4 { printf "%u\n", (($1 * 256 + $2) * 256 + $3) * 256 + $4; exit }'
}

check_install_journal

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/canban-install.XXXXXX")"

log "Checking Canban releases from $REPO ($PLATFORM, $INSTALL_MODE)..."
RELEASES_JSON="$TMP_DIR/releases.json"
if [[ "$API" == file://* ]]; then
  FEED_DIR="${API#file://}"
  if [ -f "$FEED_DIR/repos/$REPO/releases.json" ]; then
    cp "$FEED_DIR/repos/$REPO/releases.json" "$RELEASES_JSON"
  elif [ -f "$FEED_DIR/repos/$REPO/releases" ]; then
    cp "$FEED_DIR/repos/$REPO/releases" "$RELEASES_JSON"
  elif [ -f "$FEED_DIR/releases.json" ]; then
    cp "$FEED_DIR/releases.json" "$RELEASES_JSON"
  else
    fail "Local feed releases.json not found under $FEED_DIR"
  fi
else
  curl -fsSL --retry 2 \
    -H "Accept: application/vnd.github+json" \
    -H "User-Agent: Canban-Installer" \
    "$API/repos/$REPO/releases?per_page=100" \
    -o "$RELEASES_JSON" || fail "Failed to fetch releases from $API/repos/$REPO/releases"
fi

select_release() {
  local releases_file="$1"
  local target_ver="$2"
  local platform="$3"
  local mode="$4"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      ObjC.import("Foundation");
      function readText(path) {
        var data = $.NSData.dataWithContentsOfFile(path);
        if (!data) throw new Error("Failed to read " + path);
        var s = $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding);
        if (!s) throw new Error("Invalid UTF-8 in " + path);
        return s.js;
      }
      function parseVersion(tag) {
        if (typeof tag !== "string") return null;
        var m = /^(\d{8})-([1-9]\d*)$/.exec(tag);
        if (!m) return null;
        var y = parseInt(m[1].slice(0, 4), 10);
        var mo = parseInt(m[1].slice(4, 6), 10);
        var d = parseInt(m[1].slice(6, 8), 10);
        var leap = (y % 4 === 0) && (y % 100 !== 0 || y % 400 === 0);
        var mdays = [0, 31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        if (y < 2000 || mo < 1 || mo > 12 || d < 1 || d > mdays[mo]) return null;
        var seq = Number(m[2]);
        if (!Number.isSafeInteger(seq) || seq <= 0) return null;
        return [parseInt(m[1], 10), seq];
      }
      function validUrl(u) {
        return typeof u === "string" && (
          u.indexOf("https://") === 0 ||
          u.indexOf("http://127.0.0.1:") === 0 ||
          u.indexOf("http://localhost:") === 0 ||
          u.indexOf("file://") === 0
        );
      }
      function run(argv) {
        var releases = JSON.parse(readText(argv[0]));
        var target = argv[1] || "";
        var platform = argv[2];
        var mode = argv[3];
        if (!Array.isArray(releases)) throw new Error("Unexpected releases payload");
        if (target && !parseVersion(target)) throw new Error("Invalid --version format (expected YYYYMMDD-N): " + target);
        var manifestName = mode === "headless" ? ("manifest-cli-" + platform + ".json") : ("manifest-" + platform + ".json");
        var gzName = mode === "headless" ? ("canban-cli-" + platform + ".tar.gz") : ("Canban-" + platform + ".tar.gz");
        var zstName = mode === "headless" ? "" : ("Canban-" + platform + ".tar.zst");
        var candidates = [];
        for (var i = 0; i < releases.length; i++) {
          var r = releases[i];
          if (!r || r.draft === true) continue;
          var parsed = parseVersion(r.tag_name);
          if (!parsed) continue;
          if (target && r.tag_name !== target) continue;
          var assets = Array.isArray(r.assets) ? r.assets : [];
          var manifest = null, gz = null, zst = null;
          for (var j = 0; j < assets.length; j++) {
            var a = assets[j];
            if (!a) continue;
            if (a.name === manifestName) manifest = a;
            else if (a.name === gzName) gz = a;
            else if (zstName && a.name === zstName) zst = a;
          }
          if (!manifest || (!gz && !zst)) continue;
          candidates.push({
            version: r.tag_name,
            key: parsed,
            manifestUrl: manifest.browser_download_url,
            archiveUrl: (gz || zst).browser_download_url,
            archiveKind: gz ? "tar.gz" : "tar.zst"
          });
        }
        candidates.sort(function(a, b) {
          if (b.key[0] !== a.key[0]) return b.key[0] - a.key[0];
          return b.key[1] - a.key[1];
        });
        if (candidates.length === 0) {
          throw new Error(target ? ("Release " + target + " with " + platform + " assets not found") : ("No published Canban " + platform + " release found"));
        }
        var pick = candidates[0];
        if (!validUrl(pick.manifestUrl) || !validUrl(pick.archiveUrl)) {
          throw new Error("Release asset URL is not HTTPS");
        }
        return [pick.version, pick.archiveKind, pick.manifestUrl, pick.archiveUrl].join("\n");
      }
    ' "$releases_file" "$target_ver" "$platform" "$mode"
  else
    run_py '
import json, pathlib, re, sys

def parse_version(tag):
    if not isinstance(tag, str):
        return None
    m = re.match(r"^(\d{8})-([1-9]\d*)$", tag)
    if not m:
        return None
    y, mo, d = int(m.group(1)[:4]), int(m.group(1)[4:6]), int(m.group(1)[6:8])
    leap = (y % 4 == 0) and (y % 100 != 0 or y % 400 == 0)
    mdays = [0, 31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    if y < 2000 or mo < 1 or mo > 12 or d < 1 or d > mdays[mo]:
        return None
    seq = int(m.group(2))
    return (int(m.group(1)), seq) if seq > 0 else None

def valid_url(u):
    return isinstance(u, str) and u.startswith(("https://", "http://127.0.0.1:", "http://localhost:", "file://"))

releases = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
target = sys.argv[2] or ""
platform = sys.argv[3]
mode = sys.argv[4]
if not isinstance(releases, list):
    raise SystemExit("Unexpected releases payload")
if target and not parse_version(target):
    raise SystemExit(f"Invalid --version format (expected YYYYMMDD-N): {target}")

manifest_name = f"manifest-cli-{platform}.json" if mode == "headless" else f"manifest-{platform}.json"
gz_name = f"canban-cli-{platform}.tar.gz" if mode == "headless" else f"Canban-{platform}.tar.gz"
zst_name = "" if mode == "headless" else f"Canban-{platform}.tar.zst"

candidates = []
for r in releases:
    if not isinstance(r, dict) or r.get("draft") is True:
        continue
    tag = r.get("tag_name")
    parsed = parse_version(tag)
    if not parsed or (target and tag != target):
        continue
    assets = r.get("assets") if isinstance(r.get("assets"), list) else []
    manifest = gz = zst = None
    for a in assets:
        if not isinstance(a, dict):
            continue
        name = a.get("name")
        if name == manifest_name:
            manifest = a
        elif name == gz_name:
            gz = a
        elif zst_name and name == zst_name:
            zst = a
    if not manifest or (not gz and not zst):
        continue
    chosen = gz or zst
    candidates.append((
        parsed,
        tag,
        "tar.gz" if gz else "tar.zst",
        manifest.get("browser_download_url"),
        chosen.get("browser_download_url"),
    ))

candidates.sort(key=lambda x: x[0], reverse=True)
if not candidates:
    raise SystemExit(f"Release {target} with {platform} assets not found" if target else f"No published Canban {platform} release found")
_, ver, kind, m_url, a_url = candidates[0]
if not valid_url(m_url) or not valid_url(a_url):
    raise SystemExit("Release asset URL is not HTTPS")
print("\n".join([ver, kind, m_url, a_url]))
' "$releases_file" "$target_ver" "$platform" "$mode"
  fi
}

SELECTED="$(select_release "$RELEASES_JSON" "$TARGET_VERSION" "$PLATFORM" "$INSTALL_MODE")" || fail "Could not select a release for $PLATFORM ($INSTALL_MODE)"

TARGET_VERSION="$(printf '%s\n' "$SELECTED" | sed -n '1p')"
ARCHIVE_KIND="$(printf '%s\n' "$SELECTED" | sed -n '2p')"
MANIFEST_URL="$(printf '%s\n' "$SELECTED" | sed -n '3p')"
ARCHIVE_URL="$(printf '%s\n' "$SELECTED" | sed -n '4p')"

MANIFEST_JSON="$TMP_DIR/manifest.json"
curl -fsSL --retry 2 -H "User-Agent: Canban-Installer" "$MANIFEST_URL" -o "$MANIFEST_JSON" \
  || fail "Failed to download manifest from $MANIFEST_URL"

parse_manifest() {
  local manifest_file="$1"
  local expected_ver="$2"
  local kind="$3"
  local expected_platform="$4"
  local mode="$5"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      ObjC.import("Foundation");
      function readText(path) {
        var data = $.NSData.dataWithContentsOfFile(path);
        if (!data) throw new Error("Failed to read manifest");
        return $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding).js;
      }
      function run(argv) {
        var m = JSON.parse(readText(argv[0]));
        var expectedVersion = argv[1];
        var kind = argv[2];
        var expectedPlatform = argv[3];
        var mode = argv[4];
        if (m.version !== expectedVersion) throw new Error("Manifest version mismatch: " + m.version + " != " + expectedVersion);
        if (m.platform !== expectedPlatform) throw new Error("Unexpected manifest platform: " + m.platform);
        if (typeof m.commit !== "string" || !/^[0-9a-fA-F]{40}$/.test(m.commit)) throw new Error("Invalid manifest commit");
        if (!Number.isInteger(m.schema) || m.schema < 8) throw new Error("Invalid manifest schema");
        if (!Number.isInteger(m.api) || m.api < 1) throw new Error("Invalid manifest api");
        var asset, size, sha;
        if (mode === "headless") {
          asset = m.asset;
          size = m.size;
          sha = m.sha256;
        } else if (kind === "tar.gz") {
          asset = m.tar_gz;
          size = m.tar_gz_size;
          sha = m.tar_gz_sha256;
        } else {
          asset = m.asset;
          size = m.size;
          sha = m.sha256;
        }
        if (typeof asset !== "string" || !asset) throw new Error("Manifest archive name missing for " + kind);
        if (!Number.isInteger(size) || size <= 0) throw new Error("Manifest archive size invalid");
        if (typeof sha !== "string" || !/^[0-9a-fA-F]{64}$/.test(sha)) throw new Error("Manifest checksum invalid");
        return [m.version, m.commit, String(m.schema), String(m.api), asset, String(size), sha.toLowerCase()].join("\n");
      }
    ' "$manifest_file" "$expected_ver" "$kind" "$expected_platform" "$mode"
  else
    run_py '
import json, pathlib, re, sys

m = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
expected_version, kind, expected_platform, mode = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
m_ver = m.get("version")
m_plat = m.get("platform")
if m_ver != expected_version:
    raise SystemExit(f"Manifest version mismatch: {m_ver} != {expected_version}")
if m_plat != expected_platform:
    raise SystemExit(f"Unexpected manifest platform: {m_plat}")
commit = m.get("commit")
if not isinstance(commit, str) or not re.match(r"^[0-9a-fA-F]{40}$", commit):
    raise SystemExit("Invalid manifest commit")
schema = m.get("schema")
api = m.get("api")
if type(schema) is not int or schema < 8:
    raise SystemExit("Invalid manifest schema")
if type(api) is not int or api < 1:
    raise SystemExit("Invalid manifest api")
if mode == "headless":
    asset, size, sha = m.get("asset"), m.get("size"), m.get("sha256")
elif kind == "tar.gz":
    asset, size, sha = m.get("tar_gz"), m.get("tar_gz_size"), m.get("tar_gz_sha256")
else:
    asset, size, sha = m.get("asset"), m.get("size"), m.get("sha256")
if not isinstance(asset, str) or not asset:
    raise SystemExit(f"Manifest archive name missing for {kind}")
if type(size) is not int or size <= 0:
    raise SystemExit("Manifest archive size invalid")
if not isinstance(sha, str) or not re.match(r"^[0-9a-fA-F]{64}$", sha):
    raise SystemExit("Manifest checksum invalid")
print("\n".join([m["version"], commit, str(schema), str(api), asset, str(size), sha.lower()]))
' "$manifest_file" "$expected_ver" "$kind" "$expected_platform" "$mode"
  fi
}

MANIFEST_FIELDS="$(parse_manifest "$MANIFEST_JSON" "$TARGET_VERSION" "$ARCHIVE_KIND" "$PLATFORM" "$INSTALL_MODE")" || fail "Invalid release manifest"

MANIFEST_COMMIT="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '2p')"
MANIFEST_SCHEMA="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '3p')"
MANIFEST_API="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '4p')"
ARCHIVE_NAME="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '5p')"
EXPECTED_SIZE="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '6p')"
EXPECTED_SHA256="$(printf '%s\n' "$MANIFEST_FIELDS" | sed -n '7p')"

check_installed_version() {
  local info_json="$1"
  local target_ver="$2"
  local target_schema="$3"
  local force_flag="$4"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      function parseVersion(tag) {
        if (typeof tag !== "string") return null;
        var m = /^(\d{8})-([1-9]\d*)$/.exec(tag);
        if (!m) return null;
        return [parseInt(m[1], 10), Number(m[2])];
      }
      function cmpVersion(a, b) {
        if (a[0] !== b[0]) return a[0] < b[0] ? -1 : 1;
        if (a[1] !== b[1]) return a[1] < b[1] ? -1 : 1;
        return 0;
      }
      function run(argv) {
        var cur = JSON.parse(argv[0]);
        var targetVer = argv[1];
        var targetSchema = parseInt(argv[2], 10);
        var force = argv[3] === "1";
        if (typeof cur.schema === "number" && targetSchema < cur.schema) {
          return "ERR:Target release schema (" + targetSchema + ") is older than installed Canban schema (" + cur.schema + "); downgrades are not supported.";
        }
        if (cur.version === targetVer && !force) {
          return "UP_TO_DATE:" + cur.version;
        }
        var pCur = parseVersion(cur.version);
        var pTgt = parseVersion(targetVer);
        if (pCur && pTgt && cmpVersion(pTgt, pCur) < 0 && !force) {
          return "ERR:Installed Canban (" + cur.version + ") is newer than target (" + targetVer + "). Pass --force to install an older release with a compatible schema.";
        }
        return "PROCEED:" + (cur.version || "unknown");
      }
    ' "$info_json" "$target_ver" "$target_schema" "$force_flag"
  else
    run_py '
import json, re, sys

def parse_ver(tag):
    if not isinstance(tag, str):
        return None
    m = re.match(r"^(\d{8})-([1-9]\d*)$", tag)
    return (int(m.group(1)), int(m.group(2))) if m else None

cur = json.loads(sys.argv[1])
target_ver = sys.argv[2]
target_schema = int(sys.argv[3])
force = sys.argv[4] == "1"
cur_ver = cur.get("version")
cur_schema = cur.get("schema")
if isinstance(cur_schema, int) and target_schema < cur_schema:
    print(f"ERR:Target release schema ({target_schema}) is older than installed Canban schema ({cur_schema}); downgrades are not supported.")
elif cur_ver == target_ver and not force:
    print(f"UP_TO_DATE:{cur_ver}")
else:
    p_cur = parse_ver(cur_ver)
    p_tgt = parse_ver(target_ver)
    if p_cur and p_tgt and p_tgt < p_cur and not force:
        print(f"ERR:Installed Canban ({cur_ver}) is newer than target ({target_ver}). Pass --force to install an older release with a compatible schema.")
    else:
        ver_label = cur_ver or "unknown"
        print(f"PROCEED:{ver_label}")
' "$info_json" "$target_ver" "$target_schema" "$force_flag"
  fi
}

INSTALLED_CLI=""
INSTALLED_LOCATION=""
if [ "$OS_KIND" = "darwin" ]; then
  INSTALLED_CLI="$DEST/Contents/MacOS/canban"
  INSTALLED_LOCATION="$DEST"
elif [ "$INSTALL_MODE" = "headless" ]; then
  INSTALLED_CLI="$BIN_DIR/canban"
  INSTALLED_LOCATION="$BIN_DIR/canban"
else
  INSTALLED_CLI="$DEST/bin/canban"
  INSTALLED_LOCATION="$DEST"
fi

# Check existing installation before downloading large archive.
if [ -x "$INSTALLED_CLI" ]; then
  INSTALLED_INFO=""
  if INSTALLED_INFO="$(env -u CANBAN_ENDPOINT "$INSTALLED_CLI" build-info 2>/dev/null)"; then
    CHECK_INSTALLED="$(check_installed_version "$INSTALLED_INFO" "$TARGET_VERSION" "$MANIFEST_SCHEMA" "$FORCE")" || fail "Could not inspect installed Canban version"

    case "$CHECK_INSTALLED" in
      UP_TO_DATE:*)
        log "Canban ${CHECK_INSTALLED#UP_TO_DATE:} is already up to date at $INSTALLED_LOCATION"
        exit 0
        ;;
      ERR:*)
        fail "${CHECK_INSTALLED#ERR:}"
        ;;
    esac
  fi
fi

DB_PATH="$HOME_DIR/canban.sqlite"
INITIAL_DB_SCHEMA="$(read_db_schema "$DB_PATH")"
if [ "$INITIAL_DB_SCHEMA" -gt "$MANIFEST_SCHEMA" ]; then
  fail "Database schema ($INITIAL_DB_SCHEMA) at $DB_PATH is newer than target release schema ($MANIFEST_SCHEMA); downgrades are not supported."
fi

log "Downloading Canban $TARGET_VERSION ($ARCHIVE_NAME)..."
ARCHIVE_PATH="$TMP_DIR/$ARCHIVE_NAME"
curl -fsSL --retry 2 -H "User-Agent: Canban-Installer" "$ARCHIVE_URL" -o "$ARCHIVE_PATH" \
  || fail "Failed to download archive from $ARCHIVE_URL"

ACTUAL_SIZE="$(wc -c < "$ARCHIVE_PATH" | tr -d '[:space:]')"
[ "$ACTUAL_SIZE" = "$EXPECTED_SIZE" ] || fail "Archive size mismatch: expected $EXPECTED_SIZE bytes, got $ACTUAL_SIZE"

ACTUAL_SHA256="$(compute_sha256 "$ARCHIVE_PATH")"
[ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] || fail "Archive SHA-256 mismatch: expected $EXPECTED_SHA256, got $ACTUAL_SHA256"

ZSTD_BIN=""
if [ "$ARCHIVE_KIND" = "tar.zst" ]; then
  for candidate in "$(command -v zstd 2>/dev/null || true)" /opt/homebrew/bin/zstd /usr/local/bin/zstd /usr/bin/zstd; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      ZSTD_BIN="$candidate"
      break
    fi
  done
  [ -n "$ZSTD_BIN" ] || fail "Release $TARGET_VERSION only provides .tar.zst and zstd was not found in PATH"
fi

inspect_archive() {
  local list_file="$TMP_DIR/tar-list.txt"
  local verbose_file="$TMP_DIR/tar-verbose.txt"
  if [ "$ARCHIVE_KIND" = "tar.gz" ]; then
    tar -tf "$ARCHIVE_PATH" > "$list_file" || fail "Failed to list archive contents"
    tar -tvf "$ARCHIVE_PATH" > "$verbose_file" || fail "Failed to inspect archive metadata"
  else
    "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | tar -tf - > "$list_file" || fail "Failed to list zst archive contents"
    "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | tar -tvf - > "$verbose_file" || fail "Failed to inspect zst archive metadata"
  fi
  [ -s "$list_file" ] || fail "Archive is empty"
  if [ "$OS_KIND" = "darwin" ]; then
    awk '
      {
        if ($0 ~ /^\// || $0 == ".." || $0 ~ /(^|\/)\.\.($|\/)/) { exit 1 }
        if ($0 !~ /^Canban\.app(\/|$)/) { exit 1 }
      }
    ' "$list_file" || fail "Unsafe archive entry path"
    awk '
      NF > 0 && $1 !~ /^[-d]/ { exit 1 }
    ' "$verbose_file" || fail "Archive contains an unsupported symlink or special file"
  elif [ "$INSTALL_MODE" = "headless" ]; then
    awk '
      {
        if ($0 != "canban" && $0 != "./canban") { exit 1 }
      }
    ' "$list_file" || fail "Unexpected CLI archive entry path"
    awk '
      NF > 0 && $1 !~ /^-/ { exit 1 }
    ' "$verbose_file" || fail "CLI archive contains a non-file entry"
  else
    awk '
      {
        if ($0 ~ /^\// || $0 == ".." || $0 ~ /(^|\/)\.\.($|\/)/) { exit 1 }
        if ($0 !~ /^canban(\/|$)/) { exit 1 }
      }
    ' "$list_file" || fail "Unsafe archive entry path"
    awk '
      NF > 0 && $1 !~ /^[-d]/ { exit 1 }
    ' "$verbose_file" || fail "Archive contains an unsupported symlink or special file"
  fi
}

inspect_archive

EXTRACT_DIR="$TMP_DIR/extract"
mkdir -p "$EXTRACT_DIR"
if [ "$ARCHIVE_KIND" = "tar.gz" ]; then
  tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR" || fail "Failed to extract $ARCHIVE_NAME"
else
  "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | tar -xf - -C "$EXTRACT_DIR" || fail "Failed to extract $ARCHIVE_NAME"
fi

verify_candidate_build_info() {
  local info_json="$1"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      function run(argv) {
        var b = JSON.parse(argv[0]);
        if (b.version !== argv[1]) throw new Error("Candidate version mismatch: " + b.version);
        if (b.commit !== argv[2]) throw new Error("Candidate commit mismatch: " + b.commit);
        if (String(b.schema) !== argv[3]) throw new Error("Candidate schema mismatch: " + b.schema);
        if (String(b.api) !== argv[4]) throw new Error("Candidate api mismatch: " + b.api);
        if (b.platform !== argv[5]) throw new Error("Candidate platform mismatch: " + b.platform);
        return "OK";
      }
    ' "$info_json" "$TARGET_VERSION" "$MANIFEST_COMMIT" "$MANIFEST_SCHEMA" "$MANIFEST_API" "$PLATFORM" >/dev/null
  else
    run_py '
import json, sys
b = json.loads(sys.argv[1])
if (
    b.get("version") != sys.argv[2]
    or b.get("commit") != sys.argv[3]
    or str(b.get("schema")) != sys.argv[4]
    or str(b.get("api")) != sys.argv[5]
    or b.get("platform") != sys.argv[6]
):
    raise SystemExit("Candidate build-info mismatch")
' "$info_json" "$TARGET_VERSION" "$MANIFEST_COMMIT" "$MANIFEST_SCHEMA" "$MANIFEST_API" "$PLATFORM"
  fi
}

CANDIDATE=""
CANDIDATE_CLI=""

if [ "$OS_KIND" = "darwin" ]; then
  CANDIDATE="$EXTRACT_DIR/Canban.app"
  [ -d "$CANDIDATE" ] || fail "Extracted archive did not contain Canban.app"

  UNSAFE_NODES="$(find "$CANDIDATE" ! -type f ! -type d -print)"
  [ -z "$UNSAFE_NODES" ] || fail "Extracted bundle contains unsafe symlinks or special files"

  [ -f "$CANDIDATE/Contents/Info.plist" ] || fail "Missing Contents/Info.plist in extracted bundle"
  BUNDLE_ID="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$CANDIDATE/Contents/Info.plist" 2>/dev/null || true)"
  [ "$BUNDLE_ID" = "com.canban.desktop" ] || fail "Unexpected CFBundleIdentifier: $BUNDLE_ID"
  [ ! -e "$CANDIDATE/Contents/Resources/dev-instance.json" ] || fail "Downloaded bundle unexpectedly contains dev-instance.json"

  for bin_name in canban canban-ui canban-updater; do
    [ -f "$CANDIDATE/Contents/MacOS/$bin_name" ] || fail "Missing Contents/MacOS/$bin_name in extracted bundle"
    chmod +x "$CANDIDATE/Contents/MacOS/$bin_name"
  done

  log "Clearing quarantine attributes and signing bundle..."
  /usr/bin/xattr -dr com.apple.quarantine "$CANDIDATE" 2>/dev/null || true
  /usr/bin/codesign --force --deep --sign - --timestamp=none "$CANDIDATE" >/dev/null 2>&1 || fail "codesign failed on $CANDIDATE"
  /usr/bin/codesign --verify --deep --strict "$CANDIDATE" >/dev/null 2>&1 || fail "codesign verification failed on $CANDIDATE"
  CANDIDATE_CLI="$CANDIDATE/Contents/MacOS/canban"
elif [ "$INSTALL_MODE" = "headless" ]; then
  CANDIDATE_CLI="$EXTRACT_DIR/canban"
  [ -f "$CANDIDATE_CLI" ] && [ ! -L "$CANDIDATE_CLI" ] || fail "Extracted CLI archive did not contain regular file canban"
  chmod 0755 "$CANDIDATE_CLI"
else
  CANDIDATE="$EXTRACT_DIR/canban"
  [ -d "$CANDIDATE" ] || fail "Extracted archive did not contain canban/ bundle"

  UNSAFE_NODES="$(find "$CANDIDATE" ! -type f ! -type d -print)"
  [ -z "$UNSAFE_NODES" ] || fail "Extracted bundle contains unsafe symlinks or special files"

  [ -f "$CANDIDATE/bundle.json" ] || fail "Missing bundle.json in extracted Linux bundle"
  BUNDLE_ID="$(run_py 'import json, pathlib, sys; print(json.loads(pathlib.Path(sys.argv[1]).read_text()).get("app_id", ""))' "$CANDIDATE/bundle.json" 2>/dev/null || true)"
  [ "$BUNDLE_ID" = "com.canban.desktop" ] || fail "Unexpected Linux bundle app_id: $BUNDLE_ID"
  [ ! -e "$CANDIDATE/share/dev-instance.json" ] || fail "Downloaded bundle unexpectedly contains share/dev-instance.json"

  for bin_name in canban canban-ui canban-updater; do
    [ -f "$CANDIDATE/bin/$bin_name" ] || fail "Missing bin/$bin_name in extracted bundle"
    chmod 0755 "$CANDIDATE/bin/$bin_name"
  done
  CANDIDATE_CLI="$CANDIDATE/bin/canban"

  if command -v ldd >/dev/null 2>&1; then
    LDD_OUT="$(ldd "$CANDIDATE/bin/canban-ui" 2>/dev/null || true)"
    if printf '%s\n' "$LDD_OUT" | grep -q 'not found'; then
      log ""
      log "Warning: Missing shared runtime libraries required by canban-ui (e.g. GTK4 / GStreamer)."
      log "Install desktop runtime packages before launching the GUI:"
      log "  Ubuntu / Debian : sudo apt install libgtk-4-1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0"
      log "  Arch Linux      : sudo pacman -S gtk4 gstreamer gst-plugins-base"
      log "  Fedora          : sudo dnf install gtk4 gstreamer1 gstreamer1-plugins-base"
      log ""
    fi
  fi
fi

CANDIDATE_INFO="$(env -u CANBAN_ENDPOINT "$CANDIDATE_CLI" build-info)" \
  || fail "Failed to execute candidate canban build-info"

verify_candidate_build_info "$CANDIDATE_INFO" \
  || fail "Candidate build-info did not match release manifest"

# Upgrade coordination: gracefully stop existing UI, systemd user unit (if active), and AppServer while preserving herdr-core PTYs.
proc_alive() {
  local pid="$1"
  local st
  st="$(ps -p "$pid" -o state= 2>/dev/null | tr -d '[:space:]')" || return 1
  [ -n "$st" ] || return 1
  case "$st" in
    Z*) return 1 ;;
  esac
  return 0
}

UI_WAS_RUNNING=0
SERVER_WAS_RUNNING=0
SYSTEMD_MANAGED=0
SYSTEMCTL_CMD="${CANBAN_SYSTEMCTL_BIN:-systemctl}"
SYSTEMD_UNIT_DIR="${CANBAN_SYSTEMD_UNIT_DIR:-${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user}"

if [ "$INSTALL_MODE" = "desktop" ]; then
  DEST_CANON="$(canon_path "$DEST")"
  UI_PIDS=""
  if [ "$OS_KIND" = "darwin" ]; then
    UI_PIDS="$(ps -axo pid=,command= | awk -v t1="$DEST/Contents/MacOS/canban-ui" -v t2="$DEST_CANON/Contents/MacOS/canban-ui" '
      {
        pid = $1
        sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "")
        if ($0 == t1 || index($0, t1 " ") == 1 || $0 == t2 || index($0, t2 " ") == 1) {
          print pid
        }
      }
    ')"
  else
    UI_PIDS="$(run_py '
import os, sys
targets = {sys.argv[1], sys.argv[2]}
for entry in os.listdir("/proc"):
    if not entry.isdigit():
        continue
    try:
        exe = os.readlink(f"/proc/{entry}/exe")
    except OSError:
        continue
    if exe.endswith(" (deleted)"):
        exe = exe[:-10]
    if exe in targets:
        print(entry)
' "$DEST/bin/canban-ui" "$DEST_CANON/bin/canban-ui" 2>/dev/null || true)"
  fi

  if [ -n "$UI_PIDS" ]; then
    UI_WAS_RUNNING=1
    log "Closing running Canban UI..."
    for pid in $UI_PIDS; do
      kill -TERM "$pid" 2>/dev/null || true
    done
    waited=0
    while [ "$waited" -lt 150 ]; do
      still_alive=0
      for pid in $UI_PIDS; do
        if proc_alive "$pid"; then
          still_alive=1
          break
        fi
      done
      [ "$still_alive" -eq 0 ] && break
      sleep 0.1
      waited=$((waited + 1))
    done
    for pid in $UI_PIDS; do
      if proc_alive "$pid"; then
        fail "Running Canban UI (PID $pid) did not exit cleanly"
      fi
    done
  fi
fi

# On Linux, if `systemd --user` `canban.service` manages this home and is active,
# stop the unit first so `Restart=always` does not respawn the old daemon mid-upgrade.
if [ "$OS_KIND" = "linux" ] && command -v "$SYSTEMCTL_CMD" >/dev/null 2>&1 && [ -f "$SYSTEMD_UNIT_DIR/canban.service" ]; then
  UNIT_HOME="$(run_py '
import os, pathlib, shlex, sys
unit_path = pathlib.Path(sys.argv[1])
default_home = sys.argv[2]
home_env = os.environ.get("HOME", "")
resolved = default_home
for raw_line in unit_path.read_text(encoding="utf-8", errors="ignore").splitlines():
    line = raw_line.strip()
    if not line.startswith("ExecStart="):
        continue
    cmd = line[len("ExecStart="):].strip().lstrip("-+@!:")
    try:
        tokens = shlex.split(cmd)
    except ValueError:
        tokens = cmd.split()
    for idx, tok in enumerate(tokens):
        if tok == "--home" and idx + 1 < len(tokens):
            resolved = tokens[idx + 1]
            break
        if tok.startswith("--home="):
            resolved = tok.split("=", 1)[1]
            break
if home_env:
    resolved = resolved.replace("%h", home_env)
resolved = os.path.expanduser(resolved)
print(resolved)
' "$SYSTEMD_UNIT_DIR/canban.service" "$DEFAULT_HOME" 2>/dev/null || true)"
  if [ -n "$UNIT_HOME" ] && [ "$(canon_path "$UNIT_HOME")" = "$CANON_TARGET_HOME" ]; then
    if "$SYSTEMCTL_CMD" --user is-active --quiet canban.service 2>/dev/null; then
      SYSTEMD_MANAGED=1
      SERVER_WAS_RUNNING=1
      log "Stopping systemd user service canban.service (running Herdr PTYs stay alive)..."
      "$SYSTEMCTL_CMD" --user stop canban.service || fail "Failed to stop systemd user service canban.service"
    fi
  fi
fi

parse_ping_fields() {
  local ping_json="$1"
  if [ "$USE_JXA" -eq 1 ]; then
    run_jxa '
      function run(argv) {
        var p = JSON.parse(argv[0]);
        return [String(p.pid || ""), String(p.home || "")].join("\n");
      }
    ' "$ping_json" 2>/dev/null || true
  else
    run_py '
import json, sys
p = json.loads(sys.argv[1])
print(str(p.get("pid") or ""))
print(str(p.get("home") or ""))
' "$ping_json" 2>/dev/null || true
  fi
}

SOCK_PATH="$HOME_DIR/app.sock"
if [ -S "$SOCK_PATH" ]; then
  PING_JSON=""
  if PING_JSON="$(env -u CANBAN_ENDPOINT "$CANDIDATE_CLI" --home "$HOME_DIR" call app.ping '{}' 2>/dev/null)"; then
    PING_FIELDS="$(parse_ping_fields "$PING_JSON")"
    OLD_SERVER_PID="$(printf '%s\n' "$PING_FIELDS" | sed -n '1p')"
    OLD_SERVER_HOME="$(printf '%s\n' "$PING_FIELDS" | sed -n '2p')"
    if [ -n "$OLD_SERVER_PID" ] && [ -n "$OLD_SERVER_HOME" ] && [ "$(canon_path "$OLD_SERVER_HOME")" = "$CANON_TARGET_HOME" ]; then
      SERVER_WAS_RUNNING=1
      log "Stopping Canban AppServer (PID $OLD_SERVER_PID; running Herdr PTYs stay alive)..."
      env -u CANBAN_ENDPOINT "$CANDIDATE_CLI" --home "$HOME_DIR" call app.shutdown "{\"expected_pid\":$OLD_SERVER_PID}" >/dev/null 2>&1 || true
      waited=0
      while proc_alive "$OLD_SERVER_PID"; do
        if [ "$waited" -ge 300 ]; then
          fail "Canban AppServer (PID $OLD_SERVER_PID) did not shut down in time"
        fi
        sleep 0.1
        waited=$((waited + 1))
      done
    fi
  fi
fi

restart_old_if_needed() {
  if [ "$NO_RESTART" -eq 1 ]; then
    return 0
  fi
  if [ "$SYSTEMD_MANAGED" -eq 1 ]; then
    "$SYSTEMCTL_CMD" --user start canban.service >/dev/null 2>&1 || true
  fi
  if [ "$UI_WAS_RUNNING" -eq 1 ]; then
    if [ "$OS_KIND" = "darwin" ] && [ -d "$DEST" ]; then
      /usr/bin/open "$DEST" >/dev/null 2>&1 || true
    elif [ "$OS_KIND" = "linux" ] && [ -x "$DEST/bin/canban-ui" ]; then
      env -u HC_SCOPE -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET "$DEST/bin/canban-ui" >/dev/null 2>&1 </dev/null &
    fi
  elif [ "$SYSTEMD_MANAGED" -eq 0 ] && [ "$SERVER_WAS_RUNNING" -eq 1 ] && [ -x "$INSTALLED_CLI" ]; then
    mkdir -p "$HOME_DIR"
    env -u CANBAN_ENDPOINT -u HC_SCOPE -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET "$INSTALLED_CLI" --home "$HOME_DIR" serve >> "$HOME_DIR/server.log" 2>&1 </dev/null &
  fi
}

# Run transactional database migration before swapping the installation if needed.
DB_SCHEMA="$(read_db_schema "$DB_PATH")"
if [ "$DB_SCHEMA" -gt "$MANIFEST_SCHEMA" ]; then
  restart_old_if_needed
  fail "Database schema ($DB_SCHEMA) at $DB_PATH is newer than target release schema ($MANIFEST_SCHEMA); downgrades are not supported."
fi

if [ -f "$DB_PATH" ] && [ "$DB_SCHEMA" -ge 8 ] && [ "$DB_SCHEMA" -lt "$MANIFEST_SCHEMA" ]; then
  log "Migrating Canban database schema ($DB_SCHEMA -> $MANIFEST_SCHEMA)..."
  mkdir -p "$HOME_DIR/updates"
  BACKUP_BASE="$HOME_DIR/updates/backup-before-${TARGET_VERSION}.sqlite"
  if [ -e "$BACKUP_BASE" ]; then
    BACKUP_BASE="$HOME_DIR/updates/backup-before-${TARGET_VERSION}-$(date +%s).sqlite"
  fi
  cp -p "$DB_PATH" "$BACKUP_BASE"
  for suffix in -wal -shm; do
    if [ -f "${DB_PATH}${suffix}" ]; then
      cp -p "${DB_PATH}${suffix}" "${BACKUP_BASE}${suffix}"
    fi
  done
  if ! env -u CANBAN_ENDPOINT "$CANDIDATE_CLI" --home "$HOME_DIR" database --target "$MANIFEST_SCHEMA" >/dev/null; then
    restart_old_if_needed
    fail "Database migration to schema $MANIFEST_SCHEMA failed; backup preserved at $BACKUP_BASE and existing installation at $INSTALLED_LOCATION was left unchanged."
  fi
fi

FINAL_CLI=""
FINAL_DISPLAY=""

if [ "$OS_KIND" = "darwin" ]; then
  DEST_PARENT="$(dirname -- "$DEST")"
  DEST_NAME="$(basename -- "$DEST")"
  if [ ! -d "$DEST_PARENT" ]; then
    mkdir -p "$DEST_PARENT" 2>/dev/null || /usr/bin/sudo mkdir -p "$DEST_PARENT"
  fi
  DEST_PARENT_CANON="$(canon_path "$DEST_PARENT")"
  DEST_TARGET="$DEST_PARENT_CANON/$DEST_NAME"

  swap_macos_bundle() {
    local use_sudo="$1"
    local cmd_prefix=""
    if [ "$use_sudo" = "1" ]; then
      cmd_prefix="/usr/bin/sudo"
    fi

    local stage
    stage="$($cmd_prefix mktemp -d "$DEST_PARENT_CANON/.Canban-install-XXXXXX")" || fail "Could not create staging directory in $DEST_PARENT_CANON"
    STAGE_DIR="$stage"
    local staged_app="$stage/Canban.app"
    local backup_app="$stage/Canban-previous.app"

    if ! $cmd_prefix /usr/bin/ditto "$CANDIDATE" "$staged_app"; then
      $cmd_prefix rm -rf -- "$stage" 2>/dev/null || true
      STAGE_DIR=""
      fail "Failed to copy Canban.app into $DEST_PARENT_CANON"
    fi
    $cmd_prefix /usr/bin/xattr -dr com.apple.quarantine "$staged_app" 2>/dev/null || true
    if ! $cmd_prefix /usr/bin/codesign --verify --deep --strict "$staged_app" >/dev/null 2>&1; then
      $cmd_prefix rm -rf -- "$stage" 2>/dev/null || true
      STAGE_DIR=""
      fail "Staged signature verification failed"
    fi

    local had_existing=0
    if [ -e "$DEST_TARGET" ] || [ -L "$DEST_TARGET" ]; then
      had_existing=1
      if ! $cmd_prefix mv "$DEST_TARGET" "$backup_app"; then
        $cmd_prefix rm -rf -- "$stage" 2>/dev/null || true
        STAGE_DIR=""
        fail "Failed to move existing $DEST_TARGET aside"
      fi
    fi

    if ! $cmd_prefix mv "$staged_app" "$DEST_TARGET"; then
      if [ "$had_existing" -eq 1 ] && [ -e "$backup_app" ] && [ ! -e "$DEST_TARGET" ]; then
        $cmd_prefix mv "$backup_app" "$DEST_TARGET" 2>/dev/null || true
      fi
      $cmd_prefix rm -rf -- "$stage" 2>/dev/null || true
      STAGE_DIR=""
      fail "Failed to install Canban.app to $DEST_TARGET"
    fi

    $cmd_prefix touch "$DEST_TARGET" 2>/dev/null || true
    local lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    if [ -x "$lsregister" ]; then
      "$lsregister" -f "$DEST_TARGET" >/dev/null 2>&1 || true
    fi
    $cmd_prefix rm -rf -- "$stage" 2>/dev/null || true
    STAGE_DIR=""
  }

  if [ -w "$DEST_PARENT_CANON" ] && { [ ! -e "$DEST_TARGET" ] || [ -w "$DEST_TARGET" ] || [ -w "$DEST_PARENT_CANON" ]; }; then
    swap_macos_bundle 0
  else
    log "Administrator privileges required to install into $DEST_PARENT_CANON"
    swap_macos_bundle 1
  fi
  FINAL_CLI="$DEST_TARGET/Contents/MacOS/canban"
  FINAL_DISPLAY="$DEST_TARGET"
elif [ "$INSTALL_MODE" = "headless" ]; then
  mkdir -p "$BIN_DIR"
  BIN_DIR_CANON="$(canon_path "$BIN_DIR")"
  CLI_STAGE="$(mktemp "$BIN_DIR_CANON/.canban-cli-XXXXXX")" || fail "Could not create staging file in $BIN_DIR_CANON"
  cp "$CANDIDATE_CLI" "$CLI_STAGE"
  chmod 0755 "$CLI_STAGE"
  if [ -d "$BIN_DIR_CANON/canban" ] && [ ! -L "$BIN_DIR_CANON/canban" ]; then
    rm -f -- "$CLI_STAGE"
    fail "Refusing to overwrite directory $BIN_DIR_CANON/canban"
  fi
  mv -f "$CLI_STAGE" "$BIN_DIR_CANON/canban"
  FINAL_CLI="$BIN_DIR_CANON/canban"
  FINAL_DISPLAY="$FINAL_CLI"
else
  DEST_PARENT="$(dirname -- "$DEST")"
  DEST_NAME="$(basename -- "$DEST")"
  mkdir -p "$DEST_PARENT"
  DEST_PARENT_CANON="$(canon_path "$DEST_PARENT")"
  DEST_TARGET="$DEST_PARENT_CANON/$DEST_NAME"

  STAGE_DIR="$(mktemp -d "$DEST_PARENT_CANON/.canban-install-XXXXXX")" || fail "Could not create staging directory in $DEST_PARENT_CANON"
  STAGED_BUNDLE="$STAGE_DIR/canban"
  BACKUP_BUNDLE="$STAGE_DIR/canban-previous"

  cp -R "$CANDIDATE" "$STAGED_BUNDLE" || fail "Failed to copy bundle into $DEST_PARENT_CANON"
  for bin_name in canban canban-ui canban-updater; do
    chmod 0755 "$STAGED_BUNDLE/bin/$bin_name"
  done

  HAD_EXISTING=0
  if [ -e "$DEST_TARGET" ] || [ -L "$DEST_TARGET" ]; then
    HAD_EXISTING=1
    mv "$DEST_TARGET" "$BACKUP_BUNDLE" || fail "Failed to move existing $DEST_TARGET aside"
  fi
  if ! mv "$STAGED_BUNDLE" "$DEST_TARGET"; then
    if [ "$HAD_EXISTING" -eq 1 ] && [ -e "$BACKUP_BUNDLE" ] && [ ! -e "$DEST_TARGET" ]; then
      mv "$BACKUP_BUNDLE" "$DEST_TARGET" 2>/dev/null || true
    fi
    fail "Failed to install Canban bundle to $DEST_TARGET"
  fi
  touch "$DEST_TARGET" 2>/dev/null || true
  rm -rf -- "$STAGE_DIR" 2>/dev/null || true
  STAGE_DIR=""

  mkdir -p "$BIN_DIR"
  BIN_DIR_CANON="$(canon_path "$BIN_DIR")"
  for link_name in canban canban-ui; do
    if [ -d "$BIN_DIR_CANON/$link_name" ] && [ ! -L "$BIN_DIR_CANON/$link_name" ]; then
      fail "Refusing to overwrite directory $BIN_DIR_CANON/$link_name with symlink"
    fi
    TMP_LINK="$BIN_DIR_CANON/.${link_name}-link-$$"
    rm -f -- "$TMP_LINK"
    ln -s "$DEST_TARGET/bin/$link_name" "$TMP_LINK"
    mv -f "$TMP_LINK" "$BIN_DIR_CANON/$link_name"
  done

  mkdir -p "$XDG_DATA_DIR"
  XDG_CANON="$(canon_path "$XDG_DATA_DIR")"
  run_py '
import os, pathlib, tempfile, sys

dest_target = pathlib.Path(sys.argv[1])
xdg_dir = pathlib.Path(sys.argv[2])

def atomic_write(path: pathlib.Path, content: bytes):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}-", delete=False) as f:
        f.write(content)
        tmp = pathlib.Path(f.name)
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)

raw_exec = str(dest_target / "bin/canban-ui")
if any(ch in raw_exec for ch in (" ", "\t", "\"", "\x27", "\\", "`", "$", ">", "<", "~", "|", "&", ";", "*", "?", "#", "(", ")")):
    esc = raw_exec.replace("\\", "\\\\").replace("\"", "\\\"").replace("`", "\\`").replace("$", "\\$")
    exec_field = "\"" + esc + "\""
else:
    exec_field = raw_exec

desktop_text = "\n".join([
    "[Desktop Entry]",
    "Type=Application",
    "Name=Canban",
    "Comment=AI-native Kanban workbench with Git worktree workflows and Herdr PTY terminals",
    f"Exec={exec_field}",
    "Icon=com.canban.desktop",
    "Terminal=false",
    "Categories=Development;",
    "StartupWMClass=com.canban.desktop",
    "",
])
atomic_write(xdg_dir / "applications/com.canban.desktop.desktop", desktop_text.encode("utf-8"))

for rel in (
    "icons/hicolor/scalable/apps/com.canban.desktop.svg",
    "icons/hicolor/256x256/apps/com.canban.desktop.png",
    "icons/hicolor/512x512/apps/com.canban.desktop.png",
):
    src = dest_target / "share" / rel
    if src.is_file():
        atomic_write(xdg_dir / rel, src.read_bytes())
' "$DEST_TARGET" "$XDG_CANON"

  DEFAULT_USER_XDG="$(canon_path "${XDG_DATA_HOME:-${HOME}/.local/share}")"
  if [ "$XDG_CANON" = "$DEFAULT_USER_XDG" ]; then
    if command -v update-desktop-database >/dev/null 2>&1; then
      update-desktop-database "$XDG_CANON/applications" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
      gtk-update-icon-cache -f -t "$XDG_CANON/icons/hicolor" >/dev/null 2>&1 || true
    fi
  fi

  FINAL_CLI="$DEST_TARGET/bin/canban"
  FINAL_DISPLAY="$DEST_TARGET"
fi

# Reset stale in-app update banner state if updates/state.json already exists.
STATE_JSON="$HOME_DIR/updates/state.json"
if [ -f "$STATE_JSON" ]; then
  STATE_TMP="$HOME_DIR/updates/.state-tmp-$$.json"
  if [ "$USE_JXA" -eq 1 ]; then
    if run_jxa '
      ObjC.import("Foundation");
      function run(argv) {
        var data = $.NSData.dataWithContentsOfFile(argv[0]);
        if (!data) return "SKIP";
        var raw = $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding);
        if (!raw) return "SKIP";
        var s = JSON.parse(raw.js);
        s.phase = "current";
        s.available = null;
        s.progress = 0;
        s.error = null;
        var out = $.NSString.alloc.initWithUTF8String(JSON.stringify(s, null, 2) + "\n");
        if (!out.writeToFileAtomicallyEncodingError(argv[1], true, $.NSUTF8StringEncoding, null)) {
          throw new Error("Failed writing updated state.json");
        }
        return "OK";
      }
    ' "$STATE_JSON" "$STATE_TMP" >/dev/null 2>&1 && [ -f "$STATE_TMP" ]; then
      mv "$STATE_TMP" "$STATE_JSON"
    else
      rm -f -- "$STATE_TMP" 2>/dev/null || true
    fi
  else
    if run_py '
import json, pathlib, sys
p, tmp = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
s = json.loads(p.read_text(encoding="utf-8"))
s["phase"] = "current"
s["available"] = None
s["progress"] = 0
s["error"] = None
tmp.write_text(json.dumps(s, indent=2) + "\n", encoding="utf-8")
' "$STATE_JSON" "$STATE_TMP" >/dev/null 2>&1 && [ -f "$STATE_TMP" ]; then
      mv "$STATE_TMP" "$STATE_JSON"
    else
      rm -f -- "$STATE_TMP" 2>/dev/null || true
    fi
  fi
fi

wait_for_server_ping() {
  local waited=0
  while ! env -u CANBAN_ENDPOINT "$FINAL_CLI" --home "$HOME_DIR" call app.ping '{}' >/dev/null 2>&1; do
    if [ "$waited" -ge 150 ]; then
      fail "Upgraded AppServer did not respond on $HOME_DIR/app.sock"
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
}

if [ "$NO_RESTART" -eq 0 ]; then
  if [ "$SYSTEMD_MANAGED" -eq 1 ]; then
    log "Starting systemd user service canban.service..."
    "$SYSTEMCTL_CMD" --user start canban.service || fail "Failed to start systemd user service canban.service"
    wait_for_server_ping
    if [ "$UI_WAS_RUNNING" -eq 1 ] && [ "$OS_KIND" = "linux" ] && [ -x "$DEST_TARGET/bin/canban-ui" ]; then
      log "Reopening Canban..."
      env -u HC_SCOPE -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET "$DEST_TARGET/bin/canban-ui" >/dev/null 2>&1 </dev/null &
    fi
  elif [ "$UI_WAS_RUNNING" -eq 1 ]; then
    log "Reopening Canban..."
    if [ "$OS_KIND" = "darwin" ]; then
      /usr/bin/open "$DEST_TARGET"
    else
      env -u HC_SCOPE -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET "$DEST_TARGET/bin/canban-ui" >/dev/null 2>&1 </dev/null &
    fi
  elif [ "$SERVER_WAS_RUNNING" -eq 1 ]; then
    log "Restarting Canban AppServer..."
    mkdir -p "$HOME_DIR"
    env -u CANBAN_ENDPOINT -u HC_SCOPE -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET "$FINAL_CLI" --home "$HOME_DIR" serve >> "$HOME_DIR/server.log" 2>&1 </dev/null &
    wait_for_server_ping
  fi
fi

log "Installed Canban $TARGET_VERSION to $FINAL_DISPLAY"

if [ "$OS_KIND" = "linux" ] && [ "$CUSTOM_DEST" -eq 0 ] && [ "$CUSTOM_BIN_DIR" -eq 0 ]; then
  case ":${PATH:-}:" in
    *":$BIN_DIR_CANON:"*|*":${HOME}/.local/bin:"*) ;;
    *)
      log "Note: Add $BIN_DIR_CANON to your PATH to run \`canban\` and \`canban-ui\` from the terminal."
      ;;
  esac
fi

if ! command -v hc >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/hc" ] && [ ! -x /opt/homebrew/bin/hc ] && [ ! -x /usr/local/bin/hc ]; then
  log ""
  log "Note: Herdr Core (hc) was not detected in PATH."
  log "Canban can install it automatically on first launch, or you can install it now with:"
  log "  curl -fsSL https://releases.herdr.dev/core/install.sh | sh"
fi
