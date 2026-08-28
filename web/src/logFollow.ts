/**
 * 日志面板「跟不跟着往下滚」的状态机。**纯逻辑，不碰 DOM**。
 *
 * 它存在的理由是那条最容易被写错的规矩：**人一往上滚，自动跟随就得停**。
 * 一个还在往下拽的面板会把人正在读的那一行拖出屏幕，而那一行往往正是出事的那一行。
 * 停下之后要有一步回得去（「回到最新」），否则等于把人锁在过去。
 *
 * 面板只把两件事喂进来——滚动条离底还有多远、新到了几行——然后照 `following` 办事。
 * 判断全在这里，于是它能被表格化地测，而不必去起一个浏览器。
 */

/**
 * 离底多少像素之内算「贴着底」。
 *
 * 不是 0：自动滚到底之后，浏览器的四舍五入、行高变化、缩放都能留下一两个像素的零头，
 * 按 0 判会让面板在「跟随」和「暂停」之间自己抖起来。
 */
export const BOTTOM_EPSILON_PX = 24;

export interface FollowState {
  /** 有新行时是否自动滚到底。 */
  following: boolean;
  /** 暂停期间新到的行数，用来告诉人「下面还压着多少」。跟随时恒为 0。 */
  unseen: number;
}

export type FollowEvent =
  /** 面板滚动了一下——人手动滚的和自动滚到底的走同一个入口。 */
  | { type: "scrolled"; distanceFromBottom: number }
  /** 轮询取回了 `count` 行新日志。 */
  | { type: "appended"; count: number }
  /** 人点了「回到最新」。 */
  | { type: "back-to-latest" };

/** 开局就是跟随：进来第一眼要看到的是最新那几行，不是这次运行的第一行。 */
export const INITIAL_FOLLOW: FollowState = { following: true, unseen: 0 };

export function follow(state: FollowState, event: FollowEvent): FollowState {
  switch (event.type) {
    case "scrolled": {
      // 自动滚到底同样会触发一次滚动事件，但它落在底部，于是判定成「继续跟随」——
      // 所以这里不需要区分「谁滚的」，也就不需要一个说不清什么时候该清掉的标志位。
      const atBottom = event.distanceFromBottom <= BOTTOM_EPSILON_PX;
      if (!atBottom) {
        return state.following ? { following: false, unseen: 0 } : state;
      }
      // 人自己滚回底部，等同于点了「回到最新」：他要的就是最新。
      return state.following && state.unseen === 0 ? state : { following: true, unseen: 0 };
    }

    case "appended":
      if (state.following || event.count <= 0) {
        return state;
      }
      return { following: false, unseen: state.unseen + event.count };

    case "back-to-latest":
      return state.following && state.unseen === 0 ? state : INITIAL_FOLLOW;
  }
}

/** 「回到最新」这颗按钮该不该出现。跟随中就不该有——它没有活可干。 */
export function showsBackToLatest(state: FollowState): boolean {
  return !state.following;
}

/** 「回到最新」上的字：压着新行时把条数说出来，没压着就只说去处。 */
export function backToLatestLabel(state: FollowState): string {
  return state.unseen === 0 ? "回到最新" : `回到最新（${state.unseen} 条新日志）`;
}
