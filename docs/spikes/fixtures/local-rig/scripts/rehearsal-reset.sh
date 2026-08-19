#!/usr/bin/env bash
# #152 —— 把两台「主机」容器推倒重建回干净态。一条命令，供演练反复从零走。
#
# 干净态靠的是容器本身一次性：两台主机**不挂任何卷、不 build 自定义镜像**，
# 装过的包、改过的配置、落下的二进制全在可写层里，删容器即归零。
# 演练判据是「只照手册装完」（ADR-0041 §6），每走一遍都得从同一个起点走，
# 否则上一遍手工补的那步会悄悄留在机器上，把手册的缺口盖住。
set -euo pipefail
cd "$(dirname "$0")/.."

# 两台主机 depends_on 两个库的健康检查：库没起时 `up` 会顺手把 Oracle 拉起来并阻塞等健康
# （首次可达几分钟）。先点名说清楚，别让调用方对着一个没有输出的进程猜它是不是卡死了。
echo "==> 确认两个库在跑"
for c in qbs-oracle11 qbs-mysql8; do
  docker inspect -f '{{.State.Status}}' "$c" 2>/dev/null | grep -qx running \
    || { echo "!! $c 没起，先跑 ./scripts/up.sh"; exit 1; }
done

echo "==> 删掉两台主机容器（连可写层一起）"
docker compose --profile rehearsal rm -sfv host-source host-target

echo "==> 重建"
docker compose --profile rehearsal up -d host-source host-target

docker compose --profile rehearsal ps host-source host-target
echo "== 两台主机已回到干净机器态 =="
