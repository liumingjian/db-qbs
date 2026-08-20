# db-qbs 的 CentOS 7 构建镜像 —— 两个二进制的 glibc 下界由它钉死。
#
# 为什么是 centos:7：客户机是 CentOS 7（glibc 2.17）。在更新的 Linux 或 macOS 上
# 编出来的二进制装上去启动即 `GLIBC_2.xx not found`（ADR-0041 §5）。
# 为什么不换 musl 静态链接：source 要经 OCI 动态加载 Instant Client 的 libclntsh.so，
# 那是 glibc 动态库，musl 目标上加载不了 —— 产物必须是 glibc 动态链接。
ARG BASE_IMAGE=centos:7
FROM ${BASE_IMAGE}

# CentOS 7 已 EOL（2024-06-30），mirror.centos.org 的 mirrorlist 是死的，
# 装任何包之前必须先把 yum 源改指 vault 存档源，否则 yum 第一条命令就 404。
# aarch64 的存档在 altarch 下，故留成构建参数由 build.sh 按平台传。
ARG VAULT_BASE=http://vault.centos.org/7.9.2009
# 后备镜像：空格分隔的若干个「与 VAULT_BASE 同布局」的根，由 build.sh 按平台传。
# **不是冗余，是必需**：2026-08-20 下午 vault.centos.org 前面那层 CDN 对
# `*.sqlite.bz2` 回了几个小时 403（同一批 `*.xml.gz` 照常 200），而 yum 优先取 sqlite 元数据 ——
# 那几个小时里这个镜像根本构建不出来；当晚复测它自己好了。
# **正因为它会自己好、也会自己再坏**，单源就是一颗随时引爆的雷，而装机当天在客户现场引爆
# 没有第二次机会。存档内容是冻结的，镜像之间不会各说各话。同一份换法在
# docs/spikes/fixtures/local-rig/scripts/rehearsal-tunnel-up.sh 里也有一份，
# 两处的镜像集合由 test-rehearsal-tunnel.sh 第 11 条盯着。
ARG VAULT_MIRRORS="https://linuxsoft.cern.ch/centos-vault/7.9.2009 https://archive.kernel.org/centos-vault/7.9.2009"
# 签名 key 按架构不同：x86_64 是 RPM-GPG-KEY-CentOS-7，aarch64 的 altarch 包另用
# RPM-GPG-KEY-CentOS-7-aarch64（key ID 305d49d6）。镜像里有几个就全导入、全列进
# gpgkey，别为了省事关掉 gpgcheck —— 这是装到客户机上的东西的来源凭证。
# （注意：RUN 的续行会把后面所有行并成一条 shell 命令，注释只能写在 RUN 外面。）
RUN set -eux; \
    rm -f /etc/yum.repos.d/*.repo; \
    keys="$(ls /etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7*)"; \
    test -n "${keys}"; \
    rpm --import ${keys}; \
    gpgkey_line="$(printf 'file://%s ' ${keys})"; \
    for repo in os:base:Base updates:updates:Updates extras:extras:Extras; do \
      dir="${repo%%:*}"; rest="${repo#*:}"; id="${rest%%:*}"; label="${rest#*:}"; \
      urls="${VAULT_BASE}/${dir}/\$basearch/"; \
      for m in ${VAULT_MIRRORS}; do urls="${urls} ${m}/${dir}/\$basearch/"; done; \
      { \
        echo "[${id}]"; \
        echo "name=CentOS-7.9.2009 - ${label} - vault"; \
        echo "baseurl=${urls}"; \
        echo 'failovermethod=priority'; \
        echo 'gpgcheck=1'; \
        echo "gpgkey=${gpgkey_line}"; \
        echo 'enabled=1'; \
        echo; \
      } >> /etc/yum.repos.d/CentOS-Vault.repo; \
    done; \
    cat /etc/yum.repos.d/CentOS-Vault.repo; \
    yum -y install gcc make curl ca-certificates binutils tar; \
    yum clean all; \
    rm -rf /var/cache/yum
# gcc/make：rusqlite 用 bundled SQLite、oracle 用 ODPI-C，两者编译期都要 C 编译器。
# binutils：产物的 GLIBC 符号上界靠 objdump 静态核对（见 build.sh）。

# Rust 工具链装进镜像，不在构建时现下。下界是 workspace 的 1.85——Cargo.lock 里的
# zeroize 1.9.0 要 edition2024，1.83 上实测 `failed to parse manifest`。
# 默认取 1.90.0——x86_64-unknown-linux-gnu 的 glibc 下界仍是 2.17，不影响本镜像的目的。
ARG RUST_VERSION=1.90.0
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN set -eux; \
    curl -fsSL --proto '=https' --tlsv1.2 --retry 5 --retry-delay 3 https://sh.rustup.rs -o /tmp/rustup-init.sh; \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain "${RUST_VERSION}"; \
    rm -f /tmp/rustup-init.sh; \
    chmod -R a+w "${CARGO_HOME}"; \
    rustc --version; \
    cargo --version; \
    gcc --version | head -1; \
    ldd --version | head -1

WORKDIR /work
CMD ["bash"]
