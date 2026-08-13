# ADR-0001: 用 Rust 实现，同步 IO，Oracle 驱动待 spike 确认

**状态**: 已接受（Oracle 驱动选型待 M0 spike 结论）
**日期**: 2026-08-13

## 背景

项目后续完全交由 LLM 开发。技术选型的首要标准不是开发速度，而是**可验证、可排查**：
LLM 最危险的失败模式是产出「看起来正确、运行时才错」的代码，而本项目的核心风险
（Oracle → MySQL 类型映射、金额精度）恰好属于这一类，且错误直接落在生产数据上。

## 决策

**语言选 Rust。**

主要理由是编译期能拦住本项目最要命的一类错误：类型映射写成 `enum` + 穷尽 `match`，
漏一个 Oracle 类型变体直接编译失败。`cargo check` / `cargo test` 的输出精确且结构化，
作为 agent 迭代的反馈信号，质量高于运行时堆栈。附带收益：单二进制部署、无 GC、
内存占用小，适合内网机器。

**不使用 async，采用同步阻塞 IO + 线程池。**

单任务上限 10 万行 / ~100MB，一次运行量级在几十秒，同步 IO 完全够用。
async Rust + 流式数据库读写是 LLM 最容易卡死的组合（lifetime、`Send` 约束）。
需要并发时用 `std::thread` + channel。

**Oracle 驱动在 M0 spike 后确定**，候选：

| 候选 | 机制 | 代价 |
|---|---|---|
| `oracle` (kubo/rust-oracle) | 封装 ODPI-C → 官方 OCI | 源端机器须装 Oracle Instant Client；FFI 层排查体验差 |
| `oracle-rs` (stiang) | 纯 Rust TNS 协议，免客户端 | v0.1，社区体量小，成熟度未验证 |

MySQL 侧无此风险，Rust 生态成熟。

## 已知代价

1. **Oracle 驱动是本决策的真实成本。** Java 的 JDBC thin driver 是纯 Java、官方出品、
   二十年生产验证，装都不用装——Rust 生态给不了同等物。M0 spike 若证明两个候选驱动
   都无法保真处理生产表的字段类型，**应当重新审视本 ADR**，而不是硬扛。
2. **编译时间拖慢 LLM 迭代循环。** `cargo check` 数十秒 vs Go 数秒，高频试错下会累积。
3. **编译吃内存。** 开发用的服务器内存紧张，编译须派发到外部机器执行。

## 备选方案

- **Java (Spring Boot)**：Oracle 支持最好，JDBC thin 无部署依赖，类型保真度最高。
  代价是内存占用大、可验证性弱于 Rust。**这是本决策失败时的首选回退。**
- **Go**：部署最省事，`go-ora` 纯 Go 免客户端。编译快、对 LLM 友好，但类型系统拦不住
  映射遗漏（无穷尽性检查），且 `go-ora` 在 NUMBER 精度、LOB 等边界上的保真度同样需要验证。
- **Python (FastAPI)**：`oracledb` thin 模式官方支持、免客户端，开发最快。
  但运行时才暴露类型错误，与本项目的首要标准直接冲突。
