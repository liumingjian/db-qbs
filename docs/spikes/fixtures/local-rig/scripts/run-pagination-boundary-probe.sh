#!/usr/bin/env bash
# 跑 #21 的 Oracle 11g 分页边界探针。台架需已 up。
set -euo pipefail
cd "$(dirname "$0")/.."

exec docker compose exec -T client \
  sqlplus -S spike/spike123@//oracle:1521/XE \
  @/workspace/docs/spikes/fixtures/local-rig/probes/pagination-boundary.sql
