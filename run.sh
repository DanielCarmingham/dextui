#!/usr/bin/env bash
#
# run.sh — launch dex-tui against the dex store for YOUR current directory.
#
# Deliberately does not cd into the repo: dex resolves its task store from the
# working directory, so the app must inherit wherever you invoked this from.
# That means it works by absolute path from anywhere:
#
#   cd ~/some/project && ~/Developer/DanielCarmingham/dex-tui/run.sh
#
# Options:
#   -n, --no-build   skip the build and run whatever was built last
#   -r, --release    build and run the optimised binary
#   everything else is passed through, e.g. --selftest
set -euo pipefail

SOURCE="${BASH_SOURCE[0]}"
while [ -L "$SOURCE" ]; do
  DIR="$(cd -P "$(dirname "$SOURCE")" && pwd)"
  SOURCE="$(readlink "$SOURCE")"
  [[ $SOURCE != /* ]] && SOURCE="$DIR/$SOURCE"
done
REPO="$(cd -P "$(dirname "$SOURCE")" && pwd)"

CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
command -v cargo >/dev/null 2>&1 && CARGO=cargo

PROFILE=debug
BUILD=1
ARGS=()

for arg in "$@"; do
  case "$arg" in
    -n|--no-build) BUILD=0 ;;
    -r|--release)  PROFILE=release ;;
    *)             ARGS+=("$arg") ;;
  esac
done

APP="$REPO/target/$PROFILE/dex-tui"

if [ "$BUILD" -eq 1 ]; then
  # Capture output so a compile error stays readable instead of being wiped
  # when the TUI clears the screen.
  FLAGS=(--manifest-path "$REPO/Cargo.toml" --quiet)
  [ "$PROFILE" = release ] && FLAGS+=(--release)

  if ! BUILD_LOG="$("$CARGO" build "${FLAGS[@]}" 2>&1)"; then
    printf '%s\n' "$BUILD_LOG" >&2
    exit 1
  fi
fi

if [ ! -x "$APP" ]; then
  echo "run.sh: $APP not found — try without --no-build" >&2
  exit 1
fi

if ! command -v dex >/dev/null 2>&1; then
  echo "run.sh: \`dex\` is not on PATH; dex-tui reads and writes tasks through it" >&2
  exit 1
fi

exec "$APP" "${ARGS[@]+"${ARGS[@]}"}"
