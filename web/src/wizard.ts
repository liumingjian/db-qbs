// The Task Draft: a Task Definition while it is being built, and every rule that
// governs it.
//
// Everything here is a rule, not rendering, which is why none of it is allowed to
// live in a `.tsx` file. There is no DOM harness in this project, so a rule that
// sits inside a component is a rule nobody can test — and the rules that used to
// sit there were six hand-written copies of "changing X clears Y" that disagreed
// with each other about what gets cleared and about whether the person is asked
// first.
//
// The shape that fixes that is one reducer. `apply` runs it, sees what the change
// would clear, and only then decides whether a confirmation is owed. Clearing and
// confirming therefore cannot drift apart: the confirmation is derived from the
// clearing rather than written beside it.
//
// Nothing here performs IO. Fetched results (the ten-row preview, the target-table
// check) arrive as changes, which is what lets one place decide when a result has
// gone stale.

import { emptySpec } from "./api";
import type {
  BuilderColumn,
  ColumnMapping,
  PreviewResult,
  Task,
  TargetColumn,
  TargetKey,
  TaskSpec,
} from "./api";
import { matchSameNameTargets, sourceSummary, whereSummary } from "./spec";

export type Mode = "create" | "edit";
export type Step = 1 | 2 | 3 | 4;
export type FetchMode = "table" | "sql";

/** The datasource the whole draft is carried out under. Chosen before entry. */
export interface DraftBinding {
  datasource_id: string;
  name: string;
}

/** The target-table check moves to `api.ts` when its endpoint lands. */
export interface CheckFinding {
  column: string | null;
  kind: string;
  expected: string;
  actual: string;
  message: string;
}

export interface TargetCheckResult {
  ok: boolean;
  findings: CheckFinding[];
  suggested_ddl: string | null;
}

/** A fetched result, pinned to the inputs it was fetched for. */
interface Fetched<T> {
  value: T;
  inputs: string;
}

/**
 * Which parts of the draft the person made themselves.
 *
 * This is the whole basis of the confirmation rule: only a hand-made value is
 * worth interrupting someone to protect. A same-name mapping, an inferred
 * primary key, a prefilled target table name and a generated task name were all
 * put there by the machine, and clearing them costs nobody anything.
 *
 * Editing a saved task sets every flag at load: what is on screen is
 * indistinguishable from what the person typed, and wiping it silently is the
 * same fright — worse, in fact, because saving afterwards makes it permanent.
 */
interface HandMade {
  sourceTable: boolean;
  columns: boolean;
  primaryKey: boolean;
  where: boolean;
  sql: boolean;
  targetTable: boolean;
  taskName: boolean;
  /** Source columns whose target field was typed rather than matched. */
  mappings: readonly string[];
}

export interface Draft {
  mode: Mode;
  step: Step;
  taskId: string | null;
  source: DraftBinding;
  target: DraftBinding;
  /** Liveness of the target's agent. Reported in, never fetched here. */
  targetAgentOnline: boolean;
  fetchMode: FetchMode;
  spec: TaskSpec;
  name: string;
  hand: HandMade;
  sourceColumns: readonly BuilderColumn[];
  targetColumns: readonly TargetColumn[];
  targetKeys: readonly TargetKey[];
  preview: Fetched<PreviewResult> | null;
  check: Fetched<TargetCheckResult> | null;
}

export type Change =
  | { type: "source-datasource"; datasource: DraftBinding }
  | { type: "dblink"; dblink: string }
  | { type: "fetch-mode"; fetchMode: FetchMode }
  | { type: "source-table"; owner: string; table: string }
  | { type: "source-sql"; sql: string }
  | { type: "format-sql"; sql: string }
  | { type: "target-datasource"; datasource: DraftBinding; online: boolean }
  | { type: "target-agent-status"; online: boolean }
  | { type: "target-table"; table: string }
  | { type: "toggle-column"; source: string }
  | { type: "remove-column"; source: string }
  | { type: "rename-target"; source: string; target: string }
  | { type: "toggle-primary-key"; target: string }
  | { type: "where"; clause: string }
  | { type: "task-name"; name: string }
  | { type: "refresh-target-columns" }
  | { type: "drop-orphan-mappings" }
  | { type: "source-columns-arrived"; columns: readonly BuilderColumn[] }
  | {
      type: "target-columns-arrived";
      columns: readonly TargetColumn[];
      keys: readonly TargetKey[];
    }
  | { type: "preview-arrived"; preview: PreviewResult }
  | { type: "check-arrived"; check: TargetCheckResult }
  | { type: "advance" }
  | { type: "back" }
  | { type: "leave" };

/** What a change would clear. One name per rule, so the rules can be tested by name. */
export type Cleared =
  | "source-table"
  | "columns"
  | "primary-key"
  | "where"
  | "sql"
  | "target-table"
  | "mappings"
  | "draft";

/**
 * What the confirmation dialog says, in the order it says it.
 *
 * The lines are Chinese because they are read on screen, and they live here
 * rather than in the screen because "does this action clear anything worth
 * mentioning" is exactly the judgement that has to be testable.
 */
export interface Loss {
  headline: string;
  lines: string[];
}

export type Applied =
  | { kind: "done"; draft: Draft }
  | { kind: "needs-confirm"; intent: Change; loses: Loss };

/** One reason the next step is out of reach, located to a column where it can be. */
export interface Blocker {
  step: Step;
  /** The source column the problem belongs to, or `null` for a whole-step one. */
  column: string | null;
  message: string;
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

function noneHandMade(): HandMade {
  return {
    sourceTable: false,
    columns: false,
    primaryKey: false,
    where: false,
    sql: false,
    targetTable: false,
    taskName: false,
    mappings: [],
  };
}

function allHandMade(): HandMade {
  return {
    sourceTable: true,
    columns: true,
    primaryKey: true,
    where: true,
    sql: true,
    targetTable: true,
    taskName: true,
    mappings: [],
  };
}

export function openNew(
  source: DraftBinding,
  target: DraftBinding,
  targetAgentOnline = true,
): Draft {
  return {
    mode: "create",
    step: 1,
    taskId: null,
    source,
    target,
    targetAgentOnline,
    fetchMode: "table",
    spec: emptySpec(),
    name: "",
    hand: noneHandMade(),
    sourceColumns: [],
    targetColumns: [],
    targetKeys: [],
    preview: null,
    check: null,
  };
}

/**
 * Open a saved task at the caller's point of entry.
 *
 * A run failure can name the step that needs remediation, while ordinary editing
 * starts at the mapping step. The requested step is still bounded by the gates
 * before it, so an invalid saved mapping cannot be hidden by a request for a
 * later step. Metadata is deliberately absent here and is fetched by the screen
 * only when the chosen step needs it.
 */
export function openExisting(
  task: Task,
  source: DraftBinding,
  target: DraftBinding,
  targetAgentOnline = true,
  requestedStep: Step = 1,
): Draft {
  const sql = task.spec.source_sql?.trim() ?? "";
  const draft: Draft = {
    mode: "edit",
    step: 1,
    taskId: task.task_id,
    source,
    target,
    targetAgentOnline,
    fetchMode: sql === "" ? "table" : "sql",
    spec: { ...task.spec, where_clause: task.spec.where_clause ?? "" },
    name: task.name,
    hand: allHandMade(),
    sourceColumns: [],
    targetColumns: [],
    targetKeys: [],
    preview: null,
    check: null,
  };
  const prerequisiteSteps: Step[] = [1, 2, 3, 4].filter(
    (step): step is Step => step < requestedStep,
  );
  const blocked = prerequisiteSteps.find((step) => canAdvance(draft, step).length > 0);
  return { ...draft, step: blocked ?? requestedStep };
}

// ---------------------------------------------------------------------------
// The one entry point for change
// ---------------------------------------------------------------------------

/**
 * Apply a change, or ask first.
 *
 * The reducer reports what a cascading change clears, and anything hand-made in
 * that report earns a question. Explicit irreversible gestures, such as deleting
 * one source column, name their loss here and use the same confirmation result.
 */
export function apply(draft: Draft, change: Change): Applied {
  if (
    change.type === "remove-column" &&
    (draft.sourceColumns.some((column) => column.name === change.source) ||
      draft.spec.columns.some((mapping) => mapping.source === change.source))
  ) {
    return {
      kind: "needs-confirm",
      intent: change,
      loses: { headline: "确认删除这一列？", lines: [`源列 ${change.source} 将不再参与同步`] },
    };
  }
  const { draft: next, cleared } = reduce(draft, change);
  const loses = lossOf(draft, cleared);
  return loses === null
    ? { kind: "done", draft: next }
    : { kind: "needs-confirm", intent: change, loses };
}

/** Run the change the person just agreed to. */
export function confirm(draft: Draft, intent: Change): Draft {
  return reduce(draft, intent).draft;
}

/** What leaving would cost, or `null` when there is nothing to protect. */
export function leaving(draft: Draft): Loss | null {
  return lossOf(draft, ["draft"]);
}

// ---------------------------------------------------------------------------
// The clearing rules — one table, seven rules, each named
// ---------------------------------------------------------------------------

interface Reduced {
  draft: Draft;
  cleared: Cleared[];
}

function reduce(draft: Draft, change: Change): Reduced {
  switch (change.type) {
    // Rule 1. A different source database. The table list, the column dictionary
    // and everything named out of them belong to the previous library.
    case "source-datasource": {
      if (change.datasource.datasource_id === draft.source.datasource_id) {
        return { draft, cleared: [] };
      }
      return clearSourceSide(
        { ...draft, source: change.datasource, step: 1 },
        draft,
        { sql: true },
      );
    }

    // Rule 2. A different dblink is a different source database reached the long
    // way round. Same consequence, minus the SQL, which dblinks do not apply to.
    case "dblink": {
      const dblink = change.dblink.trim();
      const next = {
        ...draft,
        spec: { ...draft.spec, dblink: dblink === "" ? undefined : dblink },
      };
      return clearSourceSide(next, draft, { sql: false });
    }

    // Rule 3. Switching between "pick a table" and "write the SQL" discards the
    // other path's inputs wholesale; nothing carries across.
    case "fetch-mode": {
      if (change.fetchMode === draft.fetchMode) {
        return { draft, cleared: [] };
      }
      const next = {
        ...draft,
        fetchMode: change.fetchMode,
        step: 1 as Step,
        spec: {
          ...draft.spec,
          source_sql: change.fetchMode === "sql" ? "" : undefined,
          dblink: undefined,
        },
      };
      return clearSourceSide(next, draft, { sql: true });
    }

    // Rule 4. A different source table. The selected columns, the primary key and
    // the filter clause are all written in the previous table's column names —
    // keeping them only produces SQL that references columns that do not exist.
    // The target table name is the person's own and does not follow.
    case "source-table": {
      const prefilled =
        draft.hand.targetTable || change.table === ""
          ? draft.spec.target_table
          : change.table;
      const next: Draft = {
        ...draft,
        spec: { ...draft.spec, owner: change.owner, table: change.table, target_table: prefilled },
        hand: { ...draft.hand, sourceTable: true },
        sourceColumns: [],
      };
      return clearColumnsAndFilter(next, draft);
    }

    // Rule 5. Editing the SQL changes which result columns exist. Formatting it
    // does not — `formatSql` only moves whitespace — and that distinction is an
    // invariant this module inherits and must not lose.
    case "source-sql": {
      const next: Draft = {
        ...draft,
        spec: { ...draft.spec, source_sql: change.sql },
        hand: { ...draft.hand, sql: true },
        sourceColumns: [],
      };
      return clearColumnsAndFilter(next, draft);
    }
    case "format-sql":
      return {
        draft: { ...draft, spec: { ...draft.spec, source_sql: change.sql } },
        cleared: [],
      };

    // Rule 6. A different target database. The target table and every target
    // field name are in the previous database's namespace.
    case "target-datasource": {
      if (change.datasource.datasource_id === draft.target.datasource_id) {
        return { draft, cleared: [] };
      }
      const cleared: Cleared[] = [];
      if (draft.spec.target_table !== "") cleared.push("target-table");
      if (draft.spec.columns.some((mapping) => mapping.target !== "")) cleared.push("mappings");
      if (draft.spec.primary_key.length > 0) cleared.push("primary-key");
      return {
        draft: {
          ...draft,
          target: change.datasource,
          targetAgentOnline: change.online,
          spec: {
            ...draft.spec,
            target_table: "",
            columns: draft.spec.columns.map((mapping) => ({ ...mapping, target: "" })),
            primary_key: [],
          },
          hand: { ...draft.hand, targetTable: false, mappings: [] },
          targetColumns: [],
          targetKeys: [],
          check: null,
        },
        cleared,
      };
    }

    case "target-agent-status":
      return { draft: { ...draft, targetAgentOnline: change.online }, cleared: [] };

    // Rule 7. A different target table clears nothing. What it invalidates is
    // almost entirely same-name matching the machine did; the rows that no longer
    // point at a real column surface as row problems instead, where the person can
    // see which ones they are. Typing in this field must never wipe the mapping.
    case "target-table":
      return {
        draft: {
          ...draft,
          spec: { ...draft.spec, target_table: change.table },
          hand: { ...draft.hand, targetTable: true },
          targetColumns: [],
          targetKeys: [],
          check: null,
        },
        cleared: [],
      };

    case "toggle-column": {
      const present = draft.spec.columns.some((mapping) => mapping.source === change.source);
      const columns = present
        ? draft.spec.columns.filter((mapping) => mapping.source !== change.source)
        : [...draft.spec.columns, { source: change.source, target: change.source }];
      return {
        draft: withColumns(
          { ...draft, hand: { ...draft.hand, columns: true } },
          columns,
        ),
        cleared: [],
      };
    }

    // Deleting a column is asked about even though nothing else is cleared: on a
    // wide table it is the primary gesture, and it is not undoable.
    case "remove-column": {
      const columns = draft.spec.columns.filter(
        (mapping) => mapping.source !== change.source,
      );
      return {
        draft: withColumns(
          {
            ...draft,
            hand: {
              ...draft.hand,
              columns: true,
              mappings: draft.hand.mappings.filter((source) => source !== change.source),
            },
            sourceColumns: draft.sourceColumns.filter((column) => column.name !== change.source),
          },
          columns,
        ),
        cleared: [],
      };
    }

    case "rename-target": {
      const columns = draft.spec.columns.map((mapping) =>
        mapping.source === change.source
          ? { ...mapping, target: change.target }
          : mapping,
      );
      const mappings = draft.hand.mappings.includes(change.source)
        ? draft.hand.mappings
        : [...draft.hand.mappings, change.source];
      return {
        draft: withColumns({ ...draft, hand: { ...draft.hand, mappings } }, columns),
        cleared: [],
      };
    }

    case "toggle-primary-key": {
      const primary_key = draft.spec.primary_key.includes(change.target)
        ? draft.spec.primary_key.filter((name) => name !== change.target)
        : [...draft.spec.primary_key, change.target];
      return {
        draft: {
          ...draft,
          spec: { ...draft.spec, primary_key },
          hand: { ...draft.hand, primaryKey: true },
        },
        cleared: [],
      };
    }

    case "where":
      return {
        draft: {
          ...draft,
          spec: { ...draft.spec, where_clause: change.clause },
          hand: { ...draft.hand, where: true },
        },
        cleared: [],
      };

    case "task-name":
      return {
        draft: { ...draft, name: change.name, hand: { ...draft.hand, taskName: true } },
        cleared: [],
      };

    // "Refresh" means "bring in what changed outside", and the answer to the
    // question it asks has to be one predictable thing: the whole mapping goes
    // back to same-name matching. Preserving some rows and overwriting others is
    // not a sentence that fits in a confirmation dialog.
    case "refresh-target-columns": {
      const rematched = matchSameNameTargets(
        {
          columns: draft.spec.columns.map((mapping) => ({ ...mapping, target: "" })),
          primary_key: [],
        },
        draft.targetColumns,
        { onlyUnmapped: false },
      );
      return {
        draft: {
          ...draft,
          spec: { ...draft.spec, ...rematched },
          hand: { ...draft.hand, mappings: [] },
        },
        cleared: draft.hand.mappings.length > 0 ? ["mappings"] : [],
      };
    }

    case "drop-orphan-mappings": {
      const orphans = new Set(orphanSources(draft));
      const columns = draft.spec.columns.map((mapping) =>
        orphans.has(mapping.source) ? { ...mapping, target: "" } : mapping,
      );
      return {
        draft: withColumns(
          {
            ...draft,
            hand: {
              ...draft.hand,
              mappings: draft.hand.mappings.filter((source) => !orphans.has(source)),
            },
          },
          columns,
        ),
        cleared: [],
      };
    }

    // Reading the source columns selects all of them: on a wide table, dropping a
    // few is less work than picking a few, and the selection is the machine's
    // until the person touches it.
    case "source-columns-arrived": {
      const columns: ColumnMapping[] = draft.hand.columns
        ? draft.spec.columns
        : change.columns.map((column) => ({ source: column.name, target: column.name }));
      const matched = matchSameNameTargets(
        { columns, primary_key: draft.spec.primary_key },
        draft.targetColumns,
        { onlyUnmapped: true },
      );
      return {
        draft: withColumns(
          { ...draft, sourceColumns: change.columns },
          matched.columns,
          matched.primary_key,
        ),
        cleared: [],
      };
    }

    case "target-columns-arrived": {
      const matched = matchSameNameTargets(
        { columns: draft.spec.columns, primary_key: draft.spec.primary_key },
        change.columns,
        { onlyUnmapped: true },
      );
      const inferred = inferPrimaryKey(change.keys, matched.columns);
      return {
        draft: withColumns(
          { ...draft, targetColumns: change.columns, targetKeys: change.keys },
          matched.columns,
          draft.hand.primaryKey || inferred === null ? matched.primary_key : inferred,
        ),
        cleared: [],
      };
    }

    case "preview-arrived":
      return {
        draft: { ...draft, preview: { value: change.preview, inputs: previewInputs(draft) } },
        cleared: [],
      };

    case "check-arrived":
      return {
        draft: { ...draft, check: { value: change.check, inputs: checkInputs(draft) } },
        cleared: [],
      };

    case "advance": {
      if (draft.step === 4 || canAdvance(draft, draft.step).length > 0) {
        return { draft, cleared: [] };
      }
      return { draft: { ...draft, step: (draft.step + 1) as Step }, cleared: [] };
    }

    // Going back is free and lossless. Nothing on the step just left is discarded.
    case "back":
      return {
        draft: draft.step === 1 ? draft : { ...draft, step: (draft.step - 1) as Step },
        cleared: [],
      };

    case "leave":
      return { draft, cleared: ["draft"] };
  }
}

/** Clear owner, table, columns, primary key, filter — and optionally the SQL. */
function clearSourceSide(next: Draft, before: Draft, { sql }: { sql: boolean }): Reduced {
  const cleared: Cleared[] = [];
  if (before.spec.owner !== "" || before.spec.table !== "") cleared.push("source-table");
  if (before.spec.columns.length > 0) cleared.push("columns");
  if (before.spec.primary_key.length > 0) cleared.push("primary-key");
  if ((before.spec.where_clause ?? "") !== "") cleared.push("where");
  if (sql && (before.spec.source_sql ?? "") !== "") cleared.push("sql");
  return {
    draft: {
      ...next,
      spec: {
        ...next.spec,
        owner: "",
        table: "",
        columns: [],
        primary_key: [],
        where_clause: "",
        source_sql: sql
          ? next.fetchMode === "sql"
            ? ""
            : undefined
          : next.spec.source_sql,
      },
      hand: {
        ...next.hand,
        sourceTable: false,
        columns: false,
        primaryKey: false,
        where: false,
        sql: sql ? false : next.hand.sql,
        mappings: [],
      },
      sourceColumns: [],
      preview: null,
      check: null,
    },
    cleared,
  };
}

/** Clear the columns, the primary key and the filter, keeping the source location. */
function clearColumnsAndFilter(next: Draft, before: Draft): Reduced {
  const cleared: Cleared[] = [];
  if (before.spec.columns.length > 0) cleared.push("columns");
  if (before.spec.primary_key.length > 0) cleared.push("primary-key");
  if ((before.spec.where_clause ?? "") !== "") cleared.push("where");
  return {
    draft: {
      ...next,
      spec: { ...next.spec, columns: [], primary_key: [], where_clause: "" },
      hand: { ...next.hand, columns: false, primaryKey: false, where: false, mappings: [] },
      preview: null,
      check: null,
    },
    cleared,
  };
}

/**
 * Write a new column set back, dropping primary-key entries whose target field
 * no longer exists.
 *
 * The primary key is stored in the **target** namespace; letting it keep a name
 * that is no longer produced is the state that reaches sink's precheck as
 * "primary key column is not among the selected columns", with the person having
 * done nothing wrong.
 */
function withColumns(
  draft: Draft,
  columns: ColumnMapping[],
  primaryKey: readonly string[] = draft.spec.primary_key,
): Draft {
  const targets = new Set(
    columns.map((mapping) => mapping.target.toUpperCase()).filter((name) => name !== ""),
  );
  return {
    ...draft,
    spec: {
      ...draft.spec,
      columns,
      primary_key: primaryKey.filter((name) => targets.has(name.toUpperCase())),
    },
  };
}

/** The target table's own PRIMARY KEY, when every one of its columns is mapped. */
function inferPrimaryKey(
  keys: readonly TargetKey[],
  columns: readonly ColumnMapping[],
): string[] | null {
  const primary = keys.find((key) => key.name.toUpperCase() === "PRIMARY");
  if (primary === undefined || primary.columns.length === 0) {
    return null;
  }
  const mapped = new Map(
    columns.map((mapping) => [mapping.target.toUpperCase(), mapping.target]),
  );
  const resolved = primary.columns.map((name) => mapped.get(name.toUpperCase()));
  return resolved.every((name) => name !== undefined) ? (resolved as string[]) : null;
}

// ---------------------------------------------------------------------------
// The loss predicate
// ---------------------------------------------------------------------------

const LOSS_LINES: Record<Exclude<Cleared, "draft">, (draft: Draft) => string> = {
  "source-table": (draft) => `已选的源表 ${draft.spec.owner}.${draft.spec.table}`,
  columns: (draft) => `已选的 ${draft.spec.columns.length} 列`,
  "primary-key": (draft) => `已勾的 ${draft.spec.primary_key.length} 个主键列`,
  where: () => "手写的过滤条件",
  sql: () => "手写的自定义 SQL",
  "target-table": (draft) => `已选的目标表 ${draft.spec.target_table}`,
  mappings: (draft) => `手动改过的 ${draft.hand.mappings.length} 行字段映射`,
};

/**
 * Whether this clearing is worth asking about, and what the question lists.
 *
 * An empty draft is not interrupted: what the rule protects is the state someone
 * spent a dozen steps and half a dozen requests building, and a draft with
 * nothing hand-made in it has none of that. The cost of getting this wrong in the
 * other direction is four pointless dialogs in the first thirty seconds of use.
 */
function lossOf(draft: Draft, cleared: readonly Cleared[]): Loss | null {
  if (cleared.includes("draft")) {
    const lines = (Object.keys(LOSS_LINES) as Exclude<Cleared, "draft">[])
      .filter((kind) => handMade(draft, kind))
      .map((kind) => LOSS_LINES[kind](draft));
    return lines.length === 0
      ? null
      : { headline: "离开会清掉这份还没保存的任务草稿：", lines };
  }
  const lines = cleared
    .filter((kind): kind is Exclude<Cleared, "draft"> => kind !== "draft")
    .filter((kind) => handMade(draft, kind))
    .map((kind) => LOSS_LINES[kind](draft));
  return lines.length === 0 ? null : { headline: "这个改动会清掉：", lines };
}

function handMade(draft: Draft, kind: Exclude<Cleared, "draft">): boolean {
  switch (kind) {
    case "source-table":
      return draft.hand.sourceTable && draft.spec.table !== "";
    case "columns":
      return draft.hand.columns && draft.spec.columns.length > 0;
    case "primary-key":
      return draft.hand.primaryKey && draft.spec.primary_key.length > 0;
    case "where":
      return draft.hand.where && (draft.spec.where_clause ?? "").trim() !== "";
    case "sql":
      return draft.hand.sql && (draft.spec.source_sql ?? "").trim() !== "";
    case "target-table":
      return draft.hand.targetTable && draft.spec.target_table !== "";
    case "mappings":
      return draft.hand.mappings.length > 0;
  }
}

// ---------------------------------------------------------------------------
// Staleness — which inputs feed which fetched result
// ---------------------------------------------------------------------------

/**
 * A stale target-table check is the dangerous one: it is a gate, and a gate that
 * passes on last week's column set lets someone into step 4 to be told no by the
 * target database instead. So it is invalidated precisely, by input, rather than
 * by "anything changed" — which would make going back to fix a filter clause cost
 * a fresh round trip to the target, and going back is supposed to be free.
 */
function checkInputs(draft: Draft): string {
  return JSON.stringify([
    draft.target.datasource_id,
    draft.spec.target_table,
    draft.spec.columns,
    draft.spec.primary_key,
  ]);
}

function previewInputs(draft: Draft): string {
  return JSON.stringify([
    draft.source.datasource_id,
    draft.fetchMode,
    draft.spec.source_sql ?? "",
    draft.spec.dblink ?? "",
    draft.spec.owner,
    draft.spec.table,
    draft.spec.where_clause ?? "",
    draft.spec.columns,
  ]);
}

export function checkIsFresh(draft: Draft): boolean {
  return draft.check !== null && draft.check.inputs === checkInputs(draft);
}

export function previewIsFresh(draft: Draft): boolean {
  return draft.preview !== null && draft.preview.inputs === previewInputs(draft);
}

// ---------------------------------------------------------------------------
// The advance gate
// ---------------------------------------------------------------------------

/**
 * Unquoted Oracle identifier. **`crates/source/src/task_spec.rs::validate_identifier`
 * is the enforcing side**; this is an in-place echo of it, so that a target field
 * typed as `1st_col` is marked on its own row rather than coming back as a
 * server error naming a field the person has already scrolled past.
 *
 * Deliberately the only Rust rule restated here. The filter clause's no-semicolon
 * rule and the 37-character target-table cap stay in exactly one implementation
 * each, because both are ours and both can move; this one is Oracle's and cannot.
 */
const IDENTIFIER = /^[A-Za-z][A-Za-z0-9_$#]*$/;

/**
 * Why the next step is out of reach. Empty means it is reachable.
 *
 * Every reason that can be pinned to a column is, because "duplicate target
 * field" under a hundred rows is not a message, it is a search task.
 */
export function canAdvance(draft: Draft, step: Step): Blocker[] {
  const blockers: Blocker[] = [];
  const at = (message: string, column: string | null = null) =>
    blockers.push({ step, column, message });

  if (step === 1) {
    if (draft.fetchMode === "sql") {
      if ((draft.spec.source_sql ?? "").trim() === "") {
        at("自定义 SQL 不能为空");
      }
      if (draft.spec.dblink !== undefined) {
        at("自定义 SQL 已包含源端查询路径，不能同时设置 dblink");
      }
    } else {
      if (draft.spec.owner === "" || draft.spec.table === "") {
        at("请先选一张源表");
      } else if (!IDENTIFIER.test(draft.spec.owner) || !IDENTIFIER.test(draft.spec.table)) {
        at("源表名必须是未加引号的 Oracle 标识符");
      }
    }
    if (draft.spec.target_table.trim() === "") {
      at("请先选目标表——字段映射要对着它才有意义");
    }
    if (draft.spec.columns.length === 0) {
      at("至少要选一列");
    }

    const sources = new Map<string, number>();
    const targets = new Map<string, number>();
    for (const mapping of draft.spec.columns) {
      sources.set(
        mapping.source.toUpperCase(),
        (sources.get(mapping.source.toUpperCase()) ?? 0) + 1,
      );
      if (mapping.target !== "") {
        targets.set(
          mapping.target.toUpperCase(),
          (targets.get(mapping.target.toUpperCase()) ?? 0) + 1,
        );
      }
    }
    const orphans = new Set(orphanSources(draft));
    for (const mapping of draft.spec.columns) {
      if (mapping.target.trim() === "") {
        at("还没映射到目标字段", mapping.source);
        continue;
      }
      if (!IDENTIFIER.test(mapping.target)) {
        at("目标字段名必须是未加引号的标识符：字母开头，其余为字母、数字或 _ $ #", mapping.source);
      }
      if ((targets.get(mapping.target.toUpperCase()) ?? 0) > 1) {
        at(`目标字段 ${mapping.target} 重复`, mapping.source);
      }
      if ((sources.get(mapping.source.toUpperCase()) ?? 0) > 1) {
        at(`源列 ${mapping.source} 选了两遍`, mapping.source);
      }
      if (orphans.has(mapping.source)) {
        at(`目标表里没有 ${mapping.target} 这一列`, mapping.source);
      }
    }

    if (draft.spec.columns.length > 0 && draft.spec.primary_key.length === 0) {
      at("主键必选：至少要勾一列作为 upsert 的去重键");
    }
    const seen = new Set<string>();
    for (const name of draft.spec.primary_key) {
      if (!seen.add(name.toUpperCase())) {
        at(`主键列 ${name} 重复`);
      }
    }
  }

  if (step === 2) {
    // No completeness gate of its own. The one thing that cannot be let through
    // is the shape conflict the spec forbids outright.
    if (draft.fetchMode === "sql" && (draft.spec.where_clause ?? "").trim() !== "") {
      at("自定义 SQL 模式不能再单独配置过滤条件，请直接写进 SQL");
    }
  }

  if (step === 3) {
    // Editing ends in 保存, not a run, and the check cannot even be attempted
    // while the target's agent is down. Blocking here would mean "go revive the
    // agent before you may change one line of WHERE".
    const excused = draft.mode === "edit" && !draft.targetAgentOnline;
    if (!excused) {
      if (!checkIsFresh(draft)) {
        at("请先运行目标表检查");
      } else if (!draft.check!.value.ok) {
        at(`目标表检查未通过（${draft.check!.value.findings.length} 项）`);
      }
    }
  }

  if (step === 4 && taskName(draft).trim() === "") {
    at("任务名不能为空");
  }

  return blockers;
}

/** Source columns mapped to a target field the target table does not have. */
function orphanSources(draft: Draft): string[] {
  if (draft.targetColumns.length === 0) {
    return [];
  }
  const known = new Set(draft.targetColumns.map((column) => column.name.toUpperCase()));
  return draft.spec.columns
    .filter(
      (mapping) => mapping.target !== "" && !known.has(mapping.target.toUpperCase()),
    )
    .map((mapping) => mapping.source);
}

// ---------------------------------------------------------------------------
// Derived values
// ---------------------------------------------------------------------------

/**
 * The generated name, unless the person wrote one.
 *
 * A hand-written name is never overwritten by a later table change. Refreshing a
 * mapping may overwrite hand work because a mapping can be wrong; a task name has
 * no wrong, it is a label, and taking it back buys nothing.
 */
export function taskName(draft: Draft): string {
  if (draft.hand.taskName) {
    return draft.name;
  }
  const source = draft.fetchMode === "sql" ? "自定义 SQL" : `${draft.spec.owner}.${draft.spec.table}`;
  if (draft.spec.target_table === "" || source === ".") {
    return "";
  }
  return `${source} → ${draft.spec.target_table}`;
}

export interface MappingRow {
  source: string;
  target: string;
  selected: boolean;
  /** Read-only text plus the 自动匹配 mark, or a dropdown to be filled in. */
  control: "auto" | "manual";
  primaryKey: boolean;
  /** Why the primary-key tick cannot be moved, or `null`. */
  primaryKeyLock: string | null;
  /** The row's own problem, or `null`. */
  problem: string | null;
}

export interface ContextView {
  sourceName: string;
  targetName: string;
  targetAgentOnline: boolean;
  fetchMode: FetchMode;
  sourceLabel: string;
  targetTable: string;
  summary: string[];
}

export interface RailEntry {
  step: Step;
  label: string;
  state: "done" | "current" | "todo";
  /** Always false: the rail is linear, back-and-forth only, never a jump. */
  jumpable: false;
}

export type StepView =
  | { step: 1; rows: MappingRow[]; orphans: string[]; blockers: Blocker[] }
  | {
      step: 2;
      where: string;
      whereEditable: boolean;
      preview: { state: "none" | "stale" | "fresh"; value: PreviewResult | null };
      blockers: Blocker[];
    }
  | {
      step: 3;
      check: { state: "none" | "stale" | "fresh"; value: TargetCheckResult | null };
      excused: string | null;
      blockers: Blocker[];
    }
  | { step: 4; confirm: ConfirmView; blockers: Blocker[] };

export interface ConfirmView {
  name: string;
  nameGenerated: boolean;
  sourceLabel: string;
  where: string;
  mappings: ColumnMapping[];
  primaryKey: string[];
  targetTable: string;
  findings: CheckFinding[];
  preview: PreviewResult | null;
  /** What the bottom of the last step offers. Creating may run; editing saves. */
  actions: ("start" | "save-only" | "save")[];
}

export interface WizardView {
  context: ContextView;
  rail: RailEntry[];
  step: StepView;
}

const RAIL_LABELS: Record<Step, string> = {
  1: "选列与字段映射",
  2: "过滤与验证",
  3: "目标表检查",
  4: "确认并运行",
};

export function view(draft: Draft, step: Step = draft.step): WizardView {
  const blockers = canAdvance(draft, step);
  return {
    context: contextView(draft),
    rail: ([1, 2, 3, 4] as Step[]).map((entry) => ({
      step: entry,
      label: RAIL_LABELS[entry],
      state: entry < draft.step ? "done" : entry === draft.step ? "current" : "todo",
      jumpable: false,
    })),
    step: stepView(draft, step, blockers),
  };
}

function contextView(draft: Draft): ContextView {
  const source = sourceSummary(draft.spec);
  return {
    sourceName: draft.source.name,
    targetName: draft.target.name,
    targetAgentOnline: draft.targetAgentOnline,
    fetchMode: draft.fetchMode,
    sourceLabel: source.kind === "table" && source.label === "." ? "尚未选表" : source.label,
    targetTable: draft.spec.target_table === "" ? "尚未选目标表" : draft.spec.target_table,
    summary: [
      `已选 ${draft.spec.columns.length} 列`,
      draft.spec.primary_key.length === 0
        ? "主键未定"
        : `主键 ${draft.spec.primary_key.join(", ")}`,
      whereSummary(draft.spec),
    ],
  };
}

function stepView(draft: Draft, step: Step, blockers: Blocker[]): StepView {
  switch (step) {
    case 1: {
      const orphans = orphanSources(draft);
      const orphanSet = new Set(orphans);
      const locked = lockedPrimaryKey(draft);
      const problems = new Map<string, string>();
      for (const blocker of blockers) {
        if (blocker.column !== null && !problems.has(blocker.column)) {
          problems.set(blocker.column, blocker.message);
        }
      }
      const selected = new Map(
        draft.spec.columns.map((mapping) => [mapping.source, mapping]),
      );
      const known =
        draft.sourceColumns.length > 0
          ? draft.sourceColumns.map((column) => column.name)
          : draft.spec.columns.map((mapping) => mapping.source);
      const rows = known.map<MappingRow>((source) => {
        const mapping = selected.get(source);
        const target = mapping?.target ?? "";
        const auto =
          !draft.hand.mappings.includes(source) &&
          target !== "" &&
          target.toUpperCase() === source.toUpperCase() &&
          !orphanSet.has(source);
        return {
          source,
          target,
          selected: mapping !== undefined,
          control: auto ? "auto" : "manual",
          primaryKey: target !== "" && draft.spec.primary_key.includes(target),
          primaryKeyLock: locked,
          problem: problems.get(source) ?? null,
        };
      });
      return { step: 1, rows, orphans, blockers };
    }
    case 2:
      return {
        step: 2,
        where: draft.spec.where_clause ?? "",
        whereEditable: draft.fetchMode === "table",
        preview: {
          state: draft.preview === null ? "none" : previewIsFresh(draft) ? "fresh" : "stale",
          value: previewIsFresh(draft) ? draft.preview!.value : null,
        },
        blockers,
      };
    case 3:
      return {
        step: 3,
        check: {
          state: draft.check === null ? "none" : checkIsFresh(draft) ? "fresh" : "stale",
          value: checkIsFresh(draft) ? draft.check!.value : null,
        },
        excused:
          draft.mode === "edit" && !draft.targetAgentOnline
            ? `目标端 Agent「${draft.target.name}」不在线，这一步查不了；保存不受影响，运行要等它回来`
            : null,
        blockers,
      };
    case 4:
      return {
        step: 4,
        confirm: {
          name: taskName(draft),
          nameGenerated: !draft.hand.taskName,
          sourceLabel: sourceSummary(draft.spec).full,
          where: whereSummary(draft.spec),
          mappings: draft.spec.columns,
          primaryKey: draft.spec.primary_key,
          targetTable: draft.spec.target_table,
          findings: checkIsFresh(draft) ? draft.check!.value.findings : [],
          preview: previewIsFresh(draft) ? draft.preview!.value : null,
          actions:
            draft.mode === "edit"
              ? ["save"]
              : draft.targetAgentOnline
                ? ["start", "save-only"]
                : ["save-only"],
        },
        blockers,
      };
  }
}

/**
 * Why the primary key cannot be picked by hand, or `null`.
 *
 * A disabled control without a reason beside it reads as broken.
 */
function lockedPrimaryKey(draft: Draft): string | null {
  if (draft.hand.primaryKey) {
    return null;
  }
  const inferred = inferPrimaryKey(draft.targetKeys, draft.spec.columns);
  return inferred === null
    ? null
    : `目标表已定义主键（${inferred.join(", ")}），按它锁定`;
}

// ---------------------------------------------------------------------------
// Out
// ---------------------------------------------------------------------------

/** The spec as the server takes it. Neither shape carries the other's fields. */
export function toSpec(draft: Draft): TaskSpec {
  const spec: TaskSpec = {
    owner: draft.fetchMode === "sql" ? "" : draft.spec.owner,
    table: draft.fetchMode === "sql" ? "" : draft.spec.table,
    target_table: draft.spec.target_table.trim(),
    columns: draft.spec.columns.map((mapping) => ({ ...mapping })),
    primary_key: [...draft.spec.primary_key],
    where_clause: draft.fetchMode === "sql" ? "" : (draft.spec.where_clause ?? ""),
  };
  if (draft.fetchMode === "sql") {
    spec.source_sql = draft.spec.source_sql ?? "";
  } else if (draft.spec.dblink !== undefined) {
    spec.dblink = draft.spec.dblink;
  }
  return spec;
}

/**
 * The line to write into run history when a saved task's definition moves under
 * it, or `null` when nothing that changes an outcome moved.
 *
 * Run history records the SQL each run actually executed but **not which
 * datasource it ran against**, so after a source swap the older rows still read
 * as though they belong to the same library. The divider is what stops someone
 * comparing a failure on the new source with a success on the old one. Renaming
 * the task is not on the list: it changes nothing about comparability, and
 * cutting the history into slices nobody needs teaches people to ignore it.
 */
export function historyDivider(before: Task, draft: Draft): string | null {
  const after = toSpec(draft);
  const changes: string[] = [];
  if (before.source_datasource_id !== draft.source.datasource_id) {
    changes.push("源端数据源");
  }
  if (before.target_datasource_id !== draft.target.datasource_id) {
    changes.push("目标端数据源");
  }
  if ((before.spec.dblink ?? "") !== (after.dblink ?? "")) changes.push("dblink");
  if ((before.spec.source_sql ?? "") !== (after.source_sql ?? "")) changes.push("自定义 SQL");
  if (before.spec.owner !== after.owner || before.spec.table !== after.table) {
    changes.push("源表");
  }
  if (before.spec.target_table !== after.target_table) changes.push("目标表");
  if (JSON.stringify(before.spec.columns) !== JSON.stringify(after.columns)) {
    changes.push("列与映射");
  }
  if (JSON.stringify(before.spec.primary_key) !== JSON.stringify(after.primary_key)) {
    changes.push("主键");
  }
  if ((before.spec.where_clause ?? "") !== (after.where_clause ?? "")) {
    changes.push("过滤条件");
  }
  return changes.length === 0 ? null : `任务定义已变更：${changes.join("、")}`;
}
