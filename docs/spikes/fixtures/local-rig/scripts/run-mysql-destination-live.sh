#!/usr/bin/env bash
# #196 / #262: the `MysqlDestination` behaviours only a real MySQL can answer.
# MySQL only — no Oracle, no client container, no full acceptance run.
# Idempotent; safe to re-run. Every table the tests create is dropped again.
#
# One suite, two versions (#262). MySQL 5.7 joined the support matrix as an
# **addition, not a replacement**, so the way to believe both is to point the same
# tests at each in turn — not to fork the tests, and not to trust 8.0 for 5.7.
#
#   ./run-mysql-destination-live.sh          # 8.0, the default, unchanged
#   ./run-mysql-destination-live.sh 5.7      # 5.7, on the emulated container
#   ./run-mysql-destination-live.sh both     # the matrix: 8.0 then 5.7
#
# `both` runs them one after the other rather than side by side: the two servers have
# separate process lists, but a single cargo target directory is not something to
# share between two concurrent runs.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO_ROOT=$(cd ../../../.. && pwd)

VERSION=${1:-8.0}

wait_for_health() {
  local container=$1
  echo "==> waiting for the health check on $container"
  local deadline=$(( SECONDS + 600 ))
  while :; do
    status=$(docker inspect -f '{{.State.Health.Status}}' "$container" 2>/dev/null || echo starting)
    [[ "$status" == healthy ]] && { echo "    $container healthy"; return; }
    (( SECONDS > deadline )) && { echo "!! $container timed out, status=$status"; docker logs --tail=50 "$container"; exit 1; }
    sleep 5
  done
}

run_one() {
  local version=$1
  local port

  case "$version" in
    8.0)
      echo "==> starting MySQL 8.0 (native arm64)"
      docker compose up -d mysql
      wait_for_health qbs-mysql8
      port=3306
      ;;
    5.7)
      # There is no official arm64 image for mysql:5.7 and there will not be one, so on
      # Apple Silicon this container runs under emulation. It is slow to come up, which
      # is why the health deadline above is as generous as it is.
      echo "==> starting MySQL 5.7 (linux/amd64, emulated)"
      docker compose --profile mysql57 up -d mysql57
      wait_for_health qbs-mysql57
      port=3307
      ;;
    *)
      echo "!! unknown MySQL version '$version'; expected 8.0, 5.7 or both" >&2
      exit 2
      ;;
  esac

  # Same credentials as docker-compose.yml. The tests take the connection in parts,
  # never as a DSN string, because that is the shape `TargetConnection` has.
  export DB_QBS_TEST_MYSQL_HOST=127.0.0.1
  export DB_QBS_TEST_MYSQL_PORT=$port
  export DB_QBS_TEST_MYSQL_USER=spike
  export DB_QBS_TEST_MYSQL_PASSWORD=spike123
  export DB_QBS_TEST_MYSQL_DATABASE=qbs
  # #262: one test lowers `max_allowed_packet` below the ritual's hard gate to read back
  # the message an untuned 5.7 really produces, then puts the old value back. Moving a
  # global needs a privilege the migration account deliberately does not have, so the
  # root password is its own variable — and, like the other five, it is required rather
  # than defaulted: a test that quietly decides to do nothing still reports `ok`.
  export DB_QBS_TEST_MYSQL_ROOT_PASSWORD=spike123

  # `--ignored`: these tests are `#[ignore]` so that a plain `cargo test` never needs
  # docker. This script is the thing that asks for them by name.
  # One thread: the ritual test reads `information_schema.PROCESSLIST` to see that a
  # pooled connection is busy and a second test's traffic there would muddy it, and the
  # packet test moves a *global* variable, which every other test would feel.
  echo "==> cargo test -p db-qbs-sink --test mysql_destination_live -- --ignored  [MySQL $version, port $port]"
  ( cd "$REPO_ROOT" && cargo test -p db-qbs-sink --test mysql_destination_live -- --ignored --test-threads=1 )
  echo "==> MySQL $version: green"
}

if [[ "$VERSION" == both ]]; then
  run_one 8.0
  run_one 5.7
  echo "==> the double-version matrix is green: MySQL 8.0 and MySQL 5.7"
else
  run_one "$VERSION"
fi
