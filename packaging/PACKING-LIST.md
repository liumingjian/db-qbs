# 带去现场的行李清单（#149 B.9）

出发前逐项核对。**两份装机手册都从这份清单开头**——清单是一份，手册引它，不各抄一份。

装机那天现场没有第二次机会，也多半没有可用的外网：**凡是"到时候下一个"的东西，都要在这里出现**。

| # | 东西 | 从哪来 | 装到哪 | 核对方法 |
|---|---|---|---|---|
| 1 | `db-qbs-source` / `db-qbs-source-run`（x86_64） | `packaging/centos7/build.sh` 的产物 | 源端主机 | `file` 看架构；干净 `centos:7` 上启动过一次（`packaging/centos7/verify.sh`） |
| 2 | `db-qbs-sink`（x86_64） | 同上 | 目标端主机 | 同上 |
| 3 | Oracle Instant Client 19c **Basic** 包（x86_64，zip） | [直链](https://download.oracle.com/otn_software/linux/instantclient/1932000/instantclient-basic-linux.x64-19.32.0.0.0dbru.zip)（免 Oracle 账号），**出发前下好** | 源端主机，解到 `oracle_client_lib_dir`，**并把该目录注册进 `ldconfig`** | 源端自检 S2–S4 |
| 4 | `preflight-source.sh` | `packaging/preflight/` | 源端主机任意目录 | 上机第一件事跑它 |
| 5 | `preflight-target.sh` | `packaging/preflight/` | 目标端主机任意目录 | 上机第一件事跑它 |
| 6 | stunnel 配置模板（两端各一份 + systemd unit） | `packaging/stunnel/{source,target}-side/` | `/etc/stunnel/db-qbs/` | `packaging/stunnel/README.md` 的六步 |
| 7 | stunnel 双端证书材料 | `packaging/stunnel/gen-certs.sh` **出发前跑一次** | 两端 `/etc/stunnel/db-qbs/` | 私钥 `chmod 600`；**不进版本库** |
| 8 | 六个包的 rpm（离线包）：`libaio` `stunnel` `openssl` `unzip` `curl` `iproute` | CentOS 7 vault 存档源，**出发前下好**（在一台能上网的 CentOS 7 上 `yumdownloader --resolve --destdir=... <六个包>`） | 两端 | 现场 yum 多半上不了外网。`libaio` 是 Instant Client 的硬依赖；没有 `unzip` 连 Instant Client 都解不开；`ss` 来自 `iproute`，**干净的 CentOS 7 上它不在**（演练台实测） |
| 9 | `config/source.toml.example` / `config/sink.toml.example` | 仓库 `config/` | 两端，填完 `chmod 0600` | 自检会从 `/etc/db-qbs/` 下读它们 |
| 10 | 两份装机手册 | 源端 [`docs/install/source-centos7.md`](../docs/install/source-centos7.md)（#155）；目标端 `docs/install/target-centos7.md`（#156） | 打印或离线带一份 | 每一行都在演练台上走过，实录在 [`docs/install/records/`](../docs/install/records/) |

## 外部依赖（产品这边自证不了）

- **目标端公网入口**：IP / 端口 / 白名单，由客户提供。拿不到就是装机当天彻底停摆（规格 #149「唯一外部阻塞项」）。
- **两个库的账号**：Oracle 只读账号、MySQL 目标库账号（要能建表、改表、删暂存表）。
- **目标端 MySQL 的 `max_allowed_packet` ≥ 64 MiB**：改它要重启 MySQL，属于客户 DBA 的窗口，早点约。
