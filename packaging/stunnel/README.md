# stunnel 双端隧道（#153 / ADR-0041 §4）

`source` 把**目标库的 MySQL 口令随每个 run 的请求明文过线**给 `sink`（`ADR-0037` §4）。
那条前提原文是「通道必须可信——同主机、可信内网，或部署者自建 TLS / 隧道」。
第二版第一次让它落到实处：通道是**互联网**，兑现方式是这一套。

同时它让 `sink` 那条兜底原样成立：`ADR-0024`
说 `sink` 不做鉴权、靠**只绑回环**兜底。隧道服务端就落在目标端主机的回环上，
公网上露出来的只有那个要证书才进得来的隧道口——`sink` 的 `listen` 一个字都不用改。

```
 源端主机（CentOS 7）                                  目标端主机（CentOS 7）
 ┌───────────────────────────┐                        ┌───────────────────────────┐
 │ source                    │                        │                           │
 │   sink_base_url =         │                        │                           │
 │   http://127.0.0.1:8080   │                        │                           │
 │        │ 明文，只在回环上  │      公网 · TLS 1.2     │                           │
 │        ▼                  │      双向证书           │                           │
 │ stunnel 客户端 127.0.0.1:8080 ──────────────────▶ 白名单口 :15443 stunnel 服务端 │
 │                           │                        │        │ 明文，只在回环上   │
 │                           │                        │        ▼                  │
 │                           │                        │   sink 127.0.0.1:8080     │
 └───────────────────────────┘                        └───────────────────────────┘
```

**产品代码零改动**：源端填的仍是 `http://127.0.0.1:8080`，scheme 仍是 `http`，
`crates/source/src/protocol.rs` 那条「非 http 一律拒」的校验一个字不动。
明文只在两端各自的回环上走一小段，出机器之前已经进了 TLS。

> **2026-08-24（ADR-0044 §5）**：那个地址不再写在 `source.toml::sink_base_url` 里
> （该字段已退役），而是在界面「目标端 Agent」屏注册一台 agent 时填的 `base_url`。
> **隧道形态一个字未改**，方向也没变（源端拨、目标端听）；改的只是「这个地址存在哪儿、
> 归谁管」。下面凡提到 `sink_base_url` 的地方，读作「注册 agent 时填的地址」。

**为什么不做产品内 TLS**：`source` 的 `ureq` 编译时没带 TLS，加上放开 scheme 校验、
`sink` 侧终结 TLS、四份台架重跑——一周的预算里它挤掉的是装机演练本身。
隧道给到的机密性与它等价，代价是装机多两步。裁定与退役信号见 ADR-0041 §4。

## 目录里有什么

| 文件 | 装到哪 |
| --- | --- |
| `source-side/stunnel-sink.conf` | 源端 `/etc/stunnel/db-qbs/stunnel-sink.conf` |
| `source-side/db-qbs-stunnel.service` | 源端 `/etc/systemd/system/`（**真机**才有 systemd） |
| `target-side/stunnel-sink.conf` | 目标端 `/etc/stunnel/db-qbs/stunnel-sink.conf` |
| `target-side/db-qbs-stunnel.service` | 目标端 `/etc/systemd/system/` |
| `gen-certs.sh` | 不装；跑一次出两端的证书材料，随行李带走 |

证书材料落在 `out/`，**不进版本库**（同目录 `.gitignore`）。

## 装法

下面六步两端各走一遍，不同的地方逐条标了「源端 / 目标端」。
所有命令按 `root` 写（ADR-0041 §8：有 root）。

### 0. 换 yum 源 —— 两端都要，且是第一步

CentOS 7 已 EOL（2024-06-30），`mirrorlist.centos.org` 已停服，**不换源 `yum install stunnel`
直接 404**。换法与构建镜像里那段是同一份：把 repo 指到 `vault.centos.org`，
见 [`../centos7/Dockerfile`](../centos7/Dockerfile) 顶部那条 `RUN`。

> **这一步在演练台上和真机上一模一样**——`centos:7` 容器同样装不上任何包。
> 手册（#155/#156）把它写成两端共同的第 0 步。

### 1. 装 stunnel

```sh
yum -y install stunnel openssl
stunnel -version 2>&1 | head -2      # CentOS 7 给的是 4.56
```

`openssl` 是给 `gen-certs.sh` 和自检用的；只在生成证书那台机器上必需。

### 2. 出证书（**只跑一次，在你自己那台机器上**）

```sh
packaging/stunnel/gen-certs.sh
```

两端各一张自签证书，互相把对方那张钉进 `CAfile`。**私钥不走这条隧道本身传输**，
也不走网络——所有者本人到场装机，两边的文件随身拷贝。

拷法：`out/source-side/*` → 源端，`out/target-side/*` → 目标端。

```sh
mkdir -p /etc/stunnel/db-qbs
cp source.crt source.key target.crt /etc/stunnel/db-qbs/     # 源端
cp target.crt target.key source.crt /etc/stunnel/db-qbs/     # 目标端
chmod 600 /etc/stunnel/db-qbs/*.key
```

装完两端可以拿 `gen-certs.sh` 末尾打的 SHA-256 指纹对一眼，确认拷的是同一批。

### 3. 填模板

模板里的 `@@...@@` 占位符**必须全部填掉**，一个都不能留。

**目标端**（`target-side/stunnel-sink.conf`）：

| 占位符 | 填什么 | 演练台上的值 |
| --- | --- | --- |
| `@@WHITELIST_PORT@@` | 客户开的那个白名单端口 | `15443` |
| `@@SINK_PORT@@` | `sink` 的 `listen` 端口 | `8080` |

**源端**（`source-side/stunnel-sink.conf`）：

| 占位符 | 填什么 | 演练台上的值 |
| --- | --- | --- |
| `@@SINK_LOCAL_PORT@@` | 与 `sink_base_url` 里的端口一致 | `8080` |
| `@@TARGET_HOST@@` | 目标端主机的公网 IP / 域名 | `host.docker.internal` |
| `@@TARGET_PORT@@` | 白名单端口，与目标端的 `@@WHITELIST_PORT@@` 同一个 | `15443` |

```sh
# 目标端
sed -i 's/@@WHITELIST_PORT@@/15443/; s/@@SINK_PORT@@/8080/' \
  /etc/stunnel/db-qbs/stunnel-sink.conf

# 源端（TARGET_HOST 换成客户给的公网 IP / 域名）
sed -i 's/@@SINK_LOCAL_PORT@@/8080/; s/@@TARGET_HOST@@/203.0.113.10/; s/@@TARGET_PORT@@/15443/' \
  /etc/stunnel/db-qbs/stunnel-sink.conf

# 填完两端各查一遍：一个占位符都不许剩
grep -nE '@@[A-Z_]+@@' /etc/stunnel/db-qbs/stunnel-sink.conf && echo '还有没填的！'
```

> `@@TARGET_HOST@@` 里若带 `/`（不会，IP 与域名都不带），`sed` 的分隔符要换成 `|`。

`@@SINK_LOCAL_PORT@@` 与 `sink_base_url` 的端口是**同一个值**：填不一致的话 `source` 会去连一个
没人听的回环端口，报的是「连不上 sink」，而不是「隧道配错了」。

### 4. 起

**真机**（有 systemd）：

```sh
cp db-qbs-stunnel.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now db-qbs-stunnel
systemctl status db-qbs-stunnel
```

**演练台上怎么起**：`centos:7` 容器里没有 systemd，直接

```sh
stunnel /etc/stunnel/db-qbs/stunnel-sink.conf     # 配置里 foreground = no，它自己转后台
```

**起的顺序**：目标端先起（`sink` → stunnel 服务端），源端后起。反过来的话源端起得来
（stunnel 客户端不预连），但第一次搬运会以连接被拒收场。

### 5. 自检

```sh
# 目标端：隧道口在听，sink 只在回环上
ss -ltnp | grep -E '15443|8080'
# 源端：本机隧道入口在听，且经它摸得到 sink
ss -ltnp | grep 8080
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/v1/runs/__tunnel-probe__
```

**最后那条怎么读**：`sink` 没有健康检查端点（路由全集是 `/v1/runs*` 与 `/v1/target/*`），
所以这里故意打一个不存在的 run。**拿到任何 HTTP 状态码就算通**——那说明隧道两端都在、
`sink` 在应答；`404` / `405` 都是预期的。**拿到 `000` 或 `curl` 直接报错才是没通**，
那时按 `目标端 sink 没起 → 目标端 stunnel 没起 → 白名单口不通 → 源端 stunnel 没起` 的顺序往回查。

两端自检脚本（把这些并成「缺什么一次列全」）是 #154 的事，本目录只给隧道这一段。

## 真机上会不一样的地方

演练台是 mac Docker 里的 `centos:7` 容器：root 是默认的、没装过任何东西、网络是通的、
`host.docker.internal` 更是 Docker Desktop 才有的东西。真机上以下四条要现场吸收
（ADR-0041 增补 1 明文接受这个代价：装的人就是写手册的人本人）：

| 差异 | 真机上要做什么 |
| --- | --- |
| **防火墙** | 目标端 `firewall-cmd --permanent --add-port=15443/tcp && firewall-cmd --reload`；容器里没有 firewalld，这一步演练台上撞不到 |
| **SELinux** | `enforcing` 下 stunnel 绑非标准端口可能被拦。`sealert -a /var/log/audit/audit.log` 看一眼；实在不行 `semanage port -a -t http_port_t -p tcp 15443`。**别直接关 SELinux** |
| **目标端地址** | `@@TARGET_HOST@@` 是客户给的公网 IP，不是 `host.docker.internal`。**拿不到它就是装机当天彻底停摆**——这是第二版唯一的外部阻塞项（ADR-0041「风险」） |
| **端口已被占** | 真机上 `8080` 可能已经有别人在听。换端口的话，源端 `accept`、`sink_base_url`、目标端 `connect` 三处要一起改 |

## 演练台上打通它

演练台（两台 `centos:7` 容器）上的一键版本与判据：

```sh
cd docs/spikes/fixtures/local-rig
./scripts/rehearsal-up.sh              # 两台主机就位（前提：./scripts/up.sh 起了两个库）
./scripts/rehearsal-tunnel-up.sh       # 照上面六步在两台主机上装隧道
./scripts/rehearsal-tunnel-check.sh    # 判据 T0–T11，逐条打印实测
```

判据要证的四件事与本票判据一一对应：经隧道到达目标端**回环上**的服务、
公网那一跳走的是**加密**流量（明文打上去拿不到东西，TLS 握手才拿得到）、
目标端除白名单口外不暴露、产品代码零改动。详见
[`../../docs/spikes/fixtures/local-rig/README.md`](../../docs/spikes/fixtures/local-rig/README.md)
的「装机演练台」一节。
