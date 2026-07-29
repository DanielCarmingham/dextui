#!/usr/bin/env bash
#
# seed-demo.sh [dir]
#
# Creates a throwaway dex store with a realistic nested task tree, so the UI can
# be looked at with real data. Defaults to ./dextui-demo under your temp dir.
#
#   scripts/seed-demo.sh              # seed and print where it went
#   cd <that dir> && dextui          # look at it
#
# It runs `git init` on purpose: dex uses a repo-local .dex inside a git repo,
# and falls back to the SHARED GLOBAL store at ~/.config/dex/local outside one.
# Without the repo this would pollute your real global task list.
set -euo pipefail

DIR="${1:-${TMPDIR:-/tmp}/dextui-demo}"

command -v dex >/dev/null 2>&1 || { echo "seed-demo.sh: dex is not on PATH" >&2; exit 1; }

rm -rf "$DIR"
mkdir -p "$DIR"
cd "$DIR"
git init -q .

new() { dex create "$@" 2>&1 | head -1 | awk '{print $3}'; }

ROOT=$(new "Ship dextui v1" -d "Two-pane terminal browser over the dex CLI.

Steps:

- resolve the store with \`dex dir\`
- read via **dex list --json**
  - never parse tasks.jsonl directly
- write only through the CLI

\`\`\`rust
fn main() {
    println!(\"hello\");
}
\`\`\`

> Refresh must never disturb the user.")

CORE=$(new "Core data layer" -d "Client, tree building and the refresh rules." --parent "$ROOT")
new "Wire up the file watcher" -d "Debounced FS events plus a 10s safety poll." --parent "$CORE" >/dev/null
new "Parse the mixed-case JSON" -d "snake_case and camelCase in one payload." --parent "$CORE" >/dev/null

UI=$(new "Terminal UI" -d "Header, tree, detail pane" --parent "$ROOT" -p 2)
KEYS=$(new "Keybindings" -d "s/c/e/n/a/d plus search and filter" --parent "$UI")
new "Progress rollups" -d "Three-state meters on parent rows." --parent "$UI" >/dev/null

new "Write the docs" -d "Onboarding notes for future sessions." --parent "$ROOT" -p 3 >/dev/null
new "Long name to show how the tree handles a task title that runs well past the pane" \
    -d "Checks truncation and the right-hand gutter." >/dev/null

# A spread of states, so the meters and ages have something to show.
dex start "$ROOT" >/dev/null
dex start "$CORE" >/dev/null
dex complete "$KEYS" --result "All bindings wired up." --no-commit >/dev/null

echo
echo "seeded: $DIR"
echo
echo "  cd $DIR"
echo "  dextui"
echo
dex list
