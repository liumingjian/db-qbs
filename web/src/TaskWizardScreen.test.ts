import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { BuilderColumn, TargetColumn } from "./api";
import type { DatasourceOption } from "./entry";
import { SqlEditorPanel } from "./SqlEditor";
import { TaskEntryDialog } from "./TaskEntryDialog";
import { sqlEcho, TaskWizardScreen, WizardConfirmDialog } from "./TaskWizardScreen";
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

/**
 * 三列的映射草稿：一行照旧自动匹配、一行取消勾选、一行目标字段留空。
 *
 * 要的是「同一屏上既有已定妥的行，又有被拒的控件，而且这一步确实过不去」。
 * #261 之前这三件事靠「一列主键都没勾」一句话就凑齐了；现在没勾主键是合法的，
 * 拦路的事得由映射本身提供，于是需要第三列。
 */
function refusalDraft(): Draft {
  let draft = done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" }));
  draft = done(apply(draft, { type: "source-table", owner: "APP", table: "T_CUSTOMER" }));
  draft = done(apply(draft, {
    type: "source-columns-arrived",
    columns: [sourceColumn("ID"), sourceColumn("C_NAME"), sourceColumn("C_CITY")],
  }));
  draft = done(apply(draft, {
    type: "target-columns-arrived",
    columns: [targetColumn("ID", 1), targetColumn("C_NAME", 2), targetColumn("C_CITY", 3)],
    keys: [],
  }));
  draft = done(apply(draft, { type: "toggle-column", source: "C_NAME" }));
  return done(apply(draft, { type: "rename-target", source: "C_CITY", target: "" }));
}

/**
 * 一份**第 1 步真的过不去**的草稿。
 *
 * 原来这个位置用的是「一列主键都没勾」，而 #261 之后那不再是问题——它是一个合法的
 * 选择（纯追加写）。改用「目标字段留空」：那是这一步真正还没做完的事。
 */
function blockedMappingDraft(): Draft {
  return done(apply(mappingDraft(), { type: "rename-target", source: "ID", target: "" }));
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

const SOURCE_OPTION = { ...SOURCE, connection: "prod/orcl", agentName: "", agentStatus: null } as const;
const SOURCE_OPTION_2 = {
  datasource_id: "ds-oracle-2", name: "备库 Oracle", connection: "standby/orcl", agentName: "", agentStatus: null,
} as const;
const TARGET_OPTION = {
  ...TARGET, connection: "report/mysql", agentName: "sink-1", agentStatus: "online",
} as const;
const TARGET_OPTION_2 = {
  datasource_id: "ds-mysql-2", name: "分析 MySQL", connection: "olap/mysql", agentName: "sink-2", agentStatus: "online",
} as const;

/**
 * 唯一的渲染缝。数据源清单是可选的：不给就是「压根没拿到部署信息」，两侧的数据源行
 * 一起不渲染——那与「恰好只有一个可选数据源」是两回事（#249）。
 */
function renderWizard(
  draft: Draft,
  options: {
    sourceOptions?: readonly DatasourceOption[];
    targetOptions?: readonly DatasourceOption[];
  } = {},
): string {
  return renderToStaticMarkup(createElement(TaskWizardScreen, {
    initial: draft,
    onCancel: () => undefined,
    onSubmit: async () => undefined,
    ...options,
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
    // 量的是「先说从哪儿取数、再给上下文」这个顺序。改版之后取数方式住在选择屏、
    // 左栏上下文住在第 2 步之后，两者再也不同屏——拿一个去比另一个只会拿到 -1，
    // 而 `-1 < 任何下标` 恒真，那条断言就成了空转。所以改成各在自己的屏上量：
    // 选择屏里取数方式在两栏之前，第 2 步之后左栏上下文确实还在。
    const onSelection = renderWizard(openNew(SOURCE, TARGET));
    const mode = onSelection.indexOf('class="wizard-mode"');
    const panes = onSelection.indexOf('class="wizard-panes"');
    expect(mode).toBeGreaterThanOrEqual(0);
    expect(panes).toBeGreaterThanOrEqual(0);
    expect(mode).toBeLessThan(panes);

    expect(renderWizard({ ...mappingDraft(), step: 2 })).toContain(
      'class="wizard-context-scroll"',
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

  it("puts the schedule, its switch and the timezone read-out on the last screen", () => {
    const html = renderWizard({
      ...mappingDraft(),
      step: 4,
      spec: { ...mappingDraft().spec, schedule_cron: "0 2 * * *", schedule_enabled: true },
      sourceColumns: [],
      targetColumns: [],
      targetKeys: [],
    });
    expect(html).toContain("周期调度");
    expect(html).toContain("cron 表达式");
    expect(html).toContain('value="0 2 * * *"');
    expect(html).toContain(">已启用<");
    // 时区那一行永远在——它是这一格里唯一一句不依赖输入的话。静态渲染下读数还没回来，
    // 所以这里能证明的是「格子在」，读数本身由 `api.test.ts` 与服务端那侧钉。
    expect(html).toContain("时区");
    expect(html).toContain("下次触发");
  });

  it("offers described datasource changes only while editing", () => {
    // 数据源两行住在第 1 步「选择数据」里（#245），所以拿一份还没选完数据的草稿来渲染——
    // 选完了的草稿开在映射步上，那一屏本来就没有这两行。
    const edit = renderToStaticMarkup(createElement(TaskWizardScreen, {
      initial: { ...openNew(SOURCE, TARGET), mode: "edit", taskId: "task-1" },
      onCancel: () => undefined,
      onSubmit: async () => undefined,
      sourceOptions: [{ ...SOURCE, connection: "prod/orcl", agentName: "", agentStatus: null }],
      targetOptions: [{ ...TARGET, connection: "report/mysql", agentName: "sink-1", agentStatus: "online" }],
    }));
    expect(edit).toContain("源端数据源");
    expect(edit).toContain("生产 Oracle · prod/orcl");
    expect(edit).toContain("目标端数据源");
    expect(edit).toContain("报表 MySQL · report/mysql");
    expect(renderWizard(openNew(SOURCE, TARGET))).not.toContain("源端数据源");
  });

  it("keeps DBLINK inside the source datasource row instead of a row of its own", () => {
    // 多一整行就多一个 --ctl-h 加一个 gap，源端卡片的上沿当场比目标端低一截（#249）。
    const html = renderWizard(
      done(apply(openNew(SOURCE, TARGET), { type: "dblink", dblink: "FIN_LINK" })),
      { sourceOptions: [SOURCE_OPTION, SOURCE_OPTION_2], targetOptions: [TARGET_OPTION] },
    );
    const rowStart = html.indexOf('<div class="wizard-pane-dsrow">');
    const colbar = html.indexOf('class="wizard-pane-colbar"', rowStart);
    // 先证明两个锚点都真的在，免得下面切出来的是一段空串（空串什么都「不包含」）。
    expect(rowStart).toBeGreaterThanOrEqual(0);
    expect(colbar).toBeGreaterThan(rowStart);
    const row = html.slice(rowStart, colbar);
    expect(row).toContain("源端数据源");
    expect(row).toContain("DBLINK");
    expect(row).toContain("FIN_LINK");
    // 没有 DBLINK 的库不多这一行，也不多这一个控件。
    expect(renderWizard(openNew(SOURCE, TARGET), { sourceOptions: [SOURCE_OPTION] }))
      .not.toContain("DBLINK");
  });

  it("anchors the missing-table badge at the field but explains it outside the two panes", () => {
    const html = renderWizard({
      ...done(apply(openNew(SOURCE, TARGET), { type: "target-table", table: "t_customer" })),
      targetTableExists: false,
    });
    // 徽标在字段旁：它就在目标表输入框那个定位容器里。切之前先证明两个锚点都真的在，
    // 免得切出来的是一段空串——空串什么都「不包含」，那种断言是空转的。
    const fieldStart = html.indexOf('<span class="wizard-pane-target-input has-badge">');
    const fieldEnd = html.indexOf('<button class="icon-button"', fieldStart);
    expect(fieldStart).toBeGreaterThanOrEqual(0);
    expect(fieldEnd).toBeGreaterThan(fieldStart);
    const field = html.slice(fieldStart, fieldEnd);
    expect(field).toContain(">尚不存在</span>");
    // 解释在两栏容器**之外**：两栏收尾的 </div> 之后紧跟着它，出现或消失都不动两栏几何。
    const note = /<p class="wizard-missing-note" id="([^"]+)">/.exec(html);
    expect(note).not.toBeNull();
    expect(html).toContain('<div class="wizard-panes">');
    expect(html).toContain(`</section></div><p class="wizard-missing-note" id="${note?.[1]}">`);
    // 而且这个 id 是输入框自己指过去的那一个（#238：理由是看得见的一行字，不是 title）。
    expect(field).toContain(`aria-describedby="${note?.[1]}"`);
  });

  it("renders equivalent datasource info on both sides when one side has a single option", () => {
    const html = renderWizard(openNew(SOURCE, TARGET), {
      sourceOptions: [SOURCE_OPTION],
      targetOptions: [TARGET_OPTION, TARGET_OPTION_2],
    });
    // 两侧同进同退：数据源行各一段，一侧下拉、一侧只读文本，都不缺。
    expect(html.match(/class="wizard-pane-dsrow"/g)).toHaveLength(2);
    expect(html).toContain('<p class="wizard-pane-ds-fixed">生产 Oracle · prod/orcl</p>');
    expect(html).toContain("目标端数据源");
    expect(html).toContain("报表 MySQL · report/mysql");
    // 没得选的那侧不摆一个 disabled 下拉：整屏只此一个 <select>，就是目标端那一个。
    expect(html.match(/<select/g)).toHaveLength(1);
    expect(html).toMatch(/目标端数据源<select/);
  });

  it("says the connection is unknown rather than quietly dropping it", () => {
    // 这一行的形状是「名字 · 连接串」。一侧有清单、另一侧没有，正是这一行存在的理由，
    // 而没清单的那一侧连接串这个进程压根没拿到——退回只报名字，两侧看着都是一行字，
    // 人却读不出这两行说的详略根本不同。
    const html = renderWizard(openNew(SOURCE, TARGET), {
      targetOptions: [TARGET_OPTION, TARGET_OPTION_2],
    });
    expect(html).toContain('<p class="wizard-pane-ds-fixed">生产 Oracle · 连接串未知</p>');
  });

  it("stops repeating the datasource name in the pane headers", () => {
    const html = renderWizard(openNew(SOURCE, TARGET), {
      sourceOptions: [SOURCE_OPTION, SOURCE_OPTION_2],
      targetOptions: [TARGET_OPTION, TARGET_OPTION_2],
    });
    // 名字由数据源行统一负责，表头只报这是哪一栏。
    expect(html).toContain('<header class="wizard-pane-head"><strong>源端</strong></header>');
    expect(html).toContain('<header class="wizard-pane-head"><strong>目标端</strong></header>');
    expect(html).not.toContain("源端 · 生产 Oracle");
    expect(html).not.toContain("目标端 · 报表 MySQL");
  });

  it("names what the source control row reports in each fetch mode", () => {
    // 控件行报的是「现在选中的是什么」，刷新按钮就摆在它报的那个状态旁边。
    const sql = renderWizard(done(apply(openNew(SOURCE, TARGET), { type: "fetch-mode", fetchMode: "sql" })));
    expect(sql).toContain('<span class="wizard-pane-colbar-label">结果列</span>');
    expect(sql).toContain("刷新结果列");
    expect(sql).not.toContain('<span class="wizard-pane-colbar-label">源表</span>');

    const byTable = renderWizard(openNew(SOURCE, TARGET));
    expect(byTable).toContain('<span class="wizard-pane-colbar-label">源表</span>');
    expect(byTable).toContain("刷新源表");
    expect(byTable).not.toContain('<span class="wizard-pane-colbar-label">结果列</span>');
  });

  it("distinguishes settled rows, exposes deletion, and explains disabled controls", () => {
    const html = renderWizard(refusalDraft());
    expect(html).toContain("自动匹配");
    expect(html).toContain('aria-label="删除列 ID"');
    // 拒绝理由是**正文**，不是 title：只能悬停看到的解释，键盘用户拿不到（#238）。
    expect(html).not.toContain('title="先勾选这一列"');
    expect(html).not.toContain('title="请先处理当前步骤中的问题"');
    expect(html).toContain(">先勾选这一列</small>");
    expect(html).toContain(">请先处理当前步骤中的问题</small>");
  });

  it("hangs each refusal on the control it blocks as that control's description", () => {
    const html = renderWizard(refusalDraft());
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

  it("puts an unfinished mapping beside the controls it belongs to and disables next", () => {
    // #261 之后「没勾主键」不再是一条 blocker——那是纯追加写，一个合法的选择。
    // 这一步真正过不去的是「这一列还没映射到目标字段」，理由挂在那一行上。
    const html = renderWizard(blockedMappingDraft());
    const step = view(blockedMappingDraft()).step;
    if (step.step !== 1) throw new Error("expected mapping step");
    expect(step.blockers).toContainEqual({
      step: 1,
      kind: "todo",
      column: "ID",
      message: "还没映射到目标字段",
    });
    expect(html).toContain("还没映射到目标字段");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>下一步<\/button>/);
  });

  it("lets a fully mapped step through even with no primary key at all", () => {
    // 同一条判定的另一半：勾不勾主键都不挡路，挡路的是没填完的映射。
    const html = renderWizard(mappingDraft());
    expect(canAdvance(mappingDraft(), 1)).toEqual([]);
    expect(html).not.toMatch(/<button[^>]*disabled=""[^>]*>下一步<\/button>/);
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
    // class 上多了 is-selection / is-steps（#245），容器是哪一个仍旧看 task-wizard。
    expect(html).toMatch(/<section class="task-wizard[^"]*" tabindex="-1"/);
  });

  it("echoes the first six lines of the SQL and says how many there are in all", () => {
    // 映射步上那块只读回显只为「认一眼这是不是我写的那段」（#245），不是读全文。
    const short = "SELECT ID,\n       C_NAME\n  FROM APP.T_CUSTOMER";
    expect(sqlEcho(short)).toBe(short);
    // 正好六行还是原样，第七行才开始截——边界上多截一行少截一行都在这儿露出来。
    const six = ["1", "2", "3", "4", "5", "6"].join("\n");
    expect(sqlEcho(six)).toBe(six);
    const eight = ["1", "2", "3", "4", "5", "6", "7", "8"].join("\n");
    expect(sqlEcho(eight)).toBe("1\n2\n3\n4\n5\n6\n…（共 8 行）");
    // 行数报的是**全文**的行数，不是截出来那几行的。
    expect(sqlEcho(eight)).not.toContain("共 6 行");
    // 前后的空白不算内容：只有空白的一段就是「还没写」，不是一段看不见的 SQL。
    expect(sqlEcho("   \n  ")).toBe("（还没写）");
    expect(sqlEcho("")).toBe("（还没写）");
    expect(sqlEcho(`\n${short}\n\n`)).toBe(short);
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

describe("写入模式那一格（#261）", () => {
  it("摆在第 1 步，且排在映射表之前——也就是紧挨着「主键」那一列", () => {
    // 规格原话是「放在挑目标表与定主键的那一步」，而向导里没有这样一步：目标表在
    // 进四步之前的选择屏上挑。两个决定里更该并排做的是写入模式与主键——它们一起
    // 决定了写出去的是哪条语句。这条断言量的就是那个相邻关系。
    const html = renderWizard(mappingDraft());

    const card = html.indexOf("写入模式");
    const table = html.indexOf("wizard-mapping");
    expect(card).toBeGreaterThan(-1);
    expect(table).toBeGreaterThan(-1);
    expect(card).toBeLessThan(table);
  });

  it("语句那一行是推导出来的，界面上说清楚它不由这里选", () => {
    const html = renderWizard(mappingDraft());

    expect(html).toContain("纯追加写");
    expect(html).toContain("由目标表有没有主键决定，不由这里选");
    expect(html).toContain("重跑会产生重复数据");
  });

  it("勾上主键之后，同一格改口说 upsert", () => {
    const html = renderWizard(
      done(apply(mappingDraft(), { type: "toggle-primary-key", target: "ID" })),
    );

    expect(html).toContain("按主键 upsert");
    expect(html).not.toContain("重跑会产生重复数据");
  });

  it("模式本身是一组单选按钮，不是一行只读文字——多一档时这里什么都不用改", () => {
    const html = renderWizard(mappingDraft());

    expect(html).toContain('name="write-mode"');
    expect(html).toContain("追加写");
  });
});
