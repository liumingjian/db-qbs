# 演练台拓扑判据实录（2026-08-20 01:20Z，改判后重跑）

> **⚠️ 这一轮仍然缺一条：宿主网关那条路（2026-08-20 01:45Z 判定）。**
> 黑洞路由挡的是 IPv4 侧网与 default 网，而两个库在**宿主**上各发布了一个端口，
> 「公网一跳」的落点正是宿主——实测源端主机经 `host.docker.internal:3306` **连得上 MySQL**。
> 下面的 PASS 25 / FAIL 0 原样保留（记录就是记录，不回头改数），
> 但「两库之间网络不通」要以
> [`rehearsal-topology-20260820T014500Z.md`](rehearsal-topology-20260820T014500Z.md)
> 为准（切断改为两层，第 2 层按端口判，裁定见 ADR-0041 增补 5）。

- **驱动脚本**：`scripts/rehearsal-topology-check.sh --reset`（进仓库，视觉门禁通则 4 的同一条纪律）
- **落点**：mac Docker Desktop（rexec 派发），Docker OperatingSystem = `Docker Desktop`
- **结果**：**PASS 25 / FAIL 0**
- **为什么重跑**：`fa2c708` 落地后的两轴 `/code-review` 逮到 R6 的证明强度不够。
  照它说的改成按 IP 判之后，**R6 当场 FAIL** —— 那条「切断」原本就不成立（见下）。

## 这一轮改了什么，为什么

### 1. 三条负判据原本是假绿的：R3 / R5 / R6 都按容器名判

`fa2c708` 那份实录写的是 PASS 20 / FAIL 0，其中：

| 判据 | 原写法 | 为什么假绿 |
|---|---|---|
| R3 源端摸不到 MySQL | `tcp $SRC mysql 3306` | `mysql` 这个名字在 `qbs-src-side` 上本来就解析不到 |
| R5 目标端摸不到 Oracle | `tcp $DST oracle 1521` | 同上，`oracle` 在 `qbs-dst-side` 上解析不到 |
| R6 跨容器直达被切断 | `read_token $SRC $DST 15443` | `qbs-host-target` 在源端那张网上解析不到 |

三条得出的都是「不通」，但成因是**名字解析失败**，不是路由被切断——
而「DNS 查不到」恰恰是脚本自己在注释里点名的假绿成因。

**改成按 IP 判之后，R6 立刻露出真相**（2026-08-20 01:05Z 的那一跑）：

```
  R6a  PASS  目标端主机经自己的 side-net IP 自连 15443     实测=QBS-REHEARSAL-WHITELIST
  R6   FAIL  源端主机 → 172.29.0.3:15443（按 IP 直达）     期望=无 实测=QBS-REHEARSAL-WHITELIST
  R6b  PASS  源端主机 → qbs-host-target:15443（按容器名）  实测=无
```

**Docker Desktop 在两张 bridge 网之间是转发的。** 两台主机各在自己那张网上并不构成隔离；
`172.30.0.3 → 172.29.0.3:15443` 直接拿到了令牌。也就是说 `fa2c708` 声称搭出来的
「两库之间网络不通、只有一跳走白名单口」这张拓扑，**在演练台上从来没成立过**。

### 2. 切断改由台架显式施加，不再指望 Docker 的默认行为

被演练的两台机器必须是干净的 `centos:7` —— 里面连 `ip` 都没有（`iproute` 没装），
也不该为台架自己的布线往里装东西。所以从**外部**给它们的网络命名空间打黑洞路由：

```bash
docker run --rm --network "container:$1" --cap-add NET_ADMIN alpine:3 \
  sh -c 'for n; do ip route replace blackhole "$n"; done' sh "${@:2}"
```

- 源端黑洞掉 `dst-side` 与 `default` 两张网的网段，目标端反过来。
  **`default` 那一条不能省**：两个库在 default 网上各还有一个 IP，只挡侧网那个等于没挡。
- 各自那一侧的库走同网直连，前缀比 `/16` 黑洞更长，不受影响。
- 「公网一跳」走 `host.docker.internal`，Docker Desktop 给的是 IPv6（`fdc4:f303:9324::254`），
  与这几条 IPv4 黑洞无关。
- 机器本身一个字节没动，切断像客户现场的防火墙那样是**外部事实**。删容器即归零，
  `rehearsal-up.sh` 每次起完重打，`rehearsal-reset.sh` 走同一条路径。

裁定写在 **ADR-0041 增补 4**。

### 3. 负判据一律配一条**同址**正对照

不再满足于「监听端活着」这种旁证——R3 的正对照就是 R4（同一个 MySQL IP、同一个端口，
从目标端连必须通），R5 的正对照是 R2。R6 配 R6a（目标端经同一个 IP 自连拿得到令牌），
把「IP 取错 / 取不到」这个成因也排掉。

## 判据原样输出

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
  R9a  PASS  重建前源端主机上的痕迹文件                  实测=有
  R9b  PASS  重建前目标端主机上的痕迹文件               实测=有
    推倒重建中（rehearsal-reset.sh）……
  R9r  PASS  rehearsal-reset.sh 跑通（R9c/R9d 的前提）         实测=成功
  R9c  PASS  重建后源端主机上的痕迹文件                  实测=无
  R9d  PASS  重建后目标端主机上的痕迹文件               实测=无
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
==> R10 收尾：探针监听端回收干净，15443 留给 #153 的 stunnel 服务端
  R10  PASS  目标端主机自连 15443（应已无人听）         实测=不通

==== 拓扑判据：PASS=25 FAIL=0 ====
```

## 判据脚本自身的两处改动

- **`set -euo pipefail` → `set -uo pipefail`**，与既有四份判据脚本
  （`run-m1/m2/m3/v1-acceptance.sh`）同一条纪律：逐条判完再算总账。
  改之前 `rehearsal-reset.sh` 一失败就掐断，其后各条一条都不打印，退出码也不是脚本自述的 1。
  重建的成败改成显式判 **R9r**——它是 R9c/R9d 的前提，重建没成的话「痕迹没了」什么也证明不了。
- **默认不再破坏性**：默认 `--no-reset` 语义，R9 不判；要判干净态显式给 `--reset`。
  演练进行到一半来复核拓扑是常态，不该顺手把已装好的 source / stunnel 抹掉。

## 一处实测到的脆弱点：每次 `up` 都要联网

本机 `centos:7` 这个 tag 指的是 **arm64** 镜像（`sha256:c9a1fdca…` linux/arm64），
而两台主机声明 `platform: linux/amd64`——compose 每次都得联网解析 amd64 的 manifest。
2026-08-20 01:13Z 那一跑就撞上一次代理超时：

```
 host-source Error Head "https://registry.dockermirror.com/v2/library/centos/manifests/7":
   proxyconnect tcp: dial tcp 192.168.127.1:3128: i/o timeout
```

复跑即过（`host-source Pulled`）。**离线起不来**，这一条记在 README 的边界里。

## 四份既有台架

**未重跑。** 本轮改动落在 `rehearsal` profile 的两台主机、三支演练脚本与判据脚本上；
`docker-compose.yml` 的 diff **只有注释**（`git diff docker-compose.yml` 的非注释改动为零：
`image` / `platform` / `profiles` 三行只加了行尾注释，字段值一个字符没变），
四份台架的服务定义、网络、端口零改动。黑洞路由只打在 `qbs-host-source` / `qbs-host-target`
两个网络命名空间里，四份台架用的 `qbs-client` / `oracle` / `mysql` 一条都没碰。
`fa2c708` 那一轮的四份台架实跑（M1 9/9、M2 A1–A14、M3 6/6、v1 6/6）仍然成立。

## 三份视觉走查

**未跑**，逐条给出封存点与其后的 diff 证据（ADR-0041 §6 第 4 条 / ADR-0040 增补 1）：

- **V1–V25**：封存点 `e581056`。`git log e581056..HEAD -- docs/design-system/` **零个提交**
  ——两条触发条件（设计系统 README 改动 / `tokens.css` 改动）都不成立。
- **W1–W6**：封存点 `1348df1`。其后动过 `web/src/app.css` 的只有 `e63c492`、`aa510db` 两个提交，
  **两者已由 ADR-0040 增补 1 逐行核过并判定不触发**（`.precheck-reports` 与 `DiagnosticTable`
  在那段 diff 里命中 0 次）；本轮之后没有新的前端提交。
- **X1–X9**：触发对象是数据源屏 / 构建器映射列与目标下拉 / 运行历史重跑入口 /
  ADR-0039 §9 那四条 `app.css` 规则。本轮 diff 里 `web/` 下**零个文件**。

本轮改动的 diff 全部落在 `docs/adr/` 与 `docs/spikes/fixtures/local-rig/` 两处，
`.precheck-reports` / `DiagnosticTable` / `tokens.css` 三个串在整段 diff 里命中 0 次
（唯一一次字面出现是上一份实录叙述「哪些没碰」的那句话本身）。

## 边界 —— 这套演练台不能答什么

- `host.docker.internal` 是 Docker Desktop 才有的东西；真机上对应的是客户给的公网 IP 与
  白名单端口。判据脚本会先打印 `docker info` 的 OperatingSystem，不是 Desktop 就点名说清楚
  R7 为什么会恒 FAIL。
- **切断是台架施加的，不是 Docker 白送的**（本轮的主要发现）。换台机器、换个 Docker 版本，
  黑洞路由这一步一旦没打上，R3/R5/R6 会**响亮地 FAIL**——这正是负判据该有的样子，
  而不是像 `fa2c708` 那样悄悄 PASS。
- 容器里 root 是默认的、没装过任何东西——真机上最常卡人的恰恰是这两样
  （ADR-0041 增补 1 明文接受这个代价）。**yum 源不算差异**：CentOS 7 已 EOL，
  容器和真机同样得先改到 vault 才装得上包。
- 本票只搭拓扑，**没有跑任何搬运**：隧道在 #153、自检在 #154、手册与实录在其后。
