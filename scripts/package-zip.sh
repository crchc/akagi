#!/usr/bin/env bash
#
# Package a portable zip for the given target triple.
#
# Layout:
#   dist/akagi-<version>-<os>-<arch>.zip
#     akagi-<version>-<os>-<arch>/
#       akagi[.exe]
#       frontend/index.html + assets/...
#       runtime/python/<triple>/...
#       runtime/uv/<triple>/...
#       LICENSE.txt
#       NOTICE
#       README.txt
#
# Usage:
#   scripts/package-zip.sh <target-triple>
#
# Prerequisites:
#   1. Binary built into target/<triple>/release/akagi[.exe]
#      (e.g. via `cargo build --release --target <triple>`)
#   2. Runtime fetched into runtime/{python,uv}/<triple>/
#      (e.g. via `scripts/fetch-runtime.sh <triple>`)
#
# Reads the package version from the first `version = "..."` line in
# Cargo.toml — that's the [package] version because [package] is the
# first table.

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <target-triple>" >&2
  exit 2
fi

TARGET="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(grep -m1 '^version = ' "$ROOT/Cargo.toml" | cut -d'"' -f2)"

case "$TARGET" in
  x86_64-unknown-linux-gnu) OS=linux;   ARCH=x64;   EXE=akagi ;;
  aarch64-apple-darwin)     OS=macos;   ARCH=arm64; EXE=akagi ;;
  x86_64-pc-windows-msvc)   OS=windows; ARCH=x64;   EXE=akagi.exe ;;
  *)
    echo "unknown target triple: $TARGET" >&2
    exit 2
    ;;
esac

PKG="akagi-${VERSION}-${OS}-${ARCH}"
STAGE_ROOT="$ROOT/dist/staging"
STAGE="$STAGE_ROOT/$PKG"
ARCHIVE="$ROOT/dist/${PKG}.zip"

BIN_SRC="$ROOT/target/$TARGET/release/$EXE"
PY_SRC="$ROOT/runtime/python/$TARGET"
UV_SRC="$ROOT/runtime/uv/$TARGET"

if [[ ! -f "$BIN_SRC" ]]; then
  echo "binary not found at $BIN_SRC — run cargo build first" >&2
  exit 1
fi
if [[ ! -d "$PY_SRC" ]] || [[ ! -d "$UV_SRC" ]]; then
  echo "runtime tree missing — run scripts/fetch-runtime.sh $TARGET first" >&2
  exit 1
fi
if [[ ! -f "$ROOT/frontend/dist/index.html" ]]; then
  echo "frontend not built — run npm run --prefix frontend build first" >&2
  exit 1
fi

rm -rf "$STAGE"
mkdir -p "$STAGE/runtime/python" "$STAGE/runtime/uv"
mkdir -p "$STAGE/frontend"

cp "$BIN_SRC" "$STAGE/$EXE"
if [[ "$OS" != "windows" ]]; then
  chmod +x "$STAGE/$EXE"
fi

# -RP preserves symlinks (python-build-standalone uses internal symlinks
# like bin/python3.12 -> bin/python on linux/mac; without -P they get
# duplicated and the zip nearly doubles).
cp -RP "$PY_SRC" "$STAGE/runtime/python/$TARGET"
cp -RP "$UV_SRC" "$STAGE/runtime/uv/$TARGET"

cp "$ROOT/LICENSE.txt" "$STAGE/LICENSE.txt"
cp "$ROOT/NOTICE"      "$STAGE/NOTICE"
cp -R "$ROOT/frontend/dist/." "$STAGE/frontend/"

cat > "$STAGE/README.txt" <<EOF
Akagi $VERSION — portable build ($OS-$ARCH)

Quick start
-----------
1. Move this folder anywhere you have write permission (e.g. ~/Apps/, Desktop, etc.).
2. Run the binary:
     Linux/macOS:  ./akagi
     Windows:      akagi.exe
3. Akagi opens http://127.0.0.1:3000 in your default browser. Keep the
   terminal window running while using the Web UI.
4. On first launch, Akagi creates these directories alongside the binary:
     config.toml   logs/   history/   ca/   mjai_bot/

Platform notes
--------------
EOF

case "$OS" in
  windows)
    cat >> "$STAGE/README.txt" <<'EOF'
- The binary is unsigned, so SmartScreen will warn on first launch.
  Click "More info" then "Run anyway".
EOF
    ;;
  macos)
    cat >> "$STAGE/README.txt" <<EOF
- The binary is unsigned. macOS Gatekeeper will block the first launch.
  Either run once with the quarantine bit removed:
    xattr -cr "\$(pwd)/$PKG"
  or right-click the binary and choose "Open" the first time.
- Apple Silicon only — no Intel build.
EOF
    ;;
  linux)
    cat >> "$STAGE/README.txt" <<'EOF'
- Built on ubuntu-22.04, requires glibc 2.35 or newer.
EOF
    ;;
esac

cat >> "$STAGE/README.txt" <<'EOF'

Full documentation: https://github.com/shinkuan/AkagiV3
EOF

# Pick a zip tool. windows-latest GHA runners ship 7z but not zip;
# linux/macos runners (and most user shells) ship the `zip` command.
mkdir -p "$ROOT/dist"
rm -f "$ARCHIVE"

if [[ "$OS" == "windows" ]]; then
  # 7z preserves the directory tree and is faster than PowerShell's
  # Compress-Archive on this many small files.
  (cd "$STAGE_ROOT" && 7z a -tzip -mx=5 "$ARCHIVE" "$PKG/" >/dev/null)
else
  # -y preserves the symlinks inside the python-build-standalone tree.
  (cd "$STAGE_ROOT" && zip -r -y -9 "$ARCHIVE" "$PKG/" >/dev/null)
fi

echo "wrote $ARCHIVE"
