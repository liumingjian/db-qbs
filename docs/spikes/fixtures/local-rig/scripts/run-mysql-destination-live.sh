#!/usr/bin/env bash
# #196: the three `MysqlDestination` behaviours only a real MySQL can answer.
# MySQL only — no Oracle, no client container, no full acceptance run.
# Idempotent; safe to re-run. Every table the tests create is dropped again.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT=$(cd ../../../.. && pwd)

echo "==> starting MySQL 8.0 (native arm64)"
docker compose up -d mysql

echo "==> waiting for the health check"
deadline=$(( SECONDS + 300 ))
while :; do
  status=$(docker inspect -f '{{.State.Health.Status}}' qbs-mysql8 2>/dev/null || echo starting)
  [[ "$status" == healthy ]] && { echo "    qbs-mysql8 healthy"; break; }
  (( SECONDS > deadline )) && { echo "!! qbs-mysql8 timed out, status=$status"; docker logs --tail=50 qbs-mysql8; exit 1; }
  sleep 5
done

# Same credentials as docker-compose.yml. The tests take the connection in parts,
# never as a DSN string, because that is the shape `TargetConnection` has.
export DB_QBS_TEST_MYSQL_HOST=127.0.0.1
export DB_QBS_TEST_MYSQL_PORT=3306
export DB_QBS_TEST_MYSQL_USER=spike
export DB_QBS_TEST_MYSQL_PASSWORD=spike123
export DB_QBS_TEST_MYSQL_DATABASE=qbs

# `--ignored`: the three tests are `#[ignore]` so that a plain `cargo test` never
# needs docker. This script is the thing that asks for them by name.
# One thread: the ritual test reads `information_schema.PROCESSLIST` to see that a
# pooled connection is busy, and a second test's traffic there would muddy that.
echo "==> cargo test -p db-qbs-sink --test mysql_destination_live -- --ignored"
cd "$REPO_ROOT"
exec cargo test -p db-qbs-sink --test mysql_destination_live -- --ignored --test-threads=1
