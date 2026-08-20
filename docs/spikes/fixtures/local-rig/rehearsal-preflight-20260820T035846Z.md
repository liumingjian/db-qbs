# 两端环境自检实录（#154）

- **时间**：2026-08-20 03:58 UTC
- **机器**：mac Docker（rexec 派发），Docker 29.7.2
- **台架**：`docs/spikes/fixtures/local-rig/`，两台 `centos:7`（`linux/amd64`）主机容器
- **被判的东西**：`packaging/preflight/preflight-source.sh` / `preflight-target.sh`
- **判据脚本**：`scripts/test-preflight-classify.sh`（C1–C9）、`scripts/rehearsal-preflight-check.sh --phase both`（P0–P11）

**这一趟跑在一套从零重建的台架上**：做到一半 mac 的 Docker Desktop 升级（29.3.1 → 29.7.2）
把容器与镜像全清了，Oracle XE 镜像、`centos:7`、client 镜像重新拉/重新建、Oracle 重新初始化
（`up.sh` + `rehearsal-up.sh` 共 203 秒），然后重跑。对判据来说这是加分：干净得不能再干净。

## 结果

| 判据 | 结果 |
|---|---|
| C1–C9 目标端自检按 sink 的回答分档 | **9/9 PASS** |
| P0–P11 干净容器先红 + 装隧道后转绿 | **13/13 PASS** |
| T0–T11 隧道判据（#153，确认本票改动没碰坏它） | **17/17 PASS** |
| `npm run typecheck` / `npm test` | PASS（96 tests） |
| `cargo test --workspace` | PASS |

四条票面判据的落点：

| 票面判据 | 判在哪 | 结果 |
|---|---|---|
| 1. 干净源端容器上跑源端自检，缺项一次列全（先红），逐项可按输出处置 | P1a–P1d | PASS |
| 2. 干净目标端容器上跑目标端自检，缺项一次列全（先红） | P5a–P5d | PASS |
| 3. 检查项覆盖规格 #149 D.14 的两端清单，不缺项 | `test-rehearsal-preflight.sh` 第 5 条（按 D.14 原话逐条对） | PASS |
| 4. 两个脚本进仓库 | 同上第 1 条（`git ls-files`）与第 10 条（列进行李清单） | PASS |

## 「先红」不是「全红」

干净的 `centos:7` 上 glibc 就是 2.17，演练台上两个库也确实够得着。所以判据对的是一张**期望表**，
不是「一片红」——一张全红的表会把「脚本恒红」这种假绿放进来。实测：

```
源端   S1=PASS S2=FAIL S3=FAIL S4=FAIL S5=PASS S6=FAIL S7=FAIL S8=FAIL
目标端 D1=PASS D2=FAIL D3=FAIL D4=FAIL D5=FAIL D6=FAIL D7=FAIL D8=FAIL D9=FAIL
```

装上隧道（`rehearsal-tunnel-up.sh`，一个字不改地复用 #153 那支）之后：

```
源端   S6=PASS S7=PASS S8=FAIL    ← 该转绿的转绿了；S8 仍红，理由见下
目标端 D8=PASS D9=PASS D2=FAIL
```

**S8 / D2 仍红是对的**：#153 的桩 sink 不是真 sink，而自检判的是「那头**真是 sink**
（回 `RUN_UNKNOWN` 那个错误码）」而不是「有人应答」。P11 把这个理由本身也判了一道——
退化成端口探活的话，隧道通到别的服务上会假绿。真 sink 装上来是 #156 的事。

## 台架够不着的那一档：C1–C9

目标端三项开连接仪式前提是**问 sink 要的**，按它的报错措辞分档。真 sink 要等 #156 才装得上来，
在那之前除「连不上」外每一档在演练台上一次都没被走过——没走过的分支就是没有的分支。
`preflight-sink-stub.py` 把每一档的原话（抄自 `crates/sink/src/mysql_destination.rs`）喂进去：

| 档 | 期望 | 实测 |
|---|---|---|
| C1 全通 | D4–D7 全 PASS | 一致 |
| C2 连不上 | 三项前提一律未判定 | 一致 |
| C3 卡在 `SET NAMES` | D5 FAIL，后两项未判定 | 一致 |
| C4 卡在 `sql_mode` | D6 FAIL，另两项未判定 | 一致 |
| C5 回读失败 | 三项一律未判定 | 一致 |
| C6 三项都不合格 | 一次列全 | 一致 |
| C7 只有 packet 不合格 | 只红一条 | 一致 |
| C8 只有 sql_mode 不合格 | 只红一条 | 一致 |
| C9 认不出的回答 | 四条一律未判定 | 一致 |

**仪式是有先后的**，而三条判词说的都是「回读回来是这个值」——所以**只有回读跑完那一档
才产生得了 PASS**：仪式在回读之前停下时，卡住的那一步记 FAIL、其余记未判定
（「设过了」不等于「就是这个值」，中间层改写会话变量正是产品那道回读要防的事）。
把未判定记成 PASS 是这里最危险的假绿——自检替环境作了一个它没验过的保证，
而票面判据正是「自检说 OK 之后不该再出现环境类失败」。C3–C5 判的就是它，
C9 判的是同一件事的另一面（认不出的回答也不许算合格）。

## 本次实跑抓到的两件事

1. **`$var` 紧挨着中文，在 mac 的 bash 3.2 上当场炸**（`name?: unbound variable`）。
   炸的是判据脚本自己，等于把整份判据吞掉。全部改成 `${var}`，并把这条不成文的规矩
   变成门禁（`test-rehearsal-preflight.sh` 第 11 条）。
2. **`vault.centos.org` 前面那层 CDN 开始对 `*.sqlite.bz2` 回 403**（`*.xml.gz` 照常 200），
   而 yum 优先取 sqlite 元数据——当天上午还装得上，下午起 `rehearsal-tunnel-up.sh` 的
   `yum -y install stunnel` 一律失败。改成 vault + CERN + kernel.org 三源
   加 `failovermethod=priority`（存档内容冻结，镜像之间不会各说各话）。
   **这条本来会在 #157 的终局演练上炸**，而现场那台机器多半连不上任何一个源——
   行李清单第 8 项（离线 rpm）就是为这个存在的。
   **留一笔账**：`packaging/centos7/Dockerfile` 那份换法还是单源，下次构建镜像会撞上同一个 403。
   没在本票里一并改，是因为它的 `VAULT_BASE` 按架构分（aarch64 在 altarch 下），
   后备镜像的路径也得跟着分，而那要连着重建一次镜像才算数——归 #151 / #157。

## 一轮代码评审后改掉的六处

评审（`/code-review high`）报了 6 条，全部改掉，改完在重建过的台架上重跑（就是本实录这一趟）：

1. **S5 的 Oracle 地址不许猜**：`oracle_connect_string` 按 ADR-0037 §10 已退役，
   运维照做删掉之后自检原本会静默退回 `127.0.0.1:1521`——本机恰好有个 1521 在听就是假绿。
   现在读不到地址就记「未判定」，处置指向 `QBS_ORACLE_HOST`。
2. **D3 先判 `listen` 本身**：目标端是双网卡，`listen = "10.0.0.5:8080"` 而 hostname
   解析到另一张网卡时，反向探针「不通」会为一个绑在可路由地址上的无鉴权 sink 判绿。
3. **S2 要求软链解得开、S4 收 `ldd` 的退出码**：悬空软链原本 S2 判绿、S4 也判绿
   （`ldd` 失败时 stdout 是空的），等于告诉运维「依赖都齐了」。
4. **D5–D7 只有回读跑完才产生 PASS**（见上一节；C4/C5 的期望随之改。这一条是顺着评审
   第 4 条把口径拉齐的——原本 `SET` 过了就记 PASS，同样是一个没验过的保证）。
5. **`"ok":true` 容空格**：与同段截 message 的 sed 一致，中间层重排响应不该判成红。
6. **`failovermethod=priority`**：yum 默认 roundrobin 是随机挑起点，
   「vault 优先」原本只是句愿望。

另外把 mac bash 3.2 那道门禁的覆盖面补到 `test-preflight-classify.sh` 与它自己
（补的时候它先把自己判红了一次——报错句里那个字面量正好长成被禁的形状，已改写）。

## 三份视觉走查：**未跑**

本票零 UI 改动——动的是 `packaging/`、台架脚本与文档，`web/` 与
`docs/design-system/` 一个字节没碰，`DiagnosticTable`、`.precheck-reports`、
数据源屏 / 构建器 / 运行历史全部未触及。按 `CLAUDE.md` Visual gates 通则 3，
V1–V25、W1–W6、X1–X9 **三份都未跑**，理由是无触发；封存点见 ADR-0040 §6.1/§6.2 与
v1 的 `v1-visual-walkthrough-20260819T173957Z.md`。

## 自检证不了的那一档（明写，别当它已经保过了）

- **Oracle 的账号 / 口令 / 服务名**：Instant Client Basic 包不带 `sqlplus`，本票也不给客户机加装它。
  S5 只判监听口可达，且地址取不到时记「未判定」而不是猜一个。
  那一档装完 source 之后用界面的「测试连接」证一次（ADR-0037 §9）。
- **隧道的加密与认人**：自检只判「通不通、那头是不是 sink」。加密取证是 T6–T8 的事。

## 原样输出

C1–C9、P0–P11、T0–T11 一趟到底（`--phase both`，容器从删除重建开始）。

```
==> C1–C9 目标端自检按 sink 的回答分档
  C1   PASS  全通                                   实测=D2=PASS D4=PASS D5=PASS D6=PASS D7=PASS
  C2   PASS  连不上：三项前提一律未判定  实测=D2=PASS D4=FAIL D5=FAIL D6=FAIL D7=FAIL
  C3   PASS  卡在 SET NAMES：后两项未判定    实测=D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL
  C4   PASS  卡在 sql_mode：另两项未判定     实测=D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL
  C5   PASS  回读失败：三项一律未判定     实测=D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL
  C6   PASS  三项都不合格，一次列全        实测=D2=PASS D4=PASS D5=FAIL D6=FAIL D7=FAIL
  C7   PASS  只有 packet 不合格                  实测=D2=PASS D4=PASS D5=PASS D6=PASS D7=FAIL
  C8   PASS  只有 sql_mode 不合格                实测=D2=PASS D4=PASS D5=PASS D6=FAIL D7=PASS
  C9   PASS  认不出的回答：不许当成合格  实测=D2=PASS D4=FAIL D5=FAIL D6=FAIL D7=FAIL

==== 分档判据：PASS=9 FAIL=0 ====
===SPLIT===
==> 前置：两台主机在跑（在此之前一切判据都不成立）
  P0a   PASS  源端主机 qbs-host-source                         实测=running
  P0b   PASS  目标端主机 qbs-host-target                      实测=running
==> --phase both：先把两台主机推回干净态（这一步是破坏性的，装过的东西全没）
 Container qbs-host-source Stopping 
 Container qbs-host-target Stopping 
 Container qbs-host-target Stopped 
 Container qbs-host-source Stopped 
 Container qbs-host-target Removing 
 Container qbs-host-source Removing 
 Container qbs-host-source Removed 
 Container qbs-host-target Removed 
 Image centos:7 Pulling 
 Image centos:7 Pulling 
 Image centos:7 Pulled 
 Image centos:7 Pulled 
 Container qbs-mysql8 Running 
 Container qbs-oracle11 Running 
 Container qbs-host-source Creating 
 Container qbs-host-target Creating 
 Container qbs-host-source Created 
 Container qbs-host-target Created 
 Container qbs-oracle11 Waiting 
 Container qbs-mysql8 Waiting 
 Container qbs-oracle11 Healthy 
 Container qbs-mysql8 Healthy 
 Container qbs-host-target Starting 
 Container qbs-host-source Starting 
 Container qbs-host-source Started 
 Container qbs-host-target Started 
==> P1–P4 干净源端容器：缺项一次列全（先红）
    │ ==> 没找到 source.toml（还没装到这一步是正常的）；缺的值走环境变量与默认值
    │     Instant Client=/opt/oracle/instantclient   Oracle=oracle:1521   sink=http://127.0.0.1:8080
    │ 
    │ ==> S1 glibc 版本（客户机是 CentOS 7，二进制按 glibc 2.17 编）
    │   S1   PASS  glibc 版本 ≥ 2.17                          实测=2.17
    │ ==> S2–S4 Instant Client（ODPI-C 是运行时 dlopen 它的，缺什么只在连库那一刻才炸）
    │   S2   FAIL  Instant Client 目录里有 libclntsh.so 且读得到 实测=/opt/oracle/instantclient 里没有
    │        └ 处置：把 Instant Client 19c Basic 包解到 /opt/oracle/instantclient（行李清单里带着），并让 source.toml 的 oracle_client_lib_dir 指向它
    │   S3   FAIL  Instant Client 架构与本机一致           实测=前提未满足（S2 先红）
    │        └ 处置：先按 S2 处置
    │   S4   FAIL  Instant Client 的动态依赖全解析得开  实测=前提未满足（S2 先红）
    │        └ 处置：先按 S2 处置
    │ ==> S5 Oracle 连通
    │   S5   PASS  Oracle 监听口 oracle:1521 可达            实测=通
    │        注：账号 / 口令 / 服务名对不对不在本项内，装完 source 后用界面的「测试连接」证一次
    │ ==> S6–S8 隧道（stunnel 客户端 → 目标端的 sink）
    │   S6   FAIL  stunnel 客户端进程在跑                  实测=/var/run/db-qbs-stunnel-sink.pid 指不到活进程
    │        └ 处置：配置还没铺：照 packaging/stunnel/README.md 把 source-side/ 那套装到 /etc/stunnel/db-qbs/stunnel-sink.conf
    │   S7   FAIL  隧道入口 127.0.0.1:8080 在听             实测=不通
    │        └ 处置：stunnel 客户端的 accept 口没起来或端口对不上；核对 /etc/stunnel/db-qbs/stunnel-sink.conf 的 accept 与 source.toml 的 sink_base_url
    │   S8   FAIL  经隧道摸得到目标端的 sink            实测=前提未满足（S7 先红）
    │        └ 处置：先按 S7 处置
    │ 
    │ ==== 源端自检：PASS=2 FAIL=6 ====
    │ 上面每条 FAIL 各带一行处置，逐条清完再重跑本脚本；不要装到一半再来。
  P1a   PASS  源端自检：检查项一次列全（编号全集，按出现顺序） 实测=S1 S2 S3 S4 S5 S6 S7 S8
  P1b   PASS  源端自检：逐条判定与期望表一致        实测=S1=PASS S2=FAIL S3=FAIL S4=FAIL S5=PASS S6=FAIL S7=FAIL S8=FAIL
  P1c   PASS  源端自检：有 FAIL 时退出码为 1            实测=1
  P1d   PASS  源端自检：每条 FAIL 都带一行处置（FAIL=6） 实测=6
==> P5–P8 干净目标端容器：缺项一次列全（先红）
    │ ==> 没找到 sink.toml（还没装到这一步是正常的）；缺的值走环境变量与默认值
    │     sink=127.0.0.1:8080   MySQL=mysql:3306   库=qbs   本机地址=172.21.0.3
    │ 
    │ ==> D1 目标库这一跳
    │   D1   PASS  MySQL 监听口 mysql:3306 可达              实测=通
    │ ==> D2–D3 sink 起在回环（ADR-0024：sink 不做鉴权，靠只绑回环兜底）
    │   D2   FAIL  sink 在 127.0.0.1:8080 应答                 实测=没有应答
    │        └ 处置：sink 没起：装好后 db-qbs-sink --config /etc/db-qbs/sink.toml，起完再跑本脚本
    │   D3   FAIL  sink 没越出回环                           实测=前提未满足（D2 先红，摸不到不算证据）
    │        └ 处置：先按 D2 处置
    │ ==> D4–D7 经 sink 开连接仪式（与搬运那条链同一个驱动、同一套会话设置）
    │   D4   FAIL  sink 用给定凭据连得上目标库         实测=未判定（sink 没应答（D2 先红））
    │        └ 处置：先按 D2 处置，sink 起来之后这四条才判得了
    │   D5   FAIL  会话字符集三项都是 utf8mb4            实测=未判定（sink 没应答（D2 先红））
    │        └ 处置：先按 D2 处置，sink 起来之后这四条才判得了
    │   D6   FAIL  sql_mode 设得成 STRICT_ALL_TABLES           实测=未判定（sink 没应答（D2 先红））
    │        └ 处置：先按 D2 处置，sink 起来之后这四条才判得了
    │   D7   FAIL  max_allowed_packet ≥ 64 MiB                  实测=未判定（sink 没应答（D2 先红））
    │        └ 处置：先按 D2 处置，sink 起来之后这四条才判得了
    │ ==> D8–D9 stunnel 服务端（公网上露出来的只有这一个口）
    │   D8   FAIL  stunnel 服务端进程在跑                  实测=/var/run/db-qbs-stunnel-sink.pid 指不到活进程
    │        └ 处置：配置还没铺：照 packaging/stunnel/README.md 把 target-side/ 那套装到 /etc/stunnel/db-qbs/stunnel-sink.conf
    │   D9   FAIL  白名单口在听                             实测=/etc/stunnel/db-qbs/stunnel-sink.conf 里读不到 accept 端口
    │        └ 处置：先按 D8 处置（配置铺好、占位符填完）
    │ 
    │ ==== 目标端自检：PASS=1 FAIL=8 ====
    │ 上面每条 FAIL 各带一行处置，逐条清完再重跑本脚本；不要装到一半再来。
  P5a   PASS  目标端自检：检查项一次列全（编号全集，按出现顺序） 实测=D1 D2 D3 D4 D5 D6 D7 D8 D9
  P5b   PASS  目标端自检：逐条判定与期望表一致     实测=D1=PASS D2=FAIL D3=FAIL D4=FAIL D5=FAIL D6=FAIL D7=FAIL D8=FAIL D9=FAIL
  P5c   PASS  目标端自检：有 FAIL 时退出码为 1         实测=1
  P5d   PASS  目标端自检：每条 FAIL 都带一行处置（FAIL=8） 实测=8
==> 装隧道（#153 的 rehearsal-tunnel-up.sh，一个字不改地复用）
==> P9–P11 装上隧道之后：该转绿的转绿，桩 sink 仍不算 sink
  P9    PASS  源端：stunnel 客户端与隧道入口转绿（先红不是写死的） 实测=S6=PASS S7=PASS S8=FAIL
  P10   PASS  目标端：stunnel 服务端与白名单口转绿   实测=D8=PASS D9=PASS D2=FAIL
  P11   PASS  S8/D2 仍红的理由是「应答的不是 sink」（桩 sink 不是真 sink） 实测=应答不是sink

==== 两端自检判据：PASS=13 FAIL=0 ====
===SPLIT===
==> 前置：两台主机在跑（在此之前一切判据都不成立）
  T0a   PASS  源端主机 qbs-host-source                               实测=running
  T0b   PASS  目标端主机 qbs-host-target                            实测=running
==> T1–T2 两端 stunnel 在位（pid 文件也验掉——真机的 systemd unit 靠它守进程）
  T1    PASS  源端 stunnel 客户端                                   实测=在跑
  T2    PASS  目标端 stunnel 服务端                                实测=在跑
    源端 src-side IP=172.20.0.3   目标端 dst-side IP=172.21.0.3
==> T3–T4 sink 只绑回环（ADR-0024 的兜底形态原样成立）
  T3    PASS  目标端自连 127.0.0.1:8080（回环上的服务活着） 实测=QBS-TUNNEL-OK
  T4    PASS  目标端经自己的 172.21.0.3:8080（回环之外摸不到 sink） 实测=不通
==> T5 主判据：源端经本机隧道口一跳到达目标端回环上的服务
  T5    PASS  源端 curl http://127.0.0.1:8080/（product 的 sink_base_url 原样） 实测=QBS-TUNNEL-OK
==> T6–T8 「公网」那一跳走的是加密流量，且认人
  T6    PASS  源端明文 HTTP 打宿主:15443（拿不到东西）     实测=无
  T6b   PASS  同一跳的对端首字节实测=空（对端未回字节） 实测=非明文
  T7    PASS  源端带客户端证书握手同一地址（T6/T6b/T8 的正对照） 实测=QBS-TUNNEL-OK
  T7b   PASS  对端证书 CN（钉的是这一张，不是任何公共 CA） 实测=db-qbs-target
  T7c   PASS  协商出来的协议（套件=ECDHE-RSA-AES256-GCM-SHA384） 实测=TLSv1.2
  T8    PASS  源端不带客户端证书握手（verify=2 双向认证在生效） 实测=无
==> T9–T11 露在外面的只有白名单那一个口
  T9    PASS  目标端容器对外发布的端口全集                 实测=15443/tcp
  T10a  PASS  目标端经自己的 172.21.0.3:15443 自连（T10 的正对照） 实测=通
  T10   PASS  源端按 IP 直达 172.21.0.3:15443（跨容器直达仍被切断） 实测=不通
  T11   PASS  源端经自己的 172.20.0.3:8080（隧道入口只绑回环） 实测=不通

==== 隧道判据：PASS=17 FAIL=0 ====
--- [本地 mac 执行] exit=0 耗时=92s ---
```
