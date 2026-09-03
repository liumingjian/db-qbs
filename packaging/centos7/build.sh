#!/usr/bin/env bash
# 一条命令：在 centos:7 里把 db-qbs 的三个二进制编出来，两个平台各一套，
# 并各自在干净 centos:7 上验一遍启动。
#
#   packaging/centos7/build.sh                       # linux/amd64 + linux/arm64，编完自动验
#   packaging/centos7/build.sh --platform linux/arm64   # 只编一个平台（可重复给）
#   packaging/centos7/build.sh --no-verify           # 只编不验
#   packaging/centos7/build.sh --skip-web            # 复用已有的 web/dist，不重跑前端构建
#
# 产物在 packaging/centos7/out/bin/<linux-amd64|linux-arm64>/，不进版本库。
# 依赖：docker（在 mac 上跑，服务器内存不够）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="$HERE/out"

DEFAULT_PLATFORMS=(linux/amd64 linux/arm64)
PLATFORMS=()
RUST_VERSION="${RUST_VERSION:-1.90.0}"
NODE_IMAGE="${NODE_IMAGE:-node:22-bookworm-slim}"
BASE_IMAGE="${BASE_IMAGE:-centos:7}"
BASE_IMAGE="${BASE_IMAGE:-centos:7}"
DO_VERIFY=1
DO_WEB=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) [[ $# -ge 2 ]] || { echo "--platform 要跟一个值，如 linux/amd64" >&2; exit 2; }
                PLATFORMS+=("$2"); shift 2 ;;
    --rust-version) [[ $# -ge 2 ]] || { echo "--rust-version 要跟一个值，如 1.90.0" >&2; exit 2; }
                RUST_VERSION="$2"; shift 2 ;;
    --no-verify) DO_VERIFY=0; shift ;;
    --skip-web) DO_WEB=0; shift ;;
    -h|--help) sed -n '2,13p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done
[[ ${#PLATFORMS[@]} -gt 0 ]] || PLATFORMS=("${DEFAULT_PLATFORMS[@]}")

HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

echo "==> 目标平台 ${PLATFORMS[*]}，Rust ${RUST_VERSION}"

if (( DO_WEB )); then
  # 前端资源由 node 容器出，不进 centos:7 —— Node 18+ 的官方二进制要 glibc 2.28，
  # 在 centos:7 里根本起不来。crates/source/build.rs 在没有 npm 时会复用现成的
  # web/dist（见其注释），正是这里喂给它的这一份。产物是平台无关的 JS，两个平台共用一份。
  # 在容器内的临时目录里装依赖，不碰仓库的 node_modules —— 免得 linux 的
  # esbuild/rollup 原生包盖掉 mac 上那份。
  echo "==> 用 ${NODE_IMAGE} 出 web/dist（宿主架构，两个平台共用）"
  mkdir -p "$ROOT/web/dist"
  docker run --rm \
    -v "$ROOT":/src:ro \
    -v "$ROOT/web/dist":/out \
    -v db-qbs-npm-cache:/root/.npm \
    -e HOST_UID="$HOST_UID" -e HOST_GID="$HOST_GID" \
    "$NODE_IMAGE" bash -euc '
      mkdir -p /build/docs && cd /build
      cp -a /src/package.json /src/package-lock.json /src/tsconfig.json /src/vite.config.ts /build/
      cp -a /src/web /build/web
      cp -a /src/mock /build/mock
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

for platform in "${PLATFORMS[@]}"; do
  slug="${platform//\//-}"
  # aarch64 的 CentOS 7 存档在 altarch 下，路径与 x86_64 不同 —— 后备镜像也一样要分。
  # 后备镜像不是冗余：vault.centos.org 前面那层 CDN 会对 `*.sqlite.bz2` 回 403
  # （2026-08-20 下午实测，持续几个小时后自愈），而 yum 优先取 sqlite 元数据，
  # 单源时这一步当场装不上任何包。成因与三处换法的关系见 Dockerfile 里 VAULT_MIRRORS 的注释。
  case "$platform" in
    */arm64|*/arm64/*|*/aarch64)
      vault_leg="altarch/7.9.2009" ;;
    *)
      vault_leg="7.9.2009" ;;
  esac
  vault_base="http://vault.centos.org/${vault_leg}"
  vault_mirrors="https://linuxsoft.cern.ch/centos-vault/${vault_leg} https://archive.kernel.org/centos-vault/${vault_leg}"
  image="db-qbs-build-centos7:${RUST_VERSION}-${slug}"
  bin_out="$OUT/bin/$slug"

  echo
  echo "######## $platform ########"
  # 先把基础镜像拉到本地：buildkit 直接解析 centos:7 的 manifest 在 Docker Desktop 上
  # 会偶发 `failed size validation`（两个平台各撞过一次），而 docker pull 走的是另一条路，
  # 拉下来之后 build 就正常了。
  echo "==> 拉基础镜像 ${BASE_IMAGE}（${platform}）"
  docker pull --platform "$platform" "$BASE_IMAGE" >/dev/null

  echo "==> 构建 centos:7 构建镜像 ${image}（yum 源 ${vault_base}，后备 ${vault_mirrors}）"
  docker build --platform "$platform" \
    --build-arg "BASE_IMAGE=$BASE_IMAGE" \
    --build-arg "RUST_VERSION=$RUST_VERSION" \
    --build-arg "VAULT_BASE=$vault_base" \
    --build-arg "VAULT_MIRRORS=$vault_mirrors" \
    -t "$image" "$HERE"

  echo "==> 在 centos:7 里 cargo build --release --locked"
  # target 与 registry 各平台一份：同一个 target 目录在两个架构间来回用会整棵重编。
  docker run --rm --platform "$platform" \
    -v "$ROOT":/work -w /work \
    -v "db-qbs-centos7-cargo-registry-${slug}:/usr/local/cargo/registry" \
    -v "db-qbs-centos7-target-${slug}:/target" \
    -e CARGO_TARGET_DIR=/target \
    -e HOST_UID="$HOST_UID" -e HOST_GID="$HOST_GID" \
    -e OUT_SLUG="$slug" \
    "$image" bash -euc '
      cargo build --release --locked --workspace --bins
      dest="/work/packaging/centos7/out/bin/$OUT_SLUG"
      # 先清空：若 install 中途失败，留下的会是新旧混在一起的目录，
      # 而 verify.sh 分不出哪个是这一次编的。
      rm -rf "$dest"
      install -d "$dest"
      for b in db-qbs-source db-qbs-source-run db-qbs-sink; do
        install -m 0755 "/target/release/$b" "$dest/"
      done

      # 静态核对：产物引用的 GLIBC 符号版本上界必须 <= 2.17，
      # 这是「装到客户机上启动即死」在构建期就能撞掉的那一半。
      echo "--- GLIBC 符号上界（下界 2.17）---"
      fail=0
      for b in "$dest"/*; do
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

  echo "==> 产物"
  ls -l "$bin_out"
done

if (( DO_VERIFY )); then
  PLATFORMS="${PLATFORMS[*]}" "$HERE/verify.sh"
else
  echo "==> 按 --no-verify 跳过干净容器验证"
fi
