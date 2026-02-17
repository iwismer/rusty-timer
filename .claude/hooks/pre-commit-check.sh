#!/usr/bin/env bash
# Claude Code PreToolUse hook for Bash tool.
# Intercepts git commit calls and runs the pre-commit hook first.
# This script receives the tool input as JSON on stdin.

input=$(cat)

# Extract the bash command from the JSON input
command=$(printf '%s' "$input" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d.get('tool_input', {}).get('command', ''))
except:
    print('')
" 2>/dev/null || echo "")

# Only run if the bash command contains 'git commit' but not '--no-verify'
if printf '%s' "$command" | grep -qE 'git[[:space:]]+commit' && \
   ! printf '%s' "$command" | grep -q '\-\-no\-verify'; then
  REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo '.')"
  if [ -f "$REPO_ROOT/.githooks/pre-commit" ]; then
    echo "Claude pre-commit hook: running checks before commit..." >&2
    bash "$REPO_ROOT/.githooks/pre-commit" >&2
  fi
fi
