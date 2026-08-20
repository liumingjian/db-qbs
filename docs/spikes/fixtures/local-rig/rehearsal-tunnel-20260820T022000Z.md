# stunnel 双端隧道实录（2026-08-20 02:20Z，#153）

- **驱动脚本**：`scripts/rehearsal-tunnel-up.sh` + `scripts/rehearsal-tunnel-check.sh`
  （连同两份配置模板一起进仓库——工具不进仓库的门禁，下一台机器会静默跳过；
  `CLAUDE.md` 视觉门禁通则 4 的同一条纪律）
- **落点**：mac Docker Desktop（rexec 派发），两台 `centos:7`（amd64）主机容器
- **起点**：`rehearsal-reset.sh` 推倒重建，**两台主机是刚出炉的干净机器**，
  stunnel 与证书全部在这一趟里从零装出来（`packaging/stunnel/out/` 也先删干净了）
- **结果**：拓扑判据 **PASS 19 / FAIL 0**；隧道判据 **PASS 17 / FAIL 0**；全程 115 秒
- **这一趟跑的是两轴 `/code-review` 改完之后的最终版文件**——评审逮到的五处（README 教了一条
  `sink` 上不存在的 `/healthz` 自检、配置注释与 systemd unit 互相矛盾、判据区间三处各说各话、
  源端填模板命令缺失、「零改动」门禁按分支 diff 判会在合入后变成空判）改完才重跑的

## 0. 这一趟证到了什么

票面四条判据，逐条对上实测：

| 票面判据 | 判在哪 | 实测 |
|---|---|---|
| stunnel 双端配置模板与装法说明进仓库 | `packaging/stunnel/`（模板 + `gen-certs.sh` + `README.md` 六步装法 + 两份 systemd unit） | 本趟就是照那六步跑的，`rehearsal-tunnel-up.sh` 是它的可执行回放 |
| 演练台上打通：源端经本机隧道端口访问，跨容器一跳到达目标端回环上的服务 | `T5`（主判据）、正对照 `T3` | 源端 `curl http://127.0.0.1:8080/` 取回 `QBS-TUNNEL-OK` |
| 目标端除隧道端口外不暴露服务；回环之外摸不到 sink | `T4` / `T9` / `T10` / `T11` | 目标端**对外发布**的端口全集 = `15443/tcp`（`T9`）；`sink` 在目标端主机的非回环面上摸不到（`T4`，按 `dst-side` IP 判）；跨容器直达 `15443` 仍不通（`T10`） |
| 产品代码零改动 | `scripts/test-rehearsal-tunnel.sh` 第 9 条（静态判，不靠台架） | `protocol.rs` 仍硬性拒绝非 `http`；两份 `*.toml.example` 的 `sink_base_url` / `listen` 仍是 `127.0.0.1:8080` |

「公网那一跳走的是加密流量」这一条由 `T6/T6b/T7/T7b/T7c/T8` 六条合起来判，见下面第 4 节。

**第三条的边界说清楚**：`T9` 判的是**宿主发布面**（`docker port`），它证不了「目标端主机上除
`15443` 外没有别的进程在监听」——那是一台机器的内部事实，本票不主张。`T4/T11` 按 IP 判，
覆盖的是**该主机非回环那一面够不够得着**；`host-target` 在 compose 里只挂 `dst-side` 一张网，
所以这两条合起来就是它的全部非回环面。结论成立，但成立的范围是这个，不是「全机无监听」。

## 1. 隧道形态（本趟实际填进去的值）

```
 源端主机 qbs-host-source                        目标端主机 qbs-host-target
   source（本票用桩代替）                          stunnel 服务端
     sink_base_url = http://127.0.0.1:8080          accept  = 0.0.0.0:15443
          │ 明文，只走回环                          connect = 127.0.0.1:8080
          ▼                                              │ 明文，只走回环
   stunnel 客户端                                        ▼
     accept  = 127.0.0.1:8080                       桩 sink 127.0.0.1:8080
     connect = host.docker.internal:15443  ── TLS 1.2 ──▶（真机上是客户的公网 IP）
```

`8080` 正是 `config/source.toml.example` / `config/sink.toml.example` 里的现值：
**「产品零改动」不止是没改代码，连示例配置里那个值都没动**（ADR-0041 增补 6(b)）。

**落点是桩 sink，不是真 sink**——真 `sink` 归 #156。桩照 `127.0.0.1:8080` 绑，
「只绑回环」这条与真 sink 一模一样，`T4` 判的就是它（ADR-0041 增补 6(c)）。

## 2. 拓扑判据：装隧道之前先跑一遍（19/19）

顺序是**先拓扑、后隧道**：`R7a/R8a` 要在目标端自己起探针监听端占用 `15443`，
而隧道装完之后那个端口归 stunnel（ADR-0041 增补 6(d)）。

```
==> 前置：这套判据的「公网一跳」靠 Docker Desktop 的 host.docker.internal 打到宿主回环
    Docker OperatingSystem = Docker Desktop
==> R1 两台主机容器在跑（在此之前一切判据都不成立）
  R1a  PASS  源端主机 qbs-host-source                             实测=running
  R1b  PASS  目标端主机 qbs-host-target                          实测=running
==> R0 两台主机跟客户机同架构、同 glibc 下界（#151 的构建目标就落在这上面）
  R0a  PASS  源端主机架构                                       实测=x86_64
  R0b  PASS  目标端主机架构                                    实测=x86_64
  R0c  PASS  源端主机 glibc（客户机的硬下界）            实测=2.17
  R0d  PASS  目标端主机 glibc                                    实测=2.17
==> R9 干净态：留痕迹 → 先确认痕迹真的留下了 → 一键推倒重建 → 痕迹应当消失
    （默认不重建：R9 不判。要判就跑 ./scripts/rehearsal-topology-check.sh --reset —— 它会抹掉两台主机上已装的东西）
==> R2–R5 各自够得着自己那一侧的库，够不着对面那一侧
    oracle: src-side=172.30.0.2 default=172.27.0.3    mysql: dst-side=172.29.0.2 default=172.27.0.2
  R2   PASS  源端主机 → Oracle 172.30.0.2:1521（也是 R5 的正对照） 实测=通
  R4   PASS  目标端主机 → MySQL 172.29.0.2:3306（也是 R3 的正对照） 实测=通
  R3   PASS  源端主机 → MySQL 172.29.0.2:3306（两库不通） 实测=不通
  R5   PASS  目标端主机 → Oracle 172.30.0.2:1521（两库不通） 实测=不通
  R3b  PASS  源端主机 → MySQL 172.27.0.2:3306（default 网那个 IP） 实测=不通
  R5b  PASS  目标端主机 → Oracle 172.27.0.3:1521（default 网那个 IP） 实测=不通
==> R7a/R8a 正对照：目标端两个监听端确实活着（负判据全靠它们才有意义）
  R7a  PASS  目标端主机自连 15443（监听端活着）         实测=QBS-REHEARSAL-WHITELIST
  R8a  PASS  目标端主机自连 15444（监听端活着）         实测=QBS-REHEARSAL-BLOCKED
==> R6 跨容器直达被切断：监听端就在那儿听着，源端按 IP 直连仍必须摸不到
    目标端在 qbs-dst-side 上的 IP = 172.29.0.3
  R6a  PASS  目标端主机经自己的 side-net IP 自连 15443     实测=QBS-REHEARSAL-WHITELIST
  R6   PASS  源端主机 → 172.29.0.3:15443（按 IP 直达，拿不到令牌） 实测=无
  R6b  PASS  源端主机 → qbs-host-target:15443（按容器名，名字也不该解析到） 实测=无
==> R7–R8 公网那一跳只能走暴露端口（白名单），别的端口摸不到
  R7   PASS  源端主机 → 宿主:15443 → 目标端（白名单口） 实测=QBS-REHEARSAL-WHITELIST
  R8   PASS  源端主机 → 宿主:15444（目标端在听但没暴露） 实测=不通
==> R3c/R5c 宿主网关那条路：白名单口能过，两个库的发布端口不能过
    源端主机看到的 host.docker.internal = fdc4:f303:9324::254 host.docker.internal
  R3c  PASS  源端主机 → 宿主:3306（宿主上 MySQL 的发布端口；正对照是 R7） 实测=不通
  R5d  PASS  目标端主机 → 宿主:15443（网关这条路对目标端也是活的） 实测=通
  R5c  PASS  目标端主机 → 宿主:1521（宿主上 Oracle 的发布端口；正对照是 R5d） 实测=不通
==> R10 收尾：探针监听端回收干净，15443 留给 #153 的 stunnel 服务端
  R10  PASS  目标端主机自连 15443（应已无人听）         实测=不通

==== 前置 R0（#151 的构建目标，不是 #152 的拓扑判据）：PASS=4 FAIL=0 ====
==== 拓扑判据：PASS=19 FAIL=0 ====
```

### 顺序颠倒时会怎样（同日实测，作为那条裁定的证据）

隧道还在跑时跑拓扑判据，`R6a/R7/R7a/R10` **四条一起红**，四条红的真实成因是同一个
（`15443` 被 stunnel 占着，探针 bind 不上），而且都不是拓扑出了问题：

```
  R7a  FAIL  目标端主机自连 15443（监听端活着）         期望=QBS-REHEARSAL-WHITELIST 实测=无
  R6a  FAIL  目标端主机经自己的 side-net IP 自连 15443     期望=QBS-REHEARSAL-WHITELIST 实测=无
  R7   FAIL  源端主机 → 宿主:15443 → 目标端（白名单口） 期望=QBS-REHEARSAL-WHITELIST 实测=无
  R10  FAIL  目标端主机自连 15443（应已无人听）         期望=不通 实测=通
==== 拓扑判据：PASS=15 FAIL=4 ====
```

拓扑脚本因此在起探针前加了一条占用检测，把这个成因当场说出来——
**负判据红在错误的成因上，与假绿是同一类问题的两面。**

## 3. 装隧道：照 `packaging/stunnel/README.md` 的六步

```
==> [qbs-host-source] 换 yum 源到 vault，装 stunnel + openssl
    stunnel 4.56 on x86_64-redhat-linux-gnu platform
==> [qbs-host-target] 换 yum 源到 vault，装 stunnel + openssl
    stunnel 4.56 on x86_64-redhat-linux-gnu platform
==> 出两端的证书材料
subject= /CN=db-qbs-target
==> [qbs-host-source] 铺证书与配置到 /etc/stunnel/db-qbs/
==> [qbs-host-target] 铺证书与配置到 /etc/stunnel/db-qbs/
==> 填占位符（演练台的值；真机上 @@TARGET_HOST@@ 换成客户给的公网 IP）
    源端 connect / 目标端 accept：
      源端 accept  = 127.0.0.1:8080
      源端 connect = host.docker.internal:15443
      目标端 accept  = 0.0.0.0:15443
      目标端 connect = 127.0.0.1:8080
==> [qbs-host-target] 起桩 sink（127.0.0.1:8080，只绑回环）
==> [qbs-host-target] 起 stunnel
==> [qbs-host-source] 起 stunnel
==> 等两端端口就位
== 隧道就位；判据跑 ./scripts/rehearsal-tunnel-check.sh ==
```

第 0 步（换 yum 源到 `vault.centos.org`）**在演练台上和真机上一模一样**：
`centos:7` 容器与客户那台真机同样装不上任何包，`yum install stunnel` 不换源直接 404。

## 4. 隧道判据（17/17）

```
==> 前置：两台主机在跑（在此之前一切判据都不成立）
  T0a   PASS  源端主机 qbs-host-source                               实测=running
  T0b   PASS  目标端主机 qbs-host-target                            实测=running
==> T1–T2 两端 stunnel 在位（pid 文件也验掉——真机的 systemd unit 靠它守进程）
  T1    PASS  源端 stunnel 客户端                                   实测=在跑
  T2    PASS  目标端 stunnel 服务端                                实测=在跑
    源端 src-side IP=172.30.0.3   目标端 dst-side IP=172.29.0.3
==> T3–T4 sink 只绑回环（ADR-0024 的兜底形态原样成立）
  T3    PASS  目标端自连 127.0.0.1:8080（回环上的服务活着） 实测=QBS-TUNNEL-OK
  T4    PASS  目标端经自己的 172.29.0.3:8080（回环之外摸不到 sink） 实测=不通
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
  T10a  PASS  目标端经自己的 172.29.0.3:15443 自连（T10 的正对照） 实测=通
  T10   PASS  源端按 IP 直达 172.29.0.3:15443（跨容器直达仍被切断） 实测=不通
  T11   PASS  源端经自己的 172.30.0.3:8080（隧道入口只绑回环） 实测=不通

==== 隧道判据：PASS=17 FAIL=0 ====
```

### 「加密」那一条是怎么判的

同一个地址（`host.docker.internal:15443`）上量六次：

- `T6` 明文 HTTP 打上去 —— 拿不到任何东西；
- `T6b` 对端回的首字节 —— 不是 `48 54 54 50`（`"HTTP"`）。本趟实测是**空**：
  TLS 服务端对着垃圾闭嘴是合规行为。**「闭嘴」本身也可能是「那儿没人听」**，
  这个假绿成因由同址正对照 `T7` 排掉；
- `T7` 带客户端证书握手 —— 拿到 `QBS-TUNNEL-OK`（正对照，证明那儿确实有服务）；
- `T7b` 对端证书 CN = `db-qbs-target` —— 钉的是这一张，不是任何公共 CA；
- `T7c` 协商出来的是 `TLSv1.2` / `ECDHE-RSA-AES256-GCM-SHA384`；
- `T8` 不带客户端证书握手 —— 被拒。**隧道不只加密，还认人**：
  少了 `verify = 2`，中间人换一张自签证书照样接得下来。

### 每条负判据的同址正对照

| 负判据 | 正对照 | 排掉的假绿成因 |
|---|---|---|
| `T4` 侧网 IP 摸不到 `8080` | `T3` 回环上摸得到 | 「服务压根没起」 |
| `T6` / `T6b` 明文拿不到 | `T7` 同址握手拿得到 | 「那个地址上没人听」 |
| `T8` 无证书被拒 | `T7` 有证书能进 | 「服务端整个不可用」 |
| `T10` 跨容器直达摸不到 `15443` | `T10a` 目标端经同一 IP 自连得到 | 「IP 取错 / 地址不存在」 |
| `T11` 源端侧网 IP 摸不到 `8080` | `T5` 源端回环上摸得到 | 「隧道客户端没起来」 |

## 5. 边界：本趟没证的

- **真 `sink`、真 `source` 都没上机**（#155/#156）。本票证的是隧道那一段。
- **搬运没跑**——经隧道跑通一次真实搬运是 #157 的终局演练。
- **真机差异四条没撞到**：防火墙、SELinux、客户给的公网 IP、`8080` 被占。
  容器里没有 firewalld、SELinux 不生效、`host.docker.internal` 是 Docker Desktop 才有的东西。
  ADR-0041 增补 1 明文接受这个代价，四条逐条列在 `packaging/stunnel/README.md`
  「真机上会不一样的地方」，手册（#155/#156）要把它们带过去。
- **三份视觉走查未跑**：本票零 UI 改动（改动全在 `packaging/stunnel/`、
  `docs/spikes/fixtures/local-rig/`、`docs/adr/`），`CLAUDE.md` Visual gates 的三个触发条件
  一个都没响 —— 见下一节的证据。

## 6. 视觉门禁交代（`CLAUDE.md` 通则 3）

**V1–V25 / W1–W6 / X1–X9 三份走查本票均「未跑」**，理由是触发条件未响，证据是改动清单本身：

| 走查 | 触发条件 | 本票是否触发 |
|---|---|---|
| V1–V25 | M2 验收；`docs/design-system/README.md` 或 `tokens.css` 任何改动 | 否——两份文件零改动 |
| W1–W6 | M3 验收；`web/src/app.css` 的 `.precheck-reports` 布局或 `DiagnosticTable` 列结构改动 | 否——`web/` 整棵树零改动 |
| X1–X9 | v1 验收；数据源屏 / 构建器映射列与目标下拉 / 运行历史重跑入口与发起对话框预填 / ADR-0039 §9 那四条 `app.css` 规则 | 否——同上 |

本票的改动清单（`git status`）没有一个文件落在 `web/` 或 `docs/design-system/` 下，
`scripts/test-rehearsal-tunnel.sh` 第 9 条把「产品零改动」变成了会红的门禁而不是一句自述——它按内容判，合入 `main` 之后一样会红。
