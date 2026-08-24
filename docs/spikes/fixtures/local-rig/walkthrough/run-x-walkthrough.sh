#!/usr/bin/env bash
# X1–X20 走查的一键编排：起桩后端 → 跑机器观察 → 收摊。
# 桩后端这一路跑**两趟**：常规一趟出 X1–X10 与 X12–X20，加量一趟（X_BULK=1）出 X11 的分页
# 与 X15 的跨页全选对象——填充行不进第一趟，否则前面那些态的实录会被 24 行噪声塞满。
# X15 的「未勾选禁用 / 确认框列全名字 / 汇总一行」在加量那一趟里一并观察。
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

# 端口上已经有人应答 = 上一趟的桩没收干净。**当场停下**，别接着跑：
# 那种情况下探针会对着一个陈旧的桩取观察，实录看上去正常、内容却是上一版的
# （2026-08-21 真踩过一次：X_BULK=1 的旧桩留在 18098 上，两趟都在读它）。
require_free_port() {
  if curl -sf "http://127.0.0.1:$PORT/api/datasources" >/dev/null 2>&1; then
    echo "端口 $PORT 上已经有服务在应答——多半是上一趟的桩没退干净。" >&2
    echo "先收掉它（pkill -f v1-mock.py），或用 PORT=... 换一个端口。" >&2
    exit 1
  fi
}

wait_for_mock() {
  for _ in $(seq 1 40); do
    curl -sf "http://127.0.0.1:$PORT/api/datasources" >/dev/null && return 0
    sleep 0.25
  done
  echo "桩后端没在 $PORT 上起来" >&2
  return 1
}

require_free_port
python3 "$HERE/v1-mock.py" "$PORT" &
MOCK=$!
trap 'kill $MOCK 2>/dev/null' EXIT
wait_for_mock || exit 1
"$PW_PYTHON" "$HERE/v1-probe.py" "$PORT"
STATUS=$?

kill $MOCK 2>/dev/null
wait $MOCK 2>/dev/null
# 端口交还要等一下：`kill` 只是发信号，socket 未必已经松开。
for _ in $(seq 1 20); do
  curl -sf "http://127.0.0.1:$PORT/api/datasources" >/dev/null 2>&1 || break
  sleep 0.25
done
require_free_port

echo "===== 加量一趟（X_BULK=1，跑 X11 的分页与 X15 的跨页全选） ====="
X_BULK=1 python3 "$HERE/v1-mock.py" "$PORT" &
MOCK=$!
wait_for_mock || exit 1
X_ONLY=bulk X_SHOTS="${X_SHOTS:-/tmp/v1-visual}/bulk" "$PW_PYTHON" "$HERE/v1-probe.py" "$PORT"
BULK_STATUS=$?

[[ $STATUS -eq 0 ]] || exit $STATUS
exit $BULK_STATUS
