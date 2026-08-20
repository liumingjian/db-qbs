# 终局从零演练实录 —— 2026-08-20T08:13:00Z

**票**：[#157](https://github.com/liumingjian/db-qbs/issues/157)
**手册**：[`../target-centos7.md`](../target-centos7.md)（先装）、[`../source-centos7.md`](../source-centos7.md)
**编排脚本**：`docs/spikes/fixtures/local-rig/scripts/rehearsal-final.sh`（它自己不装任何东西，见下）
**跑在哪台**：`lmjMac-mini-Pro.local`（当天队列上有两台 mac，脚本第 0 段把机器名打出来——见文末「这一趟之外的两趟」）
**判定来源**：[ADR-0041](../../adr/0041-v2-scope-trial-readiness.md) §6 与增补 9（判据是过程性的，落在本文件里；
「手册没写、临场解决」算判据未达成——回写手册、重走）

**一句话**：两台主机推倒重来，**只照两份手册**装完两端，自检先红后绿，经隧道跑通一次真实搬运，
目标库数据逐值核对无误。**手册零回写**——这一趟从头到尾没有一条「手册没写、临场解决」的命令。

## 这一趟跑在什么上面

| 面 | 实况 |
|---|---|
| 两台主机 | `qbs-host-source` / `qbs-host-target`，`centos:7` / `linux/amd64`，由 `rehearsal-topology-check.sh --reset` 推倒重建出来的干净容器 |
| 架构 / glibc | 两台都是 `x86_64` / `2.17`（R0a–R0d，#151 的构建目标） |
| 三个二进制 | 本趟现编：`packaging/centos7/build.sh --platform linux/amd64`，`db-qbs-source` 6097816 字节、`db-qbs-sink` 5333144 字节，都是 x86-64 |
| Oracle | `172.20.0.2:1521/XE`（Oracle 在 `qbs-src-side` 上的 IP，账号 `spike`），按 IP 取、不按容器名 |
| MySQL | `172.21.0.2:3306/qbs`（MySQL 在 `qbs-dst-side` 上的 IP，账号 `spike`） |
| 「公网」一跳 | `host.docker.internal:15443`（真机上是客户给的公网 IP + 白名单端口） |
| 拓扑前置 | `rehearsal-topology-check.sh --reset`：前置 R0 **4/4**、拓扑 **24/24**（先跑拓扑、后装隧道） |

**两端都是照手册装的**——这是本票与 #155/#156 的分别：那两票各自由台架准备**对端**
（`rehearsal-tunnel-up.sh --side ...`），本票**两端都由手册的回放一条条敲**，台架只做三件不属于那两台机器的事：
推倒重建、出证书（行李清单第 7 项「出发前跑一次 gen-certs.sh」）、以及在**两个库**上扮演客户 DBA。

## 逐段实测

### 第 1 段 行李清单逐项核对（票面判据 5）

```
第 1  项 OK    db-qbs-source / -source-run     x86-64，6097816 字节
第 2  项 OK    db-qbs-sink                     x86-64，5333144 字节
第 3  项 OK    Instant Client 19c Basic (x64)  缓存在 ~/.cache/db-qbs，unzip -t 通过
第 4  项 OK    preflight-source.sh             在 packaging/preflight/
第 5  项 OK    preflight-target.sh             在 packaging/preflight/
第 6  项 OK    stunnel 配置模板（两端 + unit）  两端各一份 + systemd unit，同名不同内容
第 7  项 OK    stunnel 双端证书材料             两端各一套（私钥 -rw-------，本趟就地出的）
第 8  项 不适用 离线 rpm                        演练台联网走 vault 三源；离线那条路是真机差异 ②，容器上验不到
第 9  项 OK    两份配置样例                     config/ 下两份都在
第 10 项 OK    两份装机手册                     docs/install/ 下两份都在
第 11 项 OK    给 DBA 的纸条                    目标端手册第 7 步（三前提 + 授权语句）
```

**第 8 项记的是「不适用」，不是 OK 也不是缺**（ADR-0041 增补 9(e)）：那批离线 rpm 在这一趟里
一次都没被用过，记 OK 是假绿；记缺又会把整趟挡在门口。第三档「不适用 + 为什么」是唯一诚实的记法。
**第 3 项同一条口径**：缓存里有才记 OK，不在缓存里记「不适用（演练台现下）」——
一台没缓存又不出网的机器不许在这里全绿，到源端第 4 步才炸。

### 第 2 段 推倒重建 + 拓扑判据

`rehearsal-topology-check.sh --reset`：**前置 R0 4/4、拓扑 24/24**。
R9 那一组是干净态本身的证据：重建前两台主机上留的痕迹文件「有」，`rehearsal-reset.sh` 跑通后「无」。
四条负判据（R3/R5 两库互不可达、R6 跨容器直达、R8 白名单外的端口）各自的同址正对照都在。

### 第 3 段 目标端照手册装（第 1–9 步，第 10 步延后）

**先红**：`PASS=1 FAIL=8`，退出码 1，逐条与手册第 1 步那张期望表**9/9 对齐**：

```
D1 PASS MySQL 172.21.0.2:3306 通
D2 FAIL sink 没有应答                      D6 FAIL 未判定（D2 先红）
D3 FAIL 前提未满足（D2 先红）              D7 FAIL 未判定（D2 先红）
D4 FAIL 未判定（sink 没应答）              D8 FAIL pid 文件指不到活进程
D5 FAIL 未判定（sink 没应答）              D9 FAIL 配置里读不到 accept 端口
```

**判的不是「一片红」**：干净的 `centos:7` 上 D1 本来就该绿；D4–D7 记的是「未判定」而不是裸 FAIL。

**装完全绿**：`PASS=9 FAIL=0`、退出码 0。

```
D1 PASS MySQL 监听口 172.21.0.2:3306 可达      D6 PASS sql_mode 设得成 STRICT_ALL_TABLES（合格）
D2 PASS sink 应答 RUN_UNKNOWN                  D7 PASS max_allowed_packet ≥ 64 MiB（合格）
D3 PASS 172.21.0.3:8080 不通（回环外摸不到）   D8 PASS stunnel 服务端 pid=568
D4 PASS sink 用给定凭据连得上目标库            D9 PASS 白名单口 15443 在听
D5 PASS 会话字符集三项都是 utf8mb4（合格）
```

两张证书的 SHA-256 指纹（与源端那台上同一条命令的输出一字不差）：

```
target.crt  2C:AE:6F:1F:A7:08:B2:E5:98:41:AE:30:29:CC:53:18:F9:43:83:38:AE:2D:0E:2B:6B:D2:99:37:58:13:28:E4
source.crt  DB:69:D9:FA:C6:AE:03:49:A5:A6:DF:A6:14:D4:45:0F:76:50:5F:2E:F2:06:36:EF:9D:9C:FF:CB:E9:21:B7:AD
```

收尾：`8080` 只在 `127.0.0.1`（`db-qbs-sink` pid=407），对外只有 `*:15443`（`stunnel` pid=568）；
`sink.toml`、`target.key`、`.qbs-mysql-pass` 都是 `0600`。

**第 10 步在这里延后**（ADR-0041 增补 9(b)）：那四条要在**源端那台**上敲，而此刻源端还是干净机器。
总账里它不记绿也不记红，记的是「延后，装完源端回来跑 `--only-step10`」——把没跑的记成任何一种判定都是假象。

### 第 4 段 源端照手册装（第 1–10 步）

**先红**：`PASS=2 FAIL=6`，退出码 1，与手册那张期望表 **8/8 对齐**（S1 glibc 2.17、S5 Oracle 可达本来就该绿）。

**装完全绿**：`PASS=8 FAIL=0`、退出码 0。

```
S1 PASS glibc 2.17                              S5 PASS Oracle 监听口 172.20.0.2:1521 可达
S2 PASS /opt/oracle/instantclient/libclntsh.so  S6 PASS stunnel 客户端 pid=548
S3 PASS 架构 x86_64                             S7 PASS 隧道入口 127.0.0.1:8080 在听
S4 PASS 动态依赖无缺失                          S8 PASS 经隧道摸得到 sink：应答 RUN_UNKNOWN
```

手册第 4 步后半段那一笔账（#155 演练上撞到、当时回写进手册的那条）这一趟照做并判了：
`ldconfig -p | grep -c libclntsh.so` = **2**，`ldd … | grep -c 'not found'` = **0**。

**第 10 步测试连接**（产品自己的 Oracle 连接路径，界面上那一下的等价命令）：

```
{"elapsed_ms":233,"label":"//172.20.0.2:1521/XE","ok":true}
```

收尾：`8088`（`db-qbs-source` pid=608）与 `8080`（`stunnel` pid=548）都只绑回环；两个文件都是 `0600`。

### 第 5 段 补上目标端手册第 10 步（在照手册装出来的源端上敲）

```
① 明文 curl http://host.docker.internal:15443/…  → curl: (52) Empty reply from server（拿不到）
② openssl s_client + 源端客户端证书              → RUN_UNKNOWN（经隧道到得了 sink）
③ openssl s_client 不带客户端证书                → grep -c RUN_UNKNOWN = 0（verify=2 在拒它）
④ 源端本机隧道入口 curl http://127.0.0.1:8080/…  → {"error":{"code":"RUN_UNKNOWN",…}}
```

**这四条在本票第一次落在「两端都照手册装出来」的机器上**：#156 那一趟里源端是台架准备的。

### 第 6 段 隧道判据 T0–T11（落点是真 sink）

`rehearsal-tunnel-check.sh --sink real`：**17/17 全 PASS**。要点摘录：

```
T4  PASS 172.21.0.3:8080 不通（回环之外摸不到 sink）   T7b PASS 对端证书 CN = db-qbs-target
T5  PASS 源端经本机隧道口 → RUN_UNKNOWN               T7c PASS TLSv1.2 / ECDHE-RSA-AES256-GCM-SHA384
T6  PASS 明文打宿主:15443 拿不到东西                  T8  PASS 不带客户端证书被拒
T6b PASS 对端首字节：非明文（对端未回字节）           T9  PASS 对外发布端口全集 = 15443/tcp
T10 PASS 源端按 IP 直达 172.21.0.3:15443 不通          T11 PASS 源端经自己 172.20.0.3:8080 不通
```

### 第 7 段 一次真实搬运（票面判据 3 / 规格 #149 User Story 14）

**这是本票唯一的新东西**，走的是产品自己的 `/api/*`，在**源端主机容器里**用 `curl` 打——
与所有者经 `ssh -L 8088:127.0.0.1:8088` 点界面是同一条路径（ADR-0028 §1：断言面是 API 不是 DOM）。

**① 客户的库里本来就有那张表**（DBA 的活，不在两台主机上敲）：`T_V2_TRIAL` 共 7 行，
`2026-08-20` 五行、`2026-08-19` 两行——**行数刻意不等量**，好把「过滤生效」与「整表搬了一遍」分开。

**② 两条数据源**：

```
POST /api/datasources (oracle) → 201        测连 Oracle → 200 {"ok":true}
POST /api/datasources (mysql)  → 201        测连 目标库 → 200 {"ok":true}
```

目标库那条的测连**经隧道走到 sink、再由 sink 连 MySQL**（凭据随请求过线，source 自己不建 MySQL 连接）。

**③ 查数据**：`POST /api/builder/tables → 200`，清单里查得到 `T_V2_TRIAL`；
`POST /api/builder/columns → 200`：`ROW_ID NUMBER，CUST_NAME VARCHAR2，AMOUNT NUMBER，LOAD_DATE DATE`。

**④ 加过滤条件**（一条**运行时填**的业务日期），现算出来的源端 SQL：

```sql
SELECT a.ROW_ID AS ROW_ID,
       a.CUST_NAME AS CUST_NAME,
       a.AMOUNT AS AMOUNT,
       a.LOAD_DATE AS LOAD_DATE
  FROM SPIKE.T_V2_TRIAL a
 WHERE a.LOAD_DATE = TO_DATE(:load_date,'YYYY-MM-DD')
```

值走绑定变量（ADR-0011 §2：不发明第二套转义），不是拼进去的字面量。

**⑤ 建表 DDL 由产品生成，DBA 拿去建表**（v1 手工建表，产品不替你建）：

```sql
-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。
-- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。
CREATE TABLE `V2_TRIAL` (
  `ROW_ID` DECIMAL(8,0) NOT NULL,
  `CUST_NAME` VARCHAR(80) NULL,
  `AMOUNT` DECIMAL(12,2) NULL,
  `LOAD_DATE` DATETIME(0) NULL,
  PRIMARY KEY (`ROW_ID`)
) DEFAULT CHARSET=utf8mb4;
```

MySQL 执行它**没有报错**，`describe` 回来四列，与 DDL 一致
（`ROW_ID decimal(8,0) NO PRI`、`CUST_NAME varchar(80)`、`AMOUNT decimal(12,2)`、`LOAD_DATE datetime`）。
判据取的是**实际建出来的列数**，不是「DDL 拿到了」——后者只证明产品出了字，没证明目标库认它。

**⑥⑦ 建任务并发起**：`POST /api/tasks → 201`、`POST /api/runs → 202`，
运行参数 `{"load_date":"2026-08-20"}`。`task_id=1d396e0f1d04a705eb7f2c1922d3cb65`、
`run_record_id=00c7df0bd0778264ba49b4e6610b54bd`。

**⑧ 看进度**：轮询 `GET /api/runs/{id}`，采到的第一帧是

```
阶段=PREPARING 已推行数=0 批次序号=0 累计字节=0 已用时=0ms
```

随后就落终态：

```
outcome=SUCCEEDED  target_table_effect=SWAPPED
源端行数=5  暂存行数=5  sink 回报行数=5  批数=1
暂存表=V2_TRIAL__stg_20260820081300_079e64
```

**这里要说实话：进行中的帧只采到一帧**。五行数据在一秒的轮询间隔里就搬完了，
所以「阶段线一路推进」这件事本趟**没有**看到全程——看到的是 `PREPARING` 那一帧加上终态的全套计数。
要看长时间的进行中态，去处是 M2 走查的 V1/V16/V17（`M2_KEEP_RIG` 交出来的台架固定停在 `hang-streaming`）
与 M1 的 10 万行那一档（ADR-0041 增补 9(e2)）。本票不拿一帧冒充「看完了进度」。

**⑨ 核对目标库数据**：Oracle 那一天的五行与 MySQL 目标表逐值比对，**完全一致**：

```
1|alpha|1024.50|2026-08-20 00:00:00
2|bravo|0.01|2026-08-20 00:00:00
3|charlie|99999.99|2026-08-20 00:00:00
4|delta|250.00|2026-08-20 00:00:00
5|echo|3333.33|2026-08-20 00:00:00
```

**过滤生效的反面也判了**：目标表里 `LOAD_DATE <> '2026-08-20'` 的行数 = **0**——
`2026-08-19` 那两行没被搬过去。只数行数的话，「整表搬了一遍又恰好只有五行」这种巧合会被记成绿。

## 判据对账（#157 的六条）

| # | 判据 | 结果 |
|---|---|---|
| 1 | 从干净容器起步，全程只照手册，零即兴命令 | **达成**：两台主机由 `--reset` 推倒重建（R9 判过干净态），装机命令全部出自两份手册的回放；编排脚本自己一条装机命令都没有（`test-rehearsal-final.sh` 第 2 条按内容判） |
| 2 | 两端自检先红后绿：干净时缺项一次列全，装完全绿 | **达成**：目标端 `1/8`（逐条对期望表 9/9）→ `9/0` 退出码 0；源端 `2/6`（8/8）→ `8/0` 退出码 0 |
| 3 | 经隧道完成一次真实搬运：建任务、加过滤、发起、看进度、目标库核对无误 | **达成**：`SUCCEEDED` / `SWAPPED`，源端 5 行 = 暂存 5 行 = sink 回报 5 行 = 目标库 5 行，逐值一致，过滤外 0 行。**「看进度」只采到一帧**，如实记在第 7 段⑧ |
| 4 | 演练中的临场解决全部回写手册并重走过 | **达成（手册侧为空集）**：两台机器上没有一次「手册没写、临场解决」。撞到的两件事都在**编排脚本**那一侧，各自改完重走了一整趟——见下节 |
| 5 | 行李清单逐项核对无缺 | **达成**：九项 OK、第 3 与第 8 项按「不适用 + 为什么」记；缺一项就地停的门槛没被触发 |
| 6 | 演练实录进仓库，与两份手册同处一个文档区 | **达成**：本文件在 `docs/install/records/`，与两份手册同在 `docs/install/` 下；静态自检按 `git ls-files` 判它进没进版本库 |

## 演练撞到、当场处理的事

**手册那一侧零回写。** 两台机器上的每一条命令都出自手册、逐条走通，先红形状与期望表逐条对齐，
装完两边全绿。ADR-0041 §6 那条判据在手册这一侧一次都没破。

**编排脚本那一侧撞到两件，都改完重走了整趟**（判据 4 的兑现方式对编排脚本一视同仁）：

- **`jq` 的 `//` 把 `false` 也当「缺失」。** 轮询里写成 `.live // empty`，运行落终态那一刻
  （`live: false`）取回来的是空串，于是永远等不到那个 `false`——搬运明明成了
  （`SUCCEEDED`、5 行、242 字节，日志与目标库都对得上），脚本却轮满 300 次报「搬运卡住」。
  **成因先查了一圈产品侧才落到自己身上**，教训记在脚本注释里：取原值、三态各判各的。
- **两台 mac 抢同一个 rexec 队列。** 当天 `lmjdeMacBook-Pro.local` 与 `lmjMac-mini-Pro.local`
  都在轮询，任务落到哪台是随机的，排障时探针在两台之间来回弹，现场证据（网段、容器创建时间）
  一度自相矛盾。处置：编排脚本第 0 段打印 `hostname`，实录写明这一趟是谁跑的。

**演练台比真机宽松的三处**（都不是这一趟的缺口，是台架的边界，手册里各有 `⚠ 真机差异` 标记）：
Instant Client 走本地缓存（真机上「出发前下好」）、离线 rpm 那条路没走到、MySQL 三前提天然满足。

## 这一趟之外的两趟

判据是过程性的，**跑过几趟、每趟为什么重跑，本身就是记录的一部分**：

1. **07:33–07:44（`lmjdeMacBook-Pro.local`）**：第一趟整趟绿，与本趟结论一致。
   随后 `/code-review` 报出七条（行李清单第 3 项恒 OK、建表报错被吞、轮询把非 200 当终态、
   静态自检 `grep -c` 在 `set -e` 下静默掐断、实录只判存在没判进库、两个开关不互斥、README 混了工作目录），
   **脚本改了，那份实录记的就不是仓库里这版脚本跑出来的东西**，作废重走。
2. **07:57–08:05（`lmjMac-mini-Pro.local`）**：改后第一次重走，栽在上面那个 `jq //` 上，
   装机与搬运其实全绿、只是脚本判不出终态。修掉后重走第三趟，即本文件。

## 没做的那几档，以及为什么

- **真机上才第一次成立的两段**：systemd unit（四个 `enable --now`、重启后还在）与 SELinux、firewalld。
  容器里没有 systemd、没有 firewalld、`getenforce` 也不在——这三样演练台上验不到，
  两份手册里逐条标了「真机差异」。ADR-0041 增补 1 明文接受这个代价。
- **长时间的进行中态**：见第 7 段⑧，去处是 M2 走查与 M1 的 10 万行档，本票不重做。
- **三份视觉走查**：本票零 UI 改动（改的是 `docs/`、ADR、台架脚本与 fixture），`web/` 与
  `docs/design-system/` 一个字节没动，按 `CLAUDE.md` 通则 3 记「未跑」，理由无触发；
  封存点见 ADR-0040 §6.1/§6.2 与 v1 的 `v1-visual-walkthrough-20260819T173957Z.md`。

## 四份既有台架 / 语言侧

- `npm run typecheck`：PASS；`npm test`：**96 tests / 7 files** 全过；`cargo test --workspace`：全过
  （本票零代码改动，改的是 `docs/`、ADR、台架脚本与 fixture；这一趟只为确认没有意外触碰）。
- 四份既有台架（M1/M2/M3/v1）**一个字节没动**，本票零搬运语义改动，未重跑。
- 六支静态自检（source-install / target-install / **final** / tunnel / preflight / topology）全 PASS。
- 构建产物自带三条判据：glibc 动态链接、无未解析依赖、干净 `centos:7` 上启动无 GLIBC 错误——全过。

## 怎么复现

```bash
cd docs/spikes/fixtures/local-rig
./scripts/up.sh                                                  # 两个库
../../../../packaging/centos7/build.sh --platform linux/amd64    # 三个二进制
./scripts/rehearsal-final.sh                                     # 本文件这一趟
```

**构建与演练要在同一个 rexec 任务里**：`packaging/centos7/out/` 只在根 `.gitignore` 里以锚定式出现，
分成两个任务时中途那次 `rsync --delete` 会把二进制抹掉（#156 实录里记着这一笔）。
跑完**不清场**，两台主机、隧道与搬完的目标表都留着，本文件就是照着它们抄的。
