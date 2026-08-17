import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  createTask,
  deleteTask,
  fetchBuilderColumns,
  fetchBuilderTables,
  fetchColumns,
  generateBuilderTask,
  inspectSqlShape,
  cancelRun,
  fetchRun,
  listRunHistory,
  listTasks,
  startRun,
  taskInputFrom,
  updateTask,
} from "./api";

describe("task API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads tasks from the source process", async () => {
    const tasks = [
      {
        task_id: "task-01",
        name: "持仓明细",
        source_sql: "SELECT ID, D_BIZ FROM HOLDINGS",
        source_date_col: "D_BIZ",
        target_table: "HOLDINGS",
        target_date_col: "D_BIZ",
      },
    ];
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

  it("creates a task with the five editable fields", async () => {
    const input = {
      name: "持仓明细",
      source_sql: "SELECT ID, D_BIZ FROM HOLDINGS",
      source_date_col: "D_BIZ",
      target_table: "HOLDINGS",
      target_date_col: "D_BIZ",
    };
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

  it("updates and deletes a task by its stable identity", async () => {
    const input = {
      name: "持仓日明细",
      source_sql: "SELECT ID, D_BIZ FROM HOLDINGS",
      source_date_col: "D_BIZ",
      target_table: "HOLDINGS_DAILY",
      target_date_col: "D_BIZ",
    };
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
      createTask({
        name: "",
        source_sql: "",
        source_date_col: "",
        target_table: "",
        target_date_col: "",
      }),
    ).rejects.toThrow("任务定义请求体无效");
  });

  it("projects a stored task to editable fields without its identity", () => {
    const task = {
      task_id: "task-01",
      name: "持仓明细",
      source_sql: "SELECT ID, D_BIZ FROM HOLDINGS",
      source_date_col: "D_BIZ",
      target_table: "HOLDINGS",
      target_date_col: "D_BIZ",
    };

    expect(taskInputFrom(task, { name: "持仓日明细" })).toEqual({
      name: "持仓日明细",
      source_sql: task.source_sql,
      source_date_col: task.source_date_col,
      target_table: task.target_table,
      target_date_col: task.target_date_col,
    });
  });
});

describe("run history API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads history with task and business-date filters", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response("[]", {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      listRunHistory({ taskId: "task/a", bizDate: "2026-08-14" }),
    ).resolves.toEqual([]);
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/runs?task_id=task%2Fa&biz_date=2026-08-14",
      { headers: { Accept: "application/json" } },
    );
  });

  it("starts, reads, and cancels a run by its two stable inputs", async () => {
    const accepted = { run_record_id: "record/01" };
    const live = {
      run_record_id: accepted.run_record_id,
      run_id: null,
      biz_date: null,
      staging_table: null,
      stage: null,
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

    await expect(startRun("task-01", "2026-08-14")).resolves.toEqual(accepted);
    await expect(fetchRun(accepted.run_record_id)).resolves.toEqual(live);
    await expect(cancelRun(accepted.run_record_id)).resolves.toEqual(canceled);

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/runs", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ task_id: "task-01", biz_date: "2026-08-14" }),
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

    await expect(fetchBuilderTables("FA")).resolves.toEqual(tables);
    await expect(
      fetchBuilderColumns({ dblink: "FA", owner: "HTBR45", table: "T_R_FR_ASTSTAT" }),
    ).resolves.toEqual(columns);
    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/builder/tables", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ dblink: "FA" }),
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/builder/columns", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ dblink: "FA", owner: "HTBR45", table: "T_R_FR_ASTSTAT" }),
    }));
    expect(columns[0]).not.toHaveProperty("supported");
  });

  it("generates exactly the four task-definition fields", async () => {
    const output = {
      source_sql: "SELECT a.D_BIZ AS D_BIZ FROM ORDERS a WHERE a.D_BIZ >= :biz_date AND a.D_BIZ < :biz_date + 1",
      source_date_col: "D_BIZ",
      target_table: "ORDERS",
      target_date_col: "D_BIZ",
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(output), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(generateBuilderTask({
      dblink: "",
      owner: "APP",
      table: "ORDERS",
      columns: ["D_BIZ"],
      source_date_col: "D_BIZ",
      target_table: "ORDERS",
      target_date_col: "D_BIZ",
    })).resolves.toEqual(output);
    expect(Object.keys(output)).toEqual([
      "source_sql",
      "source_date_col",
      "target_table",
      "target_date_col",
    ]);
  });

  it("requests shape feedback separately from the side-effect-free column fetch", async () => {
    const task = {
      source_sql: "SELECT D_BIZ FROM ORDERS",
      source_date_col: "D_BIZ",
      target_table: "ORDERS",
      target_date_col: "D_BIZ",
    };
    const checks = [{ rule: "business_date_range", passed: false, message: "bad range" }];
    const columns = {
      // /api/columns 回的是 type，不是 builder 那个端点的 data_type
      columns: [{ name: "D_BIZ", type: "DATE", precision: null, scale: null, length: null }],
      target_ddl: "CREATE TABLE `ORDERS` (...)",
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ checks }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(columns), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(inspectSqlShape(task)).resolves.toEqual(checks);
    await expect(fetchColumns(task)).resolves.toEqual(columns);
    expect(fetchMock).toHaveBeenNthCalledWith(1, "/api/sql-shape", expect.objectContaining({ method: "POST" }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/columns", expect.objectContaining({ method: "POST" }));
  });

  it("preserves structured shape failures from column fetches", async () => {
    const body = {
      kind: "sql_shape",
      message: "source-local SQL shape precheck failed",
      checks: [
        {
          rule: "business_date_range",
          passed: false,
          message: "bad range",
        },
      ],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify(body), { status: 422 }),
      ),
    );

    const request = fetchColumns({
      source_sql: "SELECT D_BIZ FROM ORDERS",
      source_date_col: "D_BIZ",
      target_table: "ORDERS",
      target_date_col: "D_BIZ",
    });

    const error = await request.catch((requestError: unknown) => requestError);

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      message: body.message,
      status: 422,
      body,
    });
  });
});
