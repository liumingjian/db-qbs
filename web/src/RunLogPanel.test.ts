import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { RunLogPanel } from "./RunLogPanel";

// 面板本身没有可测的判断——翻译在 `runLogLine.ts`，跟不跟随在 `logFollow.ts`，
// 两处都有自己的表格化用例。这里只钉住外壳：首屏自陈正在读，而不是装作没有日志。
describe("RunLogPanel", () => {
  it("首屏自陈正在读取，不假装这次运行没写过日志", () => {
    const html = renderToStaticMarkup(
      createElement(RunLogPanel, { runRecordId: "record-1" }),
    );

    expect(html).toContain("运行日志");
    expect(html).toContain("正在读取日志");
    // 跟随中不该有「回到最新」——它没有活可干。
    expect(html).not.toContain("回到最新");
  });

  // 抽屉里那一份只换外壳（`.panel` + `<h3>`，整屏那边是 `.card` + `<h2>`）：
  // 两处看到的必须是同一份日志，否则「详情里有日志」这句话就得分两种说法。
  it("摆进抽屉时换外壳，日志本身一个字不变", () => {
    const embedded = renderToStaticMarkup(
      createElement(RunLogPanel, { runRecordId: "record-1", embedded: true }),
    );

    expect(embedded).toContain('class="panel run-logs is-embedded"');
    expect(embedded).toContain("<h3>");
    expect(embedded).not.toContain("card-header");
    expect(embedded).toContain("运行日志");
    expect(embedded).toContain("正在读取日志");
  });
});
