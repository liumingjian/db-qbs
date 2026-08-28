import { describe, expect, it } from "vitest";

import {
  runHash,
  runLogsHash,
  runLogsRequestedFromHash,
  runRecordFromHash,
} from "./routes";

describe("运行地址", () => {
  const cases: ReadonlyArray<{
    hash: string;
    record: string | null;
    logs: boolean;
  }> = [
    { hash: "#runs/r-1", record: "r-1", logs: false },
    { hash: "#runs/r-1/logs", record: "r-1", logs: true },
    { hash: "#jobs", record: null, logs: false },
    { hash: "", record: null, logs: false },
    { hash: "#runs/", record: null, logs: false },
    { hash: "#runs/a%2Fb", record: "a/b", logs: false },
    { hash: "#runs/a%2Fb/logs", record: "a/b", logs: true },
  ];

  for (const item of cases) {
    it(`${item.hash === "" ? "（空地址）" : item.hash} 解析成 ${item.record ?? "null"}`, () => {
      expect(runRecordFromHash(item.hash)).toBe(item.record);
      expect(runLogsRequestedFromHash(item.hash)).toBe(item.logs);
    });
  }

  it("拼出来的地址解析回同一个 run_record_id", () => {
    for (const id of ["r-1", "a/b", "运行 1", "x?y=1"]) {
      expect(runRecordFromHash(runHash(id))).toBe(id);
      expect(runRecordFromHash(runLogsHash(id))).toBe(id);
      expect(runLogsRequestedFromHash(runHash(id))).toBe(false);
      expect(runLogsRequestedFromHash(runLogsHash(id))).toBe(true);
    }
  });
});
