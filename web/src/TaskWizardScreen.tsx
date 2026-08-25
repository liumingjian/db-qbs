import {
  ChevronDown,
  ChevronRight,
  LoaderCircle,
  RefreshCw,
  Search,
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
  previewBuilderRows,
  previewErrorMessage,
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
  const [busy, setBusy] = useState<"tables" | "columns" | "target" | "preview" | "submit" | null>(null);
  const targetColumnRequest = useRef(0);
  const previewRequest = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const model = view(draft);
  const advanceBlocked = model.step.blockers.length > 0;
  const sqlIdentity = JSON.stringify(toSpec(draft));

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
    if (draft.fetchMode !== "table") return;
    setBusy("tables");
    setError(null);
    try {
      const next = await fetchBuilderTables(
        draft.source.datasource_id,
        draft.spec.dblink?.trim() ?? "",
      );
      setTables(next);
      const owners = new Set(next.map((table) => table.owner));
      if (owners.size === 1) setExpandedOwners(owners);
    } catch (loadError) {
      setError(messageFrom(loadError));
    } finally {
      setBusy(null);
    }
  }

  async function loadSourceColumns() {
    setBusy("columns");
    setError(null);
    try {
      if (draft.fetchMode === "sql") {
        const sourceSql = draft.spec.source_sql?.trim() ?? "";
        if (sourceSql === "") return;
        const fetched = await fetchBuilderSqlColumns({
          datasource_id: draft.source.datasource_id,
          source_sql: sourceSql,
        });
        change({
          type: "source-columns-arrived",
          columns: fetched.map((column) => ({
            name: column.name,
            data_type: column.type,
            precision: column.precision,
            scale: column.scale,
            length: column.length,
            nullable: true,
          })),
        });
      } else if (draft.spec.owner !== "" && draft.spec.table !== "") {
        change({
          type: "source-columns-arrived",
          columns: await fetchBuilderColumns({
            datasource_id: draft.source.datasource_id,
            dblink: draft.spec.dblink?.trim() ?? "",
            owner: draft.spec.owner,
            table: draft.spec.table,
          }),
        });
      }
    } catch (loadError) {
      setError(messageFrom(loadError));
    } finally {
      setBusy(null);
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
      change({ type: "preview-arrived", preview });
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
    if (draft.fetchMode === "table" && draft.spec.owner !== "" && draft.spec.table !== "") {
      void loadSourceColumns();
    }
  }, [draft.fetchMode, draft.spec.owner, draft.spec.table]);

  useEffect(() => {
    if (draft.spec.target_table !== "") void loadTarget();
  }, [draft.target.datasource_id, draft.spec.target_table]);

  useEffect(() => {
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
  }, [sqlIdentity]);

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
    <section className="task-wizard" aria-label="新建导入">
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
            <div className="wizard-context-title"><strong>目标端 · {model.context.targetName}</strong></div>
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
            loadTarget={() => void loadTarget()}
            loadPreview={() => void loadPreview()}
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
                onClick={advance}
              >{draft.step === 3 ? "查看确认页" : "下一步"}</button>
            ) : (
              <>
                <button className="button" type="button" disabled={busy === "submit" || canAdvance(draft, 4).length > 0} onClick={() => void submit("save-only")}>只保存</button>
                <button className="button is-primary" type="button" disabled={busy === "submit" || canAdvance(draft, 4).length > 0} onClick={() => void submit("start")}>
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
  loadPreview,
}: {
  draft: Draft;
  sql: BuilderSql | null;
  sqlError: string | null;
  busy: string | null;
  change: (intent: Change) => void;
  loadSourceColumns: () => void;
  loadTarget: () => void;
  loadPreview: () => void;
}) {
  const model = view(draft).step;
  if (model.step === 1) {
    const mappingBlockers = model.blockers.filter((blocker) => blocker.column === null);
    return <section className="wizard-step">
      <header><h1>选列与字段映射</h1><p>确认要搬哪些列，以及每一列写到目标表的哪里。</p></header>
      {draft.fetchMode === "sql" && (
        <button className="button" type="button" disabled={(draft.spec.source_sql ?? "").trim() === "" || busy === "columns"} onClick={loadSourceColumns}>
          {busy === "columns" ? <LoaderCircle className="is-spinning" size={15} /> : null}识别结果列
        </button>
      )}
      {model.rows.length === 0 ? <p className="wizard-empty">{draft.fetchMode === "sql" ? "先在左侧写好 SQL，再识别结果列。" : "先在左侧选择一张源表。"}</p> : (
        <div className="table-wrap"><table className="data-grid wizard-mapping"><thead><tr><th>同步</th><th>源列</th><th>目标列</th><th>主键</th></tr></thead><tbody>
          {model.rows.map((row) => <tr className={row.problem ? "is-problem" : ""} key={row.source}>
            <td><input type="checkbox" checked={row.selected} onChange={() => change({ type: "toggle-column", source: row.source })} /></td>
            <td><span className="mono">{row.source}</span></td>
            <td>{!row.selected ? "—" : <>{row.control === "auto" ? <span>{row.target} <small className="auto-mark">自动匹配</small></span> : <select aria-invalid={row.problem ? true : undefined} value={row.target} onChange={(event) => change({ type: "rename-target", source: row.source, target: event.target.value })}><option value="">请选择</option>{draft.targetColumns.map((column) => <option key={column.name}>{column.name}</option>)}</select>}{row.problem && <small>{row.problem}</small>}</>}</td>
            <td><input type="checkbox" disabled={!row.selected || row.target === "" || row.primaryKeyLock !== null} title={row.primaryKeyLock ?? undefined} checked={row.primaryKey} onChange={() => change({ type: "toggle-primary-key", target: row.target })} />{row.primaryKeyLock && <small>{row.primaryKeyLock}</small>}</td>
          </tr>)}
        </tbody></table></div>
      )}
      {mappingBlockers.length > 0 && <div className="wizard-mapping-problems" role="alert"><strong>映射与主键</strong><ul>{mappingBlockers.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}</ul></div>}
    </section>;
  }
  if (model.step === 2) {
    return <section className="wizard-step">
      <header><h1>过滤与验证</h1><p>检查最终查询；按表取数时可以补一段自由 WHERE 条件。</p></header>
      {model.whereEditable ? <div className="where-clause-editor"><HighlightedSqlInput value={model.where} placeholder="STATUS = 'ACTIVE' AND CREATED_AT >= DATE '2026-01-01'" label="WHERE 条件" rows={5} onChange={(clause) => change({ type: "where", clause })} /></div> : <div className="wizard-readonly">自定义 SQL 的过滤条件直接写在左侧 SQL 中。</div>}
      <section className="generated-sql"><header><div><strong>构建 SQL</strong><span>实际执行的源端查询</span></div></header>{sqlError ? <div className="form-error">{sqlError}</div> : sql ? <pre className="ddl-output">{sql.source_sql}</pre> : <p className="spec-empty">先完成字段映射与主键，系统才有完整 SQL。</p>}</section>
      <section className="preview-panel">
        <header><div><strong>数据预览</strong><span>使用上方最终查询读取源端数据</span></div><button className="button" type="button" disabled={busy === "preview"} onClick={loadPreview}>{busy === "preview" ? <LoaderCircle className="is-spinning" size={15} /> : null}预览前 10 条</button></header>
        {model.preview.value ? <><div className="table-wrap"><table className="data-grid preview-table"><thead><tr>{model.preview.value.columns.map((column) => <th key={column}>{column}</th>)}</tr></thead><tbody>{model.preview.value.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, columnIndex) => <td key={`${rowIndex}-${columnIndex}`}><span className="mono">{cell ?? "NULL"}</span></td>)}</tr>)}</tbody></table></div><footer><span>{model.preview.value.rows.length} 条 · {model.preview.value.elapsed_ms} ms</span>{model.preview.value.truncated && <span>结果已截断，仅显示前 10 条</span>}</footer></> : <p className="spec-empty">点击按钮后读取真实数据；修改查询条件后需重新预览。</p>}
      </section>
      <Blockers blockers={model.blockers} />
    </section>;
  }
  if (model.step === 3) {
    return <section className="wizard-step">
      <header><h1>目标表检查</h1><p>这里将核对列、类型、长度与主键，检查通过后才能运行。</p></header>
      <div className="wizard-placeholder is-prominent"><strong>目标表检查接口正在接入</strong><span>当前页面保留完整步骤位置；#187 会在这里展示逐列结论与完整建表语句。</span><button className="button" type="button" disabled={busy === "target"} onClick={loadTarget}><RefreshCw className={busy === "target" ? "is-spinning" : ""} size={15} />刷新目标表元数据</button></div>
    </section>;
  }
  const confirmView = model.confirm;
  return <section className="wizard-step">
    <header><h1>确认并运行</h1><p>最后核对一次这次导入的完整决定。</p></header>
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
