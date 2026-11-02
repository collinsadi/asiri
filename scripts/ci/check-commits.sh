#!/usr/bin/env bash
# Checks every commit in a range against the commit rules in CONTRIBUTING.md: a Conventional
# Commits subject of at most 72 characters and no AI attribution of any kind.
#
#   scripts/ci/check-commits.sh <base> <head>
#
# A base of all zeros (a new branch) or one that isn't in the clone checks everything reachable
# from <head>.
set -euo pipefail

base="${1:?usage: check-commits.sh <base> <head>}"
head="${2:?usage: check-commits.sh <base> <head>}"
root="$(git rev-parse --show-toplevel)"
hook="$root/.githooks/commit-msg"

if [[ "$base" =~ ^0+$ ]] || ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
  range="$head"
else
  range="$base..$head"
fi

msg="$(mktemp)"; err="$(mktemp)"
trap 'rm -f "$msg" "$err"' EXIT
checked=0; failed=0
while read -r commit; do
  git log -1 --format=%B "$commit" > "$msg"
  # Author and committer must be people, not assistants.
  printf '\n%s\n%s\n' "$(git log -1 --format='Author: %an <%ae>' "$commit")" \
    "$(git log -1 --format='Committer: %cn <%ce>' "$commit")" > "$err.who"
  checked=$((checked + 1))
  if ! "$hook" "$msg" 2>"$err"; then
    failed=$((failed + 1))
    echo "✗ $(git log -1 --format='%h %s' "$commit")"
    sed 's/^/    /' "$err"
  elif grep -qiE '(claude|anthropic|openai|chatgpt|copilot|\[bot\])' "$err.who"; then
    failed=$((failed + 1))
    echo "✗ $(git log -1 --format='%h %s' "$commit")"
    sed 's/^/    /' "$err.who"
  fi
  rm -f "$err.who"
done < <(git rev-list --no-merges "$range")

echo "checked $checked commit(s), $failed failed"
[ "$failed" -eq 0 ]
