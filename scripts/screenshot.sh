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
#
# The narrow README pair is 60x13, not the 116x21 default -- short enough that
# neither shot has to crop dead space below the content, which 21 rows does at
# this width. Recorded here because it previously was not, and the only way to
# rediscover it was to measure the pixels of the committed PNG.
#   COLS=60 ROWS=13 scripts/screenshot.sh docs/img/dextui-narrow.png
#   COLS=60 ROWS=13 KEYS=Enter scripts/screenshot.sh docs/img/dextui-narrow-detail.png
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
command -v dex >/dev/null || { echo "screenshot.sh: dex is not on PATH" >&2; exit 1; }
[ -x "$REPO/target/debug/dextui" ] || { echo "screenshot.sh: cargo build first" >&2; exit 1; }
python3 -c "import PIL" 2>/dev/null || {
  echo "screenshot.sh: this python3 has no Pillow" >&2
  echo "  python3 is $(command -v python3)" >&2
  echo "  install it there, or put one that has it first on PATH" >&2
  exit 1
}

BASE="$(mktemp -d)"
DEMO="$BASE/dextui-demo"
# The repo sidebar reads its own state -- `repos.toml`, the sync log -- from
# XDG_CONFIG_HOME/XDG_STATE_HOME, the same variables `dextui` itself honours.
# Left unset, dextui would fall through to *your* real ~/.config/dextui, and
# the picture would show your actual registered repos rather than the demo's.
# Both live under `$BASE` so the one `rm -rf` below sweeps everything this
# script wrote, on top of whatever `dex`'s own writes to `.dex` add.
CONFIG_HOME="$BASE/config"
STATE_HOME="$BASE/state"
SECOND="$BASE/repoB"
SECOND_FEATURE="$BASE/repoB-feature"

cleanup() {
  tmux -L "$SOCK" kill-server 2>/dev/null
  rm -rf "$BASE"
}
trap cleanup EXIT

echo "seeding $DEMO" >&2
"$REPO/scripts/seed-demo.sh" "$DEMO" >/dev/null 2>&1 || {
  echo "screenshot.sh: seed-demo.sh failed" >&2; exit 1; }

# A second repo, registered rather than launched-into, so the sidebar has both
# of its sections to show: `here` is $DEMO (below, unregistered), `saved` is
# this one. `git worktree add` needs a real commit to branch from, so that
# commit happens before `dex` ever touches the directory -- `.dex` is never
# `git add`ed, so it is invisible to a worktree checked out from that commit
# regardless of what dex later writes into $SECOND's own working copy. That
# gives `feature` no store of its own for free, which is worth having on
# screen: CLAUDE.md and the README both promise a store-less worktree renders
# as an ordinary dim row, never an error, and a screenshot that cannot show
# that case is not really proving the claim.
echo "seeding $SECOND (a second repo, for the sidebar)" >&2
mkdir -p "$SECOND"
git -C "$SECOND" init -q . || { echo "screenshot.sh: git init failed" >&2; exit 1; }
git -C "$SECOND" -c user.name="dextui screenshot" -c user.email="screenshot@localhost" \
  commit -q --allow-empty -m "repoB" || {
  echo "screenshot.sh: git commit failed" >&2; exit 1; }
git -C "$SECOND" worktree add -q "$SECOND_FEATURE" -b feature >/dev/null 2>&1 || {
  echo "screenshot.sh: git worktree add failed" >&2; exit 1; }

# One of each state -- done, active, pending -- so the sidebar's meter shows
# every colour it can, the same reason `seed-demo.sh` does this for the tree.
(
  cd "$SECOND" || exit 1
  T1=$(dex create "Design the widget API" 2>&1 | head -1 | awk '{print $3}')
  T2=$(dex create "Wire up telemetry" 2>&1 | head -1 | awk '{print $3}')
  dex create "Ship the widget beta" >/dev/null
  dex create "Write the widget docs" >/dev/null
  dex start "$T1" >/dev/null
  dex complete "$T2" --result "Shipped." --no-commit >/dev/null
) || { echo "screenshot.sh: seeding repoB's tasks failed" >&2; exit 1; }

mkdir -p "$CONFIG_HOME/dextui"
printf 'repos = ["%s"]\n' "$SECOND" > "$CONFIG_HOME/dextui/repos.toml"

tmux -L "$SOCK" kill-server 2>/dev/null
sleep 0.3
# `env` here, not `tmux -e`: tmux's session environment is set before the pane's
# shell starts, and this machine's .zshenv unconditionally re-exports
# XDG_CONFIG_HOME on every invocation -- including a bare `-c`, which is how
# tmux launches an explicit command -- so `-e` alone was silently overwritten
# before dextui ever ran, and the very first regeneration under this rewrite
# rendered the real, current ~/.config/dextui/repos.toml into the picture.
# `env` sets these immediately adjacent to the exec of dextui itself, which no
# intervening shell startup file can get between.
tmux -L "$SOCK" new-session -d -s shot -x "$COLS" -y "$ROWS" -c "$DEMO" \
  env DEXTUI_ICONS="$ICONS" XDG_CONFIG_HOME="$CONFIG_HOME" XDG_STATE_HOME="$STATE_HOME" \
  "$REPO/target/debug/dextui"
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
