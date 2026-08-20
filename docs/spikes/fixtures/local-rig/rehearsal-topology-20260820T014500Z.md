# 演练台拓扑判据实录（2026-08-20 01:45Z，第二轮改判后重跑）

- **驱动脚本**：`scripts/rehearsal-topology-check.sh --reset`（进仓库，视觉门禁通则 4 的同一条纪律）
- **落点**：mac Docker Desktop（rexec 派发），Docker OperatingSystem = `Docker Desktop`
- **结果**：**前置 R0 PASS 4 / FAIL 0；拓扑判据 PASS 24 / FAIL 0**
- **为什么重跑**：第二轮两轴 `/code-review` 逮到「两库之间网络不通」仍然不成立——
  增补 4 的黑洞路由挡的是 IPv4 侧网与 default 网，而两个库在**宿主**上各发布了一个端口，
  「公网一跳」的落点正是宿主。裁定落 ADR-0041 增补 5。

## 1. 修之前：源端经宿主网关摸得到 MySQL（实测）

在 `548eea8`（增补 4 落地版，R3/R3b/R5/R5b 全绿）的两台主机上直接量：

```
$ docker exec qbs-host-source bash -c 'getent hosts host.docker.internal'
fdc4:f303:9324::254 host.docker.internal

$ docker exec qbs-host-source ...  /dev/tcp/host.docker.internal/3306   -> src2host_3306=通
$ docker exec qbs-host-target ...  /dev/tcp/host.docker.internal/1521   -> dst2host_1521=通
$ docker exec qbs-host-source ...  /dev/tcp/host.docker.internal/15443  -> src2host_15443=通
```

**源端主机连得上 MySQL、目标端主机连得上 Oracle**，都经宿主上的发布端口。
`README.md` 当时把「跨容器直达是切断的／没暴露的端口就是白名单外的端口」写成了全称结论，
但那张拓扑的核心前提（ADR-0041 §8「两库之间网络不通」）在演练台上仍然没有成立。

黑洞路由堵不住它：`fdc4:f303:9324::254` 是另一个地址（且是 IPv6），
而且**必须按端口区分**——同一个网关上的 `15443` 正是白名单那一跳，整条黑掉就把演练台拆了。
拆掉两个库的发布端口同样不行：四份既有台架的 source 跑在 mac 宿主上，靠的就是这两个端口。

## 2. 修之后：切断分两层，第 2 层按端口判

`rehearsal-up.sh` 起完对两台主机各施加：

1. 路由黑洞——对面那张网 + default 网的网段（增补 4 既有）。
2. 端口级 DROP，IPv4 / IPv6 两张表都打——源端封死一切 `3306` 出向，目标端封死一切 `1521` 出向。

```bash
docker run --rm --network "container:$1" --cap-add NET_ADMIN -e NETS="$2" -e PORTS="$3" alpine:3 sh -euc '
  for n in $NETS; do ip route replace blackhole "$n"; done
  apk add --no-cache iptables ip6tables >/dev/null
  for p in $PORTS; do for t in iptables ip6tables; do
    $t -C OUTPUT -p tcp --dport "$p" -j DROP 2>/dev/null || $t -A OUTPUT -p tcp --dport "$p" -j DROP
  done; done'
```

被演练的机器本身仍是一个字节没动的干净 `centos:7`，删容器即归零。
两台主机出网照旧（`vault.centos.org` 改源那一步不受影响），白名单口 `15443` 照旧。

## 3. 本轮实跑逐条（原样贴）

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
==> R3c/R5c 宿主网关那条路：白名单口能过，两个库的发布端口不能过
    源端主机看到的 host.docker.internal = fdc4:f303:9324::254 host.docker.internal
  R3c  PASS  源端主机 → 宿主:3306（宿主上 MySQL 的发布端口；正对照是 R7） 实测=不通
  R5d  PASS  目标端主机 → 宿主:15443（网关这条路对目标端也是活的） 实测=通
  R5c  PASS  目标端主机 → 宿主:1521（宿主上 Oracle 的发布端口；正对照是 R5d） 实测=不通
==> R10 收尾：探针监听端回收干净，15443 留给 #153 的 stunnel 服务端
  R10  PASS  目标端主机自连 15443（应已无人听）         实测=不通

==== 前置 R0（#151 的构建目标，不是 #152 的拓扑判据）：PASS=4 FAIL=0 ====
==== 拓扑判据：PASS=24 FAIL=0 ====
```

**R3c/R5c 不是靠「网关整条路断了」变绿的**：同一轮里 R7（源端经同一个网关拿到白名单口的令牌）
与 R5d（目标端经同一个网关连上 15443）都是通的。这两条正对照就是为这件事配的。

**两笔总账**：R0 判的是同架构、同 glibc，那是 #151 的构建目标，不是 #152 的拓扑判据，
单独记一笔，不拿别票的证据充本票的数。

## 4. 既有台架

`docker-compose.yml` 本轮**一个字符没改**，端口级规则只打在两台演练主机自己的网络命名空间里，
对四份既有台架与两个库容器没有任何作用面。冒烟 5 项在本改动之后实跑全过（`scripts/smoke.sh`，
`== 冒烟全过 ==`），两个库容器原地未动。四份验收台架未重跑，理由即上：本轮 diff 里没有它们的输入。

## 5. 静态自检

新增 `scripts/test-rehearsal-topology.sh`（与四份既有台架入口的 `test-*.sh` 同一职责，不碰 docker）：
判据编号全集、九对「负判据 ↔ 正对照」、两层切断都在、不认识的参数以 2 退出、重建那一半只有一份实现。
服务器上实跑：`rehearsal 静态自检 PASS`。

## 6. 三份视觉走查

**未跑**，三张触发表一条都没响：本轮改动只有 `docs/spikes/fixtures/local-rig/` 下的三支脚本、
README、实录，以及 `docs/adr/0041`，`web/` 与 `docs/design-system/` 下零个文件。
