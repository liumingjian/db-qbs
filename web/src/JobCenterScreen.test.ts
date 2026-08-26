import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { Task } from "./api";
import { JobCenterScreen } from "./JobCenterScreen";

const NEVER_RUN_TASK: Task = {
  task_id: "task-1",
  name: "客户主档",
  source_datasource_id: "source-1",
  target_datasource_id: "target-1",
  spec: {
    owner: "APP",
    table: "CUSTOMER",
    target_table: "customer",
    columns: [{ source: "ID", target: "ID" }],
    primary_key: ["ID"],
    where_clause: "",
  },
};

describe("job row actions", () => {
  it("omits run details when a task has never run", () => {
    const html = renderToStaticMarkup(createElement(JobCenterScreen, {
      tasks: [NEVER_RUN_TASK],
      datasources: [],
      latestRuns: new Map(),
      refreshing: false,
      onRefresh: () => undefined,
      onCreate: () => undefined,
      onEdit: () => undefined,
      onRename: () => undefined,
      onDelete: () => undefined,
      startingTaskId: null,
      onStart: () => undefined,
      onStop: () => undefined,
      onRerun: () => undefined,
      onEditFailure: () => undefined,
      onChanged: () => undefined,
      focusTaskId: null,
      onFocusConsumed: () => undefined,
    }));

    expect(html).toContain("发起运行");
    expect(html).not.toContain("运行详情");
  });
});
