# ADR-0001: 用 Rust 实现，同步 IO，Oracle 驱动用 `oracle` (ODPI-C)

**状态**: 已接受
**日期**: 2026-08-13（驱动选型于 2026-08-13 由 M0 spike 定案，见下）

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

**Oracle 驱动选 [`oracle`](https://crates.io/crates/oracle) crate（kubo/rust-oracle，封装 ODPI-C → 官方 OCI）。**
源端机器须部署 Oracle Instant Client **19c 完整 Basic 包**（不是 21c——21c 连不了 11g；
不是 Basic Light——Light 不带 `ZHS16GBK`）。

原本的另一个候选 `oracle-rs`（纯 Rust TNS，免客户端）**出局**：它要求服务端 ≥ 12.1，
客户源端是 **11g**。这不是成熟度问题，是协议版本对不上，补不出来。

MySQL 侧无此风险，Rust 生态成熟。

### 驱动选型的依据（M0 spike，[#1](https://github.com/liumingjian/db-qbs/issues/1) / [#8](https://github.com/liumingjian/db-qbs/issues/8)）

完整报告与实测数据见 [`docs/spikes/0001-oracle-driver.md`](../spikes/0001-oracle-driver.md)。
候选 B 出局后 M0 塌缩成「ODPI-C 单点验证，没有备胎」，四条判据全部成立：

| 判据 | 结论 |
|---|---|
| 类型保真度（台架覆盖的全部白名单类型） | `PASS 20 / FAIL 0`（报告 §2.2） |
| `NUMBER` 以字符串取到完整精度 —— **ADR-0003 的硬前提** | 38 位原样；同值走 `f64` 的丢精度对照留档（§2.1） |
| 流式 fetch 的内存形状 | 峰值与总行数**无关**（行数 ×10 一字不差），驱动没有内部缓冲全量结果集（§4.2） |
| 客户端每行处理开销 | 1,106 ns/行且随行数线性；外推 70 列 × 10 万行 ≈ **1.5 秒**客户端 CPU（§4.7） |
| 源端可部署 Instant Client | 可行，免 root，离线带入（§6） |

**下面「同步阻塞 IO」那条的前提只被证实了一半**：内存与**客户端**实现开销已实测（远未触及
「几十秒」的预算），**服务端吞吐的绝对秒数在本地台架上不可测**（服务端跑在 Rosetta 上），
已按 [ADR-0005](0005-local-rig-as-v1-verification-baseline.md) 转
[#2 的上线前复验清单](https://github.com/liumingjian/db-qbs/issues/2)第 4 项。
**那一项若实测远超「几十秒」，本 ADR 必须复审** —— 见下面「已知代价」第 1 条。

## 已知代价

1. **Oracle 驱动是本决策的真实成本。** Java 的 JDBC thin driver 是纯 Java、官方出品、
   二十年生产验证，装都不用装——Rust 生态给不了同等物。我们买到的替代品要付两笔：
   源端机器必须部署 Instant Client（80 MB 离线包 + 一次审批往返），
   且 11g 让免客户端的纯 Rust 路线彻底出局，**没有备胎**。
   M0 已验掉「保真度」这一半风险，**回退触发条件仍然有效**：
   #2 复验清单第 4 项（真实吞吐）远超「几十秒」，或第 3 项 GBK 往返失败且非客户端配置可解，
   或白名单外类型大面积命中——任一条成立就**重开本 ADR**，回退 Java，不要硬扛。
2. **编译时间拖慢 LLM 迭代循环。** `cargo check` 数十秒 vs Go 数秒，高频试错下会累积。
3. **编译吃内存。** 开发用的服务器内存紧张，编译须派发到外部机器执行。

## 备选方案

- **Java (Spring Boot)**：Oracle 支持最好，JDBC thin 无部署依赖，类型保真度最高。
  代价是内存占用大、可验证性弱于 Rust。**这是本决策失败时的首选回退，M0 通过后依然是**——
  回退路径本身是干净的（JDBC thin 对 11g 无版本门槛、无客户端安装）。
- **Go**：部署最省事，`go-ora` 纯 Go 免客户端。编译快、对 LLM 友好，但类型系统拦不住
  映射遗漏（无穷尽性检查），且 `go-ora` 在 NUMBER 精度、LOB 等边界上的保真度同样需要验证。
- **Python (FastAPI)**：`oracledb` thin 模式官方支持、免客户端，开发最快。
  但运行时才暴露类型错误，与本项目的首要标准直接冲突。
