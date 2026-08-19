#!/usr/bin/env bash
# 把 packaging/centos7/out/bin/ 的产物丢进一个干净的 centos:7 容器里启动一次。
# 干净 = 没装过 Rust、没装过任何依赖，只有 glibc 2.17 —— 就是客户那台机器的形状。
#
#   packaging/centos7/verify.sh            # 平台取 out/platform，没有则 linux/amd64
#   PLATFORM=linux/arm64 packaging/centos7/verify.sh
#
# 判据三条：
#   1. 是 glibc 动态链接（不是 musl 静态 —— source 要经 OCI 加载 Instant Client）；
#   2. ldd 无 "not found"；
#   3. 直接启动不出现 GLIBC 版本错误、不因动态链接器失败而 127 退出。
# 二进制自身因缺 --config 而报用法并以 1 退出是预期的：那说明进程已经跑起来了。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_OUT="${HERE}/out/bin"
# 平台优先取 build.sh 落下的那份 —— 拿 amd64 的容器去跑 arm64 的产物只会得到
# exit 126，那是校验器配错平台的假失败，不是打包问题。
PLATFORM_FILE="${HERE}/out/platform"
if [[ -z "${PLATFORM:-}" && -f "$PLATFORM_FILE" ]]; then
  PLATFORM="$(cat "$PLATFORM_FILE")"
fi
PLATFORM="${PLATFORM:-linux/amd64}"
BASE_IMAGE="${BASE_IMAGE:-centos:7}"

[[ -d "$BIN_OUT" ]] || { echo "!! $BIN_OUT 不存在，先跑 build.sh" >&2; exit 1; }

echo "==> 干净 ${BASE_IMAGE}（${PLATFORM}）上验产物"
docker run --rm --platform "$PLATFORM" \
  -v "$BIN_OUT":/opt/db-qbs/bin:ro \
  "$BASE_IMAGE" bash -euc '
    echo "--- 容器的 glibc ---"
    ldd --version | head -1

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
