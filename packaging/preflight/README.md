# 两端环境自检（#154 / 规格 #149 D.13–D.15）

**上机第一件事跑它。** 缺什么一次列全，逐条按输出处置，清完再重跑。

```sh
# 源端主机
./preflight-source.sh

# 目标端主机（口令走文件，别落进 shell 历史）
QBS_MYSQL_USER=qbs QBS_MYSQL_DATABASE=dw_stage \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass ./preflight-target.sh
```

两支都 `--help` 自带用法与全部环境变量。退出码：全绿 0，有 FAIL 1。

## 判据

「**自检说 OK 之后，现场不该再出现环境类失败**」。所以它有三条纪律：

1. **一次列全，不撞到第一个就停**（`set -e` 刻意不开）。装机现场最贵的是往返。
2. **每条 FAIL 带一行处置**。自检不是报警器，是待办清单。
3. **前提没满足的条目照样留在清单里**（记 FAIL，说明写「未判定」）。吞掉的那几条就是下一次往返。

**未判定一律不记 PASS。** 自检替环境作一个它没验过的保证，比漏一条还糟——判据的原话是
「自检说 OK 之后不该再出现环境类失败」。具体到 D5–D7：开连接仪式是
连上 → `SET NAMES` → `SET sql_mode` → 回读三项，**只有回读跑完那一档才产生得了 PASS**；
仪式在回读之前停下时，卡住的那一步记 FAIL、其余记未判定——「设过了」不等于「就是这个值」
（中间层改写会话变量正是产品那道回读要防的事）。

## 查什么

| 源端 | 查什么 | 怎么查 |
|---|---|---|
| S1 | glibc ≥ 2.17 | `getconf GNU_LIBC_VERSION` |
| S2 | Instant Client 目录里有 `libclntsh.so` **且读得到** | 目录里找，软链必须解得开 |
| S3 | Instant Client 架构与本机一致 | 读 ELF 头的 `e_machine` |
| S4 | Instant Client 的动态依赖全解析得开 | `ldd` 找 `not found`，**按产品自己的搜索路径查、不替它加 `LD_LIBRARY_PATH`**（两条常见成因：缺 `libaio`；目录没进 `ldconfig`，症状是 `libnnz19.so` 找不到） |
| S5 | Oracle 监听口可达 | TCP。地址取 `QBS_ORACLE_HOST`；取不到就记「未判定」，**不猜 `127.0.0.1`** |
| S6 | stunnel 客户端进程在跑 | pid 文件 + `/proc` |
| S7 | 隧道入口端口在听 | TCP |
| S8 | 经隧道摸得到目标端的 **sink** | HTTP 打 sink 的只读端点，认它的错误码 `RUN_UNKNOWN` |

| 目标端 | 查什么 | 怎么查 |
|---|---|---|
| D1 | MySQL 监听口可达 | TCP |
| D2 | sink 在回环上应答 | 同 S8 |
| D3 | sink 没越出回环（ADR-0024 的兜底） | 先判 `listen` 绑的是不是回环，再从本机非回环地址反向摸一次 |
| D4 | sink 用给定凭据连得上目标库 | `POST /v1/target/test-connection` |
| D5 | 会话字符集三项都是 `utf8mb4` | 同上 |
| D6 | `sql_mode` 设得成 `STRICT_ALL_TABLES` | 同上 |
| D7 | `max_allowed_packet` ≥ 64 MiB | 同上 |
| D8 | stunnel 服务端进程在跑 | pid 文件 + `/proc` |
| D9 | 白名单口在听 | TCP，端口从 stunnel 配置里读，不写死 |

## 三处刻意为之的事

- **不装任何东西就跑得起来**：只用 bash 4.2 + coreutils + glibc 自带的 `ldd`/`getconf`；
  HTTP 与 TCP 都走 bash 的 `/dev/tcp`，`curl` / `nc` / `ip` 一个都不要。
  它要在「什么都还没装」的机器上先红一次——那台机器上什么都没有。
- **目标端不装 MySQL 客户端**：CentOS 7 base 源里的客户端是 5.x，对 MySQL 8.0 的
  `caching_sha2_password` 认不了，红出来的是一条假故障。三项开连接仪式前提改由 **sink 自己**去问
  （`crates/sink/src/mysql_destination.rs` 的 `run_connection_ritual`）——
  同一个驱动、同一套会话设置，**自检问到的与搬运时用到的是同一件事**，不是近似替身。
  代价是这几项要等 sink 装上来才判得了；干净机器上它们先红，正是要的。
- **两支脚本各自独立、不抽公共库**：它们是分别搬到两台机器上的单文件，
  共享库会变成「少带了一个文件」这种现场故障。四十行重复换的是这个。

## 证不了的那一档

- **Oracle 的账号 / 口令 / 服务名**：Instant Client Basic 包不带 `sqlplus`，本票也不给客户机加装它。
  那一档装完 source 之后用界面的「测试连接」证一次——它走的是产品自己的 Oracle 连接路径（ADR-0037 §9）。
- **隧道的加密与认人**：自检只判「通不通、那头是不是 sink」。加密取证是演练台的判据
  （`docs/spikes/fixtures/local-rig/scripts/rehearsal-tunnel-check.sh` 的 T6–T8），不在装机现场重做。

## 台架上怎么验它

判据 `P0–P11` 在 `docs/spikes/fixtures/local-rig/scripts/rehearsal-preflight-check.sh`：
干净的 `centos:7` 容器上先红（逐条对期望表，不是「一片红」），装上隧道之后该转绿的转绿。
不起台架的那一半（检查项覆盖清单、脚本进仓库、与产品那几处措辞的耦合）在
`test-rehearsal-preflight.sh`。
