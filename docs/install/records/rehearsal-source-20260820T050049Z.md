# 源端装机演练实录 —— 2026-08-20T05:00:49Z

**票**：[#155](https://github.com/liumingjian/db-qbs/issues/155)
**手册**：[`../source-centos7.md`](../source-centos7.md)
**回放脚本**：`docs/spikes/fixtures/local-rig/scripts/rehearsal-source-install.sh`
**判定来源**：[ADR-0041](../../adr/0041-v2-scope-trial-readiness.md) §6（判据是过程性的，落在演练记录里；
「手册没写、临场解决」算判据未达成——回写手册、重走）

## 这一趟跑在什么上面

| 面 | 实况 |
|---|---|
| 源端主机 | `qbs-host-source`，`centos:7` / `linux/amd64`，`rehearsal-reset.sh` 推倒重建出来的干净容器 |
| 架构 / glibc | `x86_64` / `2.17`（R0a/R0c 那一笔账，#151 的构建目标） |
| Oracle | `172.20.0.2:1521/XE`（Oracle 在 `qbs-src-side` 上的 IP。按 IP 取，不按容器名——`oracle` 这个名字解析到的是被切断的 default 网那个 IP） |
| 目标端 | `qbs-host-target`：**真 sink**（`db-qbs-sink`，#151 的 x86_64 产物，只绑 `127.0.0.1:8080`）+ stunnel 服务端在白名单口 `15443` |
| 「公网」一跳 | `host.docker.internal:15443`（真机上是客户给的公网 IP） |
| 二进制 | `packaging/centos7/build.sh --platform linux/amd64` 当趟产出，GLIBC 符号上界 2.16 |
| 拓扑前置 | 同日 `rehearsal-topology-check.sh --reset`：前置 R0 **4/4**、拓扑 **24/24**（先跑拓扑、后装隧道，ADR-0041 增补 6(d)） |

**目标端不是照 #156 的手册装的**——它由台架脚本
`rehearsal-tunnel-up.sh --side target --sink real` 准备，本票只把它当作**对端事实**。
源端那一头的每一条命令都出自手册。

## 逐步实测

### 第 1 步 上机第一件事：自检先红

`PASS=2 FAIL=6`，退出码 1。**逐条与手册第 1 步那张期望表对齐，8/8 一致**：

```
S1 PASS 2.17                      S5 PASS Oracle 172.20.0.2:1521 通
S2 FAIL /opt/oracle/instantclient 里没有
S3 FAIL 前提未满足（S2 先红）      S6 FAIL pid 文件指不到活进程
S4 FAIL 前提未满足（S2 先红）      S7 FAIL 隧道入口 127.0.0.1:8080 不通
                                   S8 FAIL 前提未满足（S7 先红）
```

**判的不是「一片红」**：干净的 `centos:7` 上 glibc 本来就是 2.17（S1 该绿），
Oracle 那一跳本来就通（S5 该绿）。一张全红的期望表会把「脚本恒红」这种假绿放进来
（#154 的 P1b 判过同一件事）。

### 第 2 步 换 yum 源

`vault.centos.org` + CERN + kernel.org 三源、`failovermethod=priority`、`gpgcheck=1`。
末尾 `yum -y makecache fast` 打出「yum 源就位」。

### 第 3 步 那几个包

```
libaio-0.3.109-13.el7.x86_64   stunnel-4.56-6.el7.x86_64
openssl-1.0.2k-26.el7_9.x86_64 unzip-6.0-24.el7_9.x86_64
curl-7.29.0-59.el7_9.2.x86_64   iproute-4.11.0-30.el7.x86_64
```

### 第 4 步 Instant Client 19c

解包 → 软链 → **注册进 `ldconfig`**：

```
/opt/oracle/instantclient/libclntsh.so -> libclntsh.so.19.1
libclntsh.so.19.1  83706256 字节
ldconfig -p | grep -c libclntsh.so   → 2
ldd libclntsh.so | grep -c 'not found' → 0
```

### 第 5 步 两个二进制

`db-qbs-source` 不带 `--config` 起一次：

```
{"level":"error","event":"source_config_failed","message":"用法：db-qbs-source --config <source.toml>"}
exit=1
```

**这就是「它跑得起来」的证据**——进程已经进到自己的参数校验里了，
不是 `GLIBC_2.xx not found`，也不是 `Exec format error`。

### 第 6 步 stunnel 客户端

占位符填完（`accept = 127.0.0.1:8080` / `connect = host.docker.internal:15443`），
残留检查打「占位符已填完」，两端 `target.crt` 的 SHA-256 指纹**一字不差**：

```
源端   SHA256 Fingerprint=B2:8F:0E:2B:1B:EE:43:FF:53:1C:F7:79:D4:10:A0:BC:E5:0B:8D:1C:FD:53:D9:9A:BE:B8:13:A3:6F:6C:D7:1F
目标端 SHA256 Fingerprint=B2:8F:0E:2B:1B:EE:43:FF:53:1C:F7:79:D4:10:A0:BC:E5:0B:8D:1C:FD:53:D9:9A:BE:B8:13:A3:6F:6C:D7:1F
```

起完 pid 文件 `447`。

### 第 7–8 步 `source.toml` + 起 source

四行配置照手册写完（`0600`），起：

```
{"level":"info","event":"source_started","listen":"127.0.0.1:8088","message":"source 长驻编排进程已启动"}
```

### 第 9 步 自检全绿

```
S1 PASS 2.17
S2 PASS /opt/oracle/instantclient/libclntsh.so
S3 PASS x86_64
S4 PASS 无缺失
S5 PASS Oracle 监听口 172.20.0.2:1521 可达
S6 PASS pid=447
S7 PASS 隧道入口 127.0.0.1:8080 在听
S8 PASS 经隧道摸得到目标端的 sink —— sink 应答 RUN_UNKNOWN
==== 源端自检：PASS=8 FAIL=0 ====   退出码 0
```

**S8 是本票判据 2 的兑现点**：`RUN_UNKNOWN` 是产品自己的错误码，
「有人应答」不算——那头必须真是 sink。桩 sink 回不出这个码，所以本趟目标端用的是真 `db-qbs-sink`。

### 第 10 步 连上 Oracle

界面「测试连接」的等价命令（走的是 ADR-0037 §9 那条产品自己的 Oracle 连接路径）：

```
{"elapsed_ms":294,"label":"//172.20.0.2:1521/XE","ok":true}
```

## 演练撞到、当场回写手册的三件事

**ADR-0041 §6 的原话是「中途任何一次『手册没写、临场解决』都算判据未达成」。**
下面三件每一件都触发了「回写手册 → 从干净容器重走」，上面那份实录是重走出来的。

### 1. `ldconfig` 那一步手册里没有（本票最重要的一条发现）

第一趟：自检 **S1–S8 全绿**，紧接着第 10 步的测试连接当场炸：

```
DPI-1047: Cannot locate a 64-bit Oracle Client library:
"libnnz19.so: cannot open shared object file: No such file or directory"
```

成因：ODPI-C 按全路径 `dlopen` 的只有 `libclntsh.so` 一个，它自己还要拉起同包的
`libnnz19.so` / `libclntshcore.so.19.1`，那几个由**动态链接器按它自己的搜索路径**去找。
只解包不 `ldconfig`，产品就找不到。

**这同时破了 #154 那条判据**（「自检说 OK 之后，现场不该再出现环境类失败」）：
`preflight-source.sh` 的 S4 原先在查之前先把 Instant Client 目录塞进 `LD_LIBRARY_PATH`，
**查的不是产品会遇到的那条搜索路径**，所以它绿得理直气壮。

两处一起改：

- 手册第 4 步补 `ld.so.conf.d` + `ldconfig`，并把「为什么单解包不够」写在旁边；
- `preflight-source.sh` 的 S4 改成**按产品自己的搜索路径判**；缺的那几个若「加上
  Instant Client 目录就都在」，处置直接给 `ldconfig` 那两条命令，与「包没装」区分开。

**三条负对照**（否则等于闭着眼睛改），都跑在装完的那台机器上：

- **A：只撤 `ldconfig`。** S4 当场红，且只报 `ldconfig` 那一条成因：
  ```
  S4 FAIL 缺=libclntshcore.so.19.1,libnnz19.so（加上 /opt/oracle/instantclient 就都在）
       └ 处置：Instant Client 目录没进动态链接器的搜索路径：
          echo /opt/oracle/instantclient > /etc/ld.so.conf.d/oracle-instantclient.conf && ldconfig
  ```
- **B：再卸掉 `libaio`——两种成因同时成立。** 必须**一次列全**（脚本头纪律 1），
  不许只报一条让人清完再撞下一条：
  ```
  S4 FAIL 缺=libaio.so.1；另有 libclntshcore.so.19.1,libnnz19.so 加上 /opt/oracle/instantclient 就都在
       └ 处置：两条都要做：…yum install libaio…；以及 …ld.so.conf.d… && ldconfig
  ```
- **C：环境里 `export LD_LIBRARY_PATH=/opt/oracle/instantclient`，只撤 `ldconfig`。**
  S4 **仍然红**，与 A 一字不差。这条是评审逼出来的：把 `LD_LIBRARY_PATH` 写进 root 的 profile
  是这类机器上最常见的习惯，而 systemd 拉起来的 `db-qbs-source` 不继承 profile——
  「本脚本不加」不够，得 `env -u LD_LIBRARY_PATH` **显式抹掉继承来的那一份**。

三条之后复原，回到 8/0。

> **顺带记一条不该被误读的观察**：负对照那一刻，**已经在跑的** source 进程测试连接仍回
> `ok:true`——ODPI-C 一个进程只初始化一次，库早就加载进地址空间了。
> 也就是说这条故障只在**新进程第一次连库**时现形。自检比运行中的进程更严格，这是对的。

### 2. `ss` 在干净的 CentOS 7 上压根不在

手册的收尾核对与真机差异 ⑦ 都写着 `ss -ltnp`，而干净的 `centos:7` 上
`ss: command not found`（`getenforce` 同样不在，但那一条本来就标着「真机差异」）。
真机上 `iproute` 多半已经装着，**「多半已经装着」正是这份手册不许依赖的东西**。
改法：`iproute` 进第 3 步的包清单；回放脚本把收尾核对那两条也真跑一遍，
免得下次又只在正文里写、没人执行。重走后：

```
LISTEN 0 128 127.0.0.1:8088  users:(("db-qbs-source",pid=511,fd=14))
LISTEN 0 128 127.0.0.1:8080  users:(("stunnel",pid=451,fd=17))
-rw------- 1 root root  154 /etc/db-qbs/source.toml
-rw------- 1 root root 1704 /etc/stunnel/db-qbs/source.key
```

两个口都只在 `127.0.0.1` 上，配置与私钥都是 `0600`。

### 3. 回放脚本自己的两个坑（不影响手册，但会吞掉判据）

- **`docker exec` 不带 `-i` 时不接 stdin**，`bash -s <<'SH'` 读到的是空——第 2 步整段静默跳过，
  退出码还是 0。总账因此绿了一条根本没发生的事。改法：加 `-i`，并且第 2 步的判据
  **事后核对 repo 文件在、`yum repolist` 用得了**，不只看那段脚本的退出码。
- **`/proc/<pid>/cmdline` 的分隔符是 NUL 不是空格**，`rehearsal-tunnel-up.sh` 里
  `kill_by_marker` 按 `stunnel /etc/stunnel/db-qbs` 这个带空格的串 grep，一条都匹配不上——
  「重复跑会先把上一轮收掉」这句自述从来没成立过。第二趟起隧道时撞到
  `bind: Address already in use (98)`。改法：先 `tr '\0' ' '` 再匹配，并补一道等端口空出来。

## 判据对账（#155 的四条）

| # | 判据 | 结果 |
|---|---|---|
| 1 | 照手册在干净源端主机容器上从零装完，源端自检**从红转全绿** | **达成**：先红 `2/6` 且逐条对齐期望表 → 装完 `8/0`、退出码 0 |
| 2 | source 启动、连上 Oracle，经隧道摸得到目标端 | **达成**：`source_started`；测试连接 `ok:true`（产品自己的连接路径）；S8 `sink 应答 RUN_UNKNOWN` |
| 3 | 真机差异点逐处显式标出 | **达成**：手册十处 `⚠ 真机差异` 并按出现顺序汇总成表（yum 源已在 / 不出网 / 包已装 / 目标端公网地址 / systemd(stunnel) / SELinux / 8080 被占 / 防火墙 / 界面走 SSH 转发 / systemd(source)） |
| 4 | 行李清单建立并被手册开头引用；手册进仓库、与实录同处一个文档区 | **达成**：`packaging/PACKING-LIST.md`（#154 那一趟建的，本票补齐源端第 3 项的直链与 `ldconfig` 相关项）；手册在 `docs/install/`，本实录在 `docs/install/records/` |

## 这一趟之后手册又动过的地方

实录落下来之后还改了两轮，据实记在这儿：

1. **说明性补写**（不含要敲的命令）：第 9 步补了「S1–S8 各查什么、装在第几步」的对照表；
   第 6 步补了两条真机差异（④ 目标端地址、⑧ 防火墙）；十处标记按正文出现顺序重编了号
   （原先汇总表里的编号与正文对不上）。
2. **一处可执行改动**：第 3 步的包清单加了 `iproute`（成因见上面第 2 条），
   回放脚本同步改并把收尾核对那两条也真跑了一遍。**这一处不是纸面改动，所以又重走了一趟**
   ——见下面「复核二」。
3. **代码评审逼出的四处**（`/code-review high`）：S4 改成 `env -u LD_LIBRARY_PATH`（负对照 C）、
   两种成因一次列全（负对照 B）、第二趟 `ldd` 的退出码也收；行李清单第 8 项的离线 rpm
   从两个补成六个（`unzip` 没带的话现场连 Instant Client 都解不开）；回放脚本删掉了
   `--keep-dist`（它在装过一半的机器上必然判假：第 1 步的先红形状不成立，S6/S7 还会对着
   上一轮的老进程判绿）；`rehearsal-tunnel-up.sh` 的 `--help` 区间跟着加长的注释改回来。
   改完从干净容器重走——见「复核三」。**两处都不含任何要敲的命令**，
上面那份实录逐条对应的执行序列一个字没变；末尾「复核」那一节是补写之后从干净容器重走的那一趟。

## 复核（手册补写之后从干净容器重走）

- **复核一**（补上「S1–S8 各查什么」与两条真机差异之后）：总账六条全 PASS，自检 `8/0`、退出码 0，
  测试连接 `{"elapsed_ms":248,"label":"//172.20.0.2:1521/XE","ok":true}`。
  同一趟里四支静态自检（source-install / tunnel / topology / preflight）与分档判据 `C1–C9 9/9` 全过。
- **复核二**（补上 `iproute` 与收尾核对之后，容器内时间戳 `05:17`）：总账六条全 PASS，
  收尾核对的三条实测见上面第 2 条。
- **复核三**（代码评审的四处改完之后，容器内时间戳 `05:27`）：四支静态自检全过，
  总账六条全 PASS，`ss -ltnp` 两个口仍只在 `127.0.0.1`、配置与私钥仍是 `0600`，
  三条 S4 负对照如上。**这一趟是本实录对应的最终状态。**

## 没做的那几档，以及为什么

- **真机上才第一次成立的两段**：systemd unit（`db-qbs-stunnel` / `db-qbs-source` 的
  `enable --now`、重启后还在）与 SELinux、firewalld。容器里没有 systemd、没有 firewalld、
  `getenforce` 也不在——**这三样演练台上验不到**，手册里逐条标了「真机差异」。
  ADR-0041 增补 1 明文接受这个代价：装的人就是写手册的人本人。
- **搬通一次真实搬运**（规格 #149 User Story 14）不在本票，那是两端都装完之后的事（#157）。
  本票到「源端自检全绿 + 连得上 Oracle + 经隧道摸得到 sink」为止。
- **`T0–T11` 本趟没跑**：目标端落点换成真 sink 之后，`rehearsal-tunnel-check.sh` 的
  `T3/T5/T7` 按桩 sink 的标记 `QBS-TUNNEL-OK` 判，会红在标记上而不是隧道上。
  隧道那一段的加密取证是 #153 的账，实录在
  `docs/spikes/fixtures/local-rig/rehearsal-tunnel-20260820T022000Z.md`（已退役，见 git 历史），本票不重做。
- **`P0–P11` 因为动了 `preflight-source.sh` 而重跑**：`--phase both` **13/13**，
  `C1–C9` **9/9**，两支静态自检 PASS。
- **三份视觉走查**：本票零 UI 改动（改的是 `docs/`、`packaging/preflight/`、
  `packaging/stunnel/`、台架脚本），`web/` 与 `docs/design-system/` 一个字节没动，
  按 `CLAUDE.md` 通则 3 记「未跑」。
