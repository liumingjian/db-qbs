// 失败运行的一键重跑：**该不该给这个入口**。
//
// 重跑不是新语义，是把「回任务屏、找到那个任务、再点一次发起」这几步折成一步：
// 它走的就是**现成的**发起路径，后端互斥、映射预检一个都不绕过。
// 运行参数链退役之后连预填也没了——上一次没留下任何需要带过来的取值，
// 「重跑」与「发起」跑的是同一个函数。
//
// 判定是个纯函数，为的是能在不渲染任何组件的前提下把「什么样的行给什么入口」测干净。

import type { RunHistory, Task } from "./api";
import { historyPresentation } from "./history";
import { targetHoldState } from "./targetHold";

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
 * 目标表占用还没还回来时同样禁用（#271）：那时候「可以重跑」是一句假话。
 */
export function rerunAction(
  row: RunHistory,
  tasks: ReadonlyArray<Task> | null,
): RerunAction {
  const kind = historyPresentation(row).kind;
  if (kind !== "failed" && kind !== "unknown") {
    return { kind: "hidden" };
  }
  // 目标表占用还挂在目标端时**一律不给重跑**（#271）：这一条比「结局是什么」更硬，
  // 因为它说的不是这次跑得怎么样，而是下一次根本跑不起来——点下去只换回一个
  // `TARGET_TABLE_BUSY`。入口照旧不消失，禁用并说清楚下一步该做什么。
  // 判据与说辞都在 `targetHoldState` 一处，这里只负责把它摆到重跑这一颗上。
  const hold = targetHoldState(row);
  if (hold.kind !== "free") {
    return { kind: "disabled", reason: hold.rerunRefusal };
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
