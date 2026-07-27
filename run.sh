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
#   -n, --no-build   skip the build and run whatever was built last (faster)
#   -r, --release    build and run the Release configuration
#   everything else is passed through to the app, e.g. --selftest
set -euo pipefail

# Resolve the repo from this script's own location, following symlinks, so the
# script still works when linked into ~/.local/bin.
SOURCE="${BASH_SOURCE[0]}"
while [ -L "$SOURCE" ]; do
  DIR="$(cd -P "$(dirname "$SOURCE")" && pwd)"
  SOURCE="$(readlink "$SOURCE")"
  [[ $SOURCE != /* ]] && SOURCE="$DIR/$SOURCE"
done
REPO="$(cd -P "$(dirname "$SOURCE")" && pwd)"

CONFIG=Debug
BUILD=1
ARGS=()

for arg in "$@"; do
  case "$arg" in
    -n|--no-build) BUILD=0 ;;
    -r|--release)  CONFIG=Release ;;
    *)             ARGS+=("$arg") ;;
  esac
done

APP="$REPO/src/DexTui.App/bin/$CONFIG/net10.0/DexTui.App"

if [ "$BUILD" -eq 1 ]; then
  # Quiet on success; on failure show the errors and stop before clearing the
  # screen, otherwise the TUI would wipe them away.
  if ! BUILD_LOG="$(dotnet build "$REPO/DexTui.slnx" -c "$CONFIG" --nologo -v q 2>&1)"; then
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

# exec so the app owns the terminal and signals directly.
exec "$APP" "${ARGS[@]+"${ARGS[@]}"}"
