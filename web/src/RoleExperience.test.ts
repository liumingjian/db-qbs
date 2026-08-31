import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { AgentScreen } from "./AgentScreen";
import { navigationItemsFor } from "./App";
import { DatasourceScreen } from "./DatasourceScreen";
import { EmailAlertSettingsView, OperatorAccountView, SystemSettingsScreen } from "./SystemSettingsScreen";
import type { Agent, Datasource, EmailAlertSettings, OperatorAccount } from "./api";

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

const EMAIL_SETTINGS: EmailAlertSettings = {
  enabled: false,
  provider_preset: "TENCENT_EXMAIL",
  smtp_host: "smtp.exmail.qq.com",
  smtp_port: 465,
  smtp_security: "IMPLICIT_TLS",
  smtp_username: "",
  has_smtp_secret: false,
  sender_address: "",
  sender_name: "",
  recipients: [],
  max_retry_hours: 24,
  instance_name: "db-qbs",
  external_base_url: null,
};
const {
  has_smtp_secret: _hasSmtpSecret,
  ...EMAIL_SETTINGS_INPUT
} = EMAIL_SETTINGS;

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

  it("keeps Email Alert and Operator Account as separate System Settings views", () => {
    const shell = renderToStaticMarkup(createElement(SystemSettingsScreen));
    expect(shell).toContain("邮件告警");
    expect(shell).toContain("操作员账号");
    expect(shell).toContain("正在读取邮件告警设置");
    expect(shell).not.toContain("正在读取操作员账号");

    const email = renderToStaticMarkup(createElement(EmailAlertSettingsView, {
      settings: EMAIL_SETTINGS,
      draft: { ...EMAIL_SETTINGS_INPUT, smtp_secret: "" },
      busy: false,
      error: null,
      saved: null,
      onChange: () => undefined,
      onSubmit: () => undefined,
    }));
    expect(email).toContain("smtp.exmail.qq.com");
    expect(email).toContain("隐式 SSL/TLS");
    expect(email).toContain("STARTTLS");
    expect(email).toContain("最大重试小时数");
    expect(email).not.toContain("测试邮件");
  });
});
