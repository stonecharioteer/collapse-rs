#!/usr/bin/env bash
set -euo pipefail

workspace=/tmp/collapse-demo
output=${1:-collapse-demo.cast}
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

for command in asciinema collapse sleep-rs; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$command" >&2
    exit 1
  fi
done

rm -rf "$workspace"
mkdir -p "$workspace/bin"
install -m 755 "$(command -v collapse)" "$workspace/bin/collapse"
install -m 755 "$(command -v sleep-rs)" "$workspace/bin/sleep-rs"
install -m 755 "$repo_root/demo/demo-task" "$workspace/bin/demo-task"

cat >"$workspace/start-shell" <<'SCRIPT'
#!/bin/bash
export HOME=/tmp/collapse-demo
export USER=demo
export LOGNAME=demo
export SHELL=/bin/bash
export PATH=/tmp/collapse-demo/bin:/usr/bin:/bin
export PS1='demo@local:~/project$ '
cd /tmp/collapse-demo
exec /bin/bash --noprofile --norc -i
SCRIPT
chmod 755 "$workspace/start-shell"

printf 'Recording to %s\n' "$output"
printf 'Run this command inside the sanitized shell:\n\n'
printf "  collapse 'demo-task format' 'sleep-rs 4' 'demo-task test' 'demo-task bundle'\n\n"
printf 'Suggested flow: r, y, wait, Shift-Tab, /test, Enter, n, q, y\n\n'

SHELL=/bin/bash asciinema record \
  --window-size 110x32 \
  --idle-time-limit 1 \
  --command "$workspace/start-shell" \
  "$output"
