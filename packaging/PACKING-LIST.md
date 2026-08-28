# 带去现场的行李清单（#149 B.9）

出发前逐项核对。**两份装机手册都从这份清单开头**——清单是一份，手册引它，不各抄一份。

装机那天现场没有第二次机会，也多半没有可用的外网：**凡是"到时候下一个"的东西，都要在这里出现**。

| # | 东西 | 从哪来 | 装到哪 | 核对方法 |
|---|---|---|---|---|
| 1 | `db-qbs-source` / `db-qbs-source-run`（x86_64） | `packaging/centos7/build.sh` 的产物 | 源端主机 | `file` 看架构；干净 `centos:7` 上启动过一次（`packaging/centos7/verify.sh`） |
| 2 | `db-qbs-sink`（x86_64） | 同上 | 目标端主机 `/opt/db-qbs/bin/` | 同上；装上之后目标端自检 D2 认它的 `RUN_UNKNOWN` |
| 3 | Oracle Instant Client 19c **Basic** 包（x86_64，zip） | [直链](https://download.oracle.com/otn_software/linux/instantclient/1932000/instantclient-basic-linux.x64-19.32.0.0.0dbru.zip)（免 Oracle 账号），**出发前下好** | 源端主机，解到 `oracle_client_lib_dir`，**并把该目录注册进 `ldconfig`** | 源端自检 S2–S4 |
| 4 | `preflight-source.sh` | `packaging/preflight/` | 源端主机任意目录 | 上机第一件事跑它 |
| 5 | `preflight-target.sh` | `packaging/preflight/` | 目标端主机任意目录 | 上机第一件事跑它；口令走 `QBS_MYSQL_PASSWORD_FILE`，别进 shell 历史 |
| 6 | stunnel 配置模板（两端各一份 + systemd unit） | `packaging/stunnel/{source,target}-side/` | `/etc/stunnel/db-qbs/` | `packaging/stunnel/README.md` 的六步。**两端的模板同名不同内容**，源端拿 `source-side/`、目标端拿 `target-side/` |
| 7 | stunnel 双端证书材料 | `packaging/stunnel/gen-certs.sh` **出发前跑一次** | 两端 `/etc/stunnel/db-qbs/` | 私钥 `chmod 600`；**不进版本库**；两端各拿 `out/<side>-side/` 那一套，装完两边对 SHA-256 指纹 |
| 8 | 六个包的 rpm（离线包）：`libaio` `stunnel` `openssl` `unzip` `curl` `iproute` | CentOS 7 vault 存档源，**出发前下好**（在一台能上网的 CentOS 7 上 `yumdownloader --resolve --destdir=... <六个包>`） | 两端 | 现场 yum 多半上不了外网。`libaio` 是 Instant Client 的硬依赖；没有 `unzip` 连 Instant Client 都解不开；`ss` 来自 `iproute`，**干净的 CentOS 7 上它不在**（演练台实测）。目标端只用到其中四个（`stunnel` `openssl` `curl` `iproute`） |
| 9 | `config/source.toml.example` / `config/sink.toml.example` | 仓库 `config/` | 两端，填完 `chmod 0600` | 自检会从 `/etc/db-qbs/` 下读它们。`sink.toml` 只有 `listen` 一行，**别写已退役的 `mysql_dsn` / `database`**；`source.toml` 三行，**别写已退役的 `sink_base_url`**（目标端地址改成在界面上注册的 agent，ADR-0044 §5） |
| 10 | 两份装机手册 | **已随 `docs/` 一并删除**；取回用 `git log --diff-filter=D -- docs/install/`（目标端 `target-centos7.md` #156 **先装**，源端 `source-centos7.md` #155） | 打印或离线带一份 | 删除前每一行都在演练台上走过；演练实录同样只在 git 历史里 |
| 11 | 给客户 DBA 的纸条：MySQL 三前提 + 账号授权 | 目标端手册第 7 步（目标库 **MySQL 5.7 或 8.0**；`character-set-server=utf8mb4`、`max_allowed_packet ≥ 64M`、没有 `init_connect` 改 `sql_mode`；`GRANT SELECT, INSERT, UPDATE, CREATE, DROP`） | **出发前就发给 DBA** | 目标端自检 D4–D7；**5.7 的 `max_allowed_packet` 默认值是 4 MiB，必红**；写 my.cnf 那一半要重启 MySQL，窗口要提前约 |

## 外部依赖（产品这边自证不了）

- **目标端公网入口**：IP / 端口 / 白名单，由客户提供。拿不到就是装机当天彻底停摆（规格 #149「唯一外部阻塞项」）。
  源端配置要的是公网 IP + 白名单端口；目标端配置只要白名单端口（`accept` 绑 `0.0.0.0`）。
- **两个库的账号**：Oracle 只读账号、MySQL 目标库账号（要能建表、写表、删暂存表：`SELECT, INSERT, UPDATE, CREATE, DROP`）。
- **目标端 MySQL 是 5.7 或 8.0**（5.7 是**新增**的一档，不是替换 8.0）。两个版本都按 utf8mb4 走，
  SQL 上没有分叉；差别只在下一条。
- **目标端 MySQL 的 `max_allowed_packet` ≥ 64 MiB**（67108864 字节）：**8.0 的默认值刚好够，
  5.7 的默认值是 4 MiB，所以每一台没调过参的 5.7 都会红在这里**——这不是异常，是常态，出发前就要跟 DBA 说好。
  两条都要做：`SET GLOBAL max_allowed_packet = 67108864;` 当场生效（只对之后新建的连接生效），
  my.cnf 的 `[mysqld]` 段写 `max_allowed_packet = 64M` 让它熬得过重启。写 my.cnf 那一半要重启 MySQL，
  属于客户 DBA 的窗口，早点约。
- **目标端 MySQL 的 `character-set-server = utf8mb4`**，且没有 `init_connect` / 代理层改写会话 `sql_mode`——三项由 sink 在开连接仪式里回读判定，任一不合格整个 sink 不可用。
