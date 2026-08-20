#!/usr/bin/env bash
# 拆台架。默认连数据卷一起删 —— 台架是一次性的，重建比修快。
#
# `--profile rehearsal` 是必须的：#152 的两台演练主机在这个 profile 下，
# 不带它 `down` 会绕过这两个容器，接着因为「网络还在用」连 qbs-src-side / qbs-dst-side
# 都删不掉，留下一套半拆的台架。profile 只在 `up` 时挑选谁启动，在这里只是扩大清扫范围。
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose --profile rehearsal down -v --remove-orphans
