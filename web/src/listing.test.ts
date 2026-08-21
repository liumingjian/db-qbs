import { describe, expect, it } from "vitest";

import type { Datasource, RunHistory, Task } from "./api";
import { emptySpec } from "./api";
import {
  datasourceFilterOptions,
  DEFAULT_PAGE_SIZE,
  EMPTY_HISTORY_FILTERS,
  EMPTY_TASK_FILTERS,
  historyMatchesFilters,
  latestRunByTask,
  latestRunStatus,
  paginate,
  runStatus,
  taskMatchesFilters,
} from "./listing";

function historyRow(overrides: Partial<RunHistory> = {}): RunHistory {
  return {
    run_record_id: "record-1",
    run_id: "run-1",
    task_id: "task-1",
    run_params: {},
    source_sql: "SELECT 1 FROM DUAL",
    staging_table: "STG_1",
    started_at: "2026-08-20T10:00:00.000Z",
    finished_at: "2026-08-20T10:01:00.000Z",
    outcome: "SUCCEEDED",
    target_table_effect: "SWAPPED",
    stage: "DONE",
    source_rows: 3,
    staged_rows: 3,
    sink_reported_rows: 3,
    purged_rows: 0,
    source_batches: 1,
    received_batches: 1,
    fetch_ms: 1,
    push_ms: 1,
    commit_ms: 1,
    count_ms: 1,
    cursor_ms: 1,
    source_code: null,
    sink_code: null,
    column: null,
    value: null,
    message: null,
    failure_kind: null,
    unknown_reason: null,
    seq: 1,
    rows_pushed: 3,
    bytes: 64,
    ms: 3,
    last_ts: "2026-08-20T10:01:00.000Z",
    mapping_issues: [],
    ...overrides,
  };
}

function task(overrides: Partial<Task> = {}): Task {
  return {
    task_id: "task-1",
    name: "日终订单",
    source_datasource_id: "ds-oracle",
    target_datasource_id: "ds-mysql",
    spec: { ...emptySpec(), owner: "APP", table: "ORDERS", target_table: "dw_orders" },
    ...overrides,
  };
}

describe("runStatus", () => {
  it("认的是三轴那份判定，不另立词表", () => {
    expect(runStatus(historyRow())).toBe("succeeded");
    expect(runStatus(historyRow({ outcome: "FAILED", sink_code: "VERIFY_FAILED" }))).toBe(
      "failed",
    );
    expect(runStatus(historyRow({ outcome: null, finished_at: null }))).toBe("live");
    expect(
      runStatus(
        historyRow({ outcome: null, unknown_reason: "PROCESS_DISAPPEARED" }),
      ),
    ).toBe("unknown");
  });
});

describe("latestRunByTask", () => {
  it("空历史给空表", () => {
    expect(latestRunByTask([]).size).toBe(0);
  });

  it("同一任务多条时取发起时间最大的那条", () => {
    const latest = latestRunByTask([
      historyRow({ run_record_id: "r1", started_at: "2026-08-20T10:00:00.000Z" }),
      historyRow({ run_record_id: "r3", started_at: "2026-08-20T12:00:00.000Z" }),
      historyRow({ run_record_id: "r2", started_at: "2026-08-20T11:00:00.000Z" }),
    ]);
    expect(latest.get("task-1")?.run_record_id).toBe("r3");
  });

  it("发起时间相同时按 run_record_id 定序——同一份数据两次渲染必须一致", () => {
    const rows = [
      historyRow({ run_record_id: "r-a" }),
      historyRow({ run_record_id: "r-b" }),
    ];
    expect(latestRunByTask(rows).get("task-1")?.run_record_id).toBe("r-b");
    expect(latestRunByTask([...rows].reverse()).get("task-1")?.run_record_id).toBe(
      "r-b",
    );
  });

  it("时间戳解析不出来的那条排到最旧，不会挤掉正常记录", () => {
    const latest = latestRunByTask([
      historyRow({ run_record_id: "bad", started_at: "not-a-timestamp" }),
      historyRow({ run_record_id: "good", started_at: "2026-08-20T09:00:00.000Z" }),
    ]);
    expect(latest.get("task-1")?.run_record_id).toBe("good");
  });

  it("按 task_id 分组，互不串台", () => {
    const latest = latestRunByTask([
      historyRow({ run_record_id: "r1", task_id: "task-1" }),
      historyRow({ run_record_id: "r2", task_id: "task-2" }),
    ]);
    expect(latest.get("task-1")?.run_record_id).toBe("r1");
    expect(latest.get("task-2")?.run_record_id).toBe("r2");
  });
});

describe("latestRunStatus", () => {
  it("没有历史记录时是「尚未运行」，不是失败也不是未知", () => {
    expect(latestRunStatus(undefined)).toBe("none");
  });

  it("有历史记录时就是那条记录的结局", () => {
    expect(latestRunStatus(historyRow())).toBe("succeeded");
  });
});

describe("taskMatchesFilters", () => {
  it("空筛选条放行一切", () => {
    expect(taskMatchesFilters(task(), EMPTY_TASK_FILTERS, "none")).toBe(true);
  });

  it("关键词覆盖任务名、源表与目标表，大小写不敏感", () => {
    const subject = task();
    for (const keyword of ["订单", "app.orders", "DW_ORDERS", "task-1"]) {
      expect(
        taskMatchesFilters(subject, { ...EMPTY_TASK_FILTERS, keyword }, "none"),
      ).toBe(true);
    }
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, keyword: "库存" },
        "none",
      ),
    ).toBe(false);
  });

  it("只有空白的关键词等于没填", () => {
    expect(
      taskMatchesFilters(task(), { ...EMPTY_TASK_FILTERS, keyword: "   " }, "none"),
    ).toBe(true);
  });

  it("源端 / 目标端按数据源 id 精确判", () => {
    const subject = task();
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, sourceDatasourceId: "ds-oracle" },
        "none",
      ),
    ).toBe(true);
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, sourceDatasourceId: "ds-other" },
        "none",
      ),
    ).toBe(false);
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, targetDatasourceId: "ds-mysql" },
        "none",
      ),
    ).toBe(true);
  });

  it("认不出的数据源 id（数据源已删）不会被当成「全部」放行", () => {
    const orphan = task({ source_datasource_id: "" });
    expect(
      taskMatchesFilters(
        orphan,
        { ...EMPTY_TASK_FILTERS, sourceDatasourceId: "ds-oracle" },
        "none",
      ),
    ).toBe(false);
  });

  it("最近状态筛的是传进来的那一格，「尚未运行」也能筛", () => {
    const subject = task();
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, latestStatus: "none" },
        "none",
      ),
    ).toBe(true);
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, latestStatus: "failed" },
        "none",
      ),
    ).toBe(false);
    expect(
      taskMatchesFilters(
        subject,
        { ...EMPTY_TASK_FILTERS, latestStatus: "failed" },
        "failed",
      ),
    ).toBe(true);
  });

  it("多维同时给出时是「与」，不是「或」", () => {
    expect(
      taskMatchesFilters(
        task(),
        {
          keyword: "订单",
          sourceDatasourceId: "ds-oracle",
          targetDatasourceId: "ds-other",
          latestStatus: "",
        },
        "none",
      ),
    ).toBe(false);
  });
});

describe("historyMatchesFilters", () => {
  it("空筛选条放行一切", () => {
    expect(historyMatchesFilters(historyRow(), EMPTY_HISTORY_FILTERS)).toBe(true);
  });

  it("任务与状态都判，且是「与」", () => {
    const row = historyRow({ task_id: "task-2" });
    expect(
      historyMatchesFilters(row, { taskId: "task-2", status: "succeeded" }),
    ).toBe(true);
    expect(
      historyMatchesFilters(row, { taskId: "task-2", status: "failed" }),
    ).toBe(false);
    expect(
      historyMatchesFilters(row, { taskId: "task-1", status: "succeeded" }),
    ).toBe(false);
  });

  it("进行中与结局不明各筛各的——结局不明的 outcome 也是 null，不能混进「进行中」", () => {
    const live = historyRow({ outcome: null, finished_at: null });
    const unknown = historyRow({
      outcome: null,
      unknown_reason: "SERVICE_RESTARTED",
    });
    expect(historyMatchesFilters(live, { taskId: "", status: "live" })).toBe(true);
    expect(historyMatchesFilters(unknown, { taskId: "", status: "live" })).toBe(
      false,
    );
    expect(historyMatchesFilters(unknown, { taskId: "", status: "unknown" })).toBe(
      true,
    );
  });
});

describe("paginate", () => {
  const items = Array.from({ length: 45 }, (_, index) => index + 1);

  it("空清单仍有第 1 页，不出「第 0 / 0 页」", () => {
    const slice = paginate([], 1);
    expect(slice).toMatchObject({ total: 0, pageCount: 1, page: 1 });
    expect(slice.rows).toEqual([]);
  });

  it("默认每页 20 条", () => {
    expect(DEFAULT_PAGE_SIZE).toBe(20);
    const slice = paginate(items, 1);
    expect(slice.rows).toHaveLength(20);
    expect(slice.pageCount).toBe(3);
    expect(slice.total).toBe(45);
  });

  it("最后一页只给剩下的那几条", () => {
    expect(paginate(items, 3).rows).toEqual([41, 42, 43, 44, 45]);
  });

  it("页码越界一律夹回，不给空白页", () => {
    expect(paginate(items, 99).page).toBe(3);
    expect(paginate(items, 99).rows).toEqual([41, 42, 43, 44, 45]);
    expect(paginate(items, 0).page).toBe(1);
    expect(paginate(items, -5).page).toBe(1);
  });

  it("总数不超过一页时页数是 1", () => {
    expect(paginate([1, 2, 3], 1).pageCount).toBe(1);
  });

  it("整除时不多出一个空页", () => {
    expect(paginate(items.slice(0, 40), 1).pageCount).toBe(2);
  });

  it("页大小非法时退回 1，不除以 0", () => {
    expect(paginate(items, 1, 0).pageSize).toBe(1);
    expect(paginate(items, 1, 0).pageCount).toBe(45);
  });
});

describe("datasourceFilterOptions", () => {
  const datasources: Datasource[] = [
    {
      datasource_id: "ds-oracle",
      name: "生产核心库",
      kind: "oracle",
      connect_string: "//db:1521/ORCL",
      username: "app",
      has_password: true,
    },
    {
      datasource_id: "ds-mysql",
      name: "报表库",
      kind: "mysql",
      host: "10.0.0.12",
      port: 3306,
      database: "dw",
      username: "w",
      has_password: true,
    },
  ];

  it("空清单给空选项", () => {
    expect(datasourceFilterOptions([], [], "source")).toEqual([]);
  });

  it("源端只列 Oracle、目标端只列 MySQL", () => {
    expect(datasourceFilterOptions(datasources, [], "source")).toEqual([
      ["ds-oracle", "生产核心库"],
    ]);
    expect(datasourceFilterOptions(datasources, [], "target")).toEqual([
      ["ds-mysql", "报表库"],
    ]);
  });

  it("数据源已删但任务还引用着时，那个 id 仍然进选项——否则这批任务再也筛不出来", () => {
    const orphaned = task({ source_datasource_id: "ds-gone" });
    const options = datasourceFilterOptions(datasources, [orphaned], "source");
    // 只判**进没进选项**，不判它排第几：中文与拉丁混排的先后是 ICU 排序规则说了算，
    // 各平台不一样，钉死在用例里买的是假的确定性。
    expect(options).toHaveLength(2);
    expect(options).toEqual(
      expect.arrayContaining([
        ["ds-gone", "ds-gone"],
        ["ds-oracle", "生产核心库"],
      ]),
    );
  });

  it("同一语种内按显示名排序", () => {
    const more: Datasource[] = [
      ...datasources,
      {
        datasource_id: "ds-ora-2",
        name: "备份库",
        kind: "oracle",
        connect_string: "//db2:1521/ORCL",
        username: "app",
        has_password: true,
      },
    ];
    expect(datasourceFilterOptions(more, [], "source").map(([, name]) => name)).toEqual(
      ["备份库", "生产核心库"],
    );
  });

  it("没绑数据源的任务（空 id）不制造一个空选项", () => {
    const unbound = task({ source_datasource_id: "" });
    expect(datasourceFilterOptions(datasources, [unbound], "source")).toEqual([
      ["ds-oracle", "生产核心库"],
    ]);
  });
});
