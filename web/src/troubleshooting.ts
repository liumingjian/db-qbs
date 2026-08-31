import type { RunHistory } from "./api";
import { historyPresentation } from "./history";
import { abortRefusal } from "./runStage";
import type { HeldTargetHold } from "./targetHold";
import { targetHoldState } from "./targetHold";
import type { Step } from "./wizard";

export type RowRunAction =
  | { kind: "start"; disabled: boolean }
  /**
   * `refusal` 非空 = 这一刻停不了，附上理由。
   *
   * 规则在 `abortRefusal` 一处，运行详情屏一直读着它；列表这一颗过去无条件亮着，
   * 于是同一条规则在两个地方给出两个答案，而列表那个答案要靠一个 409 才被推翻
   * （UX 评审 P1-11）。按钮**不消失**，只是按不动并说明为什么——凭空消失会被读成
   * 「这个功能坏了」。
   */
  | { kind: "stop"; runRecordId: string; refusal: string | null }
  /**
   * 目标表占用还在（#271）：还在释放，或者释放失败了。
   *
   * 停止是异步的：接口发出信号就返回，占用要等子进程退出、父进程补发 abort 之后才真的
   * 释放。这段窗口里既不能显示「停止运行」（已经停过了），更不能显示「发起运行」——
   * 那句话是假的，点下去只换回一个 `TARGET_TABLE_BUSY`。释放失败之后那一档**可点**，
   * 点下去重发一次 abort：它是这套界面上唯一的手工补救入口。
   *
   * 两档的分别、按钮上的字、旁边那句原因，全在 `targetHoldState` 一处。
   */
  | { kind: "hold"; hold: HeldTargetHold };

export function rowRunAction(
  run: RunHistory | undefined,
  startBusy: boolean,
): RowRunAction {
  // 占用的处境**压过其余一切**：只要那张目标表还被占着，这一格就不许出现
  // 「发起运行」。这条判断放在最前面，是因为它要压住的正是后面那两条——
  // 在飞时压住「停止运行」（已经停过了），终局时压住「发起运行」（那句话是假的）。
  const hold = targetHoldState(run);
  if (hold.kind !== "free") {
    return { kind: "hold", hold };
  }
  if (run !== undefined && historyPresentation(run).kind === "live") {
    return {
      kind: "stop",
      runRecordId: run.run_record_id,
      refusal: abortRefusal(run.stage),
    };
  }
  return { kind: "start", disabled: startBusy };
}

/**
 * 一次失败之后**下一步该去哪儿**。
 *
 * 三种去处，不是两种：
 *
 * - `wizard` — 任务定义里有东西要改，带上该落在哪一步。
 * - `datasources` — 连不上库、配置不对。向导里没有一个字能改它，把人送进向导
 *   等于让他在四步里找一样不在那儿的东西。
 * - `none` — 没有可直接修改的地方，**但要说出为什么**。这一档原来是 `null`：
 *   十六个分类里有八个落在这里，界面上什么都不出，于是「这一类改不了」这句
 *   最该说的话反而是唯一没说的（UX 评审 P1-11）。
 *
 * 分类闭集见 `crates/source/src/failure_kind.rs`。**每一个值都在下面这张表里**，
 * 包括已退役的 `SHAPE_PRECHECK`——闭集只增不删，读到旧记录时它照样得有个去处。
 */
export type Remediation =
  | { kind: "wizard"; step: Step; label: string }
  | { kind: "datasources"; label: string }
  | { kind: "none"; reason: string };

export function remediationFor(run: RunHistory): Remediation | null {
  const kind = historyPresentation(run).kind;
  // 结局不明也要给一句：那一屏的核对线索（P0-4）正是从这里指过去的。
  if (kind !== "failed" && kind !== "unknown") {
    return null;
  }
  if (kind === "unknown") {
    return {
      kind: "none",
      reason: "结局不明：先按上面的核对线索到目标库确认一遍，再决定要不要重跑。写入是按主键幂等的。",
    };
  }
  switch (run.failure_kind) {
    case "CONFIG":
      return { kind: "datasources", label: "检查数据源配置" };
    case "SOURCE_CONNECT":
    case "SOURCE_DBLINK":
      return { kind: "datasources", label: "检查源端数据源" };

    case "SHAPE_PRECHECK":
    case "MAPPING_PRECHECK":
      // 字段映射是**第 1 步**。这里原来写的是 3，于是「去改映射」把人送到目标表检查。
      return { kind: "wizard", step: 1, label: "修改字段映射" };
    case "SOURCE_VALUE":
      return { kind: "wizard", step: 1, label: "调整这一列的映射" };
    case "SOURCE_QUERY":
      return { kind: "wizard", step: 2, label: "修改取数与过滤条件" };
    case "SINK_WRITE":
    case "DATA_REJECTED":
    case "VERIFY_FAILED":
      return { kind: "wizard", step: 3, label: "核对目标表" };

    case "NETWORK":
      return {
        kind: "none",
        reason: "与目标端的连接中断了，任务定义没有问题。确认目标端 Agent 在线后重跑。",
      };
    case "SINK_ENVIRONMENT":
      return {
        kind: "none",
        reason: "目标端环境配置的问题（例如 max_allowed_packet），要在目标库那边改，界面上改不了。",
      };
    case "TARGET_BUSY":
      return {
        kind: "none",
        reason: "目标表正被另一次运行的切换事务占着。等那一次结束再重跑。",
      };
    case "ORCHESTRATOR":
      return {
        kind: "none",
        reason: "这次是父进程没能把运行拉起来，任务定义没有问题。重跑一次；仍失败就要看服务端日志。",
      };
    case "DEFECT":
      return {
        kind: "none",
        reason: "这是程序缺陷，不是配置问题。请带上上面的运行记录号提一个 issue。",
      };
    default:
      // 分类闭集只增不删，但**新增的值这一份可能还不认识**——那时也不能沉默。
      return {
        kind: "none",
        reason: "这一类失败没有可以直接修改的地方，请看上面的运行证据判断下一步。",
      };
  }
}
