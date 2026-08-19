#!/usr/bin/env bash
# 一条命令：在 centos:7 里把 db-qbs 的三个二进制编出来，并在干净 centos:7 上验一遍启动。
#
#   packaging/centos7/build.sh                # 默认 linux/amd64，编完自动验
#   packaging/centos7/build.sh --platform linux/arm64
#   packaging/centos7/build.sh --no-verify    # 只编不验
#   packaging/centos7/build.sh --skip-web     # 复用已有的 web/dist，不重跑前端构建
#
# 产物在 packaging/centos7/out/bin/，不进版本库。
# 依赖：docker（在 mac 上跑，服务器内存不够）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$HERE/out"
BIN_OUT="$OUT/bin"

PLATFORM="${PLATFORM:-linux/amd64}"
RUST_VERSION="${RUST_VERSION:-1.90.0}"
NODE_IMAGE="${NODE_IMAGE:-node:22-bookworm-slim}"
DO_VERIFY=1
DO_WEB=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) [[ $# -ge 2 ]] || { echo "--platform 要跟一个值，如 linux/amd64" >&2; exit 2; }
                PLATFORM="$2"; shift 2 ;;
    --rust-version) [[ $# -ge 2 ]] || { echo "--rust-version 要跟一个值，如 1.90.0" >&2; exit 2; }
                RUST_VERSION="$2"; shift 2 ;;
    --no-verify) DO_VERIFY=0; shift ;;
    --skip-web) DO_WEB=0; shift ;;
    -h|--help) sed -n '2,12p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done

# aarch64 的 CentOS 7 存档在 altarch 下，路径与 x86_64 不同。
case "$PLATFORM" in
  */arm64|*/aarch64) VAULT_BASE="http://vault.centos.org/altarch/7.9.2009" ;;
  *)                 VAULT_BASE="http://vault.centos.org/7.9.2009" ;;
esac

IMAGE="db-qbs-build-centos7:${RUST_VERSION}"
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

echo "==> 目标平台 ${PLATFORM}，Rust ${RUST_VERSION}，yum 源 $VAULT_BASE"
mkdir -p "$BIN_OUT" "$ROOT/web/dist"

echo "==> 构建 centos:7 构建镜像 $IMAGE"
docker build --platform "$PLATFORM" \
  --build-arg "RUST_VERSION=$RUST_VERSION" \
  --build-arg "VAULT_BASE=$VAULT_BASE" \
  -t "$IMAGE" "$HERE"

if (( DO_WEB )); then
  # 前端资源由 node 容器出，不进 centos:7 —— Node 18+ 的官方二进制要 glibc 2.28，
  # 在 centos:7 里根本起不来。crates/source/build.rs 在没有 npm 时会复用现成的
  # web/dist（见其注释），正是这里喂给它的这一份。
  # 在容器内的临时目录里装依赖，不碰仓库的 node_modules —— 免得 linux 的
  # esbuild/rollup 原生包盖掉 mac 上那份。
  echo "==> 用 $NODE_IMAGE 出 web/dist（宿主架构，产物是平台无关的 JS）"
  docker run --rm \
    -v "$ROOT":/src:ro \
    -v "$ROOT/web/dist":/out \
    -v db-qbs-npm-cache:/root/.npm \
    -e HOST_UID="$HOST_UID" -e HOST_GID="$HOST_GID" \
    "$NODE_IMAGE" bash -euc '
      mkdir -p /build/docs && cd /build
      cp -a /src/package.json /src/package-lock.json /src/tsconfig.json /src/vite.config.ts /build/
      cp -a /src/web /build/web
      cp -a /src/docs/design-system /build/docs/
      rm -rf /build/web/dist
      npm ci --no-audit --no-fund
      npm run build
      test -f /build/web/dist/index.html
      rm -rf /out/* /out/.[!.]* 2>/dev/null || true
      cp -a /build/web/dist/. /out/
      chown -R "$HOST_UID:$HOST_GID" /out
    '
else
  echo "==> 跳过前端构建，复用现成的 web/dist"
fi
test -f "$ROOT/web/dist/index.html" || { echo "!! web/dist/index.html 缺失，去掉 --skip-web 重跑" >&2; exit 1; }

echo "==> 在 centos:7 里 cargo build --release --locked"
docker run --rm --platform "$PLATFORM" \
  -v "$ROOT":/work -w /work \
  -v "db-qbs-centos7-cargo-registry:/usr/local/cargo/registry" \
  -v "db-qbs-centos7-target:/target" \
  -e CARGO_TARGET_DIR=/target \
  -e HOST_UID="$HOST_UID" -e HOST_GID="$HOST_GID" \
  "$IMAGE" bash -euc '
    cargo build --release --locked --workspace --bins
    # 先清空：换平台重编时，若 install 中途失败，留下的会是新旧混在一起的 out/bin，
    # 而 verify.sh 分不出哪个是这一次编的。
    rm -rf /work/packaging/centos7/out/bin
    install -d /work/packaging/centos7/out/bin
    for b in db-qbs-source db-qbs-source-run db-qbs-sink; do
      install -m 0755 "/target/release/$b" /work/packaging/centos7/out/bin/
    done

    # 静态核对：产物引用的 GLIBC 符号版本上界必须 <= 2.17，
    # 这是「装到客户机上启动即死」在构建期就能撞掉的那一半。
    echo "--- GLIBC 符号上界（下界 2.17）---"
    fail=0
    for b in /work/packaging/centos7/out/bin/*; do
      max=$(objdump -T "$b" | grep -o "GLIBC_[0-9][0-9.]*" | sed "s/^GLIBC_//" | sort -uV | tail -1)
      if [ -z "$max" ]; then
        # 一个 GLIBC 版本符号都没有 = 要么产物是静态链接（本镜像明文禁止的形态，
        # OCI 要 dlopen libclntsh.so），要么 objdump 没在。两种都不能当"过"。
        echo "!! $(basename "$b") 取不到 GLIBC 符号，这道核对失效了，判失败"
        fail=1
        continue
      fi
      echo "$(basename "$b"): GLIBC_$max"
      if [ "$(printf "%s\n" "$max" "2.17" | sort -V | tail -1)" != "2.17" ]; then
        echo "!! $(basename "$b") 需要 GLIBC_${max}，高于 CentOS 7 的 2.17"
        fail=1
      fi
    done
    chown -R "$HOST_UID:$HOST_GID" /work/packaging/centos7/out
    [ "$fail" = 0 ]
  '

# 平台随产物落盘：verify.sh 单独跑时不必再猜这批二进制是给哪个架构编的。
printf '%s\n' "$PLATFORM" > "$OUT/platform"

echo "==> 产物"
ls -l "$BIN_OUT"

if (( DO_VERIFY )); then
  PLATFORM="$PLATFORM" "$HERE/verify.sh"
else
  echo "==> 按 --no-verify 跳过干净容器验证"
fi
