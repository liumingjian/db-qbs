import { describe, expect, it } from "vitest";

import {
  BOTTOM_EPSILON_PX,
  INITIAL_FOLLOW,
  backToLatestLabel,
  follow,
  showsBackToLatest,
} from "./logFollow";
import type { FollowEvent, FollowState } from "./logFollow";

function run(events: readonly FollowEvent[], from: FollowState = INITIAL_FOLLOW): FollowState {
  return events.reduce(follow, from);
}

describe("跟随状态机（#263）", () => {
  const cases: ReadonlyArray<{
    name: string;
    events: readonly FollowEvent[];
    expected: FollowState;
  }> = [
    {
      name: "开局跟随",
      events: [],
      expected: { following: true, unseen: 0 },
    },
    {
      name: "跟随时新行不计数",
      events: [{ type: "appended", count: 5 }],
      expected: { following: true, unseen: 0 },
    },
    {
      name: "手动上滚立刻停下跟随",
      events: [{ type: "scrolled", distanceFromBottom: 400 }],
      expected: { following: false, unseen: 0 },
    },
    {
      name: "停下之后新到的行被数着",
      events: [
        { type: "scrolled", distanceFromBottom: 400 },
        { type: "appended", count: 3 },
        { type: "appended", count: 2 },
      ],
      expected: { following: false, unseen: 5 },
    },
    {
      name: "「回到最新」一步回到实时并清空计数",
      events: [
        { type: "scrolled", distanceFromBottom: 400 },
        { type: "appended", count: 7 },
        { type: "back-to-latest" },
      ],
      expected: { following: true, unseen: 0 },
    },
    {
      name: "自己滚回底部等同于回到最新",
      events: [
        { type: "scrolled", distanceFromBottom: 400 },
        { type: "appended", count: 7 },
        { type: "scrolled", distanceFromBottom: 0 },
      ],
      expected: { following: true, unseen: 0 },
    },
    {
      name: "贴底的零头不算离开底部",
      events: [{ type: "scrolled", distanceFromBottom: BOTTOM_EPSILON_PX }],
      expected: { following: true, unseen: 0 },
    },
    {
      name: "超出零头一像素就算离开",
      events: [{ type: "scrolled", distanceFromBottom: BOTTOM_EPSILON_PX + 1 }],
      expected: { following: false, unseen: 0 },
    },
    {
      name: "暂停期间再次上滚不清空已积压的条数",
      events: [
        { type: "scrolled", distanceFromBottom: 400 },
        { type: "appended", count: 4 },
        { type: "scrolled", distanceFromBottom: 800 },
      ],
      expected: { following: false, unseen: 4 },
    },
    {
      name: "空的一页不算新行",
      events: [
        { type: "scrolled", distanceFromBottom: 400 },
        { type: "appended", count: 0 },
      ],
      expected: { following: false, unseen: 0 },
    },
  ];

  for (const item of cases) {
    it(item.name, () => {
      expect(run(item.events)).toEqual(item.expected);
    });
  }

  it("状态没变时返回同一个对象，免得面板白重画一次", () => {
    const paused = run([{ type: "scrolled", distanceFromBottom: 400 }]);
    expect(follow(paused, { type: "scrolled", distanceFromBottom: 500 })).toBe(paused);
    expect(follow(INITIAL_FOLLOW, { type: "appended", count: 3 })).toBe(INITIAL_FOLLOW);
    expect(follow(INITIAL_FOLLOW, { type: "back-to-latest" })).toBe(INITIAL_FOLLOW);
  });

  it("「回到最新」只在暂停时出现，并把积压条数说出来", () => {
    expect(showsBackToLatest(INITIAL_FOLLOW)).toBe(false);
    const paused = run([
      { type: "scrolled", distanceFromBottom: 400 },
      { type: "appended", count: 12 },
    ]);
    expect(showsBackToLatest(paused)).toBe(true);
    expect(backToLatestLabel(paused)).toBe("回到最新（12 条新日志）");
    expect(backToLatestLabel({ following: false, unseen: 0 })).toBe("回到最新");
  });
});
