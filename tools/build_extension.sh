#!/usr/bin/env bash
#
# Build a Tanoshi extension for Android (aarch64) from the exported sources.
#
# The extensions are crates that live inside the luigi311/tanoshi-extensions
# Cargo workspace (they use the workspace's `extension-utils`, `networking` and
# `tanoshi-lib` dependencies). This script sets that workspace up, copies the
# requested crate(s) into it, applies this repository's networking patch and
# builds the plugin for Android.
#
# Usage:
#   bash tools/build_extension.sh                                # all ported sources
#   bash tools/build_extension.sh --crate sources/template
#   bash tools/build_extension.sh --crate /path/to/crate --output-dir /tmp/out
#
# Environment:
#   ANDROID_NDK_HOME   Android NDK (desktop cross-build; otherwise Termux's
#                      native Rust toolchain is used)
#   WORK_DIR           build directory (default tools/.work/extension-build)
#   EXTENSIONS_SRC_DIR existing tanoshi-extensions checkout to reuse
#   EXTENSIONS_TARGET_DIR existing cargo target dir to reuse
#
# Output: android/extensions/plugins/lib<name>.so (stripped) + SHA-256, size and a
# catalog JSON snippet to paste into an extension repository's index.json.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
SOURCES_DIR="$REPO_ROOT/sources"
WORK_DIR="${WORK_DIR:-$HERE/.work/extension-build}"
SRC_DIR="${EXTENSIONS_SRC_DIR:-$WORK_DIR/extensions-src}"
TARGET_DIR="${EXTENSIONS_TARGET_DIR:-$WORK_DIR/target}"
OUTPUT_DIR="$REPO_ROOT/android/extensions/plugins"
CRATES=()
BUILD_ALL=1

die() { printf '\nerror: %s\n' "$*" >&2; exit 1; }
say() { printf '==> %s\n' "$*"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --crate) CRATES+=("${2:?--crate needs a crate directory}"); BUILD_ALL=0; shift 2 ;;
        --output-dir) OUTPUT_DIR="${2:?--output-dir needs a path}"; shift 2 ;;
        --work-dir) WORK_DIR="${2:?--work-dir needs a path}"; shift 2 ;;
        -h|--help) sed -n '2,34p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

command -v cargo >/dev/null 2>&1 || die "cargo not found (install the Rust toolchain)"
command -v git >/dev/null 2>&1 || die "git not found"

IN_TERMUX=0
if [ -n "${PREFIX:-}" ] && [ -d /data/data/com.termux/files/usr ]; then IN_TERMUX=1; fi

NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
TARGET_ARGS=()
if [ -n "$NDK" ]; then
    case "$(uname -s)" in
        Darwin) HOST_TAG="darwin-x86_64" ;;
        *) HOST_TAG="linux-x86_64" ;;
    esac
    TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST_TAG"
    export CC_aarch64_linux_android="$TOOLCHAIN/bin/aarch64-linux-android26-clang"
    export AR_aarch64_linux_android="$TOOLCHAIN/bin/llvm-ar"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
    rustup target add aarch64-linux-android || die "could not install the aarch64-linux-android target"
    TARGET_ARGS=(--target aarch64-linux-android)
    SO_PATH_REL="aarch64-linux-android/release"
    say "toolchain: Android NDK"
elif [ "$IN_TERMUX" = "1" ]; then
    SO_PATH_REL="release"
    say "toolchain: Termux native aarch64-linux-android"
else
    die "no Android toolchain: run inside Termux or set ANDROID_NDK_HOME"
fi

# ---------------------------------------------------------------------------
# One-time: fetch the extension workspace and its patches.
# ---------------------------------------------------------------------------
if [ ! -d "$SRC_DIR/.git" ]; then
    say "cloning luigi311/tanoshi-extensions"
    rm -rf "$SRC_DIR"
    git clone --depth 1 https://github.com/luigi311/tanoshi-extensions.git "$SRC_DIR"
else
    say "reusing extension workspace at $SRC_DIR"
fi
(
    cd "$SRC_DIR"
    apply_existing() {
        local patch="$1"
        [ -f "$patch" ] || return 0
        if git apply --check --whitespace=nowarn "$patch" 2>/dev/null; then
            git apply --whitespace=nowarn "$patch"
            say "applied $(basename "$patch")"
        elif git apply --3way --whitespace=nowarn "$patch" 2>/dev/null; then
            say "applied $(basename "$patch") (3-way)"
        elif git apply --check --reverse --whitespace=nowarn "$patch" 2>/dev/null; then
            say "$(basename "$patch") already applied"
        else
            die "cannot apply $(basename "$patch")"
        fi
    }
    apply_existing "$REPO_ROOT/patches/networking-helpers.patch"
    if [ -f "$REPO_ROOT/patches/extension-icons.patch" ]; then
        apply_existing "$REPO_ROOT/patches/extension-icons.patch" || true
    fi
)

# ---------------------------------------------------------------------------
# Copy the crate(s) into the workspace.
# ---------------------------------------------------------------------------
if [ "$BUILD_ALL" = "1" ]; then
    for dir in "$SOURCES_DIR"/ported/*/; do
        [ -f "$dir/Cargo.toml" ] && CRATES+=("${dir%/}")
    done
fi
[ "${#CRATES[@]}" -gt 0 ] || die "no crates to build"

mkdir -p "$OUTPUT_DIR" "$TARGET_DIR"
for crate in "${CRATES[@]}"; do
    [ -f "$crate/Cargo.toml" ] || die "no Cargo.toml in $crate"
    pkg="$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' "$crate/Cargo.toml" | head -1)"
    [ -n "$pkg" ] || die "could not read package name from $crate/Cargo.toml"
    rm -rf "$SRC_DIR/extensions/$pkg"
    cp -R "$crate" "$SRC_DIR/extensions/$pkg"
    say "building $pkg"
    (
        cd "$SRC_DIR"
        CARGO_TARGET_DIR="$TARGET_DIR" \
        RUSTFLAGS="-Clink-arg=-Wl,--allow-multiple-definition -Clink-arg=-Wl,-z,max-page-size=16384" \
            cargo build --release -p "$pkg" "${TARGET_ARGS[@]}"
    )
    so="$TARGET_DIR/$SO_PATH_REL/lib${pkg}.so"
    [ -f "$so" ] || die "build produced no $so"
    strip "$so" 2>/dev/null || true
    cp "$so" "$OUTPUT_DIR/lib${pkg}.so"
    chmod 644 "$OUTPUT_DIR/lib${pkg}.so"
    sha="$(sha256sum "$OUTPUT_DIR/lib${pkg}.so" | cut -d' ' -f1)"
    size="$(stat -c %s "$OUTPUT_DIR/lib${pkg}.so")"
    cat <<EOF

built: $OUTPUT_DIR/lib${pkg}.so
  sha256: $sha
  size:   $size

Catalog entry for your extension repository's android/extensions/index.json:
  {
    "name": "${pkg}",
    "displayName": "${pkg}",
    "description": "TODO: describe this source",
    "url": "https://raw.githubusercontent.com/<owner>/<repo>/main/android/extensions/plugins/lib${pkg}.so",
    "sha256": "${sha}",
    "size": ${size},
    "ported": true
  }
EOF
done

say "done; upload the .so file(s), update your index.json, commit, then add the repository link in the app"
