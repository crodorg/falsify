#!/bin/sh
# install.sh — build the `falsify` binary and install the /falsify Claude Code skill.
#
# Usage:
#   ./install.sh
#
# Honors CLAUDE_CONFIG_DIR (default ~/.claude); installs the binary to ~/.local/bin.
set -eu

usage() {
	sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
}

# --- parse args -------------------------------------------------------------
while [ $# -gt 0 ]; do
	case "$1" in
	-h | --help) usage; exit 0 ;;
	*) echo "unknown argument: $1" >&2; usage; exit 2 ;;
	esac
done

# --- resolve paths ----------------------------------------------------------
REPO="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-${HOME}/.claude}"
SKILL_DIR="${CLAUDE_DIR}/skills/falsify"

# --- build ------------------------------------------------------------------
echo ">> building falsify + fetchfix (release, workspace)"
(cd "$REPO" && cargo build --release --workspace)

# --- install binaries -------------------------------------------------------
mkdir -p "$BIN_DIR"
for bin in falsify fetchfix; do
	ln -sf "$REPO/target/release/$bin" "$BIN_DIR/$bin"
	echo ">> linked $BIN_DIR/$bin -> $REPO/target/release/$bin"
done

# --- install skill ----------------------------------------------------------
# SKILL.md at the skill root; subagents/ preserved so `subagents/<name>.md`
# references in SKILL.md resolve relative to the installed skill dir.
mkdir -p "$SKILL_DIR/subagents"
cp "$REPO/skill/SKILL.md" "$SKILL_DIR/SKILL.md"
cp "$REPO/skill/subagents/"*.md "$SKILL_DIR/subagents/"
echo ">> installed skill to $SKILL_DIR"

# --- post-install notes -----------------------------------------------------
cat <<EOF

Done. Next steps:
  * Ensure $BIN_DIR is on your PATH.
  * falsify audits against a plainbrain-style wiki at ~/wiki (override: FALSIFY_WIKI_ROOT,
    or \$PLAINBRAIN_WIKI). Without a canon corpus there, there is nothing to audit against.
  * In Claude Code, run:  /falsify <source-path> [--as-of YYYY-MM-DD]

See README.md for what this is and how it complements recon and plainbrain.
EOF
