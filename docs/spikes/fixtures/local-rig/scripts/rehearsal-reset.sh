#!/usr/bin/env bash
# #152 —— 把两台「主机」容器推倒重建回干净态。一条命令，供演练反复从零走。
#
# 干净态靠的是容器本身一次性：两台主机**不挂任何卷、不 build 自定义镜像**，
# 装过的包、改过的配置、落下的二进制全在可写层里，删容器即归零。
# 演练判据是「只照手册装完」（ADR-0041 §6），每走一遍都得从同一个起点走，
# 否则上一遍手工补的那步会悄悄留在机器上，把手册的缺口盖住。
#
# **删在前、起在后，且删不看库的脸色**：回到干净态跟两个库在不在跑没有关系
# （主机不挂卷，删掉就归零）。演练翻车时恰恰最想重来，那时库可能正好也是倒的——
# 把「库没起就 exit 1」挡在删之前，等于在最需要重来的时刻不让重来。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> 删掉两台主机容器（连可写层一起）"
docker compose --profile rehearsal rm -sfv host-source host-target
echo "== 两台主机已回到干净机器态（容器已删）=="

echo "==> 重建（起的那一半与 rehearsal-up.sh 是同一段，别抄第二遍）"
if ./scripts/rehearsal-up.sh; then
  echo "== 已回到干净机器态，并重建就位 =="
else
  echo "!! 两台主机已删除（干净态已达成），但**重建没成**——多半是两个库没起。"
  echo "   跑 ./scripts/up.sh 起库，再跑 ./scripts/rehearsal-up.sh 把两台主机拉起来。"
  exit 1
fi
