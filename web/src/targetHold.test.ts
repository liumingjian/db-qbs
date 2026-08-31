import { describe, expect, it } from "vitest";

import type { RunHistory } from "./api";
import { targetHoldState } from "./targetHold";

function run(overrides: Partial<RunHistory> = {}): RunHistory {
  return {
    run_record_id: "record-7",
    target_hold: null,
    target_hold_message: null,
    ...overrides,
  } as RunHistory;
}

// #271：「占用还在没有」这句话在前端只算一遍。作业中心那一格、重跑那一颗、
// 运行详情那一屏读的都是这里，所以这里说错，三处一起说错——反过来也一样。
describe("target hold state", () => {
  it("is free when nothing is holding the table", () => {
    expect(targetHoldState(run())).toEqual({ kind: "free" });
  });

  it("is free when the row predates the field", () => {
    // 旧服务端不发这两栏。那时唯一诚实的回答是「不知道有占用」——
    // 真的占用照旧被服务端那一关拦住。
    expect(targetHoldState(run({ target_hold: undefined }))).toEqual({ kind: "free" });
  });

  it("is free when there is no run at all", () => {
    expect(targetHoldState(undefined)).toEqual({ kind: "free" });
    expect(targetHoldState(null)).toEqual({ kind: "free" });
  });

  it("says 停止中… while the hold is still being released", () => {
    const hold = targetHoldState(run({ target_hold: "RELEASING" }));
    expect(hold.kind).toBe("releasing");
    expect(hold).toMatchObject({ label: "停止中…", runRecordId: "record-7" });
  });

  it("says 锁未释放，点此重试 and quotes the target end verbatim", () => {
    const hold = targetHoldState(
      run({ target_hold: "HELD", target_hold_message: "暂存表 drop 不掉" }),
    );
    expect(hold).toMatchObject({
      kind: "held",
      label: "锁未释放，点此重试",
      detail: "暂存表 drop 不掉",
      runRecordId: "record-7",
    });
  });

  it("still says why when the target end said nothing", () => {
    // 原因缺席不等于没有原因：那一格仍旧要说得出自己为什么按不动。
    expect(targetHoldState(run({ target_hold: "HELD" }))).toMatchObject({
      detail: "目标表占用没能释放",
    });
  });
});
