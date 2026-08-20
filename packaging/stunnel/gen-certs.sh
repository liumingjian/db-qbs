#!/usr/bin/env bash
# db-qbs 隧道的证书材料 —— 一条命令出两端的全套，**跑一次，随行李带走**。
#
#   packaging/stunnel/gen-certs.sh                    # 出到 packaging/stunnel/out/
#   packaging/stunnel/gen-certs.sh --out /tmp/qbs-tls # 换个落点
#   packaging/stunnel/gen-certs.sh --days 90          # 改有效期（默认 825 天）
#
# 出来的东西：
#
#   out/source-side/{source.crt,source.key,target.crt}   → 拷到源端 /etc/stunnel/db-qbs/
#   out/target-side/{target.crt,target.key,source.crt}   → 拷到目标端 /etc/stunnel/db-qbs/
#
# 两端各一张**自签**证书，互相把对方那张钉进 CAfile（配置里 `verify = 2`）。
# 不建 CA：两端一共就两个身份，签发链带来的全部好处是「以后能再签第三个」，
# 而第三个的出现（不是所有者本人装的部署）恰恰是 ADR-0041 §4 挂账里
# 隧道这条兑现方式要退役的触发信号。到那天换的是方案，不是多一层链。
#
# **私钥不进版本库**（同目录 .gitignore 挡的就是 out/），也不走这条隧道本身传输——
# 所有者本人到场装机，两边的文件由他随身拷贝（ADR-0041 §8「人」那一行）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/out"
DAYS=825   # 超过 825 天的自签证书在不少客户端上会被直接判无效，取这个上界

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)  [[ $# -ge 2 ]] || { echo "--out 要跟一个目录" >&2; exit 2; }; OUT="$2"; shift 2 ;;
    --days) [[ $# -ge 2 ]] || { echo "--days 要跟一个天数" >&2; exit 2; }; DAYS="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

command -v openssl >/dev/null || { echo "!! 没有 openssl —— CentOS 7 上 \`yum -y install openssl\`" >&2; exit 1; }

# 已有就不覆盖：重跑一次把线上正在用的那对钥匙换掉，隧道当场断，而断了之后
# 报的是握手失败，不是「证书被换了」——排障会从网络查起。要重出就先自己删。
for f in "$OUT/source-side/source.key" "$OUT/target-side/target.key"; do
  [[ -e "$f" ]] && { echo "!! $f 已存在。要重新生成请先删掉 $OUT/ —— 覆盖会静默换掉正在用的钥匙。" >&2; exit 1; }
done

mkdir -p "$OUT/source-side" "$OUT/target-side"
umask 077

gen() {  # $1=名字（source/target） $2=落点目录
  echo "==> 生成 $1 的自签证书（有效期 ${DAYS} 天，RSA 2048）"
  openssl req -x509 -nodes -newkey rsa:2048 -sha256 -days "$DAYS" \
    -subj "/CN=db-qbs-$1" \
    -keyout "$2/$1.key" -out "$2/$1.crt" 2>/dev/null
  chmod 600 "$2/$1.key"
  chmod 644 "$2/$1.crt"
}

gen source "$OUT/source-side"
gen target "$OUT/target-side"

# 交叉：每一端拿到的是**对端的证书**（公钥那一半），私钥永远不离开自己那一端。
cp "$OUT/target-side/target.crt" "$OUT/source-side/target.crt"
cp "$OUT/source-side/source.crt" "$OUT/target-side/source.crt"
chmod 644 "$OUT/source-side/target.crt" "$OUT/target-side/source.crt"

echo
echo "==== 出好了：$OUT ===="
for side in source target; do
  echo "  $side-side/"
  ls -l "$OUT/$side-side" | tail -n +2 | awk '{printf "    %s  %s\n", $1, $NF}'
done
echo
echo "指纹（两端装完后可以拿它对一眼，确认拷的是同一批）："
for side in source target; do
  printf '  %-6s %s\n' "$side" \
    "$(openssl x509 -in "$OUT/$side-side/$side.crt" -noout -fingerprint -sha256 | cut -d= -f2)"
done
