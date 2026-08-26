import {
  ChevronDown,
  ChevronRight,
  Copy,
  LoaderCircle,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";

import {
  checkTargetTable,
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchTargetColumns,
  fetchTargetTables,
  generateBuilderSql,
  previewBuilderRows,
  previewErrorMessage,
} from "./api";
import type { BuilderSql, BuilderTable, PreviewResult } from "./api";
import { messageFrom } from "./errors";
import type { DatasourceOption } from "./entry";
import { UpsertNote, UPSERT_NOTE_AHEAD } from "./components/DesignSystem";
import { HighlightedSqlInput, SqlEditor } from "./SqlEditor";
import { Modal } from "./ui";
import {
  apply,
  canAdvance,
  confirm,
  leaving,
  taskName,
  toSpec,
  view,
} from "./wizard";
import type { Change, ConfirmTargetCheck, Draft, Loss } from "./wizard";

export interface TaskWizardScreenProps {
  initial: Draft;
  onCancel: () => void;
  onSubmit: (draft: Draft, action: "start" | "save-only") => Promise<void>;
  sourceOptions?: readonly DatasourceOption[];
  targetOptions?: readonly DatasourceOption[];
}

export interface TaskWizardScreenHandle {
  requestLeave: (proceed: () => void) => void;
}

type PendingConfirmation =
  | { kind: "change"; intent: Change; loses: Loss }
  | { kind: "leave"; intent: Change; loses: Loss; proceed: () => void };

export const TaskWizardScreen = forwardRef<TaskWizardScreenHandle, TaskWizardScreenProps>(function TaskWizardScreen(
  { initial, onCancel, onSubmit, sourceOptions = [], targetOptions = [] },
  ref,
) {
  const [draft, setDraft] = useState(initial);
  const draftRef = useRef(initial);
  const [pending, setPending] = useState<PendingConfirmation | null>(null);
  const [tables, setTables] = useState<BuilderTable[]>([]);
  const [dblinks, setDblinks] = useState<string[]>([]);
  const [targetTables, setTargetTables] = useState<string[]>([]);
  const [targetMetadataKey, setTargetMetadataKey] = useState<string | null>(null);
  const [sourceFilter, setSourceFilter] = useState("");
  const [targetFilter, setTargetFilter] = useState("");
  const [expandedOwners, setExpandedOwners] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState<"tables" | "columns" | "target" | "preview" | "check" | "submit" | null>(null);
  const tableRequest = useRef(0);
  const sourceColumnRequest = useRef(0);
  const targetColumnRequest = useRef(0);
  const previewRequest = useRef(0);
  const targetCheckRequest = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const model = view(draft);
  const advanceBlocked = model.step.blockers.length > 0;
  const sqlIdentity = JSON.stringify(toSpec(draft));

  function commit(next: Draft) {
    draftRef.current = next;
    setDraft(next);
  }

  function requestChange(intent: Change) {
    const current = draftRef.current;
    const result = apply(current, intent);
    if (result.kind === "done") {
      commit(result.draft);
      return;
    }
    setPending({ kind: "change", intent: result.intent, loses: result.loses });
  }

  function requestLeave(proceed: () => void) {
    const current = draftRef.current;
    const loses = leaving(current);
    if (loses === null) {
      proceed();
      return;
    }
    setPending({ kind: "leave", intent: { type: "leave" }, loses, proceed });
  }

  function acceptPending() {
    if (pending === null) return;
    commit(confirm(draftRef.current, pending.intent));
    setPending(null);
    if (pending.kind === "leave") pending.proceed();
  }

  useImperativeHandle(ref, () => ({ requestLeave }), []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && pending === null) requestLeave(onCancel);
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onCancel, pending]);

  useEffect(() => {
    if (leaving(draft) === null) return;
    function handleBeforeUnload(event: BeforeUnloadEvent) {
      event.preventDefault();
      event.returnValue = "";
    }
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [draft]);

  async function loadTables() {
    const request = ++tableRequest.current;
    const current = draftRef.current;
    if (current.fetchMode !== "table") return;
    const datasourceId = current.source.datasource_id;
    const dblink = current.spec.dblink?.trim() ?? "";
    setBusy("tables");
    setError(null);
    try {
      const next = await fetchBuilderTables(datasourceId, dblink);
      const latest = draftRef.current;
      if (
        request !== tableRequest.current ||
        latest.fetchMode !== "table" ||
        latest.source.datasource_id !== datasourceId ||
        (latest.spec.dblink?.trim() ?? "") !== dblink
      ) return;
      setTables(next);
      const owners = new Set(next.map((table) => table.owner));
      if (owners.size === 1) setExpandedOwners(owners);
    } catch (loadError) {
      const latest = draftRef.current;
      if (
        request === tableRequest.current &&
        latest.fetchMode === "table" &&
        latest.source.datasource_id === datasourceId &&
        (latest.spec.dblink?.trim() ?? "") === dblink
      ) setError(messageFrom(loadError));
    } finally {
      if (request === tableRequest.current) {
        setBusy((currentBusy) => currentBusy === "tables" ? null : currentBusy);
      }
    }
  }

  async function loadSourceColumns() {
    const request = ++sourceColumnRequest.current;
    const current = draftRef.current;
    const datasourceId = current.source.datasource_id;
    const fetchMode = current.fetchMode;
    const dblink = current.spec.dblink?.trim() ?? "";
    const owner = current.spec.owner;
    const table = current.spec.table;
    const sourceSql = current.spec.source_sql?.trim() ?? "";
    if (fetchMode === "sql" ? sourceSql === "" : owner === "" || table === "") return;
    setBusy("columns");
    setError(null);
    try {
      let columns;
      if (fetchMode === "sql") {
        const fetched = await fetchBuilderSqlColumns({
          datasource_id: datasourceId,
          source_sql: sourceSql,
        });
        columns = fetched.map((column) => ({
          name: column.name,
          data_type: column.type,
          precision: column.precision,
          scale: column.scale,
          length: column.length,
          nullable: true,
        }));
      } else {
        columns = await fetchBuilderColumns({
          datasource_id: datasourceId,
          dblink,
          owner,
          table,
        });
      }
      const latest = draftRef.current;
      const stillCurrent =
        request === sourceColumnRequest.current &&
        latest.fetchMode === fetchMode &&
        latest.source.datasource_id === datasourceId &&
        (fetchMode === "sql"
          ? (latest.spec.source_sql?.trim() ?? "") === sourceSql
          : (latest.spec.dblink?.trim() ?? "") === dblink &&
            latest.spec.owner === owner && latest.spec.table === table);
      if (stillCurrent) {
        requestChange({ type: "source-columns-arrived", columns });
      }
    } catch (loadError) {
      const latest = draftRef.current;
      const stillCurrent =
        request === sourceColumnRequest.current &&
        latest.fetchMode === fetchMode &&
        latest.source.datasource_id === datasourceId &&
        (fetchMode === "sql"
          ? (latest.spec.source_sql?.trim() ?? "") === sourceSql
          : (latest.spec.dblink?.trim() ?? "") === dblink &&
            latest.spec.owner === owner && latest.spec.table === table);
      if (stillCurrent) setError(messageFrom(loadError));
    } finally {
      if (request === sourceColumnRequest.current) {
        setBusy((currentBusy) => currentBusy === "columns" ? null : currentBusy);
      }
    }
  }

  async function loadTarget() {
    const request = ++targetColumnRequest.current;
    const current = draftRef.current;
    const datasourceId = current.target.datasource_id;
    const table = current.spec.target_table;
    if (table.trim() === "") return;
    setBusy("target");
    setError(null);
    try {
      const metadata = await fetchTargetColumns(datasourceId, table);
      const latest = draftRef.current;
      if (
        request !== targetColumnRequest.current ||
        latest.target.datasource_id !== datasourceId ||
        latest.spec.target_table !== table
      ) {
        return;
      }
      setTargetMetadataKey(JSON.stringify([datasourceId, table]));
      requestChange({ type: "target-columns-arrived", columns: metadata.columns, keys: metadata.keys });
    } catch (loadError) {
      const latest = draftRef.current;
      if (
        request === targetColumnRequest.current &&
        latest.target.datasource_id === datasourceId &&
        latest.spec.target_table === table
      ) {
        setError(messageFrom(loadError));
      }
    } finally {
      if (request === targetColumnRequest.current) {
        setBusy((currentBusy) => currentBusy === "target" ? null : currentBusy);
      }
    }
  }

  function refreshTarget() {
    requestChange({ type: "refresh-target-columns" });
    void loadTarget();
  }

  async function loadPreview() {
    const request = ++previewRequest.current;
    const current = draftRef.current;
    const identity = previewIdentity(current);
    setBusy("preview");
    setError(null);
    try {
      const preview = await previewBuilderRows(
        current.source.datasource_id,
        toSpec(current),
        10,
      );
      if (request !== previewRequest.current || previewIdentity(draftRef.current) !== identity) {
        return;
      }
      requestChange({ type: "preview-arrived", preview });
    } catch (previewError) {
      if (request === previewRequest.current && previewIdentity(draftRef.current) === identity) {
        setError(previewErrorMessage(previewError));
      }
    } finally {
      if (request === previewRequest.current) {
        setBusy((currentBusy) => currentBusy === "preview" ? null : currentBusy);
      }
    }
  }

  async function loadCheck() {
    const request = ++targetCheckRequest.current;
    const current = draftRef.current;
    if (canAdvance(current, 1).length > 0) return;
    const inputs = JSON.stringify([
      current.source.datasource_id,
      current.target.datasource_id,
      current.spec.target_table,
      current.spec.columns,
      current.spec.primary_key,
    ]);
    setBusy("check");
    setCheckError(null);
    try {
      const result = await checkTargetTable(
        current.source.datasource_id,
        current.target.datasource_id,
        current.spec.target_table,
        toSpec(current),
      );
      const latest = draftRef.current;
      const latestInputs = JSON.stringify([
        latest.source.datasource_id,
        latest.target.datasource_id,
        latest.spec.target_table,
        latest.spec.columns,
        latest.spec.primary_key,
      ]);
      if (request === targetCheckRequest.current && inputs === latestInputs) {
        requestChange({ type: "check-arrived", check: result });
      }
    } catch (loadError) {
      if (request === targetCheckRequest.current) setCheckError(messageFrom(loadError));
    } finally {
      if (request === targetCheckRequest.current) {
        setBusy((currentBusy) => currentBusy === "check" ? null : currentBusy);
      }
    }
  }

  useEffect(() => {
    let active = true;
    void Promise.all([
      fetchBuilderDblinks(draft.source.datasource_id).catch(() => []),
      fetchTargetTables(draft.target.datasource_id),
    ])
      .then(([nextDblinks, nextTargets]) => {
        if (!active) return;
        setDblinks(nextDblinks);
        setTargetTables(nextTargets);
      })
      .catch((loadError) => active && setError(messageFrom(loadError)));
    return () => {
      active = false;
    };
  }, [draft.source.datasource_id, draft.target.datasource_id]);

  useEffect(() => {
    if (draft.fetchMode === "table") void loadTables();
    // The datasource and dblink are the table-list inputs.
  }, [draft.fetchMode, draft.source.datasource_id, draft.spec.dblink]);

  useEffect(() => {
    if (
      draft.step === 1 &&
      draft.fetchMode === "table" &&
      draft.spec.owner !== "" &&
      draft.spec.table !== ""
    ) {
      void loadSourceColumns();
    }
  }, [draft.step, draft.fetchMode, draft.source.datasource_id, draft.spec.dblink, draft.spec.owner, draft.spec.table]);

  useEffect(() => {
    if (
      draft.step !== 1 ||
      draft.fetchMode !== "sql" ||
      (draft.spec.source_sql?.trim() ?? "") === ""
    ) return;
    const timer = window.setTimeout(() => void loadSourceColumns(), 350);
    return () => window.clearTimeout(timer);
  }, [draft.step, draft.fetchMode, draft.source.datasource_id, draft.spec.source_sql]);

  useEffect(() => {
    if (
      (draft.step === 1 || draft.step === 3) &&
      draft.spec.target_table !== ""
    ) {
      void loadTarget();
    }
  }, [draft.step, draft.target.datasource_id, draft.spec.target_table]);

  useEffect(() => {
    const expectedMetadata = JSON.stringify([
      draft.target.datasource_id,
      draft.spec.target_table,
    ]);
    if (
      draft.check === null &&
      targetMetadataKey === expectedMetadata &&
      canAdvance(draft, 1).length === 0
    ) {
      void loadCheck();
    }
  }, [draft, targetMetadataKey]);

  useEffect(() => {
    if (draft.step !== 2) {
      setSql(null);
      setSqlError(null);
      return;
    }
    let active = true;
    const spec = toSpec(draft);
    setSql(null);
    setSqlError(null);
    const timer = window.setTimeout(() => {
      void generateBuilderSql(spec)
        .then((next) => {
          if (active) {
            setSql(next);
            setSqlError(null);
          }
        })
        .catch((loadError) => active && setSqlError(messageFrom(loadError)));
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [draft.step, sqlIdentity]);

  const sourceGroups = useMemo(() => {
    const query = sourceFilter.trim().toLocaleLowerCase();
    const grouped = new Map<string, BuilderTable[]>();
    for (const table of tables) {
      if (`${table.owner}.${table.name}`.toLocaleLowerCase().includes(query)) {
        grouped.set(table.owner, [...(grouped.get(table.owner) ?? []), table]);
      }
    }
    return [...grouped.entries()];
  }, [sourceFilter, tables]);
  const filteredTargets = useMemo(() => {
    const query = targetFilter.trim().toLocaleLowerCase();
    return targetTables.filter((table) => table.toLocaleLowerCase().includes(query));
  }, [targetFilter, targetTables]);

  function selectTargetTable(table: string) {
    const alreadySelected = draftRef.current.spec.target_table === table;
    requestChange({ type: "target-table", table });
    if (alreadySelected) void loadTarget();
  }

  function advance() {
    requestChange({ type: "advance" });
  }

  async function submit(action: "start" | "save-only") {
    setBusy("submit");
    setError(null);
    try {
      await onSubmit(draft, action);
    } catch (submitError) {
      setError(messageFrom(submitError));
      setBusy(null);
    }
  }

  return (
    <section className="task-wizard" aria-label={draft.mode === "edit" ? "编辑任务" : "新建导入"}>
      <aside className="wizard-context">
        <header>
          <strong>导入上下文</strong>
          <button className="icon-button" type="button" title="退出向导" aria-label="退出向导" onClick={() => requestLeave(onCancel)}><X size={16} /></button>
        </header>

        <div className="wizard-mode" role="group" aria-label="取数方式">
          <button
            type="button"
            className={draft.fetchMode === "table" ? "is-active" : ""}
            onClick={() => requestChange({ type: "fetch-mode", fetchMode: "table" })}
          >按表选择</button>
          <button
            type="button"
            className={draft.fetchMode === "sql" ? "is-active" : ""}
            onClick={() => requestChange({ type: "fetch-mode", fetchMode: "sql" })}
          >自定义 SQL</button>
        </div>

        <div className="wizard-context-scroll">
          <section className="wizard-context-section">
            <div className="wizard-context-title">
              <strong>源端 · {model.context.sourceName}</strong>
              {draft.fetchMode === "table" && (
                <button className="icon-button" type="button" title="刷新源表" aria-label="刷新源表" onClick={() => void loadTables()}>
                  <RefreshCw className={busy === "tables" ? "is-spinning" : ""} size={15} />
                </button>
              )}
            </div>
            {draft.mode === "edit" && (
              <label className="wizard-select-label">源端数据源
                <select value={draft.source.datasource_id} onChange={(event) => {
                  const option = sourceOptions.find((candidate) => candidate.datasource_id === event.target.value);
                  if (option !== undefined) requestChange({ type: "source-datasource", datasource: option });
                }}>
                  {sourceOptions.map((option) => <option key={option.datasource_id} value={option.datasource_id}>{option.name} · {option.connection}</option>)}
                </select>
              </label>
            )}
            {draft.fetchMode === "sql" ? (
              <SqlEditor
                value={draft.spec.source_sql ?? ""}
                placeholder="SELECT ID, NAME FROM APP.T_CUSTOMER"
                onChange={(value) => requestChange({ type: "source-sql", sql: value })}
                onFormat={(value) => requestChange({ type: "format-sql", sql: value })}
              />
            ) : (
              <>
                {dblinks.length > 0 && (
                  <label className="wizard-select-label">DBLINK
                    <select value={draft.spec.dblink ?? ""} onChange={(event) => requestChange({ type: "dblink", dblink: event.target.value })}>
                      <option value="">本地库</option>
                      {dblinks.map((dblink) => <option key={dblink}>{dblink}</option>)}
                    </select>
                  </label>
                )}
                <label className="tree-search">
                  <Search size={14} aria-hidden="true" />
                  <input value={sourceFilter} placeholder="筛选 owner / 表" onChange={(event) => setSourceFilter(event.target.value)} />
                </label>
                <div className="schema-tree" role="tree" aria-label="源表">
                  {sourceGroups.map(([owner, ownerTables]) => {
                    const open = expandedOwners.has(owner);
                    return <div className="schema-node" key={owner}>
                      <button className="schema-row" type="button" aria-expanded={open} onClick={() => setExpandedOwners((current) => {
                        const next = new Set(current);
                        if (open) next.delete(owner); else next.add(owner);
                        return next;
                      })}>
                        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                        <span className="schema-name">{owner}</span><span className="schema-count">{ownerTables.length}</span>
                      </button>
                      {open && <div className="table-node-list">{ownerTables.map((table) => (
                        <button
                          className={`table-node ${draft.spec.owner === owner && draft.spec.table === table.name ? "is-selected" : ""}`}
                          key={`${owner}.${table.name}`}
                          type="button"
                          onClick={() => requestChange({ type: "source-table", owner, table: table.name })}
                        >{table.name}</button>
                      ))}</div>}
                    </div>;
                  })}
                </div>
              </>
            )}
          </section>

          <section className="wizard-context-section">
            <div className="wizard-context-title">
              <strong>目标端 · {model.context.targetName}</strong>
              <button className="icon-button" type="button" disabled={draft.spec.target_table === "" || busy === "target"} title={draft.spec.target_table === "" ? "先选择目标表" : busy === "target" ? "正在刷新目标列" : "刷新目标列"} aria-label="刷新目标列" onClick={refreshTarget}>
                <RefreshCw className={busy === "target" ? "is-spinning" : ""} size={15} />
              </button>
            </div>
            {draft.mode === "edit" && (
              <label className="wizard-select-label">目标端数据源
                <select value={draft.target.datasource_id} onChange={(event) => {
                  const option = targetOptions.find((candidate) => candidate.datasource_id === event.target.value);
                  if (option !== undefined) requestChange({
                    type: "target-datasource",
                    datasource: option,
                    online: option.agentStatus === "online",
                  });
                }}>
                  {targetOptions.map((option) => <option key={option.datasource_id} value={option.datasource_id}>{option.name} · {option.connection}</option>)}
                </select>
              </label>
            )}
            <label className="tree-search">
              <Search size={14} aria-hidden="true" />
              <input value={targetFilter} placeholder="筛选目标表" onChange={(event) => setTargetFilter(event.target.value)} />
              <span className="tree-count">{filteredTargets.length} / {targetTables.length}</span>
            </label>
            <div className="schema-tree is-target" role="tree" aria-label="目标表">
              {filteredTargets.map((table) => (
                <button
                  className={`table-node ${draft.spec.target_table === table ? "is-selected" : ""}`}
                  key={table}
                  type="button"
                  onClick={() => selectTargetTable(table)}
                >{table}</button>
              ))}
            </div>
          </section>

          <section className="wizard-summary">
            <strong>当前选择</strong>
            <span>{model.context.sourceLabel}</span>
            <span>{model.context.targetTable}</span>
            {model.context.summary.map((line) => <span key={line}>{line}</span>)}
          </section>
        </div>
      </aside>

      <div className="wizard-main">
        <ol className="wizard-rail" aria-label="导入步骤">
          {model.rail.map((entry) => (
            <li className={`is-${entry.state}`} aria-current={entry.state === "current" ? "step" : undefined} key={entry.step}>
              <span>{entry.step}</span><strong>{entry.label}</strong>
            </li>
          ))}
        </ol>

        <div className="wizard-step-scroll">
          <StepBody
            draft={draft}
            sql={sql}
            sqlError={sqlError}
            checkError={checkError}
            busy={busy}
            change={requestChange}
            loadSourceColumns={() => void loadSourceColumns()}
            loadPreview={() => void loadPreview()}
            loadCheck={() => void loadCheck()}
          />
          {error !== null && <div className="form-error" role="alert">{error}</div>}
        </div>

        <footer className="wizard-footer">
          <button className="button" type="button" onClick={draft.step === 1 ? () => requestLeave(onCancel) : () => requestChange({ type: "back" })}>
            {draft.step === 1 ? "取消" : "上一步"}
          </button>
          <span className="wizard-footer-actions">
            {draft.step < 4 ? (
              <button
                className="button is-primary"
                type="button"
                disabled={advanceBlocked}
                title={advanceBlocked ? "请先处理当前步骤中的问题" : undefined}
                onClick={advance}
              >{draft.step === 3 ? "查看确认页" : "下一步"}</button>
            ) : draft.mode === "edit" ? (
              <button className="button is-primary" type="button" disabled={busy === "submit" || canAdvance(draft, 4).length > 0} title={busy === "submit" ? "正在提交" : canAdvance(draft, 4)[0]?.message} onClick={() => void submit("save-only")}>
                {busy === "submit" ? <LoaderCircle className="is-spinning" size={15} /> : null}保存
              </button>
            ) : (
              <>
                <button className="button" type="button" disabled={busy === "submit" || canAdvance(draft, 4).length > 0} title={busy === "submit" ? "正在提交" : canAdvance(draft, 4)[0]?.message} onClick={() => void submit("save-only")}>只保存</button>
                <button className="button is-primary" type="button" disabled={busy === "submit" || canAdvance(draft, 4).length > 0} title={busy === "submit" ? "正在提交" : canAdvance(draft, 4)[0]?.message} onClick={() => void submit("start")}>
                  {busy === "submit" ? <LoaderCircle className="is-spinning" size={15} /> : null}开始导入
                </button>
              </>
            )}
          </span>
        </footer>
      </div>
      {pending !== null && (
        <WizardConfirmDialog
          loss={pending.loses}
          onCancel={() => setPending(null)}
          onConfirm={acceptPending}
        />
      )}
    </section>
  );
});

export function WizardConfirmDialog({ loss, onCancel, onConfirm }: {
  loss: Loss;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return <Modal title={loss.headline} onClose={onCancel} busy={false} narrow>
    <div className="modal-body wizard-loss"><ul>{loss.lines.map((line) => <li key={line}>{line}</li>)}</ul></div>
    <footer className="modal-footer">
      <button className="button is-ghost" type="button" onClick={onCancel}>取消</button>
      <button className="button is-primary" type="button" onClick={onConfirm}>确认并继续</button>
    </footer>
  </Modal>;
}

function StepBody({
  draft,
  sql,
  sqlError,
  checkError,
  busy,
  change,
  loadSourceColumns,
  loadPreview,
  loadCheck,
}: {
  draft: Draft;
  sql: BuilderSql | null;
  sqlError: string | null;
  checkError: string | null;
  busy: string | null;
  change: (intent: Change) => void;
  loadSourceColumns: () => void;
  loadPreview: () => void;
  loadCheck: () => void;
}) {
  const model = view(draft).step;
  if (model.step === 1) {
    const mappingBlockers = model.blockers.filter((blocker) => blocker.column === null);
    return <section className="wizard-step">
      <header><h1>选列与字段映射</h1><p>系统会先做同名匹配，请判断要搬哪些列，以及每一列应写到目标表的哪里。</p></header>
      {draft.fetchMode === "sql" && (
        <button className="button" type="button" disabled={(draft.spec.source_sql ?? "").trim() === "" || busy === "columns"} title={(draft.spec.source_sql ?? "").trim() === "" ? "先在左侧填写 SQL" : busy === "columns" ? "正在刷新结果列" : undefined} onClick={loadSourceColumns}>
          {busy === "columns" ? <LoaderCircle className="is-spinning" size={15} /> : <RefreshCw size={15} />}刷新结果列
        </button>
      )}
      {model.rows.length === 0 ? <p className="wizard-empty">{draft.fetchMode === "sql" ? "先在左侧写好 SQL，系统会自动识别结果列。" : "先在左侧选择一张源表。"}</p> : (
        <div className="table-wrap"><table className="data-grid wizard-mapping"><thead><tr><th>同步</th><th>源列</th><th>目标列</th><th>主键</th><th aria-label="操作" /></tr></thead><tbody>
          {model.rows.map((row) => <tr className={row.problem ? "is-problem" : ""} key={row.source}>
            <td><input type="checkbox" checked={row.selected} onChange={() => change({ type: "toggle-column", source: row.source })} /></td>
            <td><span className="mono">{row.source}</span></td>
            <td>{!row.selected ? "—" : <>{row.control === "auto" ? <span>{row.target} <small className="auto-mark">自动匹配</small></span> : <select aria-invalid={row.problem ? true : undefined} value={row.target} onChange={(event) => change({ type: "rename-target", source: row.source, target: event.target.value })}><option value="">请选择</option>{draft.targetColumns.map((column) => <option key={column.name}>{column.name}</option>)}</select>}{row.problem && <small>{row.problem}</small>}</>}</td>
            <td><input type="checkbox" disabled={!row.selected || row.target === "" || row.primaryKeyLock !== null} title={!row.selected ? "先勾选这一列" : row.target === "" ? "先选择目标列" : row.primaryKeyLock ?? undefined} checked={row.primaryKey} onChange={() => change({ type: "toggle-primary-key", target: row.target })} />{row.primaryKeyLock && <small>{row.primaryKeyLock}</small>}</td>
            <td><button className="icon-button is-danger" type="button" title={`删除列 ${row.source}`} aria-label={`删除列 ${row.source}`} onClick={() => change({ type: "remove-column", source: row.source })}><Trash2 size={15} /></button></td>
          </tr>)}
        </tbody></table></div>
      )}
      {mappingBlockers.length > 0 && <div className="wizard-mapping-problems" role="alert"><strong>映射与主键</strong><ul>{mappingBlockers.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}</ul></div>}
    </section>;
  }
  if (model.step === 2) {
    return <section className="wizard-step">
      <header><h1>过滤与验证</h1><p>检查最终查询与样例数据，并判断是否需要补充 WHERE 条件。</p></header>
      {model.whereEditable ? <div className="where-clause-editor"><HighlightedSqlInput value={model.where} placeholder="STATUS = 'ACTIVE' AND CREATED_AT >= DATE '2026-01-01'" label="WHERE 条件" rows={5} onChange={(clause) => change({ type: "where", clause })} /></div> : <div className="wizard-readonly">自定义 SQL 的过滤条件直接写在左侧 SQL 中。</div>}
      <section className="generated-sql"><header><div><strong>构建 SQL</strong><span>实际执行的源端查询</span></div></header>{sqlError ? <div className="form-error">{sqlError}</div> : sql ? <pre className="ddl-output">{sql.source_sql}</pre> : <p className="spec-empty">正在生成最终查询。</p>}</section>
      <section className="preview-panel">
        <header><div><strong>数据预览</strong><span>使用上方最终查询读取源端数据</span></div><button className="button" type="button" disabled={busy === "preview"} onClick={loadPreview}>{busy === "preview" ? <LoaderCircle className="is-spinning" size={15} /> : null}预览前 10 条</button></header>
        {model.preview.value ? <PreviewData preview={model.preview.value} /> : <p className="spec-empty">点击按钮后读取真实数据；修改查询条件后需重新预览。</p>}
      </section>
      <Blockers blockers={model.blockers} />
    </section>;
  }
  if (model.step === 3) {
    const result = model.check.value;
    return <section className="wizard-step">
      <header><h1>目标表检查</h1><p>系统会核对列、类型、长度与主键，请根据检查结果判断是否需要调整目标表。</p></header>
      <div className="target-check-toolbar">
        <strong>{busy === "check" ? "正在检查目标表" : result?.ok ? "目标表检查通过" : "目标表需要处理"}</strong>
        <button className="button" type="button" disabled={busy === "check" || busy === "target"} onClick={loadCheck}>
          <RefreshCw className={busy === "check" ? "is-spinning" : ""} size={15} />重新检查
        </button>
      </div>
      {checkError !== null && <div className="form-error" role="alert">{checkError}</div>}
      {model.check.state === "stale" && <div className="form-error" role="alert">映射或主键已变化，请重新检查目标表。</div>}
      {model.check.state === "none" && busy !== "check" && checkError === null && <p className="wizard-empty">等待目标表元数据与字段映射就绪。</p>}
      {result !== null && !result.ok && <>
        <div className="target-check-findings">
          {result.findings.map((finding, index) => <article key={`${finding.kind}-${finding.column ?? "table"}-${index}`}>
            <header><strong>{finding.column ?? "目标表"}</strong><span>{finding.message}</span></header>
            <dl><div><dt>需要</dt><dd>{finding.expected}</dd></div><div><dt>当前</dt><dd>{finding.actual}</dd></div></dl>
          </article>)}
        </div>
        {result.suggested_ddl !== null && <section className="generated-sql target-check-ddl">
          <header><div><strong>建议建表语句</strong><span>完整 CREATE TABLE</span></div><button className="icon-button" type="button" title="复制建表语句" aria-label="复制建表语句" onClick={() => void navigator.clipboard.writeText(result.suggested_ddl ?? "")}><Copy size={15} /></button></header>
          <pre className="ddl-output">{result.suggested_ddl}</pre>
        </section>}
      </>}
      <Blockers blockers={model.blockers} />
    </section>;
  }
  const confirmView = model.confirm;
  return <section className="wizard-step">
    <header><h1>确认并运行</h1><p>最后核对系统汇总的完整决定，并判断是否可以保存或开始导入。</p></header>
    <label className="wizard-name">任务名<input value={taskName(draft)} onChange={(event) => change({ type: "task-name", name: event.target.value })} /></label>
    <dl className="wizard-confirm-grid">
      <div><dt>源端</dt><dd>{confirmView.sourceLabel}</dd></div>
      <div><dt>目标表</dt><dd>{confirmView.targetTable}</dd></div>
      <div><dt>WHERE</dt><dd>{confirmView.where}</dd></div>
      <div><dt>主键</dt><dd>{confirmView.primaryKey.join(", ")}</dd></div>
      <div className="is-wide"><dt>字段映射</dt><dd>{confirmView.mappings.map((mapping) => <span className="mapping-chip" key={mapping.source}>{mapping.source} → {mapping.target}</span>)}</dd></div>
      <div className="is-wide"><dt>目标表检查</dt><dd><ConfirmTargetCheckCell check={confirmView.targetCheck} busy={busy === "check"} onCheck={loadCheck} /></dd></div>
    </dl>
    {confirmView.preview !== null && <section className="preview-panel">
      <header><div><strong>数据预览</strong><span>最终确认的源端样例数据</span></div></header>
      <PreviewData preview={confirmView.preview} />
    </section>}
    <Blockers blockers={model.blockers} />
    {/* 写入语义的常驻交底（2026-08 UX 评审 P0-1）：这一步是「开始导入」前的最后一屏，
        而这个产品**只增量合并、不删**。不写清楚的话，第一次用的人会按「全量同步」去理解
        自己刚配好的这张目标表。它不是告警，所以不着 --crit / --warn。 */}
    <UpsertNote text={UPSERT_NOTE_AHEAD} />
  </section>;
}

/**
 * 最后一屏上「目标表检查」那一格（2026-08 UX 评审 P0-3）。
 *
 * 三态，不是两态。「没检查过」原来和「检查通过」在这里长得一模一样——都写「已通过」，
 * 因为读的是空的 findings。而**恰恰是没检查过的那条路**（目标端 agent 离线，
 * `canAdvance` 明确放行）最需要在这里被说出来：它是草稿与生产写入之间的最后一句话。
 *
 * 没检查过时把原因摆出来，并给一颗**当场就能检查**的按钮——原因写在这里而补救要退回
 * 第 3 步，等于把人赶去找路。
 */
function ConfirmTargetCheckCell({
  check,
  busy,
  onCheck,
}: {
  check: ConfirmTargetCheck;
  busy: boolean;
  onCheck: () => void;
}) {
  if (check.state === "passed") {
    return <span className="confirm-check is-passed">已通过</span>;
  }
  if (check.state === "findings") {
    return <span className="confirm-check is-findings">有 {check.findings.length} 处需要处理</span>;
  }
  return <span className="confirm-check is-unchecked">
    <strong>尚未检查</strong>
    {check.excused !== null && <span className="confirm-check-reason">{check.excused}</span>}
    <button className="button" type="button" disabled={busy} onClick={onCheck}>
      {busy ? <LoaderCircle className="is-spinning" size={15} /> : <RefreshCw size={15} />}立即检查
    </button>
  </span>;
}

function PreviewData({ preview }: { preview: PreviewResult }) {
  return <>
    <div className="table-wrap"><table className="data-grid preview-table"><thead><tr>{preview.columns.map((column) => <th key={column}>{column}</th>)}</tr></thead><tbody>{preview.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, columnIndex) => <td key={`${rowIndex}-${columnIndex}`}><span className="mono">{cell ?? "NULL"}</span></td>)}</tr>)}</tbody></table></div>
    <footer><span>{preview.rows.length} 条 · {preview.elapsed_ms} ms</span>{preview.truncated && <span>结果已截断，仅显示前 10 条</span>}</footer>
  </>;
}

function previewIdentity(draft: Draft): string {
  return JSON.stringify({
    source_datasource_id: draft.source.datasource_id,
    spec: toSpec(draft),
  });
}

function Blockers({ blockers: allBlockers }: { blockers: ReturnType<typeof canAdvance> }) {
  const blockers = allBlockers.filter((blocker) => blocker.column === null);
  return blockers.length === 0 ? null : <ul className="wizard-blockers">{blockers.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}</ul>;
}
