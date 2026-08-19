#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

runner=./scripts/run-m1-acceptance.sh
expected=$(cat <<'EOF'
wide-100k
narrow-100k
source-kill-rerun
sink-kill-rerun
commit-disconnect
commit-disconnect-discarded
empty-result
verification-failures
canonical-form
EOF
)

bash -n "$runner"
if grep -Eq '^[[:space:]]*declare[[:space:]]+-A' "$runner"; then
  echo "M1 acceptance runner must support the macOS Bash 3.2 baseline" >&2
  exit 1
fi
if grep -Eq '(^|[^[:alnum:]_])seq[[:space:]]' "$runner"; then
  echo "M1 acceptance runner must not require GNU seq on macOS" >&2
  exit 1
fi
if grep -q 'sha256sum' "$runner"; then
  echo "M1 acceptance runner must use the macOS shasum command" >&2
  exit 1
fi
grep -Fq 'shasum -a 256' "$runner" || {
  echo "M1 acceptance runner must hash target rows with SHA-256" >&2
  exit 1
}
actual=$($runner --list)
[[ "$actual" == "$expected" ]] || {
  echo "unexpected M1 scenario list" >&2
  diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual")
  exit 1
}

grep -Fq 'SOURCE_BIN=/usr/local/bin/db-qbs-source-run' "$runner" || {
  echo "M1 acceptance runner must invoke the renamed one-shot source binary" >&2
  exit 1
}
grep -Fq 'target/release/db-qbs-source-run' "$runner" || {
  echo "M1 acceptance runner must install the renamed one-shot source binary" >&2
  exit 1
}

run_id_validation_body=$(sed -n '/^assert_run_id()/,/^}/p' "$runner")
(
  fail() { return 1; }
  eval "$run_id_validation_body"
  assert_run_id "valid run_id" 20260814091530_a3f19c || exit 1
  if assert_run_id "null run_id" null; then exit 1; fi
  if assert_run_id "malformed run_id" 20260814091530_A3F19C; then exit 1; fi
) || {
  echo "M1 acceptance runner must validate JSON-log run_id values against ADR-0016" >&2
  exit 1
}

run_evidence_validation_body=$(sed -n '/^assert_run_evidence()/,/^}/p' "$runner")
evidence_dir=$(mktemp -d)
trap 'rm -rf "$evidence_dir"' EXIT
printf '%s\n' \
  '{"event":"run_opened","run_id":"20260814091530_a3f19c"}' \
  '{"event":"batch_pushed","run_id":"20260814091530_a3f19c"}' \
  '{"event":"run_finished","run_id":"20260814091530_a3f19c"}' \
  > "$evidence_dir/valid.jsonl"
printf '%s\n' \
  '{"event":"run_opened","run_id":"20260814091530_a3f19c"}' \
  '{"event":"batch_pushed","run_id":"20260814091530_b4e20d"}' \
  '{"event":"run_finished","run_id":"20260814091530_a3f19c"}' \
  > "$evidence_dir/mismatch.jsonl"
printf '%s\n' \
  '{"event":"source_started","run_id":null}' \
  '{"event":"stage_changed","run_id":"20260814091530_b4e20d"}' \
  '{"event":"run_opened","run_id":"20260814091530_a3f19c"}' \
  '{"event":"run_finished","run_id":"20260814091530_a3f19c"}' \
  > "$evidence_dir/stage-mismatch.jsonl"
printf '%s\n' \
  '{"event":"source_started"}' \
  '{"event":"run_opened","run_id":"20260814091530_a3f19c"}' \
  '{"event":"run_finished","run_id":"20260814091530_a3f19c"}' \
  > "$evidence_dir/missing-run-id.jsonl"
(
  fail() { return 1; }
  eval "$run_evidence_validation_body"
  assert_run_evidence "$evidence_dir/valid.jsonl" 20260814091530_a3f19c || exit 1
  if assert_run_evidence "$evidence_dir/mismatch.jsonl" 20260814091530_a3f19c; then exit 1; fi
  if assert_run_evidence "$evidence_dir/stage-mismatch.jsonl" 20260814091530_a3f19c; then exit 1; fi
  if assert_run_evidence "$evidence_dir/missing-run-id.jsonl" 20260814091530_a3f19c; then exit 1; fi
) || {
  echo "M1 acceptance runner must associate measured log evidence with the opened run_id" >&2
  exit 1
}

for file in \
  acceptance/oracle.sql \
  acceptance/mysql.sql \
  acceptance/task-wide.toml \
  acceptance/task-narrow.toml \
  acceptance/commit-drop-proxy.py; do
  [[ -f "$file" ]] || { echo "missing $file" >&2; exit 1; }
done

# SQL 已不在任务定义里（ADR-0036 §2）：数的是结构化规格里的列映射，不再是 `AS` 的个数。
narrow_columns=$(grep -c '{ source = ' acceptance/task-narrow.toml | tr -d ' ')
[[ "$narrow_columns" == 3 ]] || {
  echo "narrow task must map exactly 3 columns, found $narrow_columns" >&2
  exit 1
}

wide_columns=$(grep -c '{ source = ' acceptance/task-wide.toml | tr -d ' ')
[[ "$wide_columns" == 70 ]] || {
  echo "wide task must map exactly 70 columns, found $wide_columns" >&2
  exit 1
}

# 两端连接随任务文件过线（ADR-0037 §1/§8），`sink.toml` 的 mysql_dsn / database 已退役。
for task in acceptance/task-narrow.toml acceptance/task-wide.toml; do
  for section in '[oracle]' '[target]'; do
    grep -Fq "$section" "$task" || {
      echo "$task is missing its $section connection block" >&2
      exit 1
    }
  done
  grep -Fq 'primary_key = ["ROW_ID"]' "$task" || {
    echo "$task must select a primary key: upsert has no other idempotency handle" >&2
    exit 1
  }
done
if grep -Eq '^(mysql_dsn|database) ' acceptance/sink.toml; then
  echo "sink.toml must not carry the retired mysql_dsn / database fields" >&2
  exit 1
fi
grep -Fq 'target: {host:"mysql"' "$runner" || {
  echo "the direct sink runs must send the per-run target connection" >&2
  exit 1
}
wide_body=$(sed -n '/^scenario_wide()/,/^}/p' "$runner")
for assertion in '65535 / 70' 'max_rows > max_rows_per_insert'; do
  grep -Fq "$assertion" <<<"$wide_body" || {
    echo "wide scenario must prove placeholder sub-statement splitting was exercised" >&2
    exit 1
  }
done
[[ $(grep -c 'CONNECT BY LEVEL <= 100000' acceptance/oracle.sql) == 2 ]] || {
  echo "both M1 source fixtures must contain 100000 rows" >&2
  exit 1
}

if grep -En 'rows.*(==|-eq).*5000|5000.*(==|-eq).*rows' "$runner"; then
  echo "acceptance runner must not assume a batch contains 5000 rows" >&2
  exit 1
fi

source_kill_body=$(sed -n '/^scenario_source_kill()/,/^}/p' "$runner")
grep -q 'batch_pushed' <<<"$source_kill_body" || {
  echo "source-kill scenario must reach STREAMING before killing source" >&2
  exit 1
}
grep -Fq 'stage == "COMMITTING"' <<<"$source_kill_body" || {
  echo "source-kill scenario must reject a kill that raced into COMMITTING" >&2
  exit 1
}
grep -Fq 'assert_run_evidence "$killed" "$old_run"' <<<"$source_kill_body" || {
  echo "source-kill scenario must associate interrupted measurements with its run_id" >&2
  exit 1
}
for assertion in 'kill-sentinel' 'target_with_sentinel' 'target hash after source kill.*target_with_sentinel'; do
  grep -Eq "$assertion" <<<"$source_kill_body" || {
    echo "source-kill scenario must prove the target was not replaced with identical data" >&2
    exit 1
  }
done

sink_kill_body=$(sed -n '/^scenario_sink_kill()/,/^}/p' "$runner")
grep -q 'batch_pushed' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must reach STREAMING before killing the sink" >&2
  exit 1
}
grep -Fq '/tmp/m1-sink.pid' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must kill the running sink process" >&2
  exit 1
}
grep -Fq 'kill -KILL' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must kill the sink without a graceful shutdown" >&2
  exit 1
}
grep -Fq 'assert_eq "sink kill source exit" 1' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must enforce the source 0/1 exit contract" >&2
  exit 1
}
grep -Fq 'STREAMING "sink kill stage"' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must prove the run failed inside STREAMING" >&2
  exit 1
}
grep -Fq 'commit_diagnosed' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must prove no commit was diagnosed" >&2
  exit 1
}
grep -Fq 'assert_run_evidence "$killed" "$old_run"' <<<"$sink_kill_body" || {
  echo "sink-kill scenario must associate interrupted measurements with its run_id" >&2
  exit 1
}
for assertion in 'sink-kill-sentinel' 'target_with_sentinel' 'target hash after sink kill.*target_with_sentinel' 'rerun hash keeps the sentinel'; do
  grep -Eq "$assertion" <<<"$sink_kill_body" || {
    echo "sink-kill scenario must prove the target survived the kill and the rerun restored it" >&2
    exit 1
  }
done

commit_disconnect_body=$(sed -n '/^scenario_commit_disconnect()/,/^}/p' "$runner")
# 「诊断出来的 SWAPPED 确实落到了目标端」在 upsert 下靠**主键相撞的哨兵被盖掉**来证，
# 不再靠「当日范围被清空」——空结果集在 upsert 下什么都不做，那一态观察不到任何东西。
for assertion in 'commit-sentinel' 'commit disconnect sentinel overwritten'; do
  grep -Eq "$assertion" <<<"$commit_disconnect_body" || {
    echo "commit-disconnect scenario must prove the diagnosed swap reached the target" >&2
    exit 1
  }
done
grep -Fq 'assert_eq "commit disconnect source exit" 1 "$status"' <<<"$commit_disconnect_body" || {
  echo "commit-disconnect scenario must enforce the source 0/1 exit contract" >&2
  exit 1
}
grep -Fq 'assert_run_evidence "$log" "$run_id"' <<<"$commit_disconnect_body" || {
  echo "commit-disconnect scenario must associate diagnostic evidence with its run_id" >&2
  exit 1
}

commit_discard_body=$(sed -n '/^scenario_commit_disconnect_discarded()/,/^}/p' "$runner")
grep -Fq 'start_commit_drop_proxy discarded' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must drive the proxy in its discarding mode" >&2
  exit 1
}
grep -Fq 'DISCARDED "commit diagnostic terminal"' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must prove the source recovered the DISCARDED terminal" >&2
  exit 1
}
grep -Fq '目标表未被触碰' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must prove the diagnostic told the operator the target is intact" >&2
  exit 1
}
grep -Fq 'assert_eq "commit discard source exit" 1' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must enforce the source 0/1 exit contract" >&2
  exit 1
}
grep -Fq 'assert_run_evidence "$log" "$run_id"' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must associate diagnostic evidence with its run_id" >&2
  exit 1
}
grep -Eq 'commit discard target rows.*1' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must prove the discarded run left the target untouched" >&2
  exit 1
}
grep -Fq 'commit discard sentinel untouched' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must prove the colliding sentinel row was not overwritten" >&2
  exit 1
}
grep -Fq 'assert_eq "commit discard staging tables" 0' <<<"$commit_discard_body" || {
  echo "commit-discard scenario must prove the sink dropped the staging table" >&2
  exit 1
}
for mode in swapped discarded; do
  grep -Fq "\"$mode\"" acceptance/commit-drop-proxy.py || {
    echo "commit-drop proxy must support the $mode terminal" >&2
    exit 1
  }
done
grep -Fq 'M1_COMMIT_DROP_MODE' acceptance/commit-drop-proxy.py || {
  echo "commit-drop proxy must select its terminal from M1_COMMIT_DROP_MODE" >&2
  exit 1
}

prepare_body=$(sed -n '/^prepare_rig()/,/^}/p' "$runner")
grep -q 'drop_orphan_staging' <<<"$prepare_body" || {
  echo "acceptance setup must remove staging tables left by an interrupted run" >&2
  exit 1
}

cleanup_body=$(sed -n '/^cleanup()/,/^}/p' "$runner")
grep -q 'drop_orphan_staging' <<<"$cleanup_body" || {
  echo "acceptance cleanup must remove staging tables left by fault injection" >&2
  exit 1
}

stop_body=$(sed -n '/^stop_client_process()/,/^}/p' "$runner")
grep -Fq 'while kill -0 "$pid"' <<<"$stop_body" || {
  echo "acceptance runner must wait for old client processes to exit" >&2
  exit 1
}
for process in sink proxy; do
  process_body=$(sed -n "/^stop_${process}()/,/^}/p" "$runner")
  grep -Fq 'stop_client_process' <<<"$process_body" || {
    echo "acceptance runner must stop $process through the bounded wait" >&2
    exit 1
  }
done
start_sink_body=$(sed -n '/^start_sink()/,/^}/p' "$runner")
grep -Fq 'stop_sink || return 1' <<<"$start_sink_body" || {
  echo "acceptance runner must not start a sink after shutdown fails" >&2
  exit 1
}
start_proxy_body=$(sed -n '/^start_commit_drop_proxy()/,/^}/p' "$runner")
grep -Fq 'stop_proxy || return 1' <<<"$start_proxy_body" || {
  echo "acceptance runner must not start a proxy after shutdown fails" >&2
  exit 1
}

# ADR-0040 §5.1 的三处翻转：空结果集什么都不做、两个 kill 场景重跑后哨兵留存。
empty_body=$(sed -n '/^scenario_empty()/,/^}/p' "$runner")
grep -Fq "'select(.event == \"run_finished\") | .purged_rows' 0" <<<"$empty_body" || {
  echo "empty-result must assert purged_rows = 0 under the upsert write model" >&2
  exit 1
}
grep -Fq 'empty date rows untouched' <<<"$empty_body" || {
  echo "empty-result must prove the target's rows for that date were left untouched" >&2
  exit 1
}
for scenario in source_kill sink_kill; do
  body=$(sed -n "/^scenario_${scenario}()/,/^}/p" "$runner")
  grep -Fq 'assert_eq "rerun hash keeps the sentinel" "$target_with_sentinel"' <<<"$body" || {
    echo "${scenario} rerun must keep the sentinel: upsert never touches a key it did not fetch" >&2
    exit 1
  }
done

# 载荷记账（ADR-0040 §1）：客户需求「单次 10 万行、约 100MB」的兑现点在这一行上。
report_accounting=$(sed -n '/^write_report()/,/^}/p' "$runner")
for field in '载荷记账' '源行宽 bytes/row' '批体 p50' '载荷总量'; do
  grep -Fq "$field" <<<"$report_accounting" || {
    echo "M1 report is missing the payload accounting field: $field" >&2
    exit 1
  }
done

for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  grep -q "$measurement" "$runner" || {
    echo "acceptance report is missing $measurement" >&2
    exit 1
  }
done
success_body=$(sed -n '/^assert_source_success()/,/^}/p' "$runner")
grep -Fq 'assert_run_evidence "$log" "$run_id"' <<<"$success_body" || {
  echo "successful-run measurements must be associated with their opened run_id" >&2
  exit 1
}
for total in source_rows source_batches; do
  grep -Fq ".$total | numbers" <<<"$success_body" || {
    echo "successful runs must require a numeric $total terminal total" >&2
    exit 1
  }
done
grep -Fq 'source batch event count' <<<"$success_body" || {
  echo "successful runs must match source_batches to completed batch evidence" >&2
  exit 1
}
grep -Fq 'batch event sequence' <<<"$success_body" || {
  echo "successful runs must prove batch evidence covers every sequence exactly once" >&2
  exit 1
}
for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  grep -q "$measurement" <<<"$success_body" || {
    echo "successful runs must require a numeric $measurement measurement" >&2
    exit 1
  }
done
for measurement in rows bytes; do
  grep -Fq ".$measurement" <<<"$success_body" || {
    echo "successful runs must require numeric per-batch $measurement measurements" >&2
    exit 1
  }
done
[[ $(grep -c 'type.*number' <<<"$success_body") -ge 2 ]] || {
  echo "successful-run measurements must be numeric" >&2
  exit 1
}
if grep -Eq '(fetch_ms|push_ms|cursor_ms|commit_ms|count_ms|purged_rows) // 0' "$runner"; then
  echo "acceptance report must not turn unavailable terminal measurements into zero" >&2
  exit 1
fi
for measurement in fetch_ms push_ms cursor_ms commit_ms count_ms purged_rows; do
  grep -Fq ".$measurement // \"n/a\"" "$runner" || {
    echo "acceptance report must identify unavailable $measurement as n/a" >&2
    exit 1
  }
done
grep -Eq 'batch_pushed.*\.bytes|\.bytes.*batch_pushed' "$runner" || {
  echo "acceptance report is missing the batch byte distribution" >&2
  exit 1
}

report_body=$(sed -n '/^write_report()/,/^}/p' "$runner")
if grep -Fq '[[ -n "$terminal" ]] || continue' <<<"$report_body"; then
  echo "acceptance report must retain batch evidence from an interrupted run" >&2
  exit 1
fi
grep -Fq 'last // {}' <<<"$report_body" || {
  echo "acceptance report must handle an interrupted run without a terminal event" >&2
  exit 1
}
for total in source_rows source_batches; do
  grep -Fq ".$total // \"n/a\"" <<<"$report_body" || {
    echo "acceptance report must identify unavailable $total as n/a" >&2
    exit 1
  }
done
grep -q 'canonical-form\.out' <<<"$report_body" || {
  echo "acceptance report must retain the canonical-form gate output" >&2
  exit 1
}

echo "M1 acceptance script contract: PASS"
