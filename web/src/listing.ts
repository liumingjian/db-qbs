import type { Datasource, RunHistory, Task } from "./api";
import { historyPresentation } from "./history";

/**
 * 任务屏与运行历史屏共用的**列表判定**（P1，x2doris 式列表工作流）。
 *
 * 摆在组件外面的理由与 `datasource.ts` 同源：这几条是**规则**不是渲染。
 * 筛选筛掉了不该筛的行、分页把最后一页算少一页、「最近运行」认错了那条记录，
 * 三样都是用户看不出来的错——只能靠用例守着。渲染仍归走查。
 *
 * 三条边界，别在实现里悄悄挪：
 *
 * 1. **一律客户端过滤 / 客户端分页**。当前 API 没有 `limit/offset`（ADR-0039 的口径未变），
 *    做出服务端分页的假象比不分页更坏：翻到第 3 页却发现总数是刚才那一屏的。
 * 2. **「最近状态」不是一个新语义**，它就是 `historyPresentation(row).kind`。
 *    三轴（运行结局 / 目标表效果 / 错误码）不因为列表上要一个筛选项就被压成一个彩色标签。
 * 3. **「尚未运行」与「读取失败」是两回事**：前者是事实（这个任务没有历史记录），
 *    后者是这一次没读到。列表把它们混成同一格，等于替服务端下结论。
 */

/** 一条运行记录的结局归类。取值与 `HistoryPresentation["kind"]` 逐字相同，不另立词表。 */
export type RunStatus = "succeeded" | "failed" | "live" | "unknown";

/** 任务的「最近状态」比运行记录多一态：**这个任务一条历史都没有**。 */
export type LatestRunStatus = RunStatus | "none";

export const RUN_STATUS_LABELS: Readonly<Record<RunStatus, string>> = {
  succeeded: "成功",
  failed: "失败",
  live: "进行中",
  unknown: "结局不明",
};

export const LATEST_RUN_LABELS: Readonly<Record<LatestRunStatus, string>> = {
  ...RUN_STATUS_LABELS,
  none: "尚未运行",
};

/** 运行历史筛选条上「状态」下拉的选项顺序（`""` = 全部，由界面另行摆在最前）。 */
export const RUN_STATUS_ORDER: readonly RunStatus[] = [
  "succeeded",
  "failed",
  "live",
  "unknown",
];

/** 任务屏「最近状态」下拉的选项顺序。 */
export const LATEST_RUN_ORDER: readonly LatestRunStatus[] = [
  ...RUN_STATUS_ORDER,
  "none",
];

export function runStatus(row: RunHistory): RunStatus {
  return historyPresentation(row).kind;
}

function startedAtMs(row: RunHistory): number {
  const parsed = Date.parse(row.started_at);
  // 时间戳解析不出来时排到最旧，而不是变成 NaN 让比较全部为 false——
  // 那样「最近一次」会退化成「碰巧第一条」。
  return Number.isNaN(parsed) ? Number.NEGATIVE_INFINITY : parsed;
}

/**
 * 每个任务的**最近一次**运行记录。
 *
 * 按发起时间取最大；同一毫秒发起的两条按 `run_record_id` 字典序定序——
 * 不是因为 id 有语义，而是因为这一格必须**可复现**：同一份数据两次渲染不能给出不同的行。
 * `seq` 不参与排序：它是一次运行内部的进度序号，跨运行比大小没有意义。
 */
export function latestRunByTask(
  history: readonly RunHistory[],
): Map<string, RunHistory> {
  const latest = new Map<string, RunHistory>();
  for (const row of history) {
    const incumbent = latest.get(row.task_id);
    if (incumbent === undefined || isNewer(row, incumbent)) {
      latest.set(row.task_id, row);
    }
  }
  return latest;
}

function isNewer(candidate: RunHistory, incumbent: RunHistory): boolean {
  const candidateMs = startedAtMs(candidate);
  const incumbentMs = startedAtMs(incumbent);
  if (candidateMs !== incumbentMs) {
    return candidateMs > incumbentMs;
  }
  return candidate.run_record_id > incumbent.run_record_id;
}

export function latestRunStatus(row: RunHistory | undefined): LatestRunStatus {
  return row === undefined ? "none" : runStatus(row);
}

export interface TaskFilters {
  /** 任务名 / 源表 / 目标表的关键词，大小写不敏感。 */
  keyword: string;
  sourceDatasourceId: string;
  targetDatasourceId: string;
  /** `""` = 全部。 */
  latestStatus: LatestRunStatus | "";
}

export const EMPTY_TASK_FILTERS: TaskFilters = {
  keyword: "",
  sourceDatasourceId: "",
  targetDatasourceId: "",
  latestStatus: "",
};

/**
 * 任务是否命中筛选条。
 *
 * 关键词那一维仍覆盖**源表与目标表**（P0 那个搜索框就是这么用的），
 * 只是标签从「搜索」换成了「任务名」，占位符照旧写明三样都能搜。
 * 收窄成只搜任务名会当场丢掉一个在用的能力。
 */
export function taskMatchesFilters(
  task: Task,
  filters: TaskFilters,
  status: LatestRunStatus,
): boolean {
  const keyword = filters.keyword.trim().toLocaleLowerCase("zh-CN");
  if (keyword !== "") {
    const haystack = [
      task.name,
      task.task_id,
      `${task.spec.owner}.${task.spec.table}`,
      task.spec.target_table,
    ]
      .join(" ")
      .toLocaleLowerCase("zh-CN");
    if (!haystack.includes(keyword)) {
      return false;
    }
  }
  if (
    filters.sourceDatasourceId !== "" &&
    task.source_datasource_id !== filters.sourceDatasourceId
  ) {
    return false;
  }
  if (
    filters.targetDatasourceId !== "" &&
    task.target_datasource_id !== filters.targetDatasourceId
  ) {
    return false;
  }
  return filters.latestStatus === "" || filters.latestStatus === status;
}

export interface HistoryFilters {
  taskId: string;
  /** `""` = 全部。 */
  status: RunStatus | "";
}

export const EMPTY_HISTORY_FILTERS: HistoryFilters = { taskId: "", status: "" };

/**
 * 运行历史是否命中筛选条。
 *
 * 任务那一维服务端也筛（`listRunHistory({ taskId })`），这里仍然再判一遍：
 * 「查询」按的是同一份筛选条，两边口径必须一致，而只有客户端这一份能被用例守住。
 */
export function historyMatchesFilters(
  row: RunHistory,
  filters: HistoryFilters,
): boolean {
  if (filters.taskId !== "" && row.task_id !== filters.taskId) {
    return false;
  }
  return filters.status === "" || filters.status === runStatus(row);
}

export const DEFAULT_PAGE_SIZE = 20;

export interface PageSlice<T> {
  rows: T[];
  total: number;
  /** 至少 1——**空清单也有第 1 页**，否则「第 0 / 0 页」会渲染出来。 */
  pageCount: number;
  /** 夹回合法区间之后的页码。调用方应当以它为准，别再用自己传进来的那个。 */
  page: number;
  pageSize: number;
}

/**
 * 客户端分页。**页码越界一律夹回**，不抛错也不返回空页：
 * 删掉最后一页上唯一那条记录之后，界面该退回上一页，而不是显示一屏空白。
 */
export function paginate<T>(
  items: readonly T[],
  page: number,
  pageSize: number = DEFAULT_PAGE_SIZE,
): PageSlice<T> {
  const size = Math.max(1, Math.floor(pageSize));
  const total = items.length;
  const pageCount = Math.max(1, Math.ceil(total / size));
  const current = Math.min(Math.max(1, Math.floor(page) || 1), pageCount);
  const start = (current - 1) * size;
  return {
    rows: items.slice(start, start + size),
    total,
    pageCount,
    page: current,
    pageSize: size,
  };
}

/**
 * 任务屏「源端 / 目标端」两个下拉的选项：`[id, 显示名]`，按显示名排序。
 *
 * 两条不显然的规则：
 *
 * 1. **按类型分边**——源端只列 Oracle、目标端只列 MySQL（v1 固定 Oracle → MySQL）。
 *    把 MySQL 摆进源端下拉，选中之后一条也筛不出来。
 * 2. **任务还引用着、数据源清单里却没有的 id 照样进选项**（显示成 id 本身）。
 *    数据源被删掉之后，那几个任务并不会跟着消失；下拉里没有它，这批任务就再也筛不出来了。
 */
export function datasourceFilterOptions(
  datasources: readonly Datasource[],
  tasks: readonly Task[],
  side: "source" | "target",
): [string, string][] {
  const kind = side === "source" ? "oracle" : "mysql";
  const options = new Map<string, string>();
  for (const datasource of datasources) {
    if (datasource.kind === kind) {
      options.set(datasource.datasource_id, datasource.name);
    }
  }
  for (const task of tasks) {
    const id =
      side === "source" ? task.source_datasource_id : task.target_datasource_id;
    if (id !== "" && !options.has(id)) {
      options.set(id, id);
    }
  }
  return [...options.entries()].sort((left, right) =>
    left[1].localeCompare(right[1], "zh-CN"),
  );
}
