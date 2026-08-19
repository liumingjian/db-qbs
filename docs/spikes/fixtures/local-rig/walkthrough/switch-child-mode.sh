#!/usr/bin/env bash
# 走查用：把台架留下的 source 按指定 child mode 重起一遍。
# run-m2-acceptance.sh 的 start_source 只在脚本内部可用，M2_KEEP_RIG 交出来的台架固定停在
# hang-streaming；V2/V6/V7/V10~V12/V15/V22 要的是 real，只能自己按同一套参数重起。
# 产品代码、验收脚本一行不动，只是换一个环境变量重启进程。
set -uo pipefail
MODE=${1:?usage: switch-child-mode.sh <real|hang-streaming|fail-escape> <work_root>}
# work_root 是 M2_KEEP_RIG 交出来的那个临时目录，每次跑都不一样——只能由调用者给，
# 也可以走 M2_WORK_ROOT 环境变量。以前这里写死过一次某台 mac 的 /var/folders/... 路径，
# 换一台机器就是死路。
WORK_ROOT=${2:-${M2_WORK_ROOT:-}}
[[ -n "$WORK_ROOT" ]] || {
  echo "usage: switch-child-mode.sh <real|hang-streaming|fail-escape> <work_root>" >&2
  echo "  work_root = M2_KEEP_RIG 留下的临时目录（内含 source.toml），或设 M2_WORK_ROOT" >&2
  exit 1
}
REPO=$(cd "$(dirname "$0")/../../../../.." && pwd)
BIN_DIR="$REPO/target/x86_64-apple-darwin/release"
WRAPPER="$REPO/docs/spikes/fixtures/local-rig/acceptance/m2-source-run-wrapper.py"

[[ -f "$WORK_ROOT/source.toml" ]] || { echo "no source.toml under $WORK_ROOT" >&2; exit 1; }

pkill -f "$WRAPPER" 2>/dev/null || true
pids=$(lsof -ti tcp:18088 2>/dev/null || true)
[[ -n "$pids" ]] && kill -KILL $pids 2>/dev/null
for _ in $(seq 1 100); do lsof -ti tcp:18088 >/dev/null 2>&1 || break; sleep 0.05; done
rm -f "$WORK_ROOT/child.pid" "$WORK_ROOT/release-child"

M2_CHILD_MODE="$MODE" \
M2_REAL_SOURCE_BIN="$BIN_DIR/db-qbs-source-run" \
M2_CHILD_PID_FILE="$WORK_ROOT/child.pid" \
M2_CHILD_RELEASE_FILE="$WORK_ROOT/release-child" \
  nohup "$BIN_DIR/db-qbs-source" --config "$WORK_ROOT/source.toml" \
  >> "$WORK_ROOT/source.jsonl" 2>&1 &
disown

for _ in $(seq 1 200); do
  [[ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18088/api/tasks)" == 200 ]] && {
    echo "source restarted with child mode: $MODE"; exit 0; }
  sleep 0.1
done
echo "source did not come up" >&2
exit 1
