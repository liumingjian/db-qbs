#!/usr/bin/env bash
# Run the #21 Oracle 11g pagination-boundary probe. The rig must already be up.
set -euo pipefail
cd "$(dirname "$0")/.."

exec docker compose exec -T client \
  sqlplus -S spike/spike123@//oracle:1521/XE \
  @/workspace/docs/spikes/fixtures/local-rig/probes/pagination-boundary.sql
