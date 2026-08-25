import {
  ChevronDown,
  ChevronRight,
  LoaderCircle,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchTargetColumns,
  fetchTargetTables,
  generateBuilderSql,
} from "./api";
import type { BuilderSql, BuilderTable } from "./api";
import { messageFrom } from "./errors";
import { HighlightedSqlInput, SqlEditor } from "./SqlEditor";
import {
  apply,
  canAdvance,
  confirm,
  leaving,
  taskName,
  toSpec,
  view,
} from "./wizard";
import type { Change, Draft } from "./wizard";

export interface TaskWizardScreenProps {
  initial: Draft;
  onCancel: () => void;
  onSubmit: (draft: Draft, action: "start" | "save-only") => Promise<void>;
}

export function TaskWizardScreen({ initial, onCancel, onSubmit }: TaskWizardScreenProps) {
  const [draft, setDraft] = useState(initial);
  const draftRef = useRef(initial);
  const [tables, setTables] = useState<BuilderTable[]>([]);
  const [dblinks, setDblinks] = useState<string[]>([]);
  const [targetTables, setTargetTables] = useState<string[]>([]);
  const [sourceFilter, setSourceFilter] = useState("");
  const [targetFilter, setTargetFilter] = useState("");
  const [expandedOwners, setExpandedOwners] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState<"tables" | "columns" | "target" | "submit" | null>(null);
  const tableRequest = useRef(0);
  const sourceColumnRequest = useRef(0);
  const targetColumnRequest = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const model = view(draft);
  const advanceBlocked = model.step.blockers.length > 0;

  function change(intent: Change) {
    const current = draftRef.current;
    const result = apply(current, intent);
    const next =
      result.kind === "done"
        ? result.draft
        : window.confirm(
              `${result.loses.headline}\n\n${result.loses.lines.map((line) => `- ${line}`).join("\n")}`,
            )
          ? confirm(current, result.intent)
          : current;
    draftRef.current = next;
    setDraft(next);
  }

  function cancel() {
    const loss = leaving(draft);
    if (
      loss !== null &&
      !window.confirm(`${loss.headline}\n\n${loss.lines.map((line) => `- ${line}`).join("\n")}`)
    ) {
      return;
    }
    onCancel();
  }

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
        change({ type: "source-columns-arrived", columns });
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
      change({ type: "target-columns-arrived", columns: metadata.columns, keys: metadata.keys });
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
    change({ type: "refresh-target-columns" });
    void loadTarget();
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
    if (draft.step !== 2) {
      setSql(null);
      setSqlError(null);
      return;
    }
    if (canAdvance(draft, 1).length > 0) {
      setSql(null);
      setSqlError(null);
      return;
    }
    let active = true;
    const timer = window.setTimeout(() => {
      void generateBuilderSql(toSpec(draft))
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
  }, [draft]);

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
    change({ type: "target-table", table });
    if (alreadySelected) void loadTarget();
  }

  function advance() {
    if (draft.step === 3) {
      // #187 replaces this bridge with the real check result and normal `advance` gate.
      const next = { ...draftRef.current, step: 4 as const };
      draftRef.current = next;
      setDraft(next);
      return;
    }
    change({ type: "advance" });
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
          <button className="text-button" type="button" onClick={cancel}>退出</button>
        </header>

        <div className="wizard-mode" role="group" aria-label="取数方式">
          <button
            type="button"
            className={draft.fetchMode === "table" ? "is-active" : ""}
            onClick={() => change({ type: "fetch-mode", fetchMode: "table" })}
          >按表选择</button>
          <button
            type="button"
            className={draft.fetchMode === "sql" ? "is-active" : ""}
            onClick={() => change({ type: "fetch-mode", fetchMode: "sql" })}
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
            {draft.fetchMode === "sql" ? (
              <SqlEditor
                value={draft.spec.source_sql ?? ""}
                placeholder="SELECT ID, NAME FROM APP.T_CUSTOMER"
                onChange={(value) => change({ type: "source-sql", sql: value })}
                onFormat={(value) => change({ type: "format-sql", sql: value })}
              />
            ) : (
              <>
                {dblinks.length > 0 && (
                  <label className="wizard-select-label">DBLINK
                    <select value={draft.spec.dblink ?? ""} onChange={(event) => change({ type: "dblink", dblink: event.target.value })}>
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
                          onClick={() => change({ type: "source-table", owner, table: table.name })}
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
            busy={busy}
            change={change}
            loadSourceColumns={() => void loadSourceColumns()}
            loadTarget={refreshTarget}
          />
          {error !== null && <div className="form-error" role="alert">{error}</div>}
        </div>

        <footer className="wizard-footer">
          <button className="button" type="button" onClick={draft.step === 1 ? cancel : () => change({ type: "back" })}>
            {draft.step === 1 ? "取消" : "上一步"}
          </button>
          <span className="wizard-footer-actions">
            {draft.step < 4 ? (
              <button
                className="button is-primary"
                type="button"
                disabled={draft.step !== 3 && advanceBlocked}
                title={draft.step !== 3 && advanceBlocked ? "请先处理当前步骤中的问题" : undefined}
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
    </section>
  );
}

function StepBody({
  draft,
  sql,
  sqlError,
  busy,
  change,
  loadSourceColumns,
  loadTarget,
}: {
  draft: Draft;
  sql: BuilderSql | null;
  sqlError: string | null;
  busy: string | null;
  change: (intent: Change) => void;
  loadSourceColumns: () => void;
  loadTarget: () => void;
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
      <section className="generated-sql"><header><div><strong>构建 SQL</strong><span>实际执行的源端查询</span></div></header>{sqlError ? <div className="form-error">{sqlError}</div> : sql ? <pre className="ddl-output">{sql.source_sql}</pre> : <p className="spec-empty">先完成字段映射与主键，系统才有完整 SQL。</p>}</section>
      <div className="wizard-placeholder"><strong>数据预览</strong><span>前 10 行真实数据预览将在 #188 接入。</span></div>
      <Blockers blockers={model.blockers} />
    </section>;
  }
  if (model.step === 3) {
    return <section className="wizard-step">
      <header><h1>目标表检查</h1><p>系统会核对列、类型、长度与主键，请根据检查结果判断是否需要调整目标表。</p></header>
      <div className="wizard-placeholder is-prominent"><strong>目标表检查接口正在接入</strong><span>当前页面保留完整步骤位置；#187 会在这里展示逐列结论与完整建表语句。</span><button className="button" type="button" disabled={busy === "target"} onClick={loadTarget}><RefreshCw className={busy === "target" ? "is-spinning" : ""} size={15} />刷新目标表元数据</button></div>
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
      <div className="is-wide"><dt>目标表检查</dt><dd>{confirmView.findings.length === 0 ? "检查功能待 #187 接入" : `${confirmView.findings.length} 项结论`}</dd></div>
    </dl>
    <Blockers blockers={model.blockers} />
  </section>;
}

function Blockers({ blockers: allBlockers }: { blockers: ReturnType<typeof canAdvance> }) {
  const blockers = allBlockers.filter((blocker) => blocker.column === null);
  return blockers.length === 0 ? null : <ul className="wizard-blockers">{blockers.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}</ul>;
}
