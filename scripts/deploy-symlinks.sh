#!/usr/bin/env bash
set -euo pipefail

script_name() { basename -- "$0"; }

print_usage() {
  cat <<EOF
Usage: $(script_name) [options] [BIN ...]

Create symlinks in ~/.local/bin for workspace binaries built in target/release.

Options:
  --dest DIR       Destination directory (default: \"$HOME/.local/bin\")
  --target DIR     Source directory (default: REPO_ROOT/target/release)
  --build          Run \"cargo build --release\" before linking
  -f, --force      Overwrite existing non-symlink files at destination
  -h, --help       Show this help

If no BIN names are provided, executables are auto-detected in the target dir.
EOF
}

die() { echo "Error: $*" >&2; exit 1; }

# Resolve important paths
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
TARGET_DIR_DEFAULT="${REPO_ROOT}/target/release"
DEST_DIR_DEFAULT="${HOME}/.local/bin"

DEST_DIR="$DEST_DIR_DEFAULT"
TARGET_DIR="$TARGET_DIR_DEFAULT"
FORCE=0
DO_BUILD=0

BINS=()

# Parse arguments
while [ $# -gt 0 ]; do
  case "$1" in
    --dest)
      shift || die "--dest requires an argument"
      [ -n "${1-}" ] || die "--dest requires an argument"
      DEST_DIR="$1"
      ;;
    --target)
      shift || die "--target requires an argument"
      [ -n "${1-}" ] || die "--target requires an argument"
      TARGET_DIR="$1"
      ;;
    --build)
      DO_BUILD=1
      ;;
    -f|--force)
      FORCE=1
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    --)
      shift
      while [ $# -gt 0 ]; do BINS+=("$1"); shift; done
      break
      ;;
    *)
      BINS+=("$1")
      ;;
  esac
  shift || true
done

# Optionally build
if [ "$DO_BUILD" -eq 1 ]; then
  ( cd "$REPO_ROOT" && cargo build --release )
fi

# Ensure destination directory exists
mkdir -p -- "$DEST_DIR"

# Validate target dir
[ -d "$TARGET_DIR" ] || die "Target directory not found: $TARGET_DIR"

# Auto-detect executables if none provided
if [ ${#BINS[@]} -eq 0 ]; then
  while IFS= read -r path; do
    # Basename and basic filters
    base_name="$(basename -- "$path")"
    case "$base_name" in
      *.dylib|*.so|*.a|*.rlib|*.rmeta) continue ;;
    esac
    BINS+=("$base_name")
  done < <(find "$TARGET_DIR" -maxdepth 1 -type f -perm -111 2>/dev/null)
fi

[ ${#BINS[@]} -gt 0 ] || die "No binaries specified or found in $TARGET_DIR"

# Resolve absolute source dir once
TARGET_ABS="$(cd -- "$TARGET_DIR" && pwd -P)"

linked=0
skipped=0
for bin in "${BINS[@]}"; do
  src="$TARGET_ABS/$bin"
  dest="$DEST_DIR/$bin"

  if [ ! -f "$src" ] || [ ! -x "$src" ]; then
    echo "skip: $bin (not an executable file in $TARGET_ABS)" >&2
    skipped=$((skipped+1))
    continue
  fi

  if [ -e "$dest" ] && [ ! -L "$dest" ] && [ "$FORCE" -ne 1 ]; then
    echo "exists: $dest (not a symlink) — use --force to overwrite" >&2
    skipped=$((skipped+1))
    continue
  fi

  # Create/replace symlink
  if ln -sfn ${FORCE:+-f} -- "$src" "$dest"; then
    echo "linked: $dest -> $src"
    linked=$((linked+1))
  else
    echo "failed: $dest" >&2
    skipped=$((skipped+1))
  fi
done

echo "Summary: $linked linked, $skipped skipped."
echo "Ensure '$DEST_DIR' is on your PATH."

