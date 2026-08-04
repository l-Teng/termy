#!/usr/bin/env bash
set -euo pipefail

BINARY="target/release/termy"
TIMEOUT_SECONDS=10
SETTLE_SECONDS=2
MAX_LAUNCH_MS=500
MAX_TREE_RSS_MB=160
MAX_TREE_CPU_PERCENT=5
MAX_PROCESS_COUNT=""
PLUGIN_SOURCE=""
WORKSPACE_STORE_SOURCE=""
EXPECT_NO_BUN=false
TEMP_ROOT=""
PID=""

usage() {
  cat <<EOF
Usage: $0 [options]

Launch the GPUI Termy binary with an isolated config and gate first-frame
readiness plus settled process-tree RSS and CPU.

Options:
  --binary PATH                 Executable to test (default: target/release/termy)
  --timeout-seconds N           Readiness timeout (default: 10)
  --settle-seconds N            Idle settling window (default: 2)
  --max-launch-ms N             Maximum process-to-ready time (default: 500)
  --max-tree-rss-mb N           Maximum settled process-tree RSS (default: 160)
  --max-tree-cpu-percent N      Maximum settled process-tree CPU (default: 5)
  --max-process-count N         Optional process-tree count ceiling
  --plugins PATH                Copy plugins from PATH into the isolated config
  --workspace-store PATH        Seed the isolated workspaces.db from PATH
  --expect-no-bun               Fail if a Bun plugin host remains after settling
EOF
}

fail() {
  echo "Error: $*" >&2
  if [[ -n "$TEMP_ROOT" && -s "$TEMP_ROOT/app.log" ]]; then
    echo "--- app output ---" >&2
    sed -n '1,160p' "$TEMP_ROOT/app.log" >&2
  fi
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary)
      [[ $# -ge 2 ]] || fail "--binary requires a value"
      BINARY="$2"
      shift 2
      ;;
    --timeout-seconds)
      [[ $# -ge 2 ]] || fail "--timeout-seconds requires a value"
      TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    --settle-seconds)
      [[ $# -ge 2 ]] || fail "--settle-seconds requires a value"
      SETTLE_SECONDS="$2"
      shift 2
      ;;
    --max-launch-ms)
      [[ $# -ge 2 ]] || fail "--max-launch-ms requires a value"
      MAX_LAUNCH_MS="$2"
      shift 2
      ;;
    --max-tree-rss-mb)
      [[ $# -ge 2 ]] || fail "--max-tree-rss-mb requires a value"
      MAX_TREE_RSS_MB="$2"
      shift 2
      ;;
    --max-tree-cpu-percent)
      [[ $# -ge 2 ]] || fail "--max-tree-cpu-percent requires a value"
      MAX_TREE_CPU_PERCENT="$2"
      shift 2
      ;;
    --max-process-count)
      [[ $# -ge 2 ]] || fail "--max-process-count requires a value"
      MAX_PROCESS_COUNT="$2"
      shift 2
      ;;
    --plugins)
      [[ $# -ge 2 ]] || fail "--plugins requires a value"
      PLUGIN_SOURCE="$2"
      shift 2
      ;;
    --workspace-store)
      [[ $# -ge 2 ]] || fail "--workspace-store requires a value"
      WORKSPACE_STORE_SOURCE="$2"
      shift 2
      ;;
    --expect-no-bun)
      EXPECT_NO_BUN=true
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

for value in "$TIMEOUT_SECONDS" "$SETTLE_SECONDS" "$MAX_LAUNCH_MS" "$MAX_TREE_RSS_MB"; do
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "integer thresholds must be positive"
done
[[ "$MAX_TREE_CPU_PERCENT" =~ ^[0-9]+([.][0-9]+)?$ ]] \
  || fail "--max-tree-cpu-percent must be numeric"
if [[ -n "$MAX_PROCESS_COUNT" ]]; then
  [[ "$MAX_PROCESS_COUNT" =~ ^[1-9][0-9]*$ ]] \
    || fail "--max-process-count must be a positive integer"
fi

BINARY="$(cd "$(dirname "$BINARY")" && pwd -P)/$(basename "$BINARY")"
[[ -x "$BINARY" ]] || fail "missing executable: $BINARY"
if [[ -n "$PLUGIN_SOURCE" ]]; then
  [[ -d "$PLUGIN_SOURCE" ]] || fail "missing plugin directory: $PLUGIN_SOURCE"
  PLUGIN_SOURCE="$(cd "$PLUGIN_SOURCE" && pwd -P)"
fi
if [[ -n "$WORKSPACE_STORE_SOURCE" ]]; then
  [[ -f "$WORKSPACE_STORE_SOURCE" ]] \
    || fail "missing workspace store: $WORKSPACE_STORE_SOURCE"
  WORKSPACE_STORE_SOURCE="$(
    cd "$(dirname "$WORKSPACE_STORE_SOURCE")"
    pwd -P
  )/$(basename "$WORKSPACE_STORE_SOURCE")"
fi

cleanup() {
  if [[ -n "$PID" ]]; then
    kill "$PID" >/dev/null 2>&1 || true
    wait "$PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TEMP_ROOT" && -d "$TEMP_ROOT" ]]; then
    rm -rf "$TEMP_ROOT"
  fi
}
trap cleanup EXIT

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/termy-gpui-perf.XXXXXX")"
ISOLATED_HOME="$TEMP_ROOT/home"
CONFIG_HOME="$TEMP_ROOT/config"
WORKING_DIR="$TEMP_ROOT/workspace"
PROBE_FILE="$TEMP_ROOT/window-ready.txt"
mkdir -p "$ISOLATED_HOME" "$CONFIG_HOME" "$WORKING_DIR"
if [[ -n "$PLUGIN_SOURCE" ]]; then
  mkdir -p "$CONFIG_HOME/termy/plugins"
  cp -R "$PLUGIN_SOURCE"/. "$CONFIG_HOME/termy/plugins/"
fi
if [[ -n "$WORKSPACE_STORE_SOURCE" ]]; then
  mkdir -p "$CONFIG_HOME/termy"
  cp "$WORKSPACE_STORE_SOURCE" "$CONFIG_HOME/termy/workspaces.db"
  printf 'native_tab_persistence = true\n' >"$CONFIG_HOME/termy/config.txt"
fi

env -i \
  HOME="$ISOLATED_HOME" \
  XDG_CONFIG_HOME="$CONFIG_HOME" \
  PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
  SHELL="/bin/zsh" \
  TMPDIR="$TEMP_ROOT" \
  TERMY_LAUNCH_PROBE_FILE="$PROBE_FILE" \
  "$BINARY" --working-directory "$WORKING_DIR" \
  >"$TEMP_ROOT/app.log" 2>&1 &
PID=$!

attempts=$((TIMEOUT_SECONDS * 20))
for ((attempt = 0; attempt < attempts; attempt++)); do
  [[ -s "$PROBE_FILE" ]] && break
  kill -0 "$PID" >/dev/null 2>&1 || fail "app exited before its first usable frame"
  sleep 0.05
done
[[ -s "$PROBE_FILE" ]] || fail "usable GPUI frame did not appear within ${TIMEOUT_SECONDS}s"

probe_value() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { print $2; exit }' "$PROBE_FILE"
}

probe_pid="$(probe_value pid)"
visible="$(probe_value visible)"
terminal_ready="$(probe_value terminal_ready)"
elapsed_ms="$(probe_value elapsed_ms)"
[[ "$probe_pid" == "$PID" ]] || fail "probe PID $probe_pid differs from launched PID $PID"
[[ "$visible" == "true" ]] || fail "probe did not report a visible frame"
[[ "$terminal_ready" == "true" ]] || fail "probe did not report a terminal"
[[ "$elapsed_ms" =~ ^[0-9]+$ ]] || fail "invalid launch duration: ${elapsed_ms:-missing}"
((elapsed_ms <= MAX_LAUNCH_MS)) \
  || fail "launch took ${elapsed_ms}ms (limit: ${MAX_LAUNCH_MS}ms)"

sleep "$SETTLE_SECONDS"
kill -0 "$PID" >/dev/null 2>&1 || fail "app exited during idle settling"

tree_pids="$PID"
frontier="$PID"
while [[ -n "$frontier" ]]; do
  next_frontier=""
  for parent_pid in $frontier; do
    children="$(pgrep -P "$parent_pid" 2>/dev/null || true)"
    [[ -z "$children" ]] && continue
    tree_pids="$tree_pids $children"
    next_frontier="$next_frontier $children"
  done
  frontier="$next_frontier"
done
process_count="$(printf '%s\n' $tree_pids | awk 'END { print NR }')"
if [[ -n "$MAX_PROCESS_COUNT" ]] && ((process_count > MAX_PROCESS_COUNT)); then
  fail "idle process tree had $process_count processes (limit: $MAX_PROCESS_COUNT)"
fi

if [[ "$EXPECT_NO_BUN" == true ]]; then
  for process_pid in $tree_pids; do
    process_command="$(ps -o comm= -p "$process_pid" 2>/dev/null | awk '{$1=$1; print}' || true)"
    case "$process_command" in
      bun|*/bun)
        fail "Bun plugin host remained alive after ${SETTLE_SECONDS}s"
        ;;
    esac
  done
fi

read -r tree_rss_kb tree_cpu_percent < <(
  for process_pid in $tree_pids; do
    ps -o rss= -o %cpu= -p "$process_pid" 2>/dev/null || true
  done | awk '{ rss += $1; cpu += $2 } END { printf "%.0f %.2f\n", rss, cpu }'
)
tree_rss_mb="$(awk -v rss="$tree_rss_kb" 'BEGIN { printf "%.1f", rss / 1024 }')"
awk -v actual="$tree_rss_mb" -v limit="$MAX_TREE_RSS_MB" \
  'BEGIN { exit !(actual <= limit) }' \
  || fail "idle process-tree RSS was ${tree_rss_mb} MiB (limit: ${MAX_TREE_RSS_MB} MiB)"
awk -v actual="$tree_cpu_percent" -v limit="$MAX_TREE_CPU_PERCENT" \
  'BEGIN { exit !(actual <= limit) }' \
  || fail "idle process-tree CPU was ${tree_cpu_percent}% (limit: ${MAX_TREE_CPU_PERCENT}%)"

echo "GPUI launch/idle gate passed: ${elapsed_ms}ms, ${tree_rss_mb} MiB, ${tree_cpu_percent}% CPU, ${process_count} processes"
