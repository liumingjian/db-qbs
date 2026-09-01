import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { AgentScreen } from "./AgentScreen";
import { navigationItemsFor } from "./App";
import { DatasourceScreen } from "./DatasourceScreen";
import { EmailAlertSettingsView, EmailDeliveryHistoryView, OperatorAccountView, SystemSettingsScreen } from "./SystemSettingsScreen";
import type { Agent, Datasource, EmailAlertSettings, EmailDeliveryHistory, OperatorAccount } from "./api";

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
  latest_test_result: null,
};
const {
  has_smtp_secret: _hasSmtpSecret,
  latest_test_result: _latestTestResult,
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
      onTest: () => undefined,
    }));
    expect(email).toContain("smtp.exmail.qq.com");
    expect(email).toContain("隐式 SSL/TLS");
    expect(email).toContain("STARTTLS");
    expect(email).toContain("最大重试小时数");
    expect(email).toContain("发送测试邮件");
    expect(email).not.toContain("最新测试结果");

    const failedTest = renderToStaticMarkup(createElement(EmailAlertSettingsView, {
      settings: {
        ...EMAIL_SETTINGS,
        latest_test_result: {
          status: "FAILED",
          tested_at: "2026-08-31T10:00:00+00:00",
          error: "SMTP 连接或响应超时",
        },
      },
      draft: { ...EMAIL_SETTINGS_INPUT, smtp_secret: "" },
      busy: false,
      error: null,
      saved: null,
      onChange: () => undefined,
      onSubmit: () => undefined,
      onTest: () => undefined,
    }));
    expect(failedTest).toContain("最新测试结果");
    expect(failedTest).toContain("发送失败");
    expect(failedTest).toContain("SMTP 连接或响应超时");
  });

  it("requires an explicit acknowledgement before disabling email delivery", () => {
    const renderDisable = (confirmed: boolean) => renderToStaticMarkup(createElement(EmailAlertSettingsView, {
      settings: { ...EMAIL_SETTINGS, enabled: true },
      draft: { ...EMAIL_SETTINGS_INPUT, enabled: false, smtp_secret: "" },
      busy: false,
      error: null,
      saved: null,
      disableConfirmed: confirmed,
      onChange: () => undefined,
      onSubmit: () => undefined,
      onTest: () => undefined,
    }));

    const warning = renderDisable(false);
    expect(warning).toContain("停用会立即终止所有待发送和重试中的邮件");
    expect(warning).toContain("以后重新启用也不会补发");
    expect(warning).toContain('<button class="button is-primary" type="submit" disabled="">');

    const confirmed = renderDisable(true);
    expect(confirmed).toContain('<button class="button is-primary" type="submit">');
  });

  it("shows recipient diagnostics and manual retry only for FAILED deliveries", () => {
    const delivery = (state: EmailDeliveryHistory["state"], recipient: string): EmailDeliveryHistory => ({
      delivery_id: `delivery-${state}`,
      alert_id: "alert-record-1",
      run_record_id: "record-1",
      task_id: "task-1",
      task_name: "持仓同步",
      failed_at: "2026-08-31T10:00:00+00:00",
      recipient,
      state,
      attempt_count: state === "FAILED" ? 5 : 1,
      first_attempt_at: "2026-08-31T10:00:00+00:00",
      last_attempt_at: "2026-08-31T10:15:00+00:00",
      next_attempt_at: state === "PENDING" ? "2026-08-31T10:31:00+00:00" : null,
      retry_window_started_at: "2026-08-31T10:00:00+00:00",
      retry_deadline_at: "2026-08-31T11:00:00+00:00",
      last_error: state === "FAILED" ? "SMTP 服务器暂时拒绝请求" : null,
    });
    const markup = renderToStaticMarkup(createElement(EmailDeliveryHistoryView, {
      deliveries: [delivery("FAILED", "failed@example.com"), delivery("SENT", "sent@example.com")],
      busy: false,
      onRetry: () => undefined,
    }));

    expect(markup).toContain("投递历史");
    expect(markup).toContain("failed@example.com");
    expect(markup).toContain("SMTP 服务器暂时拒绝请求");
    expect(markup).toContain("重新发送给 failed@example.com");
    expect(markup).not.toContain("重新发送给 sent@example.com");
    expect(markup).toContain("alert-record-1");
  });
});
