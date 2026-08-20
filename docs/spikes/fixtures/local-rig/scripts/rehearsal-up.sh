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
#
# 为什么从外部施加：被演练的那两台机器必须是**干净的 centos:7**——里面连 `ip` 都没有，
# 更不该为台架自己的布线往里装东西。所以借一个一次性 alpine 共享它的网络命名空间打路由：
# 机器本身一个字节没动，切断像现场的防火墙那样是外部事实。删容器即归零，这里每次起完重打。
CUT_IMAGE=alpine:3
subnet() { docker network inspect "$1" -f '{{(index .IPAM.Config 0).Subnet}}'; }
DEFAULT_NET=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{"\n"}}{{end}}' qbs-oracle11 | grep -v -- -side | head -1)

cut_off() {  # $1=容器 $2...=要黑洞掉的子网 —— 幂等（replace），起完重复跑安全
  docker run --rm --network "container:$1" --cap-add NET_ADMIN "$CUT_IMAGE" \
    sh -c 'for n; do ip route replace blackhole "$n"; done' sh "${@:2}"
}

SRC_SUB=$(subnet qbs-src-side)
DST_SUB=$(subnet qbs-dst-side)
DEF_SUB=$(subnet "$DEFAULT_NET")
echo "    src-side=$SRC_SUB  dst-side=$DST_SUB  default=$DEF_SUB"
# 源端黑洞掉目标端那张网和 default（mysql 在 default 上也有一个 IP，不挡就等于没挡）；
# 目标端反过来。各自那一侧的库走同网直连，前缀更长，不受 /16 黑洞影响。
# 「公网一跳」走 host.docker.internal，Docker Desktop 给的是 IPv6 地址，与这几条 IPv4 黑洞无关。
cut_off qbs-host-source "$DST_SUB" "$DEF_SUB"
cut_off qbs-host-target "$SRC_SUB" "$DEF_SUB"

docker compose --profile rehearsal ps host-source host-target
echo "== 演练台就位；拓扑判据跑 ./scripts/rehearsal-topology-check.sh =="
