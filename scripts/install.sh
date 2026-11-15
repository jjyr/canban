#!/bin/bash
if [ -z "${BASH_VERSION:-}" ]; then
  printf 'canban install error: bash is required (run: curl -fsSL https://canban.dev/install.sh | bash)\n' >&2
  exit 1
fi
set -euo pipefail

# Canban macOS installer and upgrader.
# Safe to run on fresh Macs (no Xcode Command Line Tools required) and over existing installations.

DEFAULT_REPO="jjyr/canban"
DEFAULT_API="https://api.github.com"
DEFAULT_DEST="/Applications/Canban.app"
DEFAULT_HOME="${HOME}/.canban"

REPO="${CANBAN_INSTALL_REPO:-$DEFAULT_REPO}"
API="${CANBAN_INSTALL_API:-$DEFAULT_API}"
API="${API%/}"
DEST="${CANBAN_INSTALL_DEST:-$DEFAULT_DEST}"
HOME_DIR="${CANBAN_HOME:-$DEFAULT_HOME}"
TARGET_VERSION=""
FORCE=0
NO_RESTART=0

TMP_DIR=""
STAGE_DIR=""

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Install or upgrade Canban.app on macOS (Apple Silicon / arm64).

Options:
  --version <YYYYMMDD-N>   Install a specific release tag (default: latest release)
  --dest <path>            Destination bundle path (default: /Applications/Canban.app)
  --home <path>            Canban data home directory (default: $CANBAN_HOME or ~/.canban)
  --force                  Reinstall even if the same version (or a newer version) is installed
  --no-restart             Do not restart Canban UI or AppServer after upgrading
  -h, --help               Show this help message

Environment overrides:
  CANBAN_INSTALL_REPO      GitHub owner/repo (default: jjyr/canban)
  CANBAN_INSTALL_API       GitHub API base URL (default: https://api.github.com)
  CANBAN_INSTALL_DEST      Default destination path
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
    /bin/rm -rf -- "$STAGE_DIR" 2>/dev/null || true
  fi
  if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
    /bin/rm -rf -- "$TMP_DIR" 2>/dev/null || true
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
      shift 2
      ;;
    --home)
      [ $# -ge 2 ] || fail "--home requires an argument"
      HOME_DIR="$2"
      shift 2
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

[ "$(uname -s)" = "Darwin" ] || fail "Canban currently supports macOS only (got $(uname -s))"
[ "$(uname -m)" = "arm64" ] || fail "Canban currently supports Apple Silicon (arm64) only (got $(uname -m))"

LIVE_HOME="${CANBAN_HOME:-$DEFAULT_HOME}"
CANON_TARGET_HOME="$(canon_path "$HOME_DIR")"
CANON_LIVE_HOME="$(canon_path "$LIVE_HOME")"
if [ -n "${CANBAN_CARD:-}" ] && [ -z "${CANBAN_INSTALL_ALLOW_AGENT:-}" ] && [ "$CANON_TARGET_HOME" = "$CANON_LIVE_HOME" ]; then
  fail "Refusing to replace Canban from inside a Card Agent session targeting the live home ($CANON_TARGET_HOME)"
fi

if [ -e "$DEST/Contents/Resources/dev-instance.json" ]; then
  fail "Refusing to overwrite a branch-bound development bundle at $DEST"
fi

run_jxa() {
  /usr/bin/osascript -l JavaScript -e "$1" -- "${@:2}"
}

check_install_journal() {
  local journal="$HOME_DIR/updates/install.json"
  if [ ! -e "$journal" ] && [ ! -L "$journal" ]; then
    return 0
  fi
  local status
  status="$(run_jxa '
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
  ' "$journal")" || status="ERROR"

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
  sz="$(/usr/bin/wc -c < "$db" | /usr/bin/tr -d ' ')"
  if [ "${sz:-0}" -lt 64 ]; then
    printf '0\n'
    return 0
  fi
  /usr/bin/od -An -tu1 -j 60 -N 4 "$db" | /usr/bin/awk 'NF == 4 { printf "%u\n", (($1 * 256 + $2) * 256 + $3) * 256 + $4 }'
}

check_install_journal

TMP_DIR="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/canban-install.XXXXXX")"

log "Checking Canban releases from $REPO..."
RELEASES_JSON="$TMP_DIR/releases.json"
/usr/bin/curl -fsSL --retry 2 \
  -H "Accept: application/vnd.github+json" \
  -H "User-Agent: Canban-Installer" \
  "$API/repos/$REPO/releases?per_page=100" \
  -o "$RELEASES_JSON" || fail "Failed to fetch releases from $API/repos/$REPO/releases"

SELECTED="$(run_jxa '
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
    return typeof u === "string" && (u.indexOf("https://") === 0 || u.indexOf("http://127.0.0.1:") === 0 || u.indexOf("http://localhost:") === 0);
  }
  function run(argv) {
    var releases = JSON.parse(readText(argv[0]));
    var target = argv[1] || "";
    if (!Array.isArray(releases)) throw new Error("Unexpected releases payload");
    if (target && !parseVersion(target)) throw new Error("Invalid --version format (expected YYYYMMDD-N): " + target);
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
        if (a.name === "manifest-macos-aarch64.json") manifest = a;
        else if (a.name === "Canban-macos-aarch64.tar.gz") gz = a;
        else if (a.name === "Canban-macos-aarch64.tar.zst") zst = a;
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
      throw new Error(target ? ("Release " + target + " with macOS arm64 assets not found") : "No published Canban macOS arm64 release found");
    }
    var pick = candidates[0];
    if (!validUrl(pick.manifestUrl) || !validUrl(pick.archiveUrl)) {
      throw new Error("Release asset URL is not HTTPS");
    }
    return [pick.version, pick.archiveKind, pick.manifestUrl, pick.archiveUrl].join("\n");
  }
' "$RELEASES_JSON" "$TARGET_VERSION")" || fail "Could not select a release"

TARGET_VERSION="$(printf '%s\n' "$SELECTED" | /usr/bin/sed -n '1p')"
ARCHIVE_KIND="$(printf '%s\n' "$SELECTED" | /usr/bin/sed -n '2p')"
MANIFEST_URL="$(printf '%s\n' "$SELECTED" | /usr/bin/sed -n '3p')"
ARCHIVE_URL="$(printf '%s\n' "$SELECTED" | /usr/bin/sed -n '4p')"

MANIFEST_JSON="$TMP_DIR/manifest.json"
/usr/bin/curl -fsSL --retry 2 -H "User-Agent: Canban-Installer" "$MANIFEST_URL" -o "$MANIFEST_JSON" \
  || fail "Failed to download manifest from $MANIFEST_URL"

MANIFEST_FIELDS="$(run_jxa '
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
    if (m.version !== expectedVersion) throw new Error("Manifest version mismatch: " + m.version + " != " + expectedVersion);
    if (m.platform !== "macos-aarch64") throw new Error("Unexpected manifest platform: " + m.platform);
    if (typeof m.commit !== "string" || !/^[0-9a-fA-F]{40}$/.test(m.commit)) throw new Error("Invalid manifest commit");
    if (!Number.isInteger(m.schema) || m.schema < 8) throw new Error("Invalid manifest schema");
    if (!Number.isInteger(m.api) || m.api < 1) throw new Error("Invalid manifest api");
    var asset, size, sha;
    if (kind === "tar.gz") {
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
' "$MANIFEST_JSON" "$TARGET_VERSION" "$ARCHIVE_KIND")" || fail "Invalid release manifest"

MANIFEST_COMMIT="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '2p')"
MANIFEST_SCHEMA="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '3p')"
MANIFEST_API="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '4p')"
ARCHIVE_NAME="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '5p')"
EXPECTED_SIZE="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '6p')"
EXPECTED_SHA256="$(printf '%s\n' "$MANIFEST_FIELDS" | /usr/bin/sed -n '7p')"

# Check existing installation before downloading large archive.
if [ -x "$DEST/Contents/MacOS/canban" ]; then
  INSTALLED_INFO=""
  if INSTALLED_INFO="$(env -u CANBAN_ENDPOINT "$DEST/Contents/MacOS/canban" build-info 2>/dev/null)"; then
    CHECK_INSTALLED="$(run_jxa '
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
    ' "$INSTALLED_INFO" "$TARGET_VERSION" "$MANIFEST_SCHEMA" "$FORCE")" || fail "Could not inspect installed Canban version"

    case "$CHECK_INSTALLED" in
      UP_TO_DATE:*)
        log "Canban ${CHECK_INSTALLED#UP_TO_DATE:} is already up to date at $DEST"
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
/usr/bin/curl -fsSL --retry 2 -H "User-Agent: Canban-Installer" "$ARCHIVE_URL" -o "$ARCHIVE_PATH" \
  || fail "Failed to download archive from $ARCHIVE_URL"

ACTUAL_SIZE="$(/usr/bin/wc -c < "$ARCHIVE_PATH" | /usr/bin/tr -d ' ')"
[ "$ACTUAL_SIZE" = "$EXPECTED_SIZE" ] || fail "Archive size mismatch: expected $EXPECTED_SIZE bytes, got $ACTUAL_SIZE"

ACTUAL_SHA256="$(/usr/bin/shasum -a 256 "$ARCHIVE_PATH" | /usr/bin/awk '{print tolower($1)}')"
[ "$ACTUAL_SHA256" = "$EXPECTED_SHA256" ] || fail "Archive SHA-256 mismatch: expected $EXPECTED_SHA256, got $ACTUAL_SHA256"

ZSTD_BIN=""
if [ "$ARCHIVE_KIND" = "tar.zst" ]; then
  for candidate in "$(command -v zstd 2>/dev/null || true)" /opt/homebrew/bin/zstd /usr/local/bin/zstd; do
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
    /usr/bin/tar -tf "$ARCHIVE_PATH" > "$list_file" || fail "Failed to list archive contents"
    /usr/bin/tar -tvf "$ARCHIVE_PATH" > "$verbose_file" || fail "Failed to inspect archive metadata"
  else
    "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | /usr/bin/tar -tf - > "$list_file" || fail "Failed to list zst archive contents"
    "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | /usr/bin/tar -tvf - > "$verbose_file" || fail "Failed to inspect zst archive metadata"
  fi
  [ -s "$list_file" ] || fail "Archive is empty"
  /usr/bin/awk '
    {
      if ($0 ~ /^\// || $0 == ".." || $0 ~ /(^|\/)\.\.($|\/)/) { exit 1 }
      if ($0 !~ /^Canban\.app(\/|$)/) { exit 1 }
    }
  ' "$list_file" || fail "Unsafe archive entry path"
  /usr/bin/awk '
    NF > 0 && $1 !~ /^[-d]/ { exit 1 }
  ' "$verbose_file" || fail "Archive contains an unsupported symlink or special file"
}

inspect_archive

EXTRACT_DIR="$TMP_DIR/extract"
/bin/mkdir -p "$EXTRACT_DIR"
if [ "$ARCHIVE_KIND" = "tar.gz" ]; then
  /usr/bin/tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR" || fail "Failed to extract $ARCHIVE_NAME"
else
  "$ZSTD_BIN" -dc "$ARCHIVE_PATH" | /usr/bin/tar -xf - -C "$EXTRACT_DIR" || fail "Failed to extract $ARCHIVE_NAME"
fi

CANDIDATE="$EXTRACT_DIR/Canban.app"
[ -d "$CANDIDATE" ] || fail "Extracted archive did not contain Canban.app"

UNSAFE_NODES="$(/usr/bin/find "$CANDIDATE" ! -type f ! -type d -print)"
[ -z "$UNSAFE_NODES" ] || fail "Extracted bundle contains unsafe symlinks or special files"

[ -f "$CANDIDATE/Contents/Info.plist" ] || fail "Missing Contents/Info.plist in extracted bundle"
BUNDLE_ID="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$CANDIDATE/Contents/Info.plist" 2>/dev/null || true)"
[ "$BUNDLE_ID" = "com.canban.desktop" ] || fail "Unexpected CFBundleIdentifier: $BUNDLE_ID"
[ ! -e "$CANDIDATE/Contents/Resources/dev-instance.json" ] || fail "Downloaded bundle unexpectedly contains dev-instance.json"

for bin_name in canban canban-ui canban-updater; do
  [ -f "$CANDIDATE/Contents/MacOS/$bin_name" ] || fail "Missing Contents/MacOS/$bin_name in extracted bundle"
  /bin/chmod +x "$CANDIDATE/Contents/MacOS/$bin_name"
done

log "Clearing quarantine attributes and signing bundle..."
/usr/bin/xattr -dr com.apple.quarantine "$CANDIDATE" 2>/dev/null || true
/usr/bin/codesign --force --deep --sign - --timestamp=none "$CANDIDATE" >/dev/null 2>&1 || fail "codesign failed on $CANDIDATE"
/usr/bin/codesign --verify --deep --strict "$CANDIDATE" >/dev/null 2>&1 || fail "codesign verification failed on $CANDIDATE"

CANDIDATE_INFO="$(env -u CANBAN_ENDPOINT "$CANDIDATE/Contents/MacOS/canban" build-info)" \
  || fail "Failed to execute candidate canban build-info"

run_jxa '
  function run(argv) {
    var b = JSON.parse(argv[0]);
    if (b.version !== argv[1]) throw new Error("Candidate version mismatch: " + b.version);
    if (b.commit !== argv[2]) throw new Error("Candidate commit mismatch: " + b.commit);
    if (String(b.schema) !== argv[3]) throw new Error("Candidate schema mismatch: " + b.schema);
    if (String(b.api) !== argv[4]) throw new Error("Candidate api mismatch: " + b.api);
    if (b.platform !== "macos-aarch64") throw new Error("Candidate platform mismatch: " + b.platform);
    return "OK";
  }
' "$CANDIDATE_INFO" "$TARGET_VERSION" "$MANIFEST_COMMIT" "$MANIFEST_SCHEMA" "$MANIFEST_API" >/dev/null \
  || fail "Candidate build-info did not match release manifest"

# Upgrade coordination: gracefully stop existing UI and AppServer while preserving herdr-core PTYs.
proc_alive() {
  local pid="$1"
  local st
  st="$(/bin/ps -p "$pid" -o state= 2>/dev/null | /usr/bin/tr -d '[:space:]')" || return 1
  [ -n "$st" ] || return 1
  case "$st" in
    Z*) return 1 ;;
  esac
  return 0
}

UI_WAS_RUNNING=0
SERVER_WAS_RUNNING=0

DEST_CANON="$(canon_path "$DEST")"
UI_PIDS="$(/bin/ps -axo pid=,command= | /usr/bin/awk -v t1="$DEST/Contents/MacOS/canban-ui" -v t2="$DEST_CANON/Contents/MacOS/canban-ui" '
  {
    pid = $1
    sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "")
    if ($0 == t1 || index($0, t1 " ") == 1 || $0 == t2 || index($0, t2 " ") == 1) {
      print pid
    }
  }
')"

if [ -n "$UI_PIDS" ]; then
  UI_WAS_RUNNING=1
  log "Closing running Canban UI..."
  for pid in $UI_PIDS; do
    /bin/kill -TERM "$pid" 2>/dev/null || true
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
    /bin/sleep 0.1
    waited=$((waited + 1))
  done
  for pid in $UI_PIDS; do
    if proc_alive "$pid"; then
      fail "Running Canban UI (PID $pid) did not exit cleanly"
    fi
  done
fi

SOCK_PATH="$HOME_DIR/app.sock"
if [ -S "$SOCK_PATH" ]; then
  PING_JSON=""
  if PING_JSON="$(env -u CANBAN_ENDPOINT "$CANDIDATE/Contents/MacOS/canban" --home "$HOME_DIR" call app.ping '{}' 2>/dev/null)"; then
    PING_FIELDS="$(run_jxa '
      function run(argv) {
        var p = JSON.parse(argv[0]);
        return [String(p.pid || ""), String(p.home || "")].join("\n");
      }
    ' "$PING_JSON" 2>/dev/null || true)"
    OLD_SERVER_PID="$(printf '%s\n' "$PING_FIELDS" | /usr/bin/sed -n '1p')"
    OLD_SERVER_HOME="$(printf '%s\n' "$PING_FIELDS" | /usr/bin/sed -n '2p')"
    if [ -n "$OLD_SERVER_PID" ] && [ -n "$OLD_SERVER_HOME" ] && [ "$(canon_path "$OLD_SERVER_HOME")" = "$CANON_TARGET_HOME" ]; then
      SERVER_WAS_RUNNING=1
      log "Stopping Canban AppServer (PID $OLD_SERVER_PID; running Herdr PTYs stay alive)..."
      env -u CANBAN_ENDPOINT "$CANDIDATE/Contents/MacOS/canban" --home "$HOME_DIR" call app.shutdown "{\"expected_pid\":$OLD_SERVER_PID}" >/dev/null 2>&1 || true
      waited=0
      while proc_alive "$OLD_SERVER_PID"; do
        if [ "$waited" -ge 300 ]; then
          fail "Canban AppServer (PID $OLD_SERVER_PID) did not shut down in time"
        fi
        /bin/sleep 0.1
        waited=$((waited + 1))
      done
    fi
  fi
fi

restart_old_if_needed() {
  if [ "$NO_RESTART" -eq 1 ]; then
    return 0
  fi
  if [ "$UI_WAS_RUNNING" -eq 1 ] && [ -d "$DEST" ]; then
    /usr/bin/open "$DEST" >/dev/null 2>&1 || true
  elif [ "$SERVER_WAS_RUNNING" -eq 1 ] && [ -x "$DEST/Contents/MacOS/canban" ]; then
    /bin/mkdir -p "$HOME_DIR"
    env -u CANBAN_ENDPOINT "$DEST/Contents/MacOS/canban" --home "$HOME_DIR" serve >> "$HOME_DIR/server.log" 2>&1 </dev/null &
  fi
}

# Run transactional database migration before swapping the App bundle if needed.
DB_SCHEMA="$(read_db_schema "$DB_PATH")"
if [ "$DB_SCHEMA" -gt "$MANIFEST_SCHEMA" ]; then
  restart_old_if_needed
  fail "Database schema ($DB_SCHEMA) at $DB_PATH is newer than target release schema ($MANIFEST_SCHEMA); downgrades are not supported."
fi

if [ -f "$DB_PATH" ] && [ "$DB_SCHEMA" -ge 8 ] && [ "$DB_SCHEMA" -lt "$MANIFEST_SCHEMA" ]; then
  log "Migrating Canban database schema ($DB_SCHEMA -> $MANIFEST_SCHEMA)..."
  /bin/mkdir -p "$HOME_DIR/updates"
  BACKUP_BASE="$HOME_DIR/updates/backup-before-${TARGET_VERSION}.sqlite"
  if [ -e "$BACKUP_BASE" ]; then
    BACKUP_BASE="$HOME_DIR/updates/backup-before-${TARGET_VERSION}-$(/bin/date +%s).sqlite"
  fi
  /bin/cp -p "$DB_PATH" "$BACKUP_BASE"
  for suffix in -wal -shm; do
    if [ -f "${DB_PATH}${suffix}" ]; then
      /bin/cp -p "${DB_PATH}${suffix}" "${BACKUP_BASE}${suffix}"
    fi
  done
  if ! env -u CANBAN_ENDPOINT "$CANDIDATE/Contents/MacOS/canban" --home "$HOME_DIR" database --target "$MANIFEST_SCHEMA" >/dev/null; then
    restart_old_if_needed
    fail "Database migration to schema $MANIFEST_SCHEMA failed; backup preserved at $BACKUP_BASE and existing installation at $DEST was left unchanged."
  fi
fi

DEST_PARENT="$(dirname -- "$DEST")"
DEST_NAME="$(basename -- "$DEST")"
if [ ! -d "$DEST_PARENT" ]; then
  /bin/mkdir -p "$DEST_PARENT" 2>/dev/null || /usr/bin/sudo /bin/mkdir -p "$DEST_PARENT"
fi
DEST_PARENT_CANON="$(canon_path "$DEST_PARENT")"
DEST_TARGET="$DEST_PARENT_CANON/$DEST_NAME"

swap_bundle() {
  local use_sudo="$1"
  local cmd_prefix=""
  if [ "$use_sudo" = "1" ]; then
    cmd_prefix="/usr/bin/sudo"
  fi

  local stage
  stage="$($cmd_prefix /usr/bin/mktemp -d "$DEST_PARENT_CANON/.Canban-install-XXXXXX")" || fail "Could not create staging directory in $DEST_PARENT_CANON"
  STAGE_DIR="$stage"
  local staged_app="$stage/Canban.app"
  local backup_app="$stage/Canban-previous.app"

  if ! $cmd_prefix /usr/bin/ditto "$CANDIDATE" "$staged_app"; then
    $cmd_prefix /bin/rm -rf -- "$stage" 2>/dev/null || true
    STAGE_DIR=""
    fail "Failed to copy Canban.app into $DEST_PARENT_CANON"
  fi
  $cmd_prefix /usr/bin/xattr -dr com.apple.quarantine "$staged_app" 2>/dev/null || true
  if ! $cmd_prefix /usr/bin/codesign --verify --deep --strict "$staged_app" >/dev/null 2>&1; then
    $cmd_prefix /bin/rm -rf -- "$stage" 2>/dev/null || true
    STAGE_DIR=""
    fail "Staged signature verification failed"
  fi

  local had_existing=0
  if [ -e "$DEST_TARGET" ] || [ -L "$DEST_TARGET" ]; then
    had_existing=1
    if ! $cmd_prefix /bin/mv "$DEST_TARGET" "$backup_app"; then
      $cmd_prefix /bin/rm -rf -- "$stage" 2>/dev/null || true
      STAGE_DIR=""
      fail "Failed to move existing $DEST_TARGET aside"
    fi
  fi

  if ! $cmd_prefix /bin/mv "$staged_app" "$DEST_TARGET"; then
    if [ "$had_existing" -eq 1 ] && [ -e "$backup_app" ] && [ ! -e "$DEST_TARGET" ]; then
      $cmd_prefix /bin/mv "$backup_app" "$DEST_TARGET" 2>/dev/null || true
    fi
    $cmd_prefix /bin/rm -rf -- "$stage" 2>/dev/null || true
    STAGE_DIR=""
    fail "Failed to install Canban.app to $DEST_TARGET"
  fi

  $cmd_prefix /usr/bin/touch "$DEST_TARGET" 2>/dev/null || true
  local lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
  if [ -x "$lsregister" ]; then
    "$lsregister" -f "$DEST_TARGET" >/dev/null 2>&1 || true
  fi
  $cmd_prefix /bin/rm -rf -- "$stage" 2>/dev/null || true
  STAGE_DIR=""
}

if [ -w "$DEST_PARENT_CANON" ] && { [ ! -e "$DEST_TARGET" ] || [ -w "$DEST_TARGET" ] || [ -w "$DEST_PARENT_CANON" ]; }; then
  swap_bundle 0
else
  log "Administrator privileges required to install into $DEST_PARENT_CANON"
  swap_bundle 1
fi

# Reset stale in-app update banner state if updates/state.json already exists.
STATE_JSON="$HOME_DIR/updates/state.json"
if [ -f "$STATE_JSON" ]; then
  STATE_TMP="$HOME_DIR/updates/.state-tmp-$$.json"
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
  ' "$STATE_JSON" "$STATE_TMP" >/dev/null 2>&1; then
    if [ -f "$STATE_TMP" ]; then
      /bin/mv "$STATE_TMP" "$STATE_JSON"
    fi
  else
    /bin/rm -f -- "$STATE_TMP" 2>/dev/null || true
  fi
fi

if [ "$NO_RESTART" -eq 0 ]; then
  if [ "$UI_WAS_RUNNING" -eq 1 ]; then
    log "Reopening Canban..."
    /usr/bin/open "$DEST_TARGET"
  elif [ "$SERVER_WAS_RUNNING" -eq 1 ]; then
    log "Restarting Canban AppServer..."
    /bin/mkdir -p "$HOME_DIR"
    env -u CANBAN_ENDPOINT "$DEST_TARGET/Contents/MacOS/canban" --home "$HOME_DIR" serve >> "$HOME_DIR/server.log" 2>&1 </dev/null &
    waited=0
    while ! env -u CANBAN_ENDPOINT "$DEST_TARGET/Contents/MacOS/canban" --home "$HOME_DIR" call app.ping '{}' >/dev/null 2>&1; do
      if [ "$waited" -ge 150 ]; then
        fail "Upgraded AppServer did not respond on $HOME_DIR/app.sock"
      fi
      /bin/sleep 0.1
      waited=$((waited + 1))
    done
  fi
fi

log "Installed Canban $TARGET_VERSION to $DEST_TARGET"

if ! command -v hc >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/hc" ] && [ ! -x /opt/homebrew/bin/hc ] && [ ! -x /usr/local/bin/hc ]; then
  log ""
  log "Note: Herdr Core (hc) was not detected in PATH."
  log "Canban can install it automatically on first launch, or you can install it now with:"
  log "  curl -fsSL https://releases.herdr.dev/core/install.sh | sh"
fi
