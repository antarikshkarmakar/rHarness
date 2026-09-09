#!/usr/bin/env bash
# rHarness — local-model one-shot runner.
#
# One command to bring up a local model server and run a task through the
# finish-first loop. Handles the probe → start → wait → run flow end-to-end.
#
# Usage:
#   scripts/local-model.sh --list                          # list available profiles
#   scripts/local-model.sh <profile-id> "<task text>"      # start (if needed) + run
#
# Examples:
#   scripts/local-model.sh glm53-exl3-spark "Refactor the auth module and add tests"
#   scripts/local-model.sh qwen38-ollama "Summarize the TODO file"
#
# Optional env (applies to the run; see .env.example for details):
#   RHA_ENABLE_THINKING=true|false      GLM-5.3 thinking toggle
#   RHA_REASONING_EFFORT=low|high       GLM-5.3 reasoning effort
#   RHA_MAX_TOKENS=32768                max completion tokens
#   RHA_TOP_P, RHA_TOP_K                sampling
#   RHA_SYSTEM_PROMPT="<role prompt>"   system / role definition
#   RHA_MAX_CONTEXT=<tokens>            context window (input + output)
#   RHA_GPU_OFFLOAD=<n>                 GPU offload layers (-1 = all)
#   RHA_CPU_THREADS=<n>                 CPU thread pool
#   RHA_FLASH_ATTENTION=true|false      Flash Attention
#   RHA_RESPONSE_FORMAT=json_object     force structured JSON output
#
# Set BUN_BIN to override the bun binary (defaults to `bun` on PATH).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RH_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUN="${BUN_BIN:-bun}"

rh() { "$BUN" "$RH_ROOT/src/cli.ts" "$@"; }

usage() {
  cat <<EOF
Usage:
  $(basename "$0") --list
  $(basename "$0") <profile-id> "<task text>"

Profiles:
EOF
  rh local list | sed 's/^/  /'
  echo
  echo "Optional env (see .env.example):"
  echo "  RHA_ENABLE_THINKING, RHA_REASONING_EFFORT, RHA_MAX_TOKENS,"
  echo "  RHA_TOP_P, RHA_TOP_K, RHA_SYSTEM_PROMPT, RHA_MAX_CONTEXT,"
  echo "  RHA_GPU_OFFLOAD, RHA_CPU_THREADS, RHA_FLASH_ATTENTION, RHA_RESPONSE_FORMAT"
}

case "${1:-}" in
  -h|--help|"")
    usage
    exit 0
    ;;
  --list|-l)
    rh local list
    exit 0
    ;;
esac

PROFILE_ID="$1"
TASK="${2:-}"

if [[ -z "$TASK" ]]; then
  echo "Error: task text is required." >&2
  usage >&2
  exit 1
fi

if ! rh local list | awk '{print $1}' | grep -qx "$PROFILE_ID"; then
  echo "Error: unknown profile \"$PROFILE_ID\"." >&2
  echo
  echo "Available profiles:" >&2
  rh local list | sed 's/^/  /' >&2
  exit 1
fi

echo "→ profile : $PROFILE_ID" >&2
echo "→ task    : $TASK" >&2
echo

if rh local status "$PROFILE_ID" >/dev/null 2>&1; then
  echo "→ server  : already running" >&2
else
  echo "→ server  : not running — starting (can take minutes for large EXL3 models)…" >&2
  echo
  rh local start "$PROFILE_ID"
  echo
fi

echo "→ running finish-first loop" >&2
echo
rh run --local "$PROFILE_ID" "$TASK"
