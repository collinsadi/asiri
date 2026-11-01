#!/usr/bin/env bash
# Rejects commit messages (or PR bodies) that attribute work to an AI assistant or bot.
# Usage: check-commit-message.sh <file>      (reads stdin when no file is given)
#
# Used by .githooks/commit-msg locally and by CI for every commit in a push or PR.
set -euo pipefail

input="${1:-/dev/stdin}"
body="$(grep -v '^#' "$input" || true)"

forbidden='(co-authored-by:.*(claude|anthropic|openai|chatgpt|copilot|gemini|bard|codeium|cursor|devin|ai assistant|\[bot\]|noreply@anthropic\.com))|(generated (with|by) .*(claude|chatgpt|copilot|gpt|llm|(^|[^a-z])ai([^a-z]|$)))|(🤖)|(claude-session:)|(assisted-by:)|(ai-generated)'

if grep -qiE "$forbidden" <<<"$body"; then
  echo "commit message contains an AI attribution trailer, which this repository forbids:" >&2
  grep -inE "$forbidden" <<<"$body" >&2 || true
  exit 1
fi
