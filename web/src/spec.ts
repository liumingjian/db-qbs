// 结构化任务规格的展示口径，唯一来源。
//
// 构建器、发起面、运行历史三处都要把同一批概念摆出来：一列映到哪、过滤条件怎么读成一行、
// 源端在列表里显示成什么。各写一份迟早漂成三种说法，所以都从这里取。
//
// 词汇表随运行参数链的退役**变小了，但角色没变**：这里仍然是那三处共用的唯一一份。

import type { ColumnMapping, TaskSpec } from "./api";

/**
 * 这个源列映到哪个目标字段——没选中就是 `undefined`。
 *
 * 界面上一行代表一个**源列**，而主键存的是**目标字段**，两个名字空间要在这里换一次。
 * 恒等映射下它们同名，省掉这一层在改过名的列上就会错。
 */
export function targetFieldOf(
  columns: ReadonlyArray<ColumnMapping>,
  source: string,
): string | undefined {
  return columns.find((mapping) => mapping.source === source)?.target;
}

/**
 * 改目标字段名——**主键跟着走**。
 *
 * 这是换名字空间的单点：`columns[].target` 与 `primary_key` 存的是同一个名字，改一处
 * 另一处必须同步重写。**不许出现「界面勾着、`TaskSpec` 里是旧名」的中间态**——
 * 那会一路走到 sink 预检才炸成「主键列必须落在本次选中的列里」，而用户什么都没做错。
 *
 * 主键里的**顺序原样保留**（它是复合键的列序，不是集合）；改成一个已经在主键里的名字时
 * 不留重复项。源列没选中时原样返回，不凭空造一条映射。
 */
export function renameTargetField(
  spec: Pick<TaskSpec, "columns" | "primary_key">,
  source: string,
  nextTarget: string,
): Pick<TaskSpec, "columns" | "primary_key"> {
  const previous = targetFieldOf(spec.columns, source);
  if (previous === undefined || previous === nextTarget) {
    return spec;
  }
  const columns = spec.columns.map((mapping) =>
    mapping.source === source ? { ...mapping, target: nextTarget } : mapping,
  );
  if (!spec.primary_key.includes(previous)) {
    return { columns, primary_key: spec.primary_key };
  }
  if (nextTarget.trim() === "") {
    return {
      columns,
      primary_key: spec.primary_key.filter((name) => name !== previous),
    };
  }
  const primary_key: string[] = [];
  for (const name of spec.primary_key) {
    const renamed = name === previous ? nextTarget : name;
    if (!primary_key.includes(renamed)) {
      primary_key.push(renamed);
    }
  }
  return { columns, primary_key };
}

/**
 * 任务过滤条件的一行读法（详情抽屉里那一格）。
 *
 * WHERE 文本是用户自己写的，可能有换行、可能很长，而这里只有一行的位置：
 * 折叠空白之后原样给，**一个字符不改写**——这一格的全部价值就是它是那段原文。
 * 没写过滤时给「整表」，不是空白：空白读起来像「这里应该有点什么但没渲染」。
 */
export function whereSummary(spec: Pick<TaskSpec, "where_clause">): string {
  // `?? ""`：这个字段在服务端是 `Option<String>`，`None` 时整个不序列化。
  const collapsed = (spec.where_clause ?? "").replace(/\s+/g, " ").trim();
  return collapsed === "" ? "整表" : collapsed;
}

/**
 * 源端在列表里的一行读法（作业中心的「源表」列 + 搜索索引）。
 *
 * 自定义 SQL 的规格里 `owner` / `table` 都是空串，直接拼 `owner.table` 会渲染成一个
 * 孤零零的 `.`，搜索索引里也只剩这个点——那类任务**按源表关键字永远搜不到**。
 * 两处都从这里取，口径只有一份。
 *
 * `label` 进单元格（折叠空白、截断），`full` 进搜索索引与 `title` 提示——
 * 索引吃全文是有意的：用户记得的是 SQL 里那张表的名字，不是任务名。
 */
export interface SourceSummary {
  kind: "table" | "sql";
  /** 单元格里显示的一行：空白已折叠，过长时尾部截断成省略号。 */
  label: string;
  /** 完整原文：`owner.table`，或整条 SQL 的原样文本。 */
  full: string;
}

/** 单元格里一行放得下的字数。超出只截 `label`，`full` 永远是全文。 */
const SOURCE_LABEL_MAX = 48;

export function sourceSummary(
  spec: Pick<TaskSpec, "owner" | "table" | "source_sql">,
): SourceSummary {
  const sourceSql = spec.source_sql?.trim() ?? "";
  if (sourceSql === "") {
    const label = `${spec.owner}.${spec.table}`;
    return { kind: "table", label, full: label };
  }
  const collapsed = sourceSql.replace(/\s+/g, " ").trim();
  return {
    kind: "sql",
    label:
      collapsed.length > SOURCE_LABEL_MAX
        ? `${collapsed.slice(0, SOURCE_LABEL_MAX)}\u2026`
        : collapsed,
    full: sourceSql,
  };
}

/** 同名匹配只认名字，不认大小写——目标端是 MySQL。 */
export interface TargetColumnName {
  name: string;
}

/**
 * 把源列按**同名**接到目标字段上，返回改动后的 `columns` + `primary_key`。
 *
 * 两个调用点共用这一份：读取目标列之后的自动接线（`onlyUnmapped: true`，
 * 只补空位，绝不冲掉用户手改过的映射），以及「同名填充」那颗键
 * （`onlyUnmapped: false`，用户显式要求，覆盖）。
 * 差别只有这一个开关——两份各写一遍的时候，差别藏在函数名里看不见。
 *
 * 走 `renameTargetField` 而不是直接改 `columns`，是因为目标名一变主键就要跟着走
 * （主键存的是目标字段名）。
 */
export function matchSameNameTargets(
  spec: Pick<TaskSpec, "columns" | "primary_key">,
  targetColumns: readonly TargetColumnName[],
  { onlyUnmapped }: { onlyUnmapped: boolean },
): Pick<TaskSpec, "columns" | "primary_key"> {
  const targetByUpper = new Map(
    targetColumns.map((column) => [column.name.toUpperCase(), column.name]),
  );
  return spec.columns.reduce<Pick<TaskSpec, "columns" | "primary_key">>(
    (current, mapping) => {
      if (onlyUnmapped && mapping.target.trim() !== "") {
        return current;
      }
      const target = targetByUpper.get(mapping.source.toUpperCase());
      return target === undefined
        ? current
        : renameTargetField(current, mapping.source, target);
    },
    { columns: spec.columns, primary_key: spec.primary_key },
  );
}
