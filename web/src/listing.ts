import type { Datasource, RunHistory, Task } from "./api";
import { historyPresentation } from "./history";
import { sourceSummary } from "./spec";

/**
 * 作业中心的**列表判定**（P1 留下的那一套，P2 原样复用）。
 *
 * 摆在组件外面的理由与 `datasource.ts` 同源：这几条是**规则**不是渲染。
 * 筛选筛掉了不该筛的行、分页把最后一页算少一页、「最近一次运行」认错了那条记录，
 * 三样都是用户看不出来的错——只能靠用例守着。渲染仍归走查。
 *
 * 三条边界，别在实现里悄悄挪：
 *
 * 1. **一律客户端过滤 / 客户端分页**。当前 API 没有 `limit/offset`（ADR-0039 的口径未变），
 *    做出服务端分页的假象比不分页更坏：翻到第 3 页却发现总数是刚才那一屏的。
 * 2. **「运行状态」不是一个新语义**，它就是 `historyPresentation(row).kind` 加一个
 *    「尚未运行」。它是一格**一维索引**，不是轴二——轴二（目标表效果）与轴三（错误码）
 *    整体在详情抽屉里，形状一个没变（ADR-0043 §4）。
 * 3. **「尚未运行」是事实，不是缺数据**：这个任务一条运行记录都没有。
 *    2026-08-21 起「读取失败」不再是这一列的一态——运行记录与任务清单是同一次读取的两半，
 *    读不到就是整屏读不到（ADR-0043 §2，走查 X10）。
 *
 * 运行历史独立屏随 ADR-0043 §2 取消，`HistoryFilters` / `historyMatchesFilters`
 * 一并删除：它们唯一的调用方是那个屏。按任务筛的服务端参数 `RunHistoryFilters` 还在，
 * 那是 API 的东西，不归本文件。
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

/** 运行记录四态的排序（`""` = 全部，由界面另行摆在最前）。 */
export const RUN_STATUS_ORDER: readonly RunStatus[] = [
  "succeeded",
  "failed",
  "live",
  "unknown",
];

/** 作业中心「运行状态」下拉的选项顺序——四态加上「尚未运行」。 */
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
      // 自定义 SQL 的任务 owner / table 都是空串，直接拼这两个字段等于把它排除在
      // 关键词搜索之外。`full` 在 SQL 模式下是整条语句，用户记得的表名因此能搜到。
      sourceSummary(task.spec).full,
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

export const DEFAULT_PAGE_SIZE = 20;

/**
 * 「每页条数」下拉的取值（ADR-0043 §走查触发 X11，形态照参照物）。
 *
 * 三档而不是连续输入：现场规模是几十个任务，20 / 50 / 100 已经覆盖「一屏」「翻两页」
 * 「一次看完」三种意图；让人手填一个数只会换来 37 这种没人再想第二次的值。
 * 第一档必须等于 `DEFAULT_PAGE_SIZE`，否则下拉一进来就显示一个跟实际不符的值。
 */
export const PAGE_SIZE_OPTIONS: readonly number[] = [DEFAULT_PAGE_SIZE, 50, 100];

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
