# CentOS 7 构建环境

客户机是 **CentOS 7（glibc 2.17）**。在更新的 Linux 或 macOS 上编出来的二进制装上去
**启动即 `GLIBC_2.xx not found`** —— 这个形态必须在这里撞掉，不能留到装机现场
（[ADR-0041](../../docs/adr/0041-v2-scope-trial-readiness.md) §5）。

也**不能换 musl 静态链接**绕过去：`source` 要经 Oracle OCI 动态加载 Instant Client 的
`libclntsh.so`，那是 glibc 动态库，musl 目标上加载不了。产物必须是 **glibc 动态链接**。

## 一条命令

```sh
packaging/centos7/build.sh
```

它对 **`linux/amd64` 与 `linux/arm64` 各走一遍**：出 `web/dist`（平台无关，两边共用一份）→
构建该平台的 `centos:7` 构建镜像 → 容器里 `cargo build --release --locked` →
把产物丢进**同架构的干净 `centos:7`** 里启动一次。产物落在
`packaging/centos7/out/bin/linux-amd64/` 与 `packaging/centos7/out/bin/linux-arm64/`（不进版本库），
每个目录三个二进制：

| 二进制 | 装在哪 |
| --- | --- |
| `db-qbs-source` | 源端（Web UI + 任务编排） |
| `db-qbs-source-run` | 源端（由 `db-qbs-source` 拉起，跑一趟导入） |
| `db-qbs-sink` | 目标端 |

票面说「两个二进制」，实际是三个：`db-qbs-source-run` 是 `db-qbs-source` 运行时拉起的子进程，
少带它源端就跑不成一趟搬运，所以它跟着一起编、一起验。

常用参数：

```sh
packaging/centos7/build.sh --platform linux/arm64   # 只编一个平台（可重复给）
packaging/centos7/build.sh --skip-web               # 复用现成的 web/dist
packaging/centos7/build.sh --no-verify              # 只编不验
packaging/centos7/verify.sh                         # 只验现成产物（out/bin/ 下有几个平台就验几个）
```

**构建在 mac 的 Docker 上跑**（服务器内存不够，走 `rexec` 派发）。

## 目标架构：两个都出，现场按机器挑

默认把 `linux/amd64` 与 `linux/arm64` **都编出来**，行李清单里两套一起带走——
到了现场 `uname -m` 一看就知道拿哪个目录，不必赌客户那台是什么架构，也不必现场重编
（架构挑错的二进制同样是启动即死，报的不是 GLIBC 而是 `Exec format error`）。

在 mac（arm64）上，`linux/arm64` 是原生编译，`linux/amd64` 走模拟层、慢得多。
两个平台的 cargo target 与 registry 各用一个 docker volume——共用一个会让每次换平台整棵重编。
`arm64` 的 CentOS 7 存档在 vault 的 `altarch/` 下，路径与 x86_64 不同，脚本按平台自动选。

## 镜像里为什么装这些

| 东西 | 为什么 |
| --- | --- |
| vault 存档 yum 源 | CentOS 7 已 EOL（2024-06-30），默认 mirrorlist 是死的，不换源第一条 `yum` 就 404 |
| `gcc` / `make` | `rusqlite` 用 bundled SQLite、`oracle` 用 ODPI-C，两者编译期都要 C 编译器 |
| `binutils` | 产物的 GLIBC 符号上界用 `objdump -T` 静态核对 |
| Rust 工具链（rustup，默认 1.90.0） | workspace 的下界是 1.85（`Cargo.lock` 里的 `zeroize 1.9.0` 要 edition2024，1.83 上实测解析失败）；`x86_64-unknown-linux-gnu` 的 glibc 下界仍是 2.17 |

**Node 不进这个镜像**：Node 18+ 的官方二进制要 glibc 2.28，在 `centos:7` 里根本起不来。
前端资源改由 `node:22` 容器出，`crates/source/build.rs` 在没有 `npm` 时会复用现成的 `web/dist`
（它自己的注释里写着这条退路）。容器里的 `npm ci` 装在临时目录，不碰仓库的 `node_modules`——
免得 linux 的 esbuild / rollup 原生包盖掉 mac 上那份。

## 判据

`build.sh` 静态核对每个产物的 GLIBC 符号上界 ≤ 2.17；`verify.sh` 在干净 `centos:7` 里再验三条：
是 glibc 动态链接、`ldd` 无 `not found`、直接启动不报 GLIBC 错误也不因动态链接器失败退出。
二进制因缺 `--config` 报用法并以 1 退出是**预期**的——那正说明进程已经跑起来了。
