import type { RunHistory } from "./api";
import { mappingSuggestion } from "./m3";

/**
 * The diagnostic table for the target-side mapping precheck.
 *
 * It used to grow only on the full-screen run detail, whose sole entrance was the run
 * you had just started (`handleStart` in `App.tsx`) — reload, or come back from the
 * list a while later, and it was unreachable. The drawer now carries it too (UX review
 * P1-6): what a failed precheck lists is exactly which column offends which rule,
 * which is the thing you edit the mapping against.
 */
export function PrecheckReports({
  detail,
}: {
  /** `message` and `mapping_issues` are all it needs, so history rows and finished details both fit. */
  detail: Pick<RunHistory, "message" | "mapping_issues">;
}) {
  return (
    <div className="precheck-reports">
      <section className="is-failed">
        <header>
          <strong>映射预检</strong>
          <span>目标端</span>
        </header>
        <p>{detail.message ?? "目标端映射预检未通过。"}</p>
        <DiagnosticTable
          columns={["列", "源端", "目标端", "规则", "建议"]}
          rows={detail.mapping_issues.map((issue) => [
            issue.column ?? "—",
            issue.source ?? "—",
            issue.target ?? "—",
            issue.rule ?? issue.message ?? "—",
            mappingSuggestion(issue),
          ])}
        />
        <small>总计 {detail.mapping_issues.length} 项问题</small>
      </section>
    </div>
  );
}

function DiagnosticTable({
  columns,
  rows,
}: {
  columns: string[];
  rows: string[][];
}) {
  return (
    <div className="diagnostic-table-wrap">
      <table className="diagnostic-table">
        <thead>
          <tr>{columns.map((column) => <th key={column}>{column}</th>)}</tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr key={`${row[0]}-${rowIndex}`}>
              {row.map((value, columnIndex) => (
                <td key={`${columns[columnIndex]}-${columnIndex}`}>{value}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
