import react from "@vitejs/plugin-react";
import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";

import { version } from "./package.json" with { type: "json" };
import { mockApi } from "./mock/api";

export default defineConfig(({ mode }) => {
  // 假后端的开关。**只影响 dev server**：`VITE_MOCK=1 npm run dev`（或 `--mode mock`）
  // 时 `/api/*` 由 `mock/api.ts` 应答，不开就一个字节也不挂，行为与从前一致。
  const env = loadEnv(mode, ".", "");
  const mock = env.VITE_MOCK === "1" || mode === "mock";

  return {
    root: "web",
    // 登录页左半屏那个版本号。**构建期注入，不走接口**：让一个还没通过认证的人
    // 问出构建细节，等于白送一份指纹；而版本号本身在装机和排障时第一眼就想知道。
    define: { __APP_VERSION__: JSON.stringify(version) },
    plugins: [react(), mockApi(mock)],
    build: {
      outDir: "dist",
      emptyOutDir: true,
    },
    test: {
      environment: "node",
      include: ["src/**/*.test.ts"],
    },
  };
});
