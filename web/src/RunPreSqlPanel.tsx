import type { RunEvidence } from "./api";
import { HighlightedSql } from "./SqlEditor";
import { runPreSql } from "./writeMode";

/** Displays the immutable preSQL snapshot captured for one run. */
export function RunPreSqlPanel({ evidence }: { evidence: RunEvidence | undefined }) {
  const preSql = runPreSql(evidence);
  if (preSql === undefined || preSql.trim() === "") return null;

  return (
    <section className="panel">
      <h3>当次执行的 preSQL</h3>
      <pre className="drawer-sql"><HighlightedSql sql={preSql} /></pre>
    </section>
  );
}
