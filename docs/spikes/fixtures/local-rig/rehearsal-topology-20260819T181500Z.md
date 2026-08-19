# 演练台拓扑实录 —— #152（V2③）

- **日期（UTC）**：2026-08-19T18:15:00Z
- **在哪跑**：所有者的 mac，Docker Desktop 29.3.1（rexec 派发；ADR-0041 增补 1「演练台改用 mac Docker」）
- **判定来源**：`docs/adr/0041-v2-scope-trial-readiness.md` §8 的事实前提、规格 [#149](https://github.com/liumingjian/db-qbs/issues/149) C.10
- **驱动脚本**：`scripts/rehearsal-topology-check.sh`（进仓库，视觉门禁通则 4 的同一条纪律）

## 拓扑

```
  qbs-host-source ── qbs-src-side ── qbs-oracle11
        │
        └── 宿主 127.0.0.1:15443（扮演客户侧白名单端口）──▶ qbs-host-target:15443
                                                    │
                          qbs-host-target ── qbs-dst-side ── qbs-mysql8
```

## R0–R10 实测（`./scripts/rehearsal-topology-check.sh`，原样抄回）

```
==> 前置：这套判据的「公网一跳」靠 Docker Desktop 的 host.docker.internal 打到宿主回环
    Docker OperatingSystem = Docker Desktop
==> R1 两台主机容器在跑（在此之前一切判据都不成立）
  R1a  PASS  源端主机 qbs-host-source                    实测=running
  R1b  PASS  目标端主机 qbs-host-target                  实测=running
==> R0 两台主机跟客户机同架构、同 glibc 下界
  R0a  PASS  源端主机架构                                实测=x86_64
  R0b  PASS  目标端主机架构                              实测=x86_64
  R0c  PASS  源端主机 glibc（客户机的硬下界）            实测=2.17
  R0d  PASS  目标端主机 glibc                            实测=2.17
==> R9 干净态：留痕迹 → 先确认痕迹真的留下了 → 一键推倒重建 → 痕迹应当消失
  R9a  PASS  重建前源端主机上的痕迹文件                  实测=有
  R9b  PASS  重建前目标端主机上的痕迹文件                实测=有
    推倒重建中（rehearsal-reset.sh）……
  R9c  PASS  重建后源端主机上的痕迹文件                  实测=无
  R9d  PASS  重建后目标端主机上的痕迹文件                实测=无
==> R2–R5 各自够得着自己那一侧的库，够不着对面那一侧
  R2   PASS  源端主机 → Oracle 1521                      实测=通
  R3   PASS  源端主机 → MySQL 3306（必须摸不到）         实测=不通
  R4   PASS  目标端主机 → MySQL 3306                     实测=通
  R5   PASS  目标端主机 → Oracle 1521（必须摸不到）      实测=不通
==> R7a/R8a 正对照：目标端两个监听端确实活着
  R7a  PASS  目标端主机自连 15443（监听端活着）          实测=QBS-REHEARSAL-WHITELIST
  R8a  PASS  目标端主机自连 15444（监听端活着）          实测=QBS-REHEARSAL-BLOCKED
==> R6 跨容器直达被切断：监听端就在那儿听着，源端直连仍必须摸不到
  R6   PASS  源端主机 → qbs-host-target:15443（直达）    实测=无
==> R7–R8 公网那一跳只能走暴露端口（白名单），别的端口摸不到
  R7   PASS  源端主机 → 宿主:15443 → 目标端（白名单口）  实测=QBS-REHEARSAL-WHITELIST
  R8   PASS  源端主机 → 宿主:15444（在听但没暴露）       实测=不通
==> R10 收尾：探针监听端回收干净，15443 留给 #153 的 stunnel 服务端
  R10  PASS  目标端主机自连 15443（应已无人听）          实测=不通

==== 拓扑判据：PASS=20 FAIL=0 ====
```

R9 是**先留痕迹、确认痕迹真的在、再跑 `rehearsal-reset.sh`、然后看它没了**——一键推倒重建回
干净态的实证；R0–R8 判的都是重建之后那对全新容器；R10 证明这套判据自己不留脏东西。

## 两条负判据本来是假绿的，被 `/code-review` 逮到并修掉

留档，因为它们是这类拓扑判据的通病——**「不通」有太多种不成立的成因**：

1. **R6 曾经在监听端起来之前判**。那时目标端 15443 上根本没人听，就算两台主机被误挂到同一张
   网上，R6 照样得出「不通」而 PASS。改成：先起监听端、先用 R7a 确认它活着，再判源端直连。
2. **`read_token` 会把「摸不到」读成二进制垃圾**。`exec 3<>/dev/tcp/...` 失败**并不会**让 bash
   退出，后面的 `read -r line <&3` 于是去读 `docker exec` 继承进来的那个 fd 3——实测读出一个
   ELF 头，R6 的实测值是字符串 `ELF`。改成显式判 `exec` 的退出码、并先把 fd 3 关掉。
3. 连带修掉的还有：R8 缺正对照（改为 R8a）、探针监听端不回收会占死 #153 要用的 15443
   （改为收尾回收并加判 R10）、R9 在容器缺席时假 PASS（改为先判 R9a/R9b）、
   取值失败会让 `set -e` 把脚本掐断（所有探针改成「取不到也给得出值」）、
   Rosetta 下定长 `sleep 2` 等 python 起来不够（改成轮询）。

## 既有台架不受影响（#152 判据 3）

- **本次实测：既有容器没有被重建**——`docker compose up -d` 把 `qbs-src-side` / `qbs-dst-side`
  原地挂到了在跑的 `qbs-oracle11` / `qbs-mysql8` 上，库里 M1/M2/M3/v1 的表与数据原样还在，
  冒烟 5 项全过（`t_bulk_probe` 10 万行、`@fa` dblink 通、目标端 utf8mb4 就位）。
  **这是一次观察，不是对下一台机器的保证**：networks 列表进 compose 的配置哈希，别的版本上
  可能会走重建；真重建了也不是事故（两个库本来就没有数据卷，`up.sh` 照 initdb 重灌），
  代价是几分钟 + 上一轮验收留给视觉走查的那批数据没了。README 已把这条写进正文。
- **四份台架在本次改动之后实跑，全绿**（同一套容器上串行）：

  | 台架 | 结果 |
  |---|---|
  | M1（`run-m1-acceptance.sh`） | **PASS 9/9**，类型逐格 36/36 |
  | M2（`run-m2-acceptance.sh`） | **A1–A14 全绿**：12 条 PASS，A3/A6 照旧 `N/A（判据已随 ADR-0036 §5 退役）` |
  | M3（`run-m3-acceptance.sh`） | **PASS 6/6**（B1–B6） |
  | v1（`run-v1-acceptance.sh`） | **PASS 6/6**（C1–C6，含 C6 内存形状） |

## 一处顺手修掉的真缺口：`down.sh` 会留下半拆的台架

`docker compose down` **不管 profile 里的服务**。实测（`--dry-run`）只拆 client/oracle/mysql，
两台演练主机原地不动，接着三张网都因「Resource is still in use」删不掉。
`scripts/down.sh` 因此改成 `docker compose --profile rehearsal down -v --remove-orphans`——
profile 只在 `up` 时挑谁启动，在 `down` 这里纯粹是扩大清扫范围，对四份台架零影响。

## 三份视觉走查

**未跑**：本票零 UI 代码改动（改的是台架 compose、三支演练脚本、`down.sh` 与本目录 README），
既不碰 `docs/design-system/`，也不碰 `.precheck-reports` / `DiagnosticTable` / 数据源屏 /
构建器映射列 / 运行历史重跑入口 —— `CLAUDE.md` 三张触发表一条都没响（通则 3）。

## 边界 —— 这套演练台不能答什么

- `host.docker.internal` 是 Docker Desktop 才有的东西；真机上对应的是客户给的公网 IP 与
  白名单端口。判据脚本会先打印 `docker info` 的 OperatingSystem，不是 Desktop 就点名说清楚
  R7 为什么会恒 FAIL，而不是丢一份看不懂的红。
- 容器里 root 是默认的、没装过任何东西、网络是通的——真机上最常卡人的恰恰是这三样
  （ADR-0041 增补 1 明文接受这个代价：装的人就是写手册的人本人）。
  **yum 源不算差异**：CentOS 7 已 EOL，容器和真机同样得先改到 vault 才装得上包。
- 本票只搭拓扑，**没有跑任何搬运**：隧道在 #153、自检在 #154、手册与实录在其后。
