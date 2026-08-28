/**
 * 写入模式，以及它落到的那条语句。前端这一份是 `crates/shared/src/write_mode.rs` 的镜子。
 *
 * 两件事分开，且**只有一件是人选的**：
 *
 * - `WriteMode` 是任务定义里存的那个值，今天只有「追加写」一档；
 * - `WriteStatement` 是目标端真正会跑的语句，**没人能选它**——目标表有主键就是
 *   `ON DUPLICATE KEY UPDATE`，没有就是纯 `INSERT ... SELECT`。
 *
 * 需求方明确不给「无主键」加勾选框。那么界面上唯一能做的事就是**把这件事说出来**：
 * 无主键意味着重跑会产生重复数据，而这是这个产品第一次接受「同一任务定义跑两次、
 * 目标表状态不同」。下面这些句子因此只有一份定义——向导、任务列表、运行详情三处
 * 抄的是同一个常量，抄成三份第一次改口径就会漏掉一处。
 */

import type { TargetKey } from "./api";

export type WriteMode = "APPEND";

export type WriteStatement = "upsert" | "insert";

/** 写入模式的清单。清空后导入那一档接进来时加在这里，向导里那组单选按钮自动多一项。 */
export const WRITE_MODES: readonly { mode: WriteMode; label: string; hint: string }[] = [
  {
    mode: "APPEND",
    label: "追加写",
    hint: "把查询结果写进目标表，已有的行一行都不删。",
  },
];

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

/** 无主键那条路上必须被读到的那句话。与目标端预检的结论逐字一致。 */
export const APPEND_ONLY_CONCLUSION =
  "目标表无主键 → 本任务为纯追加写，重跑会产生重复数据";

/**
 * 写入语义的常驻交底：这一句在向导里跟着写入模式一起摆，永远不缺席。
 *
 * 两种写法各有各的坑，说同一句话等于其中一句必然是假的：upsert 的坑是「源端删掉的行
 * 不会跟着消失」，纯追加的坑是「重跑翻倍」。
 */
export function writeSemanticsNote(statement: WriteStatement): string {
  return statement === "upsert"
    ? "按主键 upsert：新增和变更会写进目标表；源端删除的行不会跟着消失。"
    : `${APPEND_ONLY_CONCLUSION}。目标表上没有可去重的唯一约束，本次写入是纯 INSERT。`;
}

/** 同一件事的过去时：跑完之后，陈述这次写入到底做了什么。 */
export function writeSemanticsDone(statement: WriteStatement): string {
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
