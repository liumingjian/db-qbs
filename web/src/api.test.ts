import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  createTask,
  deleteTask,
  emptySpec,
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchColumns,
  fetchTargetColumns,
  fetchTargetTables,
  generateBuilderSql,
  cancelRun,
  fetchRun,
  listRunHistory,
  listTasks,
  startRun,
  taskInputFrom,
  updateTask,
} from "./api";
import type { TaskInput, TaskSpec } from "./api";

/** 两个数据源 id 是绑定、不是规格（ADR-0037 §8），但它们跟 `name` 一样属于任务定义。 */
function taskInput(overrides: Partial<TaskInput> = {}): TaskInput {
  return {
    name: "持仓明细",
    source_datasource_id: "ds-oracle",
    target_datasource_id: "ds-mysql",
    spec: spec(),
    ...overrides,
  };
}

function spec(overrides: Partial<TaskSpec> = {}): TaskSpec {
  return {
    ...emptySpec(),
    owner: "APP",
    table: "HOLDINGS",
    target_table: "HOLDINGS",
    columns: [
      { source: "ID", target: "ID" },
      { source: "D_BIZ", target: "D_BIZ" },
    ],
    primary_key: ["ID"],
    ...overrides,
  };
}

describe("task API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads tasks from the source process", async () => {
    const tasks = [{ task_id: "task-01", name: "持仓明细", spec: spec() }];
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(tasks), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(listTasks()).resolves.toEqual(tasks);
    expect(fetchMock).toHaveBeenCalledWith("/api/tasks", {
      headers: { Accept: "application/json" },
    });
  });

  it("creates a task as a name plus one structured spec", async () => {
    const input = taskInput();
    const created = { task_id: "task-01", ...input };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(created), { status: 201 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(createTask(input)).resolves.toEqual(created);
    expect(fetchMock).toHaveBeenCalledWith("/api/tasks", {
      method: "POST",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      body: JSON.stringify(input),
    });
  });

  it("keeps an Oracle dblink in the structured task spec", async () => {
    const input = taskInput({ spec: spec({ dblink: "FA" }) });
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ task_id: "task-01", ...input }), { status: 201 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await createTask(input);

    expect(fetchMock).toHaveBeenCalledWith("/api/tasks", expect.objectContaining({
      body: JSON.stringify(input),
    }));
  });

  it("never sends a SQL string with the task definition", async () => {
    const input = taskInput({
      name: "持仓明细",
      spec: spec({ where_clause: "D_BIZ = DATE '2026-08-14'" }),
    });
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify({ task_id: "t", ...input }), { status: 201 }));
    vi.stubGlobal("fetch", fetchMock);

    await createTask(input);

    // SQL 由规格现算，任务定义里一个字都不存——过滤那段文本除外，它本来就是定义。
    const body = String(fetchMock.mock.calls[0][1].body);
    expect(body).not.toContain("SELECT");
    expect(body).not.toContain("source_sql");
  });

  it("updates and deletes a task by its stable identity", async () => {
    const input = taskInput({
      name: "持仓日明细",
      spec: spec({ target_table: "HOLDINGS_DAILY" }),
    });
    const task = { task_id: "task/id", ...input };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(task), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(task), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(updateTask(task.task_id, input)).resolves.toEqual(task);
    await expect(deleteTask(task.task_id)).resolves.toEqual(task);
    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/tasks/task%2Fid", {
      method: "PUT",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
      body: JSON.stringify(input),
    });
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/tasks/task%2Fid", {
      method: "DELETE",
      headers: { Accept: "application/json" },
    });
  });

  it("preserves a readable API error message", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({ error: { message: "任务定义请求体无效" } }),
          { status: 400 },
        ),
      ),
    );

    await expect(
      createTask(taskInput({ name: "", spec: emptySpec() })),
    ).rejects.toThrow("任务定义请求体无效");
  });

  it("projects a stored task to name plus spec without its identity", () => {
    const task = { task_id: "task-01", ...taskInput() };

    expect(taskInputFrom(task, { name: "持仓日明细" })).toEqual({
      name: "持仓日明细",
      source_datasource_id: "ds-oracle",
      target_datasource_id: "ds-mysql",
      spec: task.spec,
    });
  });
});

describe("run history API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("filters run history by task and nothing else", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response("[]", {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    // 业务日期那一维早就没了：它从来不是一个跨任务的通用维度。
    await expect(listRunHistory({ taskId: "task/a" })).resolves.toEqual([]);
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(fetchMock).toHaveBeenCalledWith("/api/runs?task_id=task%2Fa", {
      headers: { Accept: "application/json" },
    });
  });

  it("starts, reads, and cancels a run by its stable identity alone", async () => {
    const accepted = { run_record_id: "record/01" };
    const live = {
      run_record_id: accepted.run_record_id,
      run_id: null,
      source_sql: "SELECT a.ID AS ID\n  FROM APP.HOLDINGS a",
      staging_table: null,
      stage: null,
      total_rows: null,
      precount_ms: null,
      seq: 0,
      rows_pushed: 0,
      bytes: 0,
      ms: 0,
      last_ts: null,
      live: true,
    };
    const canceled = { message: "已发送 SIGTERM" };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(accepted), { status: 202 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(live), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(canceled), { status: 202 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(startRun("task-01")).resolves.toEqual(accepted);
    await expect(fetchRun(accepted.run_record_id)).resolves.toEqual(live);
    await expect(cancelRun(accepted.run_record_id)).resolves.toEqual(canceled);

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/runs", expect.objectContaining({
      method: "POST",
      // 发起的**全部**请求体就是任务身份：没有 `run_params`，也没有别的字段。
      body: JSON.stringify({ task_id: "task-01" }),
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/runs/record%2F01", {
      headers: { Accept: "application/json" },
    });
    expect(fetchMock).toHaveBeenNthCalledWith(3, "/api/runs/record%2F01/cancel", expect.objectContaining({
      method: "POST",
    }));
  });

});

describe("SQL builder API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads transient table and column metadata without type judgments", async () => {
    const tables = [{ owner: "HTBR45", name: "T_R_FR_ASTSTAT" }];
    const columns = [
      {
        name: "C_MEMO",
        data_type: "CLOB",
        precision: null,
        scale: null,
        length: null,
        nullable: true,
      },
    ];
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(tables), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(columns), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchBuilderTables("ds-oracle", "FA")).resolves.toEqual(tables);
    await expect(
      fetchBuilderColumns({
        datasource_id: "ds-oracle",
        dblink: "FA",
        owner: "HTBR45",
        table: "T_R_FR_ASTSTAT",
      }),
    ).resolves.toEqual(columns);
    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/builder/tables", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ datasource_id: "ds-oracle", dblink: "FA" }),
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/builder/columns", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({
        datasource_id: "ds-oracle",
        dblink: "FA",
        owner: "HTBR45",
        table: "T_R_FR_ASTSTAT",
      }),
    }));
    expect(columns[0]).not.toHaveProperty("supported");
  });

  it("loads DBLINK suggestions by Oracle datasource id", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(["FA", "REPORTING"]), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchBuilderDblinks("ds-oracle")).resolves.toEqual(["FA", "REPORTING"]);
    expect(fetchMock).toHaveBeenCalledWith("/api/builder/dblinks", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ datasource_id: "ds-oracle" }),
    }));
  });

  it("describes columns from a custom source SELECT", async () => {
    const columns = [
      {
        name: "CUSTOMER_ID",
        type: "NUMBER",
        precision: 10,
        scale: 0,
        length: null,
        fsp: null,
        support: "ok",
      },
    ];
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(columns), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      fetchBuilderSqlColumns({
        datasource_id: "ds-oracle",
        source_sql: "SELECT CUSTOMER_ID FROM APP.CUSTOMER",
      }),
    ).resolves.toEqual(columns);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/builder/sql-columns",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          datasource_id: "ds-oracle",
          source_sql: "SELECT CUSTOMER_ID FROM APP.CUSTOMER",
        }),
      }),
    );
  });

  it("asks the target end for tables and columns by datasource id alone", async () => {
    // 界面只报数据源 id——凭据由 source 解一次再过线，前端一次也不碰（ADR-0037 §1/§8）。
    const metadata = {
      columns: [
        {
          name: "CREATE_TIME",
          column_type: "datetime",
          data_type: "datetime",
          precision: null,
          scale: null,
          length: null,
          datetime_precision: 0,
          nullable: false,
          character_set: null,
          ordinal: 1,
          default_value: "CURRENT_TIMESTAMP",
          extra: "DEFAULT_GENERATED",
        },
      ],
      keys: [{ name: "PRIMARY", columns: ["ID"] }],
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ tables: ["T_POSITION"] }), { status: 200 }),
      )
      .mockResolvedValueOnce(new Response(JSON.stringify(metadata), { status: 200 }))
      // 表不存在回空清单，不是错误（ADR-0038 §9）。
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ columns: [], keys: [] }), { status: 200 }),
      );
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchTargetTables("ds-mysql")).resolves.toEqual(["T_POSITION"]);
    await expect(fetchTargetColumns("ds-mysql", "T_POSITION")).resolves.toEqual(metadata);
    await expect(fetchTargetColumns("ds-mysql", "NO_SUCH_TABLE")).resolves.toEqual({
      columns: [],
      keys: [],
    });

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/target/tables", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ datasource_id: "ds-mysql" }),
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/target/columns", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ datasource_id: "ds-mysql", target_table: "T_POSITION" }),
    }));
  });

  it("exchanges a spec for the read-only SQL and its run parameters", async () => {
    const output = {
      source_sql:
        "SELECT a.ID AS ID\n  FROM APP.HOLDINGS a\n WHERE D_BIZ = DATE '2026-08-14'",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(output), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const input = spec({ where_clause: "D_BIZ = DATE '2026-08-14'" });
    await expect(generateBuilderSql(input)).resolves.toEqual(output);
    // 只出不进：送出去的是规格，回来的是 SQL，没有把 SQL 传回后端的路。
    expect(fetchMock).toHaveBeenCalledWith("/api/builder/sql", expect.objectContaining({
      method: "POST",
      body: JSON.stringify(input),
    }));
  });

  it("fetches columns from the spec, with column precision kept off the task definition", async () => {
    const columns = {
      // /api/columns 回的是 type，不是 builder 那个端点的 data_type
      columns: [{ name: "D_BIZ", type: "DATE", precision: null, scale: null, length: null }],
      target_ddl: "CREATE TABLE `ORDERS` (...)",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(columns), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(columns), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    const input = spec();
    await expect(fetchColumns("ds-oracle", input)).resolves.toEqual(columns);
    await expect(
      fetchColumns("ds-oracle", input, { N_AMT: [20, 4] }),
    ).resolves.toEqual(columns);

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/columns", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ datasource_id: "ds-oracle", spec: input }),
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/columns", expect.objectContaining({
      body: JSON.stringify({
        datasource_id: "ds-oracle",
        spec: input,
        column_precision: { N_AMT: [20, 4] },
      }),
    }));
  });

  it("preserves structured target-DDL failures from column fetches", async () => {
    const body = {
      kind: "target_ddl",
      message: "2 column(s) cannot be expressed in the target table",
      columns: [{ column: "C_MEMO", source: "CLOB", message: "不支持的类型" }],
      described_columns: [],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify(body), { status: 422 }),
      ),
    );

    const error = await fetchColumns("ds-oracle", spec()).catch(
      (requestError: unknown) => requestError,
    );

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ message: body.message, status: 422, body });
  });
});
