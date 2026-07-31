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
#
# The pane is 120x36 unless DEXTUI_RENDER_COLS / DEXTUI_RENDER_ROWS say
# otherwise, which is how you look at the layouts that only appear when the
# terminal is too small for the last one:
#
#   DEXTUI_RENDER_COLS=60 DEXTUI_RENDER_ROWS=20 scripts/render-check.sh "?"
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP="$REPO/target/debug/dextui"
SOCK="dextui-render"
SESSION="render"
WORKDIR="${DEXTUI_RENDER_CWD:-$PWD}"
# The size is worth reaching: most of what this app gets wrong, it gets wrong
# only at a width or height it has to shed something at.
COLS="${DEXTUI_RENDER_COLS:-120}"
ROWS="${DEXTUI_RENDER_ROWS:-36}"
KEYS="${1:-}"

if [ ! -x "$APP" ]; then
  echo "build first: cargo build --manifest-path $REPO/Cargo.toml" >&2
  exit 1
fi

cleanup() { tmux -L "$SOCK" kill-server 2>/dev/null; }
trap cleanup EXIT

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.3

tmux -L "$SOCK" new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" -c "$WORKDIR" "$APP"
sleep 3

for k in $KEYS; do
  tmux -L "$SOCK" send-keys -t "$SESSION" "$k"
  sleep 1
done

[ -n "$KEYS" ] && sleep 1

tmux -L "$SOCK" capture-pane -t "$SESSION" -p
