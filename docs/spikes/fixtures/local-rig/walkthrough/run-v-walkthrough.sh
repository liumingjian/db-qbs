#!/usr/bin/env bash
# 整份 V1–V25 走查的一键编排：起桩后端 → 跑机器观察 → 收摊。
# 桩与探针都在本目录下（与四份 acceptance 台架同处一棵树，一起入库）。
# 探针要 playwright，装在仓库外的虚拟环境里：默认 ~/pwvenv，用 PW_PYTHON 覆盖。
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
PORT=${PORT:-18097}
PW_PYTHON=${PW_PYTHON:-$HOME/pwvenv/bin/python}

[[ -x "$PW_PYTHON" ]] || {
  echo "找不到装了 playwright 的 python：$PW_PYTHON（用 PW_PYTHON=... 指过来）" >&2
  exit 1
}

python3 "$HERE/v-mock.py" "$PORT" &
MOCK=$!
trap 'kill $MOCK 2>/dev/null' EXIT
for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:$PORT/api/tasks" >/dev/null && break
  sleep 0.25
done
"$PW_PYTHON" "$HERE/v-probe.py" "$PORT"
