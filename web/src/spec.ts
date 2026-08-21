// 结构化任务规格的展示口径，唯一来源（ADR-0036 §1）。
//
// 构建器、发起面、运行历史三处都要把同一批概念摆出来：比较符怎么写、参数名默认叫什么、
// 一组运行参数怎么读。各写一份迟早漂成三种说法，所以都从这里取。

import type {
  ColumnMapping,
  Comparison,
  Condition,
  RunParams,
  TaskSpec,
  ValueType,
} from "./api";

/**
 * 这个源列映到哪个目标字段——没选中就是 `undefined`（ADR-0038 §2）。
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
 * 改目标字段名——**主键跟着走**（ADR-0039 增补 1）。
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

export function comparisonSymbol(operator: Comparison): string {
  switch (operator) {
    case "gt":
      return ">";
    case "lt":
      return "<";
    case "eq":
      return "=";
  }
}

/** 下拉里的三个比较符，顺序固定。第一版只做这三个（ADR-0035 §3 字面）。 */
export const COMPARISONS: ReadonlyArray<Comparison> = ["gt", "lt", "eq"];

export const VALUE_TYPE_LABELS: Readonly<Record<ValueType, string>> = {
  text: "文本",
  number: "数字",
  date: "日期",
};

/**
 * 按 Oracle 字典类型给这一条条件的 `value_type` **预填**。
 *
 * 它只是预填：`value_type` 是用户的声明，不是缓存下来的元数据（ADR-0036 §6 不许把
 * describe 的类型存进任务定义），所以用户随时可以改，改了以生成的 SQL 为准。
 */
export function defaultValueType(dataType: string | undefined): ValueType {
  const normalized = (dataType ?? "").toUpperCase();
  if (normalized.startsWith("DATE") || normalized.startsWith("TIMESTAMP")) {
    return "date";
  }
  if (
    normalized.startsWith("NUMBER") ||
    normalized.startsWith("FLOAT") ||
    normalized.startsWith("BINARY_") ||
    normalized.startsWith("INTEGER")
  ) {
    return "number";
  }
  return "text";
}

/**
 * 新条件的默认参数名：列名小写，与已有参数冲突时加序号后缀。
 *
 * 参数名由用户拥有、可改——它是运行参数与运行历史里的键，**按序号自动编号会让增删一条
 * 参数把此前所有历史里的键都对不上**（ADR-0036 §7 否掉顺序串接是同一个理由）。
 * 这里只负责给一个不撞车的起点。
 */
export function defaultParameterName(
  column: string,
  taken: ReadonlyArray<string>,
): string {
  const base = column.toLowerCase();
  const used = new Set(taken.map((name) => name.toLowerCase()));
  if (!used.has(base)) {
    return base;
  }
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `${base}_${suffix}`;
    if (!used.has(candidate)) {
      return candidate;
    }
  }
}

/** 运行时逐条填的条件，按参数名排序——与 source 侧 `runtime_parameters()` 同序。 */
export function runtimeConditions(spec: TaskSpec): Condition[] {
  return spec.conditions
    .filter((condition) => condition.value_source === "runtime")
    .sort((left, right) => left.parameter.localeCompare(right.parameter));
}

/**
 * 两个运行参数集是不是同一组（ADR-0036 §7 的规范形式：参数名 → 值，值原样字符串）。
 *
 * 这是**提示用**的比较，真正的并发判断在后端；两边用同一个口径，提示才不会与门禁打架。
 */
export function sameRunParams(left: RunParams, right: RunParams): boolean {
  const leftKeys = Object.keys(left).sort();
  const rightKeys = Object.keys(right).sort();
  if (leftKeys.length !== rightKeys.length) {
    return false;
  }
  return leftKeys.every(
    (key, index) => rightKeys[index] === key && left[key] === right[key],
  );
}

/**
 * 一组运行参数的单行读法：`参数名=值`，按参数名排序，值**原样**给。
 *
 * 不做格式化、不做类型解释——历史那一列的全部价值是它是当时的事实，
 * 值是什么字符串就显示什么字符串。无参数的任务给破折号，不给空白。
 */
export function runParamsSummary(params: RunParams): string {
  const entries = Object.entries(params).sort(([left], [right]) =>
    left.localeCompare(right),
  );
  if (entries.length === 0) {
    return "—";
  }
  return entries.map(([name, value]) => `${name}=${value}`).join(" · ");
}

/**
 * 任务条件的一行摘要（`LOAD_DATE = :load_date AND STATUS = 'OK'`）。
 *
 * 它原来长在 `App.tsx` 里给任务屏那一列用；P2 把条件收进详情抽屉（ADR-0043 §4），
 * 抽屉与构建器都要同一句话，于是搬到这里——**一个字没改**，只换了住处。
 * 一条条件都没有时写「整表」，不是空白：空白读起来像「这里应该有点什么但没渲染」。
 */
export function conditionSummary(spec: TaskSpec): string {
  if (spec.conditions.length === 0) {
    return "整表";
  }
  return spec.conditions
    .map(
      (condition) =>
        `${condition.column} ${comparisonSymbol(condition.operator)} ${
          condition.value_source === "constant"
            ? condition.constant
            : `:${condition.parameter}`
        }`,
    )
    .join(" AND ");
}
