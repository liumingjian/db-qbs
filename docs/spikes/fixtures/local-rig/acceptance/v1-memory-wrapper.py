#!/usr/bin/env python3
"""C6 的 source 侧高水位 wrapper —— ADR-0040 §3.1。

编排进程（`db-qbs-source`）按 `run_executable` 调起一次性的 `db-qbs-source-run`。
把本文件填进 `run_executable`，它就夹在中间：转发全部 argv 与 stdout，等子进程退出时
用 `os.wait4()` 顺手取回内核记的 `ru_maxrss`。

**为什么是 wait4 而不是轮询采样**：峰值是单点事件，隔一会儿读一次 RSS 会漏掉它，
漏掉之后判据给出的是**假绿**——比没有判据更坏（ADR-0040 §3.1）。`ru_maxrss` 由内核
在进程存续期内单调维护，取的是真峰值。

**stdout 必须一字不改地转发**：编排进程靠子进程的 JSON Lines 认 run（ADR-0017）。
本 wrapper 不解析、不缓冲、不插话，只在**自己的 stderr** 和结果文件里说话。

**单位不是跨平台常量**：Linux 的 `ru_maxrss` 是 kB，macOS 是字节。台架的 source 跑在
宿主 mac 上、sink 跑在 Linux 容器里，两边都要读，所以这里既记原始值也记归一化后的字节数，
两个都进报告——将来换平台跑时，报告自己说得清那个数是怎么来的。
"""

import json
import os
import signal
import subprocess
import sys
from pathlib import Path

REAL_SOURCE = os.environ.get("V1_REAL_SOURCE_BIN")
MEMORY_FILE = os.environ.get("V1_MEMORY_FILE")


def maxrss_bytes(raw: int) -> int:
    # darwin 给字节，linux 给 kB。别的平台不猜，按 linux 算并在结果里标出平台。
    return raw if sys.platform == "darwin" else raw * 1024


def main() -> int:
    if not REAL_SOURCE:
        raise SystemExit("V1_REAL_SOURCE_BIN is required")
    if not MEMORY_FILE:
        raise SystemExit("V1_MEMORY_FILE is required")

    process = subprocess.Popen(
        [REAL_SOURCE, *sys.argv[1:]],
        stdout=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    def forward_signal(signum, _frame):
        process.send_signal(signum)

    signal.signal(signal.SIGTERM, forward_signal)
    assert process.stdout is not None
    for line in process.stdout:
        print(line, end="", flush=True)

    _pid, status, usage = os.wait4(process.pid, 0)
    # `waitstatus_to_exitcode` 要 3.9+，mac 上的系统 python3 版本不由台架说了算，
    # 这里就地拆 wait status：低 7 位是信号，高 8 位是退出码。
    exit_code = -(status & 0x7F) if status & 0x7F else status >> 8
    # **追加而不是覆盖**：编排进程一次起来要跑好几趟（基线 + 一档），而 `run_executable`
    # 与它的环境在编排进程启动时就定死了，没法一趟换一个文件名。台架读最后一行即本趟。
    with open(MEMORY_FILE, "a", encoding="utf-8") as handle:
        handle.write(
            json.dumps(
                {
                    "ru_maxrss_raw": usage.ru_maxrss,
                    "ru_maxrss_bytes": maxrss_bytes(usage.ru_maxrss),
                    "platform": sys.platform,
                    "unit": "bytes" if sys.platform == "darwin" else "kilobytes",
                    "exit_code": exit_code,
                }
            )
            + "\n"
        )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
