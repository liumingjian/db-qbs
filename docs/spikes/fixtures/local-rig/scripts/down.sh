#!/usr/bin/env bash
# 拆台架。默认连数据卷一起删 —— 台架是一次性的，重建比修快。
set -euo pipefail
cd "$(dirname "$0")/.."
docker compose down -v --remove-orphans
