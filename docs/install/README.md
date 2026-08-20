# 装机手册与演练实录

第二版买的是**交付路径**（[ADR-0041](../adr/0041-v2-scope-trial-readiness.md) §1）：
这套东西能被装到客户的机器上并跑通一次。这个目录放的就是「怎么装」与「装过的记录」。

| 文件 | 是什么 |
|---|---|
| [`target-centos7.md`](target-centos7.md) | **目标端装机手册**（[#156](https://github.com/liumingjian/db-qbs/issues/156)）：CentOS 7 上从零装出 `sink`（只绑回环）+ stunnel 服务端，连上 MySQL 8.0。**先装这一台** |
| [`source-centos7.md`](source-centos7.md) | **源端装机手册**（[#155](https://github.com/liumingjian/db-qbs/issues/155)）：CentOS 7 上从零装出 `source` + Oracle Instant Client 19c + stunnel 客户端 |
| [`records/`](records/) | 演练实录。**手册是走过的记录，不是照着想象写的**（ADR-0041 §6） |

**先看行李清单**：[`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)。
两份手册都从它开头，清单只此一份。

**装的顺序是先目标端、后源端**：源端自检的最后一条（S8）要摸到目标端回环上的 sink，反过来装源端那一份的
最后一步转不了绿；目标端自己的自检（D1–D9）不依赖源端。

## 手册怎么被证明是对的

判据是过程性的，不新开台架入口（ADR-0041 §6）：在**干净的** CentOS 7 上**只照手册**装完，
自检从红转绿，跑通一次搬运。**中途任何一次「手册没写、临场解决」都算判据未达成**——
回写手册，重走。

演练台是 mac Docker 上的两台 `centos:7` 容器，起法与判据见
[`docs/spikes/fixtures/local-rig/README.md`](../spikes/fixtures/local-rig/README.md) 的「装机演练台」一节。
每份手册各配一支可执行回放与一支不起台架的静态自检（手册与回放说的必须是同一件事）：

| 手册 | 回放 | 静态自检 |
|---|---|---|
| 目标端 | `scripts/rehearsal-target-install.sh` | `scripts/test-rehearsal-target-install.sh` |
| 源端 | `scripts/rehearsal-source-install.sh` | `scripts/test-rehearsal-source-install.sh` |

演练里**对端由台架准备、本端由人照手册敲**（`rehearsal-tunnel-up.sh --side source|target`）：
脚本代劳本端，「手册是走过的记录」这句话就当场作废。
