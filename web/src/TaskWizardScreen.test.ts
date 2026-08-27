import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { BuilderColumn, TargetColumn } from "./api";
import { SqlEditorPanel } from "./SqlEditor";
import { TaskEntryDialog } from "./TaskEntryDialog";
import { TaskWizardScreen, WizardConfirmDialog } from "./TaskWizardScreen";
import { apply, canAdvance, leaving, leavingConfirmation, openNew, view } from "./wizard";
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
  it("uses one confirmation shape for the wizard module's exact loss description", () => {
    const html = renderToStaticMarkup(createElement(WizardConfirmDialog, {
      loss: { headline: "这个改动会清掉：", lines: ["手写的过滤条件", "已勾的 1 个主键列"] },
      onCancel: () => undefined,
      onConfirm: () => undefined,
    }));
    expect(html).toContain("这个改动会清掉：");
    expect(html).toContain("手写的过滤条件");
    expect(html).toContain("已勾的 1 个主键列");
    expect(html).toContain("确认并继续");
  });

  it("puts the fetch mode first and marks the current rail step", () => {
    // 量的是「先说从哪儿取数、再给上下文」这个顺序。挑第 2 步的渲染态来量：
    // 第 1 步要占满全宽、左栏上下文根本不出现，在那儿比先后只会拿到 -1。
    const onMapping = renderWizard({ ...mappingDraft(), step: 2 });
    expect(onMapping.indexOf('class="wizard-mode"')).toBeLessThan(
      onMapping.indexOf('class="wizard-context-scroll"'),
    );

    for (const step of [1, 2, 3, 4] as Draft["step"][]) {
      expect(renderWizard({ ...openNew(SOURCE, TARGET), step }).match(/aria-current="step"/g))
        .toHaveLength(1);
    }
  });

  it("gives the wizard one polite announcer and a step heading focus can be moved to", () => {
    for (const step of [1, 2, 3, 4] as Draft["step"][]) {
      const html = renderWizard({ ...openNew(SOURCE, TARGET), step });
      // 播报口只此一个，挂在向导上而不是每一步各来一个（#239）。
      expect(html.match(/class="wizard-live visually-hidden"/g)).toHaveLength(1);
      expect(html).toMatch(/<div class="wizard-live visually-hidden" role="status" aria-live="polite">/);
      // 标题能被聚焦，但不进 Tab 序。
      expect(html.match(/<h1 tabindex="-1">/g)).toHaveLength(1);
      expect(html).not.toMatch(/<h1 tabindex="0">/);
    }
  });

  it("shows saved fields without metadata and offers one save action while editing", () => {
    const html = renderWizard({
      ...mappingDraft(),
      mode: "edit",
      taskId: "task-1",
      step: 4,
      sourceColumns: [],
      targetColumns: [],
      targetKeys: [],
    });
    expect(html).toContain("ID → ID");
    expect(html).toContain(">保存</button>");
    expect(html).not.toContain("只保存");
    expect(html).not.toMatch(/>开始导入<\/button>/);
  });

  it("offers described datasource changes only while editing", () => {
    const edit = renderToStaticMarkup(createElement(TaskWizardScreen, {
      initial: { ...mappingDraft(), mode: "edit", taskId: "task-1" },
      onCancel: () => undefined,
      onSubmit: async () => undefined,
      sourceOptions: [{ ...SOURCE, connection: "prod/orcl", agentName: "", agentStatus: null }],
      targetOptions: [{ ...TARGET, connection: "report/mysql", agentName: "sink-1", agentStatus: "online" }],
    }));
    expect(edit).toContain("源端数据源");
    expect(edit).toContain("生产 Oracle · prod/orcl");
    expect(edit).toContain("目标端数据源");
    expect(edit).toContain("报表 MySQL · report/mysql");
    expect(renderWizard(mappingDraft())).not.toContain("源端数据源");
  });

  it("distinguishes settled rows, exposes deletion, and explains disabled controls", () => {
    const html = renderWizard(done(apply(mappingDraft(), { type: "toggle-column", source: "C_NAME" })));
    expect(html).toContain("自动匹配");
    expect(html).toContain('aria-label="删除列 ID"');
    // 拒绝理由是**正文**，不是 title：只能悬停看到的解释，键盘用户拿不到（#238）。
    expect(html).not.toContain('title="先勾选这一列"');
    expect(html).not.toContain('title="请先处理当前步骤中的问题"');
    expect(html).toContain(">先勾选这一列</small>");
    expect(html).toContain(">请先处理当前步骤中的问题</small>");
  });

  it("hangs each refusal on the control it blocks as that control's description", () => {
    const html = renderWizard(done(apply(mappingDraft(), { type: "toggle-column", source: "C_NAME" })));
    for (const reason of ["先勾选这一列", "请先处理当前步骤中的问题"]) {
      const note = new RegExp(`<small class="refusal-reason" id="([^"]+)">${reason}</small>`).exec(html);
      expect(note).not.toBeNull();
      expect(html).toContain(`aria-describedby="${note?.[1]}"`);
    }
  });

  it("shows the SQL fetch refusal beside its own disabled button", () => {
    const html = renderWizard({ ...openNew(SOURCE, TARGET), fetchMode: "sql" });
    expect(html).not.toContain('title="先写好 SQL"');
    const note = /<small class="refusal-reason" id="([^"]+)">先写好 SQL<\/small>/.exec(html);
    expect(note).not.toBeNull();
    expect(html).toContain(`aria-describedby="${note?.[1]}"`);
  });

  it("names every mapping control by its column and its row's source column", () => {
    // 屏幕阅读器逐行走这张表时，只听到「复选框、复选框、组合框」是没法用的。
    const html = renderWizard(done(apply(mappingDraft(), { type: "rename-target", source: "C_NAME", target: "C_NAME" })));
    expect(html).toContain('aria-label="同步 ID"');
    expect(html).toContain('aria-label="同步 C_NAME"');
    expect(html).toContain('aria-label="C_NAME 的目标列"');
    expect(html).toContain('aria-label="ID 设为主键"');
    expect(html).toContain('aria-label="C_NAME 设为主键"');
    expect(html).toContain('<span class="visually-hidden">操作</span>');
    expect(html).not.toContain('aria-label="操作"');
  });

  it("reports both duplicate target rows and blocks advance", () => {
    let draft = done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" }));
    draft = done(apply(draft, { type: "rename-target", source: "C_NAME", target: "ID" }));

    const step = view(draft).step;
    if (step.step !== 1) throw new Error("expected mapping step");
    expect(step.rows.filter((row) => row.problem !== null).map((row) => row.source))
      .toEqual(["ID", "C_NAME"]);
    expect(canAdvance(draft, 1).length).toBeGreaterThan(0);
  });

  it("puts the missing-key reason beside the mapping controls and disables next", () => {
    const html = renderWizard(mappingDraft());
    expect(html).toContain('class="wizard-mapping-problems"');
    expect(html).toContain("映射与主键");
    expect(html).toContain("主键必选：至少要勾一列作为 upsert 的去重键");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>下一步<\/button>/);
  });

  it("makes the step body a form whose submit action is the primary advance", () => {
    const html = renderWizard(mappingDraft());
    // 回车就该进下一步（#240）：主操作是这张表单的提交动作，其余按钮仍旧不提交。
    expect(html).toContain('<form class="wizard-main"');
    expect(html).toMatch(/<button[^>]*type="submit"[^>]*>下一步<\/button>/);
    expect(html).toMatch(/<button[^>]*type="button"[^>]*>取消<\/button>/);
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

describe("the fullscreen SQL editor", () => {
  function renderEditor(fullscreen: boolean): string {
    return renderToStaticMarkup(createElement(SqlEditorPanel, {
      value: "select ID from APP.T_CUSTOMER",
      placeholder: "SELECT ID, NAME FROM APP.T_CUSTOMER",
      fullscreen,
      onFullscreen: () => undefined,
      onChange: () => undefined,
      onFormat: () => undefined,
    }));
  }

  it("claims dialog semantics only while it covers the page", () => {
    const full = renderEditor(true);
    expect(full).toContain('role="dialog"');
    expect(full).toContain('aria-modal="true"');
    expect(full).toContain('aria-label="自定义 SQL 全屏编辑"');
    expect(full).toContain('tabindex="-1"');
    expect(full).toContain('aria-label="退出全屏"');

    const inline = renderEditor(false);
    expect(inline).not.toContain('role="dialog"');
    expect(inline).not.toContain("aria-modal");
  });

  it("does not claim it inside the wizard, where the editor starts inline", () => {
    const sqlDraft = done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" }));
    expect(renderWizard(sqlDraft)).not.toContain("aria-modal");
  });
});

describe("the preview step UI", () => {
  it("offers preview only as an explicit action and renders canonical null cells", () => {
    let draft = done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" }));
    draft = done(apply(draft, { type: "advance" }));
    draft = done(apply(draft, {
      type: "preview-arrived",
      preview: {
        columns: ["ID", "C_NAME"],
        rows: [["1", null]],
        truncated: true,
        elapsed_ms: 9,
      },
    }));

    const html = renderWizard(draft);
    expect(html).toContain("预览前 10 条");
    expect(html).toContain("NULL");
    expect(html).toContain("结果已截断，仅显示前 10 条");
    expect(html).toContain("9 ms");
  });

  /**
   * 最后一屏**不再重复数据预览**。
   *
   * 原来这一条叫 "keeps the fresh preview visible on final confirmation"，断言的是
   * 第 3 步跑出来的样例数据要一路带到第 4 步。那个决定被推翻了：同一份预览在
   * 「过滤与验证」看过一次，最后一屏再放一遍并不增加判断依据，反而把真正该被看见的
   * 东西挤下去。这里改成守住新的约定——预览不在，而**不会搬过去的列**在。
   */
  it("drops the preview from final confirmation and names the columns left behind", () => {
    let draft = done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" }));
    draft = done(apply(draft, { type: "advance" }));
    draft = done(apply(draft, {
      type: "preview-arrived",
      preview: {
        columns: ["ID", "C_NAME"],
        rows: [["42", "Alice"]],
        truncated: false,
        elapsed_ms: 7,
      },
    }));
    draft = done(apply(draft, { type: "advance" }));
    draft = done(apply(draft, {
      type: "check-arrived",
      check: { ok: true, findings: [], suggested_ddl: null },
    }));
    draft = done(apply(draft, { type: "advance" }));

    const html = renderWizard(draft);
    expect(html).not.toContain('class="preview-panel"');
    expect(html).not.toContain("Alice");
    /* 两列都勾着，所以这一格说的是「一列都没落下」。 */
    expect(html).toContain("源表所有列都会搬");
  });

  it("names the unmapped source columns on final confirmation", () => {
    let draft = done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" }));
    /* C_NAME 取消勾选：它不会过线，而在此之前没有任何一屏说过这件事。 */
    draft = done(apply(draft, { type: "toggle-column", source: "C_NAME" }));
    draft = done(apply(draft, { type: "advance" }));
    draft = done(apply(draft, { type: "advance" }));
    draft = done(apply(draft, {
      type: "check-arrived",
      check: { ok: true, findings: [], suggested_ddl: null },
    }));
    draft = done(apply(draft, { type: "advance" }));

    const html = renderWizard(draft);
    expect(html).toContain("不搬的列");
    expect(html).toContain("C_NAME");
  });
});

describe("leaving the wizard", () => {
  it("still has something to confirm with when the draft has nothing to list", () => {
    // 全新草稿里挑不出可列的东西，`leaving()` 因此回 null——离开仍要过一道确认（#242）。
    // 问什么、值不值得问都在 `wizard.ts` 里定好（`leavingConfirmation`），屏幕只负责摆出来。
    expect(leaving(openNew(SOURCE, TARGET))).toBeNull();

    const html = renderToStaticMarkup(createElement(WizardConfirmDialog, {
      loss: leavingConfirmation(openNew(SOURCE, TARGET)),
      leaving: true,
      onCancel: () => undefined,
      onConfirm: () => undefined,
      onDiscard: () => undefined,
    }));
    expect(html).toContain("要离开这个向导吗？");
    expect(html).toContain("这份还没保存的草稿会留着，回来接着改。");
    expect(html).toContain("保留草稿并离开");
    expect(html).toContain("丢弃草稿并离开");
    expect(html).toContain("取消");
    // 列不出东西时不摆一个空列表。
    expect(html).not.toContain("<ul>");
  });

  it("names the wizard container the Escape handler is bound to, and lets it hold focus", () => {
    // Escape 挂在这块容器上，不在 window 上（#242）：容器外面按的那一下到不了向导。
    // 容器自己可聚焦（tabindex="-1"）：不然刚打开、还没按过 Tab 时焦点在 body 上，
    // Escape 派发的目标在容器外面，这条路要先按一下 Tab 才通。
    const html = renderWizard(openNew(SOURCE, TARGET));
    expect(html).toContain('<section class="task-wizard" tabindex="-1"');
  });

  it("does not explain a busy refresh in a tooltip on the button it disables", () => {
    // 「正在刷新」正是按钮禁用那一刻的理由，挂在 title 上一个字都不会显示（#238）；
    // 按得动时的那句提示留在 title 上没问题。
    const html = renderWizard(openNew(SOURCE, TARGET));
    expect(html).not.toContain('title="正在刷新"');
    expect(html).not.toContain('title="正在刷新结果列"');
    expect(html).toContain('title="刷新目标表清单"');
  });
});
