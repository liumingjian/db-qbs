/**
 * 写入模式，以及它落到的那条语句。前端这一份是 `crates/shared/src/write_mode.rs` 的镜子。
 *
 * 两件事分开，且**只有一件是人选的**：
 *
 * - `WriteMode` 是任务定义里存的那个值：「追加写」与「先清空再导入」两档；
 * - `WriteStatement` 是目标端真正会跑的语句，**没人能选它**——目标表有主键就是
 *   `ON DUPLICATE KEY UPDATE`，没有就是纯 `INSERT ... SELECT`。
 *
 * 需求方明确不给「无主键」加勾选框。那么界面上唯一能做的事就是**把这件事说出来**：
 * 无主键意味着重跑会产生重复数据，而这是这个产品第一次接受「同一任务定义跑两次、
 * 目标表状态不同」。下面这些句子因此只有一份定义——向导、任务列表、运行详情三处
 * 抄的是同一个常量，抄成三份第一次改口径就会漏掉一处。
 */

import type { RunEvidence, TargetKey, TaskSpec } from "./api";

export type WriteMode = "APPEND" | "CLEAR_THEN_IMPORT";

export type WriteStatement = "upsert" | "insert";

export function hasPreSql(preSql: string | null | undefined): boolean {
  return (preSql ?? "").trim() !== "";
}

/** Destructive semantics marker used consistently across task and run surfaces. */
export function writeModeLabel(mode: WriteMode, preSql?: string | null): string {
  if (mode === "APPEND" && hasPreSql(preSql)) return "追加写 + preSQL 清理";
  return WRITE_MODES.find((entry) => entry.mode === mode)?.label ?? mode;
}

export function runPreSql(evidence: RunEvidence | undefined): string | undefined {
  return evidence?.parameters?.pre_sql;
}

/** 写入模式的清单。向导里那组单选按钮直接照它渲染，加一档就多一项。 */
export const WRITE_MODES: readonly { mode: WriteMode; label: string; hint: string }[] = [
  {
    mode: "APPEND",
    label: "追加写",
    hint: "把查询结果写进目标表；可选 preSQL 会先按条件清理目标表。",
  },
  {
    mode: "CLEAR_THEN_IMPORT",
    label: "先清空再导入",
    hint: "同一个事务里先清空目标表再导入，跑完之后目标表精确等于本次查询结果；原有数据不可恢复。",
  },
];

/** 这一档会不会清空目标表。判据只有一处，界面上问它，不各自比字符串。 */
export function clearsTarget(mode: WriteMode): boolean {
  return mode === "CLEAR_THEN_IMPORT";
}

/**
 * 语句形状的唯一派生：任务定义记下的主键为空，就是「目标表没有可合并的唯一约束」。
 *
 * 和后端 `WriteStatement::for_primary_key` 是同一条规则。它读的是**任务定义记下的
 * 那一份**，不是目标表此刻的样子——两者不符时由目标端预检拒跑，界面不做第二次裁决。
 */
export function writeStatementOf(primaryKey: readonly string[]): WriteStatement {
  return primaryKey.length === 0 ? "insert" : "upsert";
}

/** 一行短标签，摆在任务列表、运行详情、确认页那种一格里。 */
export function writeStatementLabel(statement: WriteStatement): string {
  return statement === "upsert" ? "按主键 upsert" : "纯追加写";
}

/**
 * 无主键那条路上必须被读到的那句话，**与目标端预检的结论逐字一致**
 * （`crates/sink/src/precheck.rs` 的 `APPEND_ONLY_CONCLUSION`）。
 *
 * 为什么这里要有一份而不是一律渲染服务端给的那句：需要说这句话的地方大多**没有**
 * 服务端的回答可读——任务清单上每一行的那个标记（`JobCenterScreen`）背后没有一次
 * 目标表检查，向导里那句常驻交底在第一次检查跑起来之前就得在。真有服务端结论的地方
 * （第 3 步那一屏）渲染的仍是 `TargetCheckResult.notes` 原文，不是这份常量。
 *
 * 两份逐字一致这件事不靠注释守，靠 `writeMode.test.ts` 里那条把 Rust 那份读出来比对
 * 的用例守——注释拦不住任何人改一个字。
 */
export const APPEND_ONLY_CONCLUSION =
  "目标表无主键 → 本任务为纯追加写，重跑会产生重复数据";

/**
 * 写入语义的常驻交底：这一句在向导里跟着写入模式一起摆，永远不缺席。
 *
 * 两种写法各有各的坑，说同一句话等于其中一句必然是假的：upsert 的坑是「源端删掉的行
 * 不会跟着消失」，纯追加的坑是「重跑翻倍」。
 */
export function writeSemanticsNote(
  statement: WriteStatement,
  mode: WriteMode = "APPEND",
  preSql?: string | null,
): string {
  if (clearsTarget(mode)) {
    // 清空模式下，上面那两个坑一个都不存在——「源端删掉的行留着」正是它来解决的，
    // 「重跑翻倍」也不会发生，因为每次都从空表开始。换来的是一笔新的代价，
    // 而它必须在这里说满：原有数据没了，且没有撤销。
    return `先清空再导入：同一个事务里先清空目标表再导入，跑完之后目标表精确等于本次查询结果；原有数据不可恢复，也没有撤销入口。写入语句仍是${writeStatementLabel(
      statement,
    )}——清空不改变它。`;
  }
  if (hasPreSql(preSql)) {
    return `追加写 + preSQL 清理：运行会先按条件清理目标表，再执行${writeStatementLabel(
      statement,
    )}；清理与导入在同一事务中提交。`;
  }
  return statement === "upsert"
    ? "按主键 upsert：新增和变更会写进目标表；源端删除的行不会跟着消失。"
    : `${APPEND_ONLY_CONCLUSION}。目标表上没有可去重的唯一约束，本次写入是纯 INSERT。`;
}

/** 同一件事的过去时：跑完之后，陈述这次写入到底做了什么。 */
export function writeSemanticsDone(
  statement: WriteStatement,
  mode: WriteMode = "APPEND",
): string {
  if (clearsTarget(mode)) {
    return "先清空再导入：目标表已整表替换，此刻它精确等于本次查询的结果；原有数据已删除，不可恢复。";
  }
  return statement === "upsert"
    ? "按主键 upsert：新增和变更已写入；源端删除的行仍保留在目标表。"
    : "纯追加写：这批数据已追加进目标表，一行都没有删；再跑一次会再追加一份。";
}

/**
 * 目标表**此刻**有没有可合并的唯一约束。
 *
 * 判据是「有没有唯一约束」而不是「有没有 PRIMARY KEY」：一条 UNIQUE 索引同样会让
 * 纯 INSERT 撞 `ERROR 1062`，目标端预检认的也是这一条，两边必须是同一句话。
 */
export function targetHasUniqueKey(keys: readonly TargetKey[]): boolean {
  return keys.length > 0;
}

/**
 * 一次运行**当时**是怎么写的：主键与写入模式都取自那一行历史上的运行证据快照，
 * 不是任务此刻的定义。
 *
 * 任务定义随时会改：主键加一列、写法从追加改成先清空再导入。回头拿现在的定义去讲
 * 过去那一次，等于让今天的一次编辑追溯性地改写历史记录里「已经做过的事」——和改名
 * 会改写过去每一行上的名字是同一个错，#259 因此把名称快照到历史行上，这里照办。
 *
 * 只有一种情况回退到当前定义：这条历史早于运行证据（`parameters` 缺席），那时没有
 * 任何快照可读，回退是唯一比空白好的答案。`write_mode` 单独缺席（早于 #264 的历史行）
 * 则按 `APPEND` 读——那时产品只有这一档。
 */
export function runWriteView(
  evidence: RunEvidence | undefined,
  spec: TaskSpec,
): { statement: WriteStatement; mode: WriteMode } {
  const parameters = evidence?.parameters ?? null;
  return parameters === null
    ? { statement: writeStatementOf(spec.primary_key), mode: spec.write_mode }
    : {
        statement: writeStatementOf(parameters.primary_key),
        mode: parameters.write_mode ?? "APPEND",
      };
}

/** 跑完之后那句「这一次做了什么」，说的是当时那一次。 */
export function runWriteSemantics(
  evidence: RunEvidence | undefined,
  spec: TaskSpec,
): string {
  const write = runWriteView(evidence, spec);
  if (write.mode === "APPEND" && hasPreSql(runPreSql(evidence))) {
    return "追加写 + preSQL 清理：清理与导入已在同一事务中提交。";
  }
  return writeSemanticsDone(write.statement, write.mode);
}
