import type { Datasource, DatasourceInput, Task } from "./api";

/**
 * 数据源屏的纯判定（ADR-0039 §2/§3/§4）。
 *
 * 摆在组件外面是因为这几条是**规则**不是渲染：「测通才让存」的门槛、只改名称的例外、
 * 两类字段集各显各的连接摘要，错一条的后果都是用户看不出来的（存进去一条连不上的数据源、
 * 或者改个名字被逼着去连一个已经下线的库）。规则有用例守着，渲染归走查。
 */

/** 「连接」一栏：两类字段集不同，**各显各的**，不拼一个假的统一连接串（ADR-0039 §2）。 */
export function connectionSummary(datasource: Datasource): string {
  return datasource.kind === "oracle"
    ? datasource.connect_string
    : `${datasource.host}:${datasource.port} / ${datasource.database}`;
}

/**
 * 表单里那组**连接字段**的指纹。
 *
 * **名称不在里面**——只改名称免测连是 ADR-0039 §3 的唯一例外，而它正是靠「名称不进指纹」
 * 实现的：改名不动指纹，指纹没动就还等于初始值，门槛自然放行。
 */
export function connectionFingerprint(input: DatasourceInput): string {
  return JSON.stringify(
    input.kind === "oracle"
      ? [input.kind, input.connect_string, input.username, input.password]
      : [
          input.kind,
          input.host,
          input.port,
          input.database,
          input.username,
          input.password,
        ],
  );
}

/**
 * 保存门槛（所有者 2026-08-19 裁定 2）：**当前这组连接字段必须有过一次成功测连**。
 *
 * 两条放行路径，只有两条：
 * 1. 刚测通的那组值就是现在表单里的这组（指纹相等）；
 * 2. 编辑态且连接字段一字未动——那组值没变，重测买不到任何新信息，
 *    却要求库在改名的那一刻恰好在线。
 *
 * 新建态没有第 2 条：库里还没有这条数据源，「没变」无从谈起。
 */
export function canSaveDatasource(
  draft: DatasourceInput,
  initial: DatasourceInput | null,
  testedFingerprint: string | null,
): boolean {
  const fingerprint = connectionFingerprint(draft);
  if (testedFingerprint === fingerprint) {
    return true;
  }
  return initial !== null && fingerprint === connectionFingerprint(initial);
}

/**
 * 编辑态的初值。**口令永远是空的**：界面不回读口令，连密文都不回（ADR-0037 §5）；
 * 空串在写入面上的含义是「不改」，与这里的「显示为空」是同一件事的两面。
 */
export function draftFrom(existing: Datasource | null): DatasourceInput {
  if (existing === null) {
    return {
      name: "",
      kind: "oracle",
      connect_string: "",
      username: "",
      password: "",
    };
  }
  return existing.kind === "oracle"
    ? {
        name: existing.name,
        kind: "oracle",
        connect_string: existing.connect_string,
        username: existing.username,
        password: "",
      }
    : {
        name: existing.name,
        kind: "mysql",
        host: existing.host,
        port: existing.port,
        database: existing.database,
        username: existing.username,
        password: "",
      };
}

/**
 * 「被引用」列的计数（ADR-0039 §2）。
 *
 * 数的是**任务**不是绑定：一个任务两端都指着同一条数据源仍然只算一个——
 * 删除被拒时服务端点名的就是任务，两处口径必须一致，否则列上写「1 个任务」、
 * 删除时列出两条同名任务。
 */
export function referenceCounts(tasks: Task[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const task of tasks) {
    for (const id of new Set([
      task.source_datasource_id,
      task.target_datasource_id,
    ])) {
      if (id !== "") {
        counts.set(id, (counts.get(id) ?? 0) + 1);
      }
    }
  }
  return counts;
}
