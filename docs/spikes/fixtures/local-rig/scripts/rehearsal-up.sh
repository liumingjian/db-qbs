#!/usr/bin/env bash
# #152 —— 起演练台的两台「主机」容器（源端 / 目标端）。幂等，重复跑安全。
#
# 两台主机在 compose 的 `rehearsal` profile 下：默认 `up.sh` 不动它们，
# 四份既有台架的起停一个字节不变（#152 判据「既有台架不受影响」）。
#
# 前提：Oracle / MySQL 已经起着（源端主机要够得着 Oracle，目标端主机要够得着 MySQL）。
# 下面那道「库在不在跑」的门开在这里、不开在 rehearsal-reset.sh 里：两台主机 depends_on
# 两个库的健康检查，库没起时 `up` 会顺手把 Oracle 拉起来并阻塞等健康（首次可达几分钟）。
# 先点名说清楚，别让调用方对着一个没有输出的进程猜它是不是卡死了。
# —— 但**删容器不该受它管**：回干净态与库无关，那一半在 rehearsal-reset.sh 里先做完。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 确认两个库在跑（演练台挂在它们两侧）"
for c in qbs-oracle11 qbs-mysql8; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/up.sh"; exit 1; }
done

echo "==> 起两台 centos:7 主机容器（amd64 —— 与客户机同架构，见 README）"
docker compose --profile rehearsal up -d host-source host-target

echo "==> 施加「跨容器直达切断」——客户现场那道防火墙的替身"
# 为什么要显式切：Docker Desktop 在两张 bridge 网之间**是转发的**。两台主机各在自己那张网上
# 并不构成隔离——2026-08-20 实测 172.30.0.3 直连 172.29.0.3:15443 拿得到令牌（ADR-0041 增补 4）。
# 不切的话演练就跑在一张比客户现场宽松的网上，手册里缺的那几步要到现场才炸。
CUT_IMAGE=alpine:3
subnet() { docker network inspect "$1" -f '{{(index .IPAM.Config 0).Subnet}}'; }
DEFAULT_NET=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{"\n"}}{{end}}' qbs-oracle11 | grep -v -- -side | head -1)

# 切断分两层，缺一条都不成立：
#   1. **路由黑洞**——把对面那张网、以及 default 网的网段整段黑掉（两个库在 default 上各还有
#      一个 IP，只挡侧网等于没挡）。这一层挡的是「对面那台机器/那个库的所有端口」。
#   2. **端口级封堵**——源端主机封死一切 3306 出向，目标端封死一切 1521 出向。
#      这一层挡的是**绕过路由的那条路**：`host.docker.internal` 在 Docker Desktop 上给的是
#      一个 IPv6 网关地址（实测 `fdc4:f303:9324::254`），宿主上 `1521:1521` / `3306:3306`
#      两个发布端口就挂在它后面——2026-08-20 实测源端经它连 MySQL 3306 **是通的**，
#      「两库之间网络不通」在只有第 1 层时从来没成立过（ADR-0041 增补 5）。
#      路由黑洞管不了它：那是另一个地址、且要按端口区分（同一个网关上的 15443 正是白名单那一跳，
#      必须留着）。所以第 2 层按端口判，IPv4 / IPv6 两张表都打。
#      两台主机各自那一侧的库用的是**另一个**端口（源端 1521、目标端 3306），不受影响。
#
# 为什么从外部施加：被演练的那两台机器必须是**干净的 centos:7**——里面连 `ip` 都没有，
# 更不该为台架自己的布线往里装东西。所以借一个一次性 alpine 共享它的网络命名空间打路由与规则：
# 机器本身一个字节没动，切断像现场的防火墙那样是外部事实。删容器即归零，这里每次起完重打。
cut_off() {  # $1=容器 $2=要黑洞掉的网段（空格分隔） $3=要封死的出向 TCP 端口（空格分隔）
  docker run --rm --network "container:$1" --cap-add NET_ADMIN \
    -e NETS="$2" -e PORTS="$3" "$CUT_IMAGE" sh -euc '
      for n in $NETS; do ip route replace blackhole "$n"; done
      apk add --no-cache iptables ip6tables >/dev/null
      # 幂等：有同样的规则就不再加一条，起完重复跑安全。
      for p in $PORTS; do
        for t in iptables ip6tables; do
          $t -C OUTPUT -p tcp --dport "$p" -j DROP 2>/dev/null \
            || $t -A OUTPUT -p tcp --dport "$p" -j DROP
        done
      done'
}

SRC_SUB=$(subnet qbs-src-side)
DST_SUB=$(subnet qbs-dst-side)
DEF_SUB=$(subnet "$DEFAULT_NET")
echo "    src-side=$SRC_SUB  dst-side=$DST_SUB  default=$DEF_SUB"
cut_off qbs-host-source "$DST_SUB $DEF_SUB" 3306
cut_off qbs-host-target "$SRC_SUB $DEF_SUB" 1521

docker compose --profile rehearsal ps host-source host-target
echo "== 演练台就位；拓扑判据跑 ./scripts/rehearsal-topology-check.sh =="
