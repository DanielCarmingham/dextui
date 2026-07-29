#!/usr/bin/env bash
#
# render-check.sh [keys]
#
# Renders dextui inside a tmux pane and prints what it actually drew.
#
# ratatui (like most TUI frameworks) needs a real terminal: under a bare pty
# such as `script` or a plain pipe, capability queries go unanswered and you get
# no usable frames. tmux is a real terminal emulator, so it renders normally.
#
# Optional argument: whitespace-separated tmux key names sent one per second
# before the capture, e.g. "Down Down", "f", "?".
#
# Runs on a private tmux socket so your own tmux sessions are untouched.
#
#   scripts/render-check.sh
#   scripts/render-check.sh "Down Down"
#   scripts/render-check.sh "f"
#
# By default it renders the dex store for your current directory; set
# DEXTUI_RENDER_CWD to point it somewhere else.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP="$REPO/target/debug/dextui"
SOCK="dextui-render"
SESSION="render"
WORKDIR="${DEXTUI_RENDER_CWD:-$PWD}"
KEYS="${1:-}"

if [ ! -x "$APP" ]; then
  echo "build first: cargo build --manifest-path $REPO/Cargo.toml" >&2
  exit 1
fi

cleanup() { tmux -L "$SOCK" kill-server 2>/dev/null; }
trap cleanup EXIT

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.3

tmux -L "$SOCK" new-session -d -s "$SESSION" -x 120 -y 36 -c "$WORKDIR" "$APP"
sleep 3

for k in $KEYS; do
  tmux -L "$SOCK" send-keys -t "$SESSION" "$k"
  sleep 1
done

[ -n "$KEYS" ] && sleep 1

tmux -L "$SOCK" capture-pane -t "$SESSION" -p
