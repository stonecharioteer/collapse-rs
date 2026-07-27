#!/usr/bin/env bash
set -euo pipefail

workspace=/tmp/collapse-demo
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cast_output="$repo_root/target/demo/collapse-demo.cast"
gif_output="$repo_root/assets/demo.gif"

usage() {
  cat <<'USAGE'
Usage: scripts/record-demo.sh [options]

Record the complete PII-safe demo and render it as an animated GIF.

Options:
  --cast PATH   Write the asciinema recording to PATH
  --gif PATH    Write the animated GIF to PATH
  -h, --help    Show this help
USAGE
}

while (($#)); do
  case "$1" in
    --cast)
      cast_output=${2:?--cast requires a path}
      shift 2
      ;;
    --gif)
      gif_output=${2:?--gif requires a path}
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown option: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

for command in asciinema agg collapse sleep-rs python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$command" >&2
    if [[ $command == agg ]]; then
      printf 'install it with: cargo install --git https://github.com/asciinema/agg --tag v1.9.0 --locked\n' >&2
    fi
    exit 1
  fi
done

rm -rf "$workspace"
mkdir -p "$workspace/bin" "$(dirname "$cast_output")" "$(dirname "$gif_output")"
install -m 755 "$(command -v collapse)" "$workspace/bin/collapse"
install -m 755 "$(command -v sleep-rs)" "$workspace/bin/sleep-rs"
install -m 755 "$repo_root/demo/demo-task" "$workspace/bin/demo-task"
install -m 755 "$repo_root/demo/drive-demo.py" "$workspace/drive-demo.py"

printf 'Recording deterministic demo...\n'
SHELL=/bin/sh asciinema record \
  --headless \
  --overwrite \
  --window-size 110x32 \
  --idle-time-limit 1 \
  --capture-env SHELL \
  --command "$workspace/drive-demo.py" \
  "$cast_output"

printf 'Rendering animated GIF...\n'
agg \
  --font-size 14 \
  --fps-cap 20 \
  --idle-time-limit 1 \
  --last-frame-duration 1 \
  --select 'event:0..' \
  --cols 110 \
  --rows 32 \
  "$cast_output" \
  "$gif_output"

printf '\nCreated:\n  cast: %s\n  gif:  %s\n' "$cast_output" "$gif_output"
printf '\nThe cast was not uploaded. Upload it manually when you are ready.\n'
