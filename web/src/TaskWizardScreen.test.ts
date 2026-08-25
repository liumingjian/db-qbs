import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { BuilderColumn, TargetColumn } from "./api";
import { TaskEntryDialog } from "./TaskEntryDialog";
import { TaskWizardScreen } from "./TaskWizardScreen";
import { apply, openNew } from "./wizard";
import type { Applied, Draft } from "./wizard";

const SOURCE = { datasource_id: "ds-oracle", name: "生产 Oracle" };
const TARGET = { datasource_id: "ds-mysql", name: "报表 MySQL" };

function done(result: Applied): Draft {
  if (result.kind !== "done") throw new Error("expected a direct draft change");
  return result.draft;
}

function sourceColumn(name: string): BuilderColumn {
  return { name, data_type: "VARCHAR2", precision: null, scale: null, length: 30, nullable: true };
}

function targetColumn(name: string, ordinal: number): TargetColumn {
  return {
    name,
    column_type: "varchar(30)",
    data_type: "varchar",
    precision: null,
    scale: null,
    length: 30,
    datetime_precision: null,
    nullable: true,
    character_set: "utf8mb4",
    ordinal,
    default_value: null,
    extra: "",
  };
}

function mappingDraft(): Draft {
  let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" }));
  draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
  draft = done(apply(draft, {
    type: "source-columns-arrived",
    columns: [sourceColumn("ID"), sourceColumn("C_NAME")],
  }));
  return done(apply(draft, {
    type: "target-columns-arrived",
    columns: [targetColumn("ID", 1), targetColumn("C_NAME", 2)],
    keys: [],
  }));
}

function renderWizard(draft: Draft): string {
  return renderToStaticMarkup(createElement(TaskWizardScreen, {
    initial: draft,
    onCancel: () => undefined,
    onSubmit: async () => undefined,
  }));
}

describe("the mapping step UI", () => {
  it("marks both duplicate target rows in place and disables next", () => {
    let draft = done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" }));
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "ID" }));

    const html = renderWizard(draft);
    expect(html.match(/class="is-problem"/g)).toHaveLength(2);
    expect(html.match(/目标字段 ID 重复/g)).toHaveLength(2);
    expect(html).toContain('aria-invalid="true"');
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>下一步<\/button>/);
  });

  it("puts the missing-key reason beside the mapping controls and disables next", () => {
    const html = renderWizard(mappingDraft());
    expect(html).toContain('class="wizard-mapping-problems"');
    expect(html).toContain("映射与主键");
    expect(html).toContain("主键必选：至少要勾一列作为 upsert 的去重键");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>下一步<\/button>/);
  });

  it("does not delegate wizard-entry validation to the browser", () => {
    const sqlDraft = done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" }));
    expect(renderWizard(sqlDraft)).not.toContain("required");

    const entry = renderToStaticMarkup(createElement(TaskEntryDialog, {
      guard: {
        kind: "open",
        sources: [{ ...SOURCE, connection: "db/prod", agentName: "", agentStatus: null }],
        targets: [{ ...TARGET, connection: "db/report", agentName: "target-agent", agentStatus: "online" }],
      },
      onClose: () => undefined,
      onFix: () => undefined,
      onContinue: () => undefined,
    }));
    expect(entry).toContain('<form noValidate="">');
    expect(entry).not.toContain("required");
  });
});
