#!/usr/bin/env bash
# X1–X8 走查的一键编排：起桩后端 → 跑机器观察 → 收摊。
# 桩与探针都在本目录下（与四份 acceptance 台架同处一棵树，一起入库）。
# 探针要 playwright，装在仓库外的虚拟环境里：默认 ~/pwvenv，用 PW_PYTHON 覆盖。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
# 端口默认分两路：桩后端 18098，活台架 18088（X_RIG=1）。
if [[ "${X_RIG:-}" == "1" ]]; then PORT=${PORT:-18088}; else PORT=${PORT:-18098}; fi
PW_PYTHON=${PW_PYTHON:-$HOME/pwvenv/bin/python}

[[ -x "$PW_PYTHON" ]] || {
  echo "找不到装了 playwright 的 python：$PW_PYTHON（用 PW_PYTHON=... 指过来）" >&2
  exit 1
}

# X_RIG=1：不起桩，直接对着 `run-v1-acceptance.sh` 跑完留下的**活台架**取观察
# （#136 判的就是这一路，所有者 2026-08-19 裁定 Q5）。端口默认换成台架的 18088。
if [[ "${X_RIG:-}" == "1" ]]; then
  curl -sf "http://127.0.0.1:$PORT/api/datasources" >/dev/null || {
    echo "活台架没在 $PORT 上应答——先跑 ./scripts/run-v1-acceptance.sh（它默认不清场）" >&2
    exit 1
  }
  BASE="http://127.0.0.1:$PORT" "$HERE/x-rig-seed.sh" >&2 || exit 1
  X_RIG=1 "$PW_PYTHON" "$HERE/v1-probe.py" "$PORT"
  exit $?
fi

python3 "$HERE/v1-mock.py" "$PORT" &
MOCK=$!
trap 'kill $MOCK 2>/dev/null' EXIT
for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:$PORT/api/datasources" >/dev/null && break
  sleep 0.25
done
"$PW_PYTHON" "$HERE/v1-probe.py" "$PORT"
