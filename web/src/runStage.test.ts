import { describe, expect, it } from "vitest";

import {
  RUN_PHASES,
  abortAllowed,
  abortRefusal,
  runPhase,
  stageLabel,
} from "./runStage";

describe("the stage vocabulary", () => {
  it("draws the phase line as the three the run is doing something in", () => {
    expect(RUN_PHASES).toEqual(["PREPARING", "STREAMING", "COMMITTING"]);
  });

  it("names every stage the run reports", () => {
    expect(stageLabel("PREPARING")).toBe("准备中");
    expect(stageLabel("STREAMING")).toBe("传输中");
    expect(stageLabel("COMMITTING")).toBe("提交中");
    expect(stageLabel("SUCCEEDED")).toBe("已完成");
    expect(stageLabel("FAILED")).toBe("已失败");
  });

  it("has no name for a run that has not reported a stage", () => {
    expect(stageLabel(null)).toBeNull();
  });

  it("shows a spelling it does not recognise exactly as it arrived", () => {
    // Same rule as failureKindLabel, same reason: an unknown value means the
    // two ends are on different versions, and that is the moment you want it
    // on screen rather than smoothed away.
    expect(stageLabel("STREAMIN")).toBe("STREAMIN");
    expect(stageLabel("streaming")).toBe("streaming");
    expect(stageLabel("RESUMING")).toBe("RESUMING");
  });

  it("keeps the two terminal stages off the phase line", () => {
    expect(runPhase("STREAMING")).toBe("STREAMING");
    expect(runPhase("SUCCEEDED")).toBeNull();
    expect(runPhase("FAILED")).toBeNull();
    expect(runPhase(null)).toBeNull();
    expect(runPhase("STREAMIN")).toBeNull();
  });
});

describe("whether the run can still be stopped", () => {
  it("allows it up to the commit point and never after", () => {
    // CONTEXT.md, Abort: once COMMITTING is entered the staging table's
    // disposition has passed wholly to sink, and source permanently forfeits
    // the right to abort. Same rule as RunStage::abort_allowed.
    expect(abortAllowed("PREPARING")).toBe(true);
    expect(abortAllowed("STREAMING")).toBe(true);
    expect(abortAllowed("COMMITTING")).toBe(false);
    expect(abortAllowed("SUCCEEDED")).toBe(false);
    expect(abortAllowed("FAILED")).toBe(false);
  });

  it("refuses while the run is accepted but not yet under way", () => {
    // The server answers 409 here too. A lit button would be a promise the
    // next click breaks.
    expect(abortAllowed(null)).toBe(false);
    expect(abortRefusal(null)).toBe("运行还没拉起来，暂时停不了");
  });

  it("refuses a spelling it cannot place, rather than guessing", () => {
    expect(abortAllowed("STREAMIN")).toBe(false);
    expect(abortRefusal("STREAMIN")).toBe("运行还没拉起来，暂时停不了");
  });

  it("gives no reason while the button is live", () => {
    expect(abortRefusal("STREAMING")).toBeNull();
  });

  it("separates losing the permission from losing the process", () => {
    expect(abortRefusal("COMMITTING")).toBe(
      "已过封口点：暂存表的处置权已经交给目标端",
    );
    expect(abortRefusal("SUCCEEDED")).toBe("运行已经结束，没有可停的进程");
    expect(abortRefusal("FAILED")).toBe("运行已经结束，没有可停的进程");
  });
});
