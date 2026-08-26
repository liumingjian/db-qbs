import type { RunHistory } from "./api";
import { mappingSuggestion } from "./m3";

/**
 * 目标端映射预检的那一份诊断表。
 *
 * 它原来只长在运行详情**整屏**里，而那一屏唯一的入口是刚点完「发起运行」的那一次
 * （`App.tsx` 的 `handleStart`）——刷新一下、或者过一会儿从列表回来看，就再也找不到它了。
 * 抽屉现在也摆它（UX 评审 P1-6）：预检失败时列出来的是**每一列具体哪里不合**，
 * 那正是要照着去改映射的东西。
 */
export function PrecheckReports({
  detail,
}: {
  /** 只要有 `message` 与 `mapping_issues` 就够，历史记录与终局详情都能喂进来。 */
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
