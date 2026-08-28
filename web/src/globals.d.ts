/** 构建期由 `vite.config.ts` 注入的版本号，见那里的 `define`。 */
declare const __APP_VERSION__: string;

/**
 * `?raw` 导入：把一个文件当字符串读进来（Vite 内建）。
 *
 * 用在**跨语言的常量比对**上——那句预检结论在 `precheck.rs` 与 `writeMode.ts` 各有
 * 一份，用例把 Rust 那份原文读进来直接比（见 `writeMode.test.ts`）。
 */
declare module "*?raw" {
  const content: string;
  export default content;
}
