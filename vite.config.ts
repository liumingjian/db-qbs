import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

import { version } from "./package.json" with { type: "json" };

export default defineConfig({
  root: "web",
  // 登录页左半屏那个版本号。**构建期注入，不走接口**：让一个还没通过认证的人
  // 问出构建细节，等于白送一份指纹；而版本号本身在装机和排障时第一眼就想知道。
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
