#!/usr/bin/env bash
#
# screenshot.sh [out.png]
#
# Regenerates the README screenshot: seeds a throwaway demo store, runs dextui
# against it in tmux, captures the pane WITH its escape sequences, and renders
# that to a PNG using the real font and the real terminal palette.
#
# The image is therefore the app's own output rather than a photograph of it --
# reproducible, and incapable of showing a colour the app does not emit.
#
#   scripts/screenshot.sh                        # -> docs/img/dextui-dark.png
#   DEXTUI_ICONS=unicode scripts/screenshot.sh out.png
#   COLS=60 scripts/screenshot.sh out.png        # the single-pane layout
#   COLS=60 KEYS=Enter scripts/screenshot.sh …   # after pressing a key
#
# Nerd Font glyphs by default, since that is the set worth showing off.
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$REPO/docs/img/dextui-dark.png}"
ICONS="${DEXTUI_ICONS:-nerd}"
SOCK="dextui-shot"
# 116 is comfortably above `single_pane_below`, so the default shot shows the
# split. Drop below that and the app draws one pane -- which is worth its own
# picture rather than a paragraph.
COLS="${COLS:-116}"
# Tall enough to show the tree's depth and the detail pane's metadata, short
# enough to stop before the demo description's fenced code block -- which is
# real output, but a half-drawn ```rust fence is a poor first impression.
ROWS="${ROWS:-21}"
# Keys to send before capturing, space-separated tmux key names.
KEYS="${KEYS:-}"

command -v tmux >/dev/null || { echo "screenshot.sh: tmux is not on PATH" >&2; exit 1; }
[ -x "$REPO/target/debug/dextui" ] || { echo "screenshot.sh: cargo build first" >&2; exit 1; }
python3 -c "import PIL" 2>/dev/null || {
  echo "screenshot.sh: this python3 has no Pillow" >&2
  echo "  python3 is $(command -v python3)" >&2
  echo "  install it there, or put one that has it first on PATH" >&2
  exit 1
}

DEMO="$(mktemp -d)/dextui-demo"
cleanup() {
  tmux -L "$SOCK" kill-server 2>/dev/null
  rm -rf "$(dirname "$DEMO")"
}
trap cleanup EXIT

echo "seeding $DEMO" >&2
"$REPO/scripts/seed-demo.sh" "$DEMO" >/dev/null 2>&1 || {
  echo "screenshot.sh: seed-demo.sh failed" >&2; exit 1; }

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.3
tmux -L "$SOCK" new-session -d -s shot -x "$COLS" -y "$ROWS" -c "$DEMO" \
  -e "DEXTUI_ICONS=$ICONS" "$REPO/target/debug/dextui"
sleep 3

# Select the root so the detail pane has something substantial to show, and so
# the selection gutter appears on a row that also carries a meter.
tmux -L "$SOCK" send-keys -t shot Down
sleep 1

for k in $KEYS; do
  tmux -L "$SOCK" send-keys -t shot "$k"
  sleep 1
done

tmux -L "$SOCK" capture-pane -t shot -p -e | python3 "$REPO/scripts/screenshot.py" "$OUT"
