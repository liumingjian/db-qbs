import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  blockingRunsFrom,
  copyTaskCurl,
  createTask,
  deleteTask,
  deleteTaskRefusalMessage,
  emptySpec,
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchColumns,
  fetchSchedulePreview,
  fetchTargetColumns,
  fetchTargetTables,
  checkTargetTable,
  generateBuilderSql,
  previewBuilderRows,
  previewErrorMessage,
  cancelRun,
  releaseTargetHold,
  fetchRun,
  fetchEmailAlertSettings,
  fetchOperatorAccount,
  fetchSession,
  isForbidden,
  listRunHistory,
  listTasks,
  startRun,
  sendTestEmail,
  onSessionLost,
  taskInputFrom,
  updateOperatorAccount,
  updateEmailAlertSettings,
  updateTask,
} from "./api";
import type { TaskInput, TaskSpec } from "./api";

describe("role-aware session and account API", () => {
  afterEach(() => {
    onSessionLost(null);
    vi.unstubAllGlobals();
  });

  it("decodes the authenticated username and role", async () => {
    const session = { authenticated: true, username: "operator", role: "OPERATOR" };
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      new Response(JSON.stringify(session), { status: 200 }),
    ));

    await expect(fetchSession()).resolves.toEqual(session);
  });

  it("broadcasts session invalidation on 401", async () => {
    const lost = vi.fn();
    onSessionLost(lost);
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ error: { message: "请先登录" } }), { status: 401 }),
    ));

    await expect(fetchOperatorAccount()).rejects.toMatchObject({ status: 401 });
    expect(lost).toHaveBeenCalledOnce();
  });

  it("preserves the stable forbidden code without treating it as session loss", async () => {
    const lost = vi.fn();
    onSessionLost(lost);
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ error: { code: "FORBIDDEN", message: "需要管理员权限" } }),
        { status: 403 },
      ),
    ));

    const failure = await fetchOperatorAccount().catch((error: unknown) => error);
    expect(isForbidden(failure)).toBe(true);
    expect(failure).toMatchObject({ status: 403, message: "需要管理员权限" });
    expect(lost).not.toHaveBeenCalled();
  });

  it("reads and updates the fixed Operator account without returning a password", async () => {
    const account = {
      username: "operator",
      role: "OPERATOR",
      enabled: true,
      has_password: true,
    };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(account), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(account), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchOperatorAccount()).resolves.toEqual(account);
    await expect(updateOperatorAccount({ enabled: true, password: "new-secret" })).resolves.toEqual(account);
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/operator-account", {
      method: "PUT",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify({ enabled: true, password: "new-secret" }),
    });
    expect(JSON.stringify(account)).not.toContain("new-secret");
  });

  it("reads and updates write-only Email Alert settings", async () => {
    const settings = {
      enabled: false,
      provider_preset: "TENCENT_EXMAIL" as const,
      smtp_host: "smtp.exmail.qq.com",
      smtp_port: 465,
      smtp_security: "IMPLICIT_TLS" as const,
      smtp_username: "mailer",
      has_smtp_secret: true,
      sender_address: "alerts@example.com",
      sender_name: "db-qbs",
      recipients: ["ops@example.com"],
      max_retry_hours: 24,
      instance_name: "db-qbs",
      external_base_url: null,
      latest_test_result: null,
    };
    const { has_smtp_secret: _hasSecret, latest_test_result: _latestTest, ...publicInput } = settings;
    const input = { ...publicInput, smtp_secret: "" };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(settings), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify(settings), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchEmailAlertSettings()).resolves.toEqual(settings);
    await expect(updateEmailAlertSettings(input)).resolves.toEqual(settings);
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/api/email-alert-settings", {
      method: "PUT",
      headers: { Accept: "application/json", "Content-Type": "application/json" },
      body: JSON.stringify(input),
    });
    expect(settings).not.toHaveProperty("smtp_secret");
  });

  it("sends a test using the saved Email Alert settings", async () => {
    const result = {
      status: "FAILED" as const,
      tested_at: "2026-08-31T10:00:00+00:00",
      error: "SMTP 连接或响应超时",
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(result), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(sendTestEmail()).resolves.toEqual(result);
    expect(fetchMock).toHaveBeenCalledWith("/api/email-alert-settings/test", {
      method: "POST",
      headers: { Accept: "application/json" },
    });
  });
});

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
    write_mode: "APPEND",
    schedule_enabled: false,
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

  // 壳只有一种（#199）：错误正文一律是 `{"error": {...}}`，平铺的 message 不再是 API 的形状。
  it("reads the error message out of the one envelope", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({ error: { message: "取列失败", kind: "request" } }),
          { status: 400 },
        ),
      ),
    );

    await expect(fetchColumns("ds-oracle", emptySpec())).rejects.toThrow("取列失败");
  });

  // 删任务撞上还没结束的运行（#270）：服务端 409 那句话要原样到界面，
  // 点名的 run_record_id 另有一份数组，界面把它摆成列表。
  it("surfaces the in-flight refusal when a task still has a run", async () => {
    const body = {
      error: {
        message:
          "任务还有运行没结束（record-01）；请先停止这次运行，等它收尾后再删除任务",
        runs: ["record-01"],
      },
    };
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status: 409 })),
    );

    const failure = await deleteTask("task-01").catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).message).toBe(body.error.message);
    expect((failure as ApiError).status).toBe(409);
    expect(blockingRunsFrom(failure)).toEqual(["record-01"]);
  });

  // 红底那句话：名字只在列表里点一遍，句子里只说拦住的是什么、下一步做什么（#270/#271）。
  // 分辨两种拦法靠报文里那一格 `reason`，不靠猜服务端那句中文。
  it("says what to do next according to the refusal reason, not the sentence", () => {
    const refusal = (reason: string) =>
      new ApiError("服务端那句话", 409, {
        error: { message: "服务端那句话", reason, runs: ["record-01"] },
      });

    expect(
      deleteTaskRefusalMessage(refusal("RUN_IN_FLIGHT"), "服务端那句话", ["record-01"]),
    ).toBe("任务还有 1 次运行没结束；请先停止它，等它收尾后再删除任务");
    expect(
      deleteTaskRefusalMessage(refusal("TARGET_HELD"), "服务端那句话", ["record-01"]),
    ).toBe(
      "任务上一次运行的目标表占用还没释放；请先在那一行点「锁未释放，点此重试」，释放成功后再删除任务",
    );
  });

  it("falls back to the server's own sentence when it cannot tell them apart", () => {
    // 认不出的 reason（旧服务端、以后新增的拦法）与拿不到点名列表，都退回原话——
    // 一句啰嗦的实话胜过一句自己编的。
    const unknown = new ApiError("原话", 409, {
      error: { message: "原话", reason: "SOMETHING_NEW", runs: ["record-01"] },
    });
    expect(deleteTaskRefusalMessage(unknown, "原话", ["record-01"])).toBe("原话");
    expect(deleteTaskRefusalMessage(unknown, "原话", [])).toBe("原话");
    expect(deleteTaskRefusalMessage(new Error("网络断了"), "原话", ["record-01"])).toBe(
      "原话",
    );
  });

  it("only reads the run list out of a 409 shaped like the delete refusal", () => {
    expect(
      blockingRunsFrom(new ApiError("坏了", 500, { error: { message: "x", runs: ["r"] } })),
    ).toEqual([]);
    expect(blockingRunsFrom(new ApiError("坏了", 409, { error: { message: "x" } }))).toEqual([]);
    expect(blockingRunsFrom(new Error("网络断了"))).toEqual([]);
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
      task_name: "持仓明细",
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

  // #271：占用泄漏之后唯一的补救入口。发的是同一条 abort，只是由人按下。
  it("retries the target-table hold release on the run's own identity", async () => {
    const released = { message: "目标表占用已释放" };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(JSON.stringify(released), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(releaseTargetHold("record/01")).resolves.toEqual(released);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/runs/record%2F01/release",
      expect.objectContaining({ method: "POST", body: "{}" }),
    );
  });
});

describe("task cURL API", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("copies the complete server response without assembling it in the browser", async () => {
    const command = "curl --request POST 'https://qbs.test/api/runs' --data '{\"task_id\":\"task/01\"}'";
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ command }), { status: 200 }),
    );
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("fetch", fetchMock);

    await expect(copyTaskCurl("task/01", writeText)).resolves.toBeUndefined();

    expect(fetchMock).toHaveBeenCalledWith("/api/tasks/task%2F01/curl", {
      headers: { Accept: "application/json" },
    });
    expect(writeText).toHaveBeenCalledWith(command);
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

  it("asks the server for the timezone and the next fire times, and never computes them here", async () => {
    const answer = {
      timezone: "CST",
      utc_offset: "+08:00",
      now: "2026-08-28 10:07",
      next_fire_times: ["2026-08-29 02:00", "2026-08-30 02:00"],
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(JSON.stringify(answer), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchSchedulePreview("0 2 * * *")).resolves.toEqual(answer);
    expect(fetchMock).toHaveBeenCalledWith("/api/builder/schedule", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ cron: "0 2 * * *" }),
    }));
  });

  it("asks for the timezone even before an expression is typed", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          timezone: "CST",
          utc_offset: "+08:00",
          now: "2026-08-28 10:07",
          next_fire_times: [],
        }),
        { status: 200 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchSchedulePreview(null)).resolves.toMatchObject({ next_fire_times: [] });
    expect(fetchMock).toHaveBeenCalledWith("/api/builder/schedule", expect.objectContaining({
      body: JSON.stringify({ cron: null }),
    }));
  });

  it("surfaces the parser's own words when the expression is refused", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ error: { kind: "request", message: "小时字段的 25 超出取值范围 0-23" } }),
        { status: 400 },
      ),
    ));

    // 保存被拒时是同一句话，因为两条路径读的是服务端同一份解析器。
    await expect(fetchSchedulePreview("0 25 * * *")).rejects.toThrow(
      "小时字段的 25 超出取值范围 0-23",
    );
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
    // `extra` 按 **MySQL 5.7** 的取值写（#262）：5.7 的 `information_schema.COLUMNS.EXTRA`
    // 对一根 `NOT NULL DEFAULT CURRENT_TIMESTAMP` 的列给空串，`DEFAULT_GENERATED` 是 8.0 才加的。
    // 夹具里写死 8.0 独有的取值，等于把「只在 8.0 上成立」当成了通例。
    // 自增列两版都报 `auto_increment`，所以这里一并放一根，两个版本共有的形状各占一行。
    const metadata = {
      columns: [
        {
          name: "ID",
          column_type: "bigint(20)",
          data_type: "bigint",
          precision: 20,
          scale: 0,
          length: null,
          datetime_precision: null,
          nullable: false,
          character_set: null,
          ordinal: 1,
          default_value: null,
          extra: "auto_increment",
        },
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
          ordinal: 2,
          default_value: "CURRENT_TIMESTAMP",
          extra: "",
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

  it("checks the target table with both datasource identities and the current spec", async () => {
    const result = {
      ok: false,
      findings: [{
        column: "C_NAME",
        kind: "insufficient_length_or_precision",
        expected: "VARCHAR(90)",
        actual: "varchar(30)",
        message: "目标 VARCHAR 长度不足",
      }],
      suggested_ddl: "CREATE TABLE `T_POSITION` (...) ",
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(result), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const input = spec();

    await expect(
      checkTargetTable("ds-oracle", "ds-mysql", "T_POSITION", input),
    ).resolves.toEqual(result);
    expect(fetchMock).toHaveBeenCalledWith("/api/target/check", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({
        source_datasource_id: "ds-oracle",
        target_datasource_id: "ds-mysql",
        target_table: "T_POSITION",
        spec: input,
      }),
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

  it("requests an explicit ten-row preview with source binding and spec", async () => {
    const output = {
      columns: ["ID", "D_BIZ"],
      rows: [["1", "2026-08-14"], ["2", null]],
      truncated: true,
      elapsed_ms: 18,
    };
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(output), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const input = spec({ source_sql: "SELECT ID, D_BIZ FROM APP.HOLDINGS" });

    await expect(previewBuilderRows("ds-oracle", input)).resolves.toEqual(output);
    expect(fetchMock).toHaveBeenCalledWith("/api/builder/preview", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ source_datasource_id: "ds-oracle", spec: input, limit: 10 }),
    }));
  });

  it("labels preview validation, source, and timeout failures distinctly", () => {
    expect(previewErrorMessage(new ApiError("owner 不能为空", 400, {})))
      .toBe("预览请求无效：owner 不能为空");
    expect(previewErrorMessage(new ApiError("源端：ORA-00942", 502, {
      error: { message: "源端：ORA-00942", kind: "oracle", failure_kind: "SOURCE_QUERY" },
    }))).toBe("源数据库预览失败（SOURCE_QUERY）：源端：ORA-00942");
    expect(previewErrorMessage(new ApiError("源端数据预览超时", 504, {})))
      .toBe("数据预览超时：源端数据预览超时");
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
      error: {
        kind: "target_ddl",
        message: "2 column(s) cannot be expressed in the target table",
        columns: [{ column: "C_MEMO", source: "CLOB", message: "不支持的类型" }],
        described_columns: [],
      },
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
    expect(error).toMatchObject({ message: body.error.message, status: 422, body });
  });
});
