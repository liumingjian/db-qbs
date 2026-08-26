import { describe, expect, it } from "vitest";

import {
  emptyPasswordForm,
  gateReasonVisible,
  loginGate,
  passwordGate,
} from "./session";

describe("loginGate", () => {
  it("两栏都填了才放行", () => {
    expect(loginGate({ username: "", password: "" })).toEqual({
      kind: "blocked",
      reason: "请输入账号",
    });
    expect(loginGate({ username: "admin", password: "" })).toEqual({
      kind: "blocked",
      reason: "请输入口令",
    });
    expect(loginGate({ username: "admin", password: "admin" })).toEqual({
      kind: "ready",
    });
  });

  it("账号只去首尾空白，口令一个字节都不动", () => {
    expect(loginGate({ username: "  ", password: "x" }).kind).toBe("blocked");
    // 口令里的空格是口令的一部分——`trim` 过的口令是另一个口令。
    expect(loginGate({ username: "admin", password: "   " }).kind).toBe("ready");
  });
});

describe("passwordGate", () => {
  it("一次只说一件事，且说的是接下来真要动的那一件", () => {
    // 三栏全空时先问当前口令，而不是先抱怨两次不一致。
    expect(passwordGate(emptyPasswordForm())).toEqual({
      kind: "blocked",
      reason: "请输入当前口令",
    });
    expect(
      passwordGate({ current: "admin", next: "", confirm: "" }),
    ).toEqual({ kind: "blocked", reason: "请输入新口令" });
    expect(
      passwordGate({ current: "admin", next: "新的", confirm: "别的" }),
    ).toEqual({ kind: "blocked", reason: "两次输入的新口令不一致" });
  });

  it("新口令与当前口令相同要拦下来", () => {
    // 它不会报错，但它是一次什么也没发生的改密，而用户会以为自己改过了。
    expect(
      passwordGate({ current: "admin", next: "admin", confirm: "admin" }),
    ).toEqual({ kind: "blocked", reason: "新口令与当前口令相同" });
  });

  it("填齐且一致就放行，不判强度", () => {
    // 出厂口令就是 `admin` 且长期有效，在它之上立强度规矩拦不住任何人。
    expect(
      passwordGate({ current: "admin", next: "a", confirm: "a" }),
    ).toEqual({ kind: "ready" });
  });
});

describe("gateReasonVisible", () => {
  it("三栏都动过之后才把理由摆出来", () => {
    // 一栏还空着就报「请输入当前口令」，是在骂一个还没开始打字的人。
    expect(gateReasonVisible(emptyPasswordForm())).toBe(false);
    expect(
      gateReasonVisible({ current: "admin", next: "新的", confirm: "" }),
    ).toBe(false);
    expect(
      gateReasonVisible({ current: "admin", next: "新的", confirm: "别的" }),
    ).toBe(true);
  });
});
