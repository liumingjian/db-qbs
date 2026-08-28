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
});
