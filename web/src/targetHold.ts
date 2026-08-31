import type { RunDetail, RunHistory } from "./api";

/**
 * 一条运行记录读得出的**目标表占用处境**（服务端 `TargetHold`，#271）。
 *
 * 服务端把判据收在 `RunState::target_hold` 一处，界面这一半就收在这里：
 * 「占用还在没有」这句话在前端只算一遍，作业中心那一格、重跑那一颗、运行详情那一屏
 * 读的都是这个值。三处各自去看 `target_hold === "RELEASING"` 的话，就是同一条规则
 * 有三份实现，而它们只要有一份说漏，界面就会在占用仍在时说「可以重跑」——
 * 那正是 #271 要禁掉的那句假话。
 *
 * 措辞也一并住在这里：按钮上那句、旁边那句原因、重跑被禁时那句解释，
 * 三样都随这个值走，不在屏上各写一遍。
 */
export type TargetHoldState =
  | { kind: "free" }
  | {
      /** 停止已经发出，占用还在还回来的路上（含重试正在路上）。按不动，等着。 */
      kind: "releasing";
      runRecordId: string;
      label: string;
      detail: string;
      rerunRefusal: string;
    }
  | {
      /** abort 失败，占用留在目标端。可点，点下去是重发一次 abort。 */
      kind: "held";
      runRecordId: string;
      label: string;
      detail: string;
      rerunRefusal: string;
    };

/** 占用**还在**的那两档。`free` 之外只有这些，屏上就照着它渲染。 */
export type HeldTargetHold = Exclude<TargetHoldState, { kind: "free" }>;

/**
 * 这条运行写的那张目标表，此刻还被占着没有。
 *
 * 拿不到这一行（作业中心里从未运行过的任务）就是 `free`：没有运行，也就没有占用。
 * 字段缺席（旧服务端不发这两栏）同样是 `free`——那时唯一诚实的回答是「不知道有占用」，
 * 而服务端那一关照旧拦得住真的占用。
 */
export function targetHoldState(
  run: RunHistory | RunDetail | undefined | null,
): TargetHoldState {
  if (run === undefined || run === null) {
    return { kind: "free" };
  }
  const runRecordId = run.run_record_id;
  if (run.target_hold === "RELEASING") {
    return {
      kind: "releasing",
      runRecordId,
      label: "停止中…",
      detail: "目标表占用尚未释放",
      rerunRefusal: "已发出停止，目标表占用还没释放。等它释放后再重跑。",
    };
  }
  if (run.target_hold === "HELD") {
    return {
      kind: "held",
      runRecordId,
      label: "锁未释放，点此重试",
      // 目标端的原话优先：它才说得出到底卡在哪儿。
      detail: run.target_hold_message ?? "目标表占用没能释放",
      rerunRefusal:
        "上一次的目标表占用没能释放，这时候重跑会被目标端拒掉。先点「锁未释放，点此重试」，释放成功后再重跑。",
    };
  }
  return { kind: "free" };
}
