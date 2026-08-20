// 失败运行的一键重跑：**该不该给这个入口、按什么预填**（ADR-0041 增补 2、规格 #149 A 段）。
//
// 重跑不是新语义，是把「回任务屏、找到那个任务、重新填一遍参数」这三步折成一步：
// 它打开的是**现成的**发起对话框，确认键仍是「发起」，并发提示、后端互斥、映射预检
// 一个都不绕过。零后端改动——历史行里已经存着当时的运行参数集，发起入口也早就在。
//
// 判定与预填在这里各自成一个纯函数，是为了能在不渲染任何组件的前提下把
// 「什么样的行给什么入口、推导出什么值」测干净（规格 #149 Testing Decisions）。

import type { RunHistory, RunParams, Task, TaskSpec } from "./api";
import { historyPresentation } from "./history";
import { runtimeConditions } from "./spec";

export type RerunAction =
  /** 这一行压根没有重跑这回事：进行中、或者已经成功了。 */
  | { kind: "hidden" }
  /** 可以重跑，跑的是**这个任务当前的**定义。 */
  | { kind: "enabled"; task: Task }
  /** 该有入口，但此刻按不动——按钮留在原地并说明为什么（规格 #149 A6）。 */
  | { kind: "disabled"; reason: string };

/**
 * 这一行该不该有「重跑」，以及能不能按。
 *
 * 终局分类**不在这里重判**，直接问 `historyPresentation`：运行历史的结局口径只有一处，
 * 否则「结局不明」这类行的归属迟早在两个文件里漂成两种说法。
 *
 * - `failed` / `unknown` → 有入口。结局不明也给，依据是按主键 upsert 幂等（ADR-0035）：
 *   哪怕那次其实是跑成了的，重跑也不会写错数据。
 * - `live` / `succeeded` → 没有入口。进行中的不该被再捅一次，成功的没有重跑的由头。
 *
 * 任务已被删除时**不让入口消失**——凭空消失会被读成「功能坏了」，禁用加一句原因才是实话。
 */
export function rerunAction(
  row: RunHistory,
  tasks: ReadonlyArray<Task> | null,
): RerunAction {
  const kind = historyPresentation(row).kind;
  if (kind !== "failed" && kind !== "unknown") {
    return { kind: "hidden" };
  }
  if (tasks === null) {
    // 读失败时 `App` 的 `tasks` 也停在 `null`，所以这句话**不许说成「还在读」**——
    // 那会让人干等一个不会到来的时刻。刷新页面是这条路径上唯一有效的动作。
    return {
      kind: "disabled",
      reason: "任务清单没读到（还在读，或者读失败了）——刷新页面后再试。",
    };
  }
  const task = tasks.find((candidate) => candidate.task_id === row.task_id);
  if (task === undefined) {
    return {
      kind: "disabled",
      reason: "任务已删除。重跑按任务当前的规格现算 SQL，规格没了就无从跑起。",
    };
  }
  return { kind: "enabled", task };
}

/**
 * 发起对话框的初值：**按任务当前规格的「运行时填」条件对齐**上一次的运行参数集。
 *
 * 三条规则（规格 #149 A4）：行里有的取行值、行里没有的留空、行里多出的丢弃。
 * 「多出的」是规格改过之后的常态——那个参数已经不存在了，把它带进新的一次发起
 * 只会被后端按未知参数拒掉。反过来，新增的参数留空由人来填，不替他猜。
 *
 * 空字符串是**有值**，不是缺值：`??` 而不是 `||`，否则「这次就是要跑空串」会被悄悄改写。
 *
 * 没有上一次时传空对象即可——出来的正好是一张空表单，所以普通发起与重跑走的是同一条路。
 */
export function rerunPrefill(spec: TaskSpec, previous: RunParams): RunParams {
  return Object.fromEntries(
    runtimeConditions(spec).map((condition) => [
      condition.parameter,
      previous[condition.parameter] ?? "",
    ]),
  );
}
