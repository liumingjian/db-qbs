import { afterEach, describe, expect, it, vi } from "vitest";

import { createTask, deleteTask, listTasks, taskInputFrom, updateTask } from "./api";

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
