import type { RunHistory } from "./api";
import { runStatus } from "./listing";
import { runPhase } from "./runStage";

/**
 * 作业中心「迁移进度」那一格的**判定**（ADR-0043 §7）。
 *
 * 摆在组件外面的理由与 `listing.ts` 同源：这几条是规则不是渲染，而且**错了看不出来**——
 * 99.98% 被显示成 `100%`、没跑过的行显示成 `0%`、计数失败的行显示成 `0%`，
 * 三样在屏幕上都长得挺正常，只有用例守得住。
 *
 * 四条边界，别在渲染里悄悄挪：
 *
 * 1. **向下取整，不四舍五入。** `100%` 只在真跑完时出现——99.98% 显示成 100% 等于拿显示撒谎。
 * 2. **不带小数、不附行数。** 小数点撑歪这一列的对齐，第二位小数对「跑到哪了」零信息量。
 * 3. **「尚未运行」是 `—`，不是 `0%`。** 0% 是「跑了但一行没动」，与「没跑过」不是一回事。
 * 4. **计数失败也是 `—`**，且自陈「未取到总行数」——但那次运行**照常跑**，
 *    状态列该是什么是什么。为了一个进度条把整次搬运判死，是拿主功能换装饰。
 */
export type ProgressCell =
  /** 这个任务一条运行记录都没有。 */
  | { kind: "none"; label: string; title: string }
  /** 跑了，但开跑前那次 `COUNT(*)` 没成功——分母缺席，进度无从算起。 */
  | { kind: "unknown"; label: string; title: string }
  /** 有分母，给一个整数百分比。 */
  | {
      kind: "value";
      percent: number;
      label: string;
      title: string;
      /** 进度条的着色：成功绿、失败红、其余主色。**颜色不承担语义**，语义在运行状态列。 */
      tone: "ok" | "bad" | "live";
    };

const EMPTY_LABEL = "—";
const countFormatter = new Intl.NumberFormat("zh-CN");
type ProgressTone = "ok" | "bad" | "live";

/**
 * `run` 为 `undefined` = 这个任务尚未运行。
 *
 * 分母取 `total_rows`（开跑前 `COUNT(*)`），分子取 `rows_pushed`（本来就有的已推送行数）。
 * `total_rows <= 0` 当作跑完（`100%`）：空表没有「还剩多少」这回事，把它算成 `0%` 会让
 * 一条真的搬完了的空表运行永远停在零。
 */
export function progressOf(run: RunHistory | undefined): ProgressCell {
  if (run === undefined) {
    return {
      kind: "none",
      label: EMPTY_LABEL,
      title: "尚未运行——这个任务还没有任何运行记录。",
    };
  }
  const status = runStatus(run);
  return progressFromCounts(
    run,
    status === "succeeded" ? "ok" : status === "failed" ? "bad" : "live",
  );
}

export function progressOfLiveRun(run: {
  total_rows: number | null;
  rows_pushed: number;
  stage: string | null;
}): ProgressCell {
  return progressFromCounts(run, "live");
}

function progressFromCounts(
  run: { total_rows: number | null; rows_pushed: number; stage: string | null },
  tone: ProgressTone,
): ProgressCell {
  const total = run.total_rows;
  if (total === null) {
    // **空分母有两种**（UX 评审 P1-8）。开跑前计数还没跑完时它当然是空的——
    // 那是这次运行最开始的几十秒，一个五十万行的表要数上小半分钟。原来这两种
    // 都写「计数没成功」，于是每一次运行的开头都自称出了一次故障。
    return {
      kind: "unknown",
      label: EMPTY_LABEL,
      // 阶段的拼写归 `runStage.ts` 管，这里不再自己写一遍 "PREPARING"。
      // 形参仍是 `string`：不认识的拼写要原样传下去（见 `stageLabel` 的理由），
      // 而 `runPhase` 对它返回 null——落到「计数没成功」那一档，与改之前一致。
      title:
        run.stage === null || runPhase(run.stage) === "PREPARING"
          ? "正在开跑前计数——总行数还没数出来，进度要等它。"
          : "未取到总行数——开跑前的计数没成功，这次运行本身不受影响。",
    };
  }
  const done = run.rows_pushed;
  const percent =
    total <= 0 ? 100 : Math.min(100, Math.floor((done / total) * 100));
  return {
    kind: "value",
    percent,
    label: `${percent}%`,
    // 行数不进这一格的**显示**，但进 `title`：列表要窄，不等于把已知事实藏起来。
    title:
      total <= 0
        ? "源端总行数 0——没有要搬的行。"
        : `已推送 ${countFormatter.format(done)} / 开跑前计数 ${countFormatter.format(total)} 行（向下取整）。`,
    tone,
  };
}
