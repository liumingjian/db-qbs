#!/usr/bin/env python3
"""M2 acceptance child seam: wrap the real runner or emit controlled JSON Lines."""

import json
import atexit
import os
import random
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


MODE = os.environ.get("M2_CHILD_MODE", "real")
REAL_SOURCE = os.environ.get("M2_REAL_SOURCE_BIN")
PID_FILE = os.environ.get("M2_CHILD_PID_FILE")
RELEASE_FILE = os.environ.get("M2_CHILD_RELEASE_FILE")


def argument(name):
    try:
        return sys.argv[sys.argv.index(name) + 1]
    except (ValueError, IndexError):
        return None


def timestamp():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def emit(event, run_id=None, **fields):
    line = {
        "ts": timestamp(),
        "level": "info",
        "event": event,
        "component": "source",
        "run_id": run_id,
        "task": argument("--task"),
    }
    line.update(fields)
    print(json.dumps(line, ensure_ascii=False), flush=True)


def fake_run(mode):
    biz_date = argument("--biz-date") or "2026-08-14"
    run_id = datetime.now(timezone.utc).strftime("%Y%m%d%H%M%S") + f"_{random.randrange(0x1000000):06x}"
    emit("source_started", biz_date=biz_date, message="M2 controlled child started")
    emit("stage_changed", run_id, stage="PREPARING", message="preparing")
    emit("run_opened", run_id, staging_table=f"M2_NARROW__stg_{run_id}", columns_checked=3)
    emit("stage_changed", run_id, stage="STREAMING", message="streaming")
    emit("batch_pushed", run_id, seq=1, rows=3, source_rows=3, bytes=96, written=3, ms=2)

    if mode == "hang-streaming":
        while True:
            time.sleep(1)

    if mode == "fail-escape":
        emit(
            "run_finished",
            run_id,
            terminal="FAILED",
            stage="STREAMING",
            message="目标端：预检哨兵逃逸，请报 issue",
            source_code=None,
            sink_code="INTERNAL_PRECHECK_ESCAPE",
            column="V_TEXT",
            value="真实业务值-1265",
            source_rows=3,
            source_batches=1,
            staged_rows=0,
            received_batches=0,
            sink_reported_rows=0,
            purged_rows=0,
            fetch_ms=1,
            push_ms=2,
            commit_ms=0,
            count_ms=0,
            cursor_ms=1,
        )
        return 1

    raise SystemExit(f"unsupported M2_CHILD_MODE: {mode}")


def real_run():
    if not REAL_SOURCE:
        raise SystemExit("M2_REAL_SOURCE_BIN is required for real child modes")
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
        if MODE == "pause-committing":
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                payload = {}
            if payload.get("event") == "stage_changed" and payload.get("stage") == "COMMITTING":
                if not RELEASE_FILE:
                    raise SystemExit("M2_CHILD_RELEASE_FILE is required for pause-committing")
                while not Path(RELEASE_FILE).exists():
                    time.sleep(0.02)
    return process.wait()


def remove_own_pid_file():
    if not PID_FILE:
        return
    path = Path(PID_FILE)
    try:
        if path.read_text(encoding="ascii").strip() == str(os.getpid()):
            path.unlink()
    except FileNotFoundError:
        pass


if PID_FILE:
    Path(PID_FILE).write_text(str(os.getpid()), encoding="ascii")
    atexit.register(remove_own_pid_file)

if MODE in ("real", "pause-committing"):
    raise SystemExit(real_run())
raise SystemExit(fake_run(MODE))
