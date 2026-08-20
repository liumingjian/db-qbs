#!/usr/bin/env bash
# 把 packaging/centos7/out/bin/<平台>/ 的产物丢进对应架构的干净 centos:7 容器里启动一次。
# 干净 = 没装过 Rust、没装过任何依赖，只有 glibc 2.17 —— 就是客户那台机器的形状。
#
#   packaging/centos7/verify.sh                     # out/bin/ 下有几个平台就验几个
#   PLATFORMS="linux/arm64" packaging/centos7/verify.sh
#
# 每个平台验三条：
#   1. 是 glibc 动态链接（不是 musl 静态 —— source 要经 OCI 加载 Instant Client）；
#   2. ldd 无 "not found"；
#   3. 直接启动不出现 GLIBC 版本错误、不因动态链接器失败而 126/127 退出。
# 二进制自身因缺 --config 而报用法并以 1 退出是预期的：那说明进程已经跑起来了。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_ROOT="${HERE}/out/bin"
BASE_IMAGE="${BASE_IMAGE:-centos:7}"

[[ -d "$BIN_ROOT" ]] || { echo "!! ${BIN_ROOT} 不存在，先跑 build.sh" >&2; exit 1; }

# 默认验 out/bin/ 里躺着的每个平台目录 —— 平台不用猜，也不用手工传：
# 拿 amd64 的容器去跑 arm64 的产物只会得到 exit 126，那是校验器配错平台的假失败。
declare -a platforms=()
if [[ -n "${PLATFORMS:-}" ]]; then
  read -r -a platforms <<< "$PLATFORMS"
else
  for d in "$BIN_ROOT"/*/; do
    [[ -d "$d" ]] || continue
    slug="$(basename "$d")"
    platforms+=("${slug//-//}")
  done
fi
[[ ${#platforms[@]} -gt 0 ]] || { echo "!! ${BIN_ROOT} 下没有平台目录，先跑 build.sh" >&2; exit 1; }

for platform in "${platforms[@]}"; do
  slug="${platform//\//-}"
  bin_out="$BIN_ROOT/$slug"
  [[ -d "$bin_out" ]] || { echo "!! ${bin_out} 不存在，先给这个平台跑 build.sh" >&2; exit 1; }

  echo
  echo "==> 干净 ${BASE_IMAGE}（${platform}）上验产物"
  docker run --rm --platform "$platform" \
    -v "$bin_out":/opt/db-qbs/bin:ro \
    "$BASE_IMAGE" bash -euc '
      echo "--- 容器的 glibc / 架构 ---"
      ldd --version | head -1
      uname -m

      fail=0
      for b in /opt/db-qbs/bin/*; do
        name=$(basename "$b")
        echo "--- $name ---"

        link=$(ldd "$b" 2>&1 || true)
        printf "%s\n" "$link"
        case "$link" in
          *"not a dynamic executable"*|*"statically linked"*)
            echo "!! $name 不是动态链接，OCI 加载 Instant Client 的前提没了"; fail=1 ;;
        esac
        case "$link" in
          *libc.so.6*) ;;
          *) echo "!! $name 没链到 glibc（libc.so.6）"; fail=1 ;;
        esac
        case "$link" in
          *"not found"*) echo "!! $name 有解析不到的动态库"; fail=1 ;;
        esac

        set +e
        out=$("$b" 2>&1); code=$?
        set -e
        printf "启动输出（exit %s）：%s\n" "$code" "$out"
        case "$out" in
          *GLIBC_*|*"error while loading shared libraries"*)
            echo "!! $name 启动时报 GLIBC/动态库错误"; fail=1 ;;
        esac
        if [ "$code" = 126 ] || [ "$code" = 127 ]; then
          echo "!! $name 根本没起来（exit ${code}）"; fail=1
        fi
      done

      [ "$fail" = 0 ] && echo "==> 三条判据全过：glibc 动态链接、无未解析依赖、干净 CentOS 7 上启动无 GLIBC 错误"
      [ "$fail" = 0 ]
    '
done
