#!/usr/bin/env bash
#
# render-check.sh [keys]
#
# Renders dex-tui inside a tmux pane and prints what it actually drew.
#
# Terminal.Gui negotiates terminal capabilities at startup, and under a bare pty
# (`script`, a pipe) nothing answers those queries so it renders no frames at all.
# tmux is a real terminal emulator, so it renders normally there.
#
# Optional argument: whitespace-separated tmux key names sent one per second
# before the capture, e.g. "Down Down", "f", "?".
#
# Runs on a private tmux socket so your own tmux sessions are untouched.
#
#   scripts/render-check.sh
#   scripts/render-check.sh "Down Down"
#   scripts/render-check.sh "f"
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP="$REPO/src/DexTui.App/bin/Debug/net10.0/DexTui.App"
SOCK="dextui-render"
SESSION="render"
WORKDIR="${DEXTUI_RENDER_CWD:-$PWD}"
KEYS="${1:-}"

if [ ! -x "$APP" ]; then
  echo "build first: dotnet build $REPO/DexTui.slnx" >&2
  exit 1
fi

cleanup() { tmux -L "$SOCK" kill-server 2>/dev/null; }
trap cleanup EXIT

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.3

tmux -L "$SOCK" new-session -d -s "$SESSION" -x 120 -y 36 -c "$WORKDIR" "$APP"
sleep 4

for k in $KEYS; do
  tmux -L "$SOCK" send-keys -t "$SESSION" "$k"
  sleep 1
done

[ -n "$KEYS" ] && sleep 1

tmux -L "$SOCK" capture-pane -t "$SESSION" -p
