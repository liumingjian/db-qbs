# db-qbs

数据库查询导入服务：对接异构数据库，执行查询并把结果导入到目标端。当前只需支持 **MySQL** 和 **Oracle** 两种数据源。

## 范围

- **支持的数据库**：MySQL、Oracle。其他数据库暂不在范围内。
- **核心能力**：连接数据源 → 执行查询 → 导出/导入结果数据。
- **形态**：服务（长期运行，对外提供接口触发导入任务）。

## 状态

项目处于初始阶段，尚无代码。技术栈、接口形式、任务调度方式待定。

## 快速开始

待补充（依赖安装、配置数据源连接、启动服务）。

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

待补充（构建、测试、本地运行）。

## Agent 配置

本仓库按 `mattpocock/skills` 的约定做了 agent 配置：

- `CLAUDE.md` — agent 指令入口，含 `## Agent skills` 块
- `docs/agents/issue-tracker.md` — issue 走 GitHub Issues（`gh` CLI）
- `docs/agents/triage-labels.md` — triage 标签词表
- `docs/agents/domain.md` — 领域文档布局（single-context：根目录 `CONTEXT.md` + `docs/adr/`）
