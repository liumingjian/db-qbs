# 装机手册与演练实录

第二版买的是**交付路径**（[ADR-0041](../adr/0041-v2-scope-trial-readiness.md) §1）：
这套东西能被装到客户的机器上并跑通一次。这个目录放的就是「怎么装」与「装过的记录」。

| 文件 | 是什么 |
|---|---|
| [`source-centos7.md`](source-centos7.md) | **源端装机手册**（[#155](https://github.com/liumingjian/db-qbs/issues/155)）：CentOS 7 上从零装出 `source` + Oracle Instant Client 19c + stunnel 客户端 |
| `target-centos7.md` | 目标端装机手册（[#156](https://github.com/liumingjian/db-qbs/issues/156)，未落） |
| [`records/`](records/) | 演练实录。**手册是走过的记录，不是照着想象写的**（ADR-0041 §6） |

**先看行李清单**：[`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)。
两份手册都从它开头，清单只此一份。

## 手册怎么被证明是对的

判据是过程性的，不新开台架入口（ADR-0041 §6）：在**干净的** CentOS 7 上**只照手册**装完，
自检从红转绿，跑通一次搬运。**中途任何一次「手册没写、临场解决」都算判据未达成**——
回写手册，重走。

演练台是 mac Docker 上的两台 `centos:7` 容器，起法与判据见
[`docs/spikes/fixtures/local-rig/README.md`](../spikes/fixtures/local-rig/README.md) 的「装机演练台」一节。
源端那一份的可执行回放是 `scripts/rehearsal-source-install.sh`，
不起台架的静态自检是 `scripts/test-rehearsal-source-install.sh`。
