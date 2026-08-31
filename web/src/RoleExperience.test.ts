import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { AgentScreen } from "./AgentScreen";
import { navigationItemsFor } from "./App";
import { DatasourceScreen } from "./DatasourceScreen";
import { OperatorAccountView } from "./SystemSettingsScreen";
import type { Agent, Datasource, OperatorAccount } from "./api";

const AGENT: Agent = {
  agent_id: "agent-1",
  name: "目标机",
  base_url: "http://127.0.0.1:8081",
  instance_id: "instance-1",
  version: "1.0.0",
  last_seen_at: "2026-08-31T00:00:00Z",
  status: "online",
  last_error: null,
  mysql_version: "8.0.40",
  mysql_collation: "utf8mb4_0900_ai_ci",
};

const DATASOURCE: Datasource = {
  datasource_id: "source-1",
  name: "源库",
  kind: "oracle",
  connect_string: "//oracle/ORCL",
  username: "app",
  has_password: true,
};

const OPERATOR: OperatorAccount = {
  username: "operator",
  role: "OPERATOR",
  enabled: true,
  has_password: true,
};

const noChange = async () => undefined;

describe("role-specific rendering", () => {
  it("shows System Settings only in Administrator navigation", () => {
    expect(navigationItemsFor("ADMIN").map((item) => item.label)).toContain("系统设置");
    expect(navigationItemsFor("OPERATOR").map((item) => item.label)).not.toContain("系统设置");
  });

  it("keeps datasource state visible but hides Operator connection and mutation controls", () => {
    const operator = renderToStaticMarkup(createElement(DatasourceScreen, {
      datasources: [DATASOURCE], agents: [AGENT], tasks: [], loading: false,
      canManage: false, onChanged: noChange,
    }));
    const admin = renderToStaticMarkup(createElement(DatasourceScreen, {
      datasources: [DATASOURCE], agents: [AGENT], tasks: [], loading: false,
      canManage: true, onChanged: noChange,
    }));

    expect(operator).toContain("源库");
    expect(operator).not.toContain("新建数据源");
    expect(operator).not.toContain("测试连接");
    expect(operator).not.toContain("编辑数据源");
    expect(operator).not.toContain("删除数据源");
    expect(operator).not.toContain(">操作<");
    expect(admin).toContain("新建数据源");
    expect(admin).toContain("测试连接");
  });

  it("keeps Agent state visible but hides Operator probe and mutation controls", () => {
    const operator = renderToStaticMarkup(createElement(AgentScreen, {
      agents: [AGENT], datasources: [], loading: false,
      canManage: false, onChanged: noChange,
    }));
    const admin = renderToStaticMarkup(createElement(AgentScreen, {
      agents: [AGENT], datasources: [], loading: false,
      canManage: true, onChanged: noChange,
    }));

    expect(operator).toContain("目标机");
    expect(operator).toContain("在线");
    expect(operator).not.toContain("注册 Agent");
    expect(operator).not.toContain("探测");
    expect(operator).not.toContain("编辑 Agent");
    expect(operator).not.toContain("删除 Agent");
    expect(admin).toContain("注册 Agent");
    expect(admin).toContain("探测");
  });

  it("renders Administrator controls for setting, resetting, enabling, and disabling Operator", () => {
    const render = (account: OperatorAccount) => renderToStaticMarkup(
      createElement(OperatorAccountView, {
        account, password: "", busy: false, error: null, saved: null,
        onPassword: () => undefined, onSubmitPassword: () => undefined, onToggle: () => undefined,
      }),
    );

    const enabled = render(OPERATOR);
    expect(enabled).toContain("操作员账号");
    expect(enabled).toContain("重置口令");
    expect(enabled).toContain("停用账号");

    const fresh = render({ ...OPERATOR, enabled: false, has_password: false });
    expect(fresh).toContain("设置口令");
    expect(fresh).toContain("启用账号");
    expect(fresh).toContain("先设置口令，再启用账号");
  });
});
