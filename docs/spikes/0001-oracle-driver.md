# Spike 0001 — Oracle 驱动选型

闸门 issue：[#1 M0: Oracle 驱动 spike（风险闸门）](https://github.com/liumingjian/db-qbs/issues/1)

> 本文档由 M0 各子任务分节填写，最终由 #8 汇总出闸门决策。
> spike 代码一次性，不进主干；本文档进主干。

## 1. 环境与样本（#2）

_待填_

## 2. `oracle` crate / ODPI-C 类型保真度（#3）

_待填_

## 3. `oracle-rs` 纯 Rust 类型保真度（#4）

_待填_

## 4. 流式 fetch 吞吐与内存（#5）

_待填_

## 5. dblink 列投影下推（#6）

_待填_

## 6. 客户源端机器部署 Instant Client 可行性（#7）

**状态：待客户答复。** 采集清单见 issue #7 的 checklist 评论。

### 6.1 源端机器环境

| 项 | 值 | 来源 |
|---|---|---|
| 操作系统 / 版本 | _待填_ | |
| CPU 架构 | _待填_ | |
| glibc 版本 | _待填_ | |
| 是否有 root / sudo | _待填_ | |
| 能否访问外网 | _待填_ | |

### 6.2 Instant Client 获取与安装

| 项 | 结论 | 备注 |
|---|---|---|
| 获取途径（在线下载 / 离线包带入） | _待填_ | |
| 是否需走软件安装审批 | _待填_ | 周期： |
| Basic Light 包是否够用 | _待填_ | |
| 库路径配置方式（`LD_LIBRARY_PATH` / `ldconfig`） | _待填_ | |

### 6.3 Oracle 服务端

| 项 | 值 | 影响 |
|---|---|---|
| 服务端版本 | _待填_ | Instant Client 版本兼容性 |
| 是否 ≥ 12.1 | _待填_ | `oracle-rs`（#4）的前置条件 |
| 字符集 `NLS_CHARACTERSET` | _待填_ | 类型保真度 |

### 6.4 结论

_待填 —— 明确回答：Instant Client 能否装？若能，途径与周期是什么？_

## 7. 闸门决策（#8）

_待填_
