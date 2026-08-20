# db-qbs

数据库查询导入服务：对接异构数据库，执行查询并把结果导入到目标端。当前只需支持 **MySQL** 和 **Oracle** 两种数据源。

## 范围

- **支持的数据库**：MySQL、Oracle。其他数据库暂不在范围内。
- **核心能力**：连接数据源 → 执行查询 → 导出/导入结果数据。
- **形态**：服务（长期运行，对外提供接口触发导入任务）。

## 状态

M1（一次性进程跑通一趟导入）、M2（source 常驻服务 + Web UI）与 M3（九行形态、映射预检与值域校核）
**已实现**，验收记录在 `docs/spikes/fixtures/local-rig/`。M4 尚未开工，但错误分类已提前实现。列名映射已明确不做，改名使用
SQL `AS` 别名。**尚未在生产环境部署过。**

两端都是 Rust（`crates/`），Web UI 是 React + Vite（`web/`），构建时由 `crates/source/build.rs`
调 `npm run build` 打包并嵌进 `db-qbs-source` 可执行文件。决策依据见 `CONTEXT.md` 与 `docs/adr/`。

## 快速开始

三个可执行文件：

| 可执行文件 | 位置 | 作用 |
| --- | --- | --- |
| `db-qbs-sink` | 目标端 | 长驻服务，写 MySQL |
| `db-qbs-source` | 源端 | 长驻服务，Web UI + 任务编排（M2） |
| `db-qbs-source-run` | 源端 | 一次性进程，跑一趟导入（由 `db-qbs-source` 拉起，也可单独跑） |

前提：源端装好 **Oracle Instant Client 19c Basic 包**（`oracle_client_lib_dir` 指向它），
目标端有 **MySQL 8.0**；构建机需要 Rust 1.85+ 与 Node.js 22+（`Cargo.lock` 里的 `zeroize` 要 edition2024，
1.85 以下的 Cargo 解析不动；node 16 编不过 `npm run build`）。

要装到 **CentOS 7（glibc 2.17）** 上的产物不能在这台构建机上直接编 —— 装上去启动即
`GLIBC_2.xx not found`。那条路走 `packaging/centos7/build.sh`：一条命令把 `linux/amd64` 与
`linux/arm64` 两套都编出来，并各自在同架构的干净 `centos:7` 上验一次启动，
见 `packaging/centos7/README.md`。

装到客户机器上的那条路还有三件东西在 `packaging/` 下：出发前逐项核对的**行李清单**
（`packaging/PACKING-LIST.md`）、上机第一件事跑的**两端环境自检**
（`packaging/preflight/`，缺什么一次列全）、以及把 `source → sink` 那一跳加密起来的
**stunnel 双端模板**（`packaging/stunnel/`）。

```sh
cargo build --release

# 目标端
cp config/sink.toml.example sink.toml && chmod 0600 sink.toml   # 填 mysql_dsn / database / listen
./target/release/db-qbs-sink --config sink.toml

# 源端
cp config/source.toml.example source.toml && chmod 0600 source.toml
./target/release/db-qbs-source --config source.toml            # 浏览器打开配置里的 listen
```

两处 `listen` **都没有鉴权**，默认只绑回环；要多人访问得自己在前面放反向代理做鉴权与 TLS
（ADR-0024 §1、§4）。不经 UI 直接跑一趟：

```sh
db-qbs-source-run --config source.toml --task task.toml --biz-date 2026-08-14
```

## 运行日志

`db-qbs-source-run` 与 `db-qbs-sink` 只向 stdout 输出 JSON Lines。失败记录可能包含业务列值；
需要重定向到文件时，须在创建文件前收紧权限：

```sh
umask 077
db-qbs-source-run --config source.toml --task task.toml --biz-date 2026-08-14 > run.jsonl
chmod 0600 run.jsonl
```

日志文件不得放宽为 0644，也不得采集或转发到目标端之外。完整字段契约见 ADR-0017 §6。

## 开发

```sh
cargo test --workspace   # Rust 单元与集成测试
npm install              # 首次
npm run typecheck        # tsc --noEmit
npm test                 # vitest run
npm run dev              # 只调前端时用（vite dev server）
```

台架验收（M1 9 条、M2 A1–A14、M3 B1–B6）与 M2/M3 渲染走查是**带触发条件的手工门禁，不进 CI**
（ADR-0014 §8）：脚本在 `docs/spikes/fixtures/local-rig/scripts/`，走查清单在
`docs/spikes/fixtures/local-rig/m2-visual-walkthrough.md` 与
`docs/spikes/fixtures/local-rig/m3-visual-walkthrough.md`。改 `docs/design-system/` 必须
重跑 M2 走查；改 M3 失败态布局或诊断表列结构必须重跑 M3 走查，并记录实际观察。

## Agent 配置

本仓库按 `mattpocock/skills` 的约定做了 agent 配置：

- `CLAUDE.md` — agent 指令入口，含 `## Agent skills` 块
- `docs/agents/issue-tracker.md` — issue 走 GitHub Issues（`gh` CLI）
- `docs/agents/triage-labels.md` — triage 标签词表
- `docs/agents/domain.md` — 领域文档布局（single-context：根目录 `CONTEXT.md` + `docs/adr/`）
