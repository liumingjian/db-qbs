# 目标端装机演练实录 —— 2026-08-20T06:12:00Z

**票**：[#156](https://github.com/liumingjian/db-qbs/issues/156)
**手册**：[`../target-centos7.md`](../target-centos7.md)
**回放脚本**：`docs/spikes/fixtures/local-rig/scripts/rehearsal-target-install.sh`
**判定来源**：[ADR-0041](../../adr/0041-v2-scope-trial-readiness.md) §6（判据是过程性的，落在演练记录里；
「手册没写、临场解决」算判据未达成——回写手册、重走）

## 这一趟跑在什么上面

| 面 | 实况 |
|---|---|
| 目标端主机 | `qbs-host-target`，`centos:7` / `linux/amd64`，`rehearsal-topology-check.sh --reset` 推倒重建出来的干净容器 |
| 架构 / glibc | `x86_64` / `2.17`（R0b/R0d 那一笔账，#151 的构建目标） |
| MySQL | `172.21.0.2:3306/qbs`（MySQL 在 `qbs-dst-side` 上的 IP，账号 `spike`）。按 IP 取、不按容器名 |
| sink 二进制 | `packaging/centos7/build.sh --platform linux/amd64` 当趟产出的 `db-qbs-sink`（5333144 字节，glibc 动态链接、GLIBC 符号上界 ≤ 2.17）|
| 源端（对端） | `qbs-host-source`：由台架脚本 `rehearsal-tunnel-up.sh --side source` 准备（stunnel 客户端 + 两端证书 + `openssl`），本票只把它当作**对端事实** |
| 「公网」一跳 | `host.docker.internal:15443`（真机上是客户给的公网 IP + 白名单端口）|
| 拓扑前置 | 同日 `rehearsal-topology-check.sh --reset`：前置 R0 **4/4**、拓扑 **24/24**（先跑拓扑、后装隧道，ADR-0041 增补 6(d)）|

**源端不是照 #155 的手册装的**——它由台架脚本 `rehearsal-tunnel-up.sh --side source` 准备，
目标端那一头的每一条命令都出自手册。第 10 步「从公网侧核一眼」那四条命令在源端那台上敲
（手册明写），回放脚本代为打出。

## 逐步实测

### 第 1 步 上机第一件事：自检先红

`PASS=1 FAIL=8`，退出码 1。**逐条与手册第 1 步那张期望表对齐，9/9 一致**：

```
D1 PASS MySQL 172.21.0.2:3306 通
D2 FAIL sink 没有应答                     D6 FAIL 未判定（D2 先红）
D3 FAIL 前提未满足（D2 先红）             D7 FAIL 未判定（D2 先红）
D4 FAIL 未判定（sink 没应答，D2 先红）    D8 FAIL pid 文件指不到活进程
D5 FAIL 未判定（sink 没应答，D2 先红）    D9 FAIL 配置里读不到 accept 端口
```

**判的不是「一片红」**：干净的 `centos:7` 上目标端与 MySQL 同内网（D1 该绿）。
一张全红的期望表会把「脚本恒红」这种假绿放进来（#154 的 P5b 判过同一件事）。
**D4–D7 记的是「未判定」而不是 FAIL 的裸值**——三项前提是问 sink 要的，sink 没起就判不了；
「未判定不许记 PASS」是自检的纪律，这里连它对偶的「未判定不等于判失败」也守住了。

### 第 2 步 换 yum 源

`vault.centos.org` + CERN + kernel.org 三源、`failovermethod=priority`、`gpgcheck=1`。
末尾 `yum -y makecache fast` 打出「yum 源就位」。与源端手册第 2 步一字不差。

### 第 3 步 那四个包

```
stunnel-4.56-6.el7.x86_64      openssl-1.0.2k-26.el7_9.x86_64
curl-7.29.0-59.el7_9.2.x86_64  iproute-4.11.0-30.el7.x86_64
```

目标端不装 MySQL 客户端（第 3 步说了为什么：base 源的 5.x 客户端对 8.0 的
`caching_sha2_password` 认不了，红出来的是假故障）。

### 第 4 步 sink 二进制

`db-qbs-sink` 不带 `--config` 起一次：

```
{"level":"error","event":"sink_unavailable","message":"用法：db-qbs-sink --config <sink.toml>"}
exit=1
```

**这就是「它跑得起来」的证据**——进程已经进到自己的参数校验里了，
不是 `GLIBC_2.xx not found`，也不是 `Exec format error`。

### 第 5 步 sink.toml

一行 `listen = "127.0.0.1:8080"`，`0600`。**没写已退役的 `mysql_dsn` / `database`**（ADR-0037 §2）。

### 第 6 步 起 sink

```
{"level":"warn","event":"sink_started","listen":"127.0.0.1:8080",
 "message":"本服务无鉴权，能连上者可用调用方给的凭据清空并重写任意暂存表与目标表；当前监听地址：127.0.0.1:8080"}
curl http://127.0.0.1:8080/v1/runs/__probe__
 → {"error":{"code":"RUN_UNKNOWN","message":"未知 run __probe__","run_id":"__probe__","details":{}}}
```

`sink_started` 那句「本服务无鉴权」的警告是**正常的**（它自己把兜底前提喊出来）。
`RUN_UNKNOWN` 是「那头是 sink」的指纹，自检 D2、隧道判据 T3/T5/T7 认的都是它。

### 第 7 步 MySQL 三前提

这一步在目标端主机上**没有命令可敲**（是给 DBA 的纸条）。演练台上的 MySQL 是 compose 起的：
`--character-set-server=utf8mb4`、`max_allowed_packet` 是 8.0 的默认 64M、账号 `spike` 对库 `qbs` 全权，
**三项天然满足——这正是演练台比真机宽松的一处**，手册以真机差异 ⑥ 标出，D4–D7 在第 9 步经真 sink 判。

### 第 8 步 stunnel 服务端

占位符填完（`accept = 0.0.0.0:15443` / `connect = 127.0.0.1:8080`），残留检查打「占位符已填完」，
两张证书的 SHA-256 指纹**与源端那台上一字不差**：

```
target.crt  1F:83:6B:42:9A:BE:3E:C5:87:09:BD:B8:12:CB:3C:BC:2F:C8:D2:60:69:5E:E2:6D:9F:8E:07:A4:2B:62:54:33
source.crt  A9:AB:25:10:87:8C:C9:A5:40:DA:BA:89:02:CB:44:BB:A7:CE:03:63:98:E0:43:99:07:37:C5:DE:02:0D:4C:F7
```

起完 pid 文件 `573`。

### 第 9 步 自检全绿

```
D1 PASS MySQL 监听口 172.21.0.2:3306 可达
D2 PASS sink 应答 RUN_UNKNOWN
D3 PASS 172.21.0.3:8080 不通（回环之外摸不到 sink）
D4 PASS sink 用给定凭据连得上目标库
D5 PASS 会话字符集三项都是 utf8mb4         （合格）
D6 PASS sql_mode 设得成 STRICT_ALL_TABLES  （合格）
D7 PASS max_allowed_packet ≥ 64 MiB        （合格）
D8 PASS stunnel 服务端 pid=573
D9 PASS 白名单口 15443 在听
==== 目标端自检：PASS=9 FAIL=0 ====   退出码 0
```

**D4–D7 是本票判据 3 的兑现点，也是这三项第一次在真 sink 上转绿**：#154 落地时台架上只有桩给的
九档分类（C1–C9），连不上之外的每一档都没在真 sink 上走过。这一趟是真 `db-qbs-sink` 用同一个驱动、
同一套开连接仪式（`crates/sink/src/mysql_destination.rs` 的 `run_connection_ritual`）连真 MySQL 判出来的。

### 第 10 步 从「公网」侧核一眼（在源端那台上敲）

票面判据 2「sink 只绑回环；从公网侧只有经 stunnel 服务端能到达它」的四条取证：

```
① 明文 curl http://host.docker.internal:15443/…    → curl: (52) Empty reply from server    （拿不到）
② openssl s_client + 源端客户端证书                → RUN_UNKNOWN                            （经隧道到得了 sink）
③ openssl s_client 不带客户端证书                  → grep -c RUN_UNKNOWN = 0                 （verify=2 双向认证在拒它）
④ 源端本机隧道入口 curl http://127.0.0.1:8080/…    → RUN_UNKNOWN                            （产品 sink_base_url 原样）
```

### 收尾核对

```
LISTEN 127.0.0.1:8080  users:(("db-qbs-sink",pid=412,fd=12))   ← sink 只在回环
LISTEN *:15443         users:(("stunnel",pid=573,fd=17))       ← 对外只有白名单口
-rw------- /etc/db-qbs/sink.toml               (0600)
-rw------- /etc/stunnel/db-qbs/target.key      (0600)
-rw------- /root/.qbs-mysql-pass               (0600)
```

## 隧道判据在真 sink 上重走（`rehearsal-tunnel-check.sh --sink real`）

#153 的 T0–T11 此前跑在桩 sink 上；本票的落点是真 `db-qbs-sink`，判据脚本随之新增
`--sink real`，T3/T5/T7 按产品的 `RUN_UNKNOWN`（不再按桩的标记 `QBS-TUNNEL-OK`）认落点。
**17/17 全 PASS**：

```
T0a/T0b PASS 两台主机在跑        T6   PASS 明文打宿主:15443 拿不到东西
T1  PASS 源端 stunnel 客户端     T6b  PASS 对端首字节：非明文（对端未回字节）
T2  PASS 目标端 stunnel 服务端   T7   PASS 带客户端证书握手 → RUN_UNKNOWN（T6/T6b/T8 的正对照）
T3  PASS 目标端自连回环 → RUN_UNKNOWN   T7b  PASS 对端证书 CN = db-qbs-target（钉住的那一张）
T4  PASS 172.21.0.3:8080 不通    T7c  PASS 协商 TLSv1.2 / ECDHE-RSA-AES256-GCM-SHA384
T5  PASS 源端经隧道口 → RUN_UNKNOWN     T8   PASS 不带客户端证书握手被拒
T9  PASS 对外发布端口全集 = 15443/tcp
T10a PASS 目标端自连 172.21.0.3:15443 通（T10 的正对照）
T10 PASS 源端按 IP 直达 172.21.0.3:15443 不通（跨容器直达仍被切断）
T11 PASS 源端经自己 172.20.0.3:8080 不通（隧道入口只绑回环）
==== 隧道判据（落点=real sink）：PASS=17 FAIL=0 ====
```

**这是隧道加密取证第一次落在真 sink 上**。#153 那份桩 sink 的实录
（[`../../spikes/fixtures/local-rig/rehearsal-tunnel-20260820T022000Z.md`](../../spikes/fixtures/local-rig/rehearsal-tunnel-20260820T022000Z.md)）
是第一份证据，本趟是第二份。#155 写明的那条代价（`--sink real` 下 T3/T5/T7 会红在桩的标记上）
在真 sink 成为交付物之后由 `--sink` 开关收回——见 ADR-0041 增补 8(b)。

## 判据对账（#156 的五条）

| # | 判据 | 结果 |
|---|---|---|
| 1 | 照手册在干净目标端主机容器上从零装完，目标端自检**从红转全绿** | **达成**：先红 `1/8` 且逐条对齐期望表 → 装完 `9/0`、退出码 0 |
| 2 | sink 只绑回环；从「公网」侧只有经 stunnel 服务端能到达它 | **达成**：`ss` 实测 `8080` 只在 `127.0.0.1`；第 10 步①明文拿不到、②带证书拿到 `RUN_UNKNOWN`、③不带证书被拒、④经源端隧道入口拿到 `RUN_UNKNOWN`；隧道判据 `--sink real` `17/17` |
| 3 | MySQL 连通且开连接仪式三前提满足（utf8mb4 / STRICT_ALL_TABLES 可设 / max_allowed_packet ≥ 64 MiB） | **达成**：D4–D7 经真 sink 判过、全合格（三项第一次在真 sink 上转绿） |
| 4 | 真机差异点逐处显式标出 | **达成**：手册十处 `⚠ 真机差异` 并按出现顺序汇总成表（yum 源已在 / 不出网 / 包已装 / 8080 被占 / systemd(sink) / MySQL 是客户的库 / 白名单端口与公网 IP 由客户给 / systemd(stunnel) / SELinux / 防火墙），静态自检按标记数 ≥ 7 与关键词判 |
| 5 | 行李清单补齐目标端条目；手册进仓库、与实录同处一个文档区 | **达成**：`packaging/PACKING-LIST.md` 补齐目标端 sink 的直链、口令文件走法、给 DBA 的纸条（第 11 项）；手册在 `docs/install/`，本实录在 `docs/install/records/` |

## 演练撞到、当场处理的事

**手册那一侧零回写**：目标端主机上每一条命令都出自手册、逐条走通，先红形状与期望表 9/9 一致，
装完 9/0，没有一次「手册没写、临场解决」。ADR-0041 §6 那条判据在手册这一侧一次都没破。

**回放台架那一侧撞到一件（不影响手册，是 rig 编排的坑）**：

- **`db-qbs-sink` 二进制在「构建」与「演练」分成两个 rexec 任务时会被中途的 `rsync --delete` 抹掉。**
  `packaging/centos7/out/` 只在**根** `.gitignore` 里以锚定式 `/packaging/centos7/out/` 出现，而 rexec agent
  的 rsync 用的是 `--filter=':- .gitignore'` 逐目录合并——根那条锚定规则没能护住这个子目录（`packaging/stunnel/out/`
  有**自己那一层** `.gitignore`，同一趟 rsync 里就活了下来，两者一对照成因就清楚了）。头两趟演练因此在
  target-install 的前提检查上停在「缺 db-qbs-sink」。**处置：把「构建」与「演练」并成同一个 rexec 任务**
  （产物在同一趟里产出、消费，中途没有第二次 `--delete` 同步）。这是台架回放的编排问题，不是手册的缺口——
  手册第 4 步的前提是「行李清单第 2 项已经在你手上」，真机上是 U 盘/scp 搬进去的，不经 rsync。

## 没做的那几档，以及为什么

- **真机上才第一次成立的两段**：systemd unit（`db-qbs-sink` / `db-qbs-stunnel` 的 `enable --now`、重启后还在）
  与 SELinux、firewalld。容器里没有 systemd、没有 firewalld、`getenforce` 也不在——**这三样演练台上验不到**，
  手册里逐条标了「真机差异」。ADR-0041 增补 1 明文接受这个代价。
- **搬通一次真实搬运**（规格 #149 User Story 14）不在本票，那是两端都装完之后的事（#157）。
  本票到「目标端自检全绿 + 三前提经真 sink 判过 + 公网侧只有经隧道到得了 sink」为止。
- **三份视觉走查**：本票零 UI 改动（改的是 `docs/`、台架脚本、`packaging/PACKING-LIST.md`），
  `web/` 与 `docs/design-system/` 一个字节没动，按 `CLAUDE.md` 通则 3 记「未跑」，理由无触发；
  封存点见 ADR-0040 §6.1/§6.2 与 v1 的 `v1-visual-walkthrough-20260819T173957Z.md`。

## 四份既有台架照旧全绿 / 语言侧

- `npm run typecheck`：PASS；`npm test`：**96 tests** 全过；`cargo test --workspace`：全过（本票零代码改动，
  改的是 `docs/`、台架脚本、`packaging/`；这一趟只为确认没有意外触碰）。
- 拓扑判据 `--reset`：前置 R0 **4/4**、拓扑 **24/24**；隧道判据 `--sink real`：**17/17**；
  五支静态自检（source-install / target-install / tunnel / preflight / topology）全 PASS。
