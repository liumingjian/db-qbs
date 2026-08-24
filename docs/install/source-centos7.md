# 源端装机手册 —— CentOS 7 上从零装出 source + Oracle Instant Client 19c

**票**：[#155](https://github.com/liumingjian/db-qbs/issues/155)（规格 [#149](https://github.com/liumingjian/db-qbs/issues/149) E.16/E.17，判定 [ADR-0041](../adr/0041-v2-scope-trial-readiness.md) §6）
**读者**：所有者本人。命令为主，不解释产品是什么。
**目标端那一份**：`target-centos7.md`（[#156](https://github.com/liumingjian/db-qbs/issues/156)）。

> **出发前先过一遍 [`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)。**
> 本手册假设清单上第 1、3、4、6、7、8、9 项已经在你手上（源端要用的那几样），
> 装机现场多半没有可用的外网，**「到时候下一个」在这份手册里一次都不成立**。

## 这份手册的前提

- 机器是**干净的 CentOS 7 x86_64**，有 root，什么都没装过。凡是要用到的东西，下面每一步自己装。
- 目标端主机**已经先装完**（`target-centos7.md`）：`sink` 在它的回环 `8080` 上、
  stunnel 服务端在白名单口上。顺序反过来，源端也装得完，但第 9 步的 S8 转不了绿。
- 全程 `root`。
- **每一处标了 `⚠ 真机差异` 的地方，演练台上撞不到，真机上要现场处置**——
  演练台是 mac Docker 里的 `centos:7` 容器（ADR-0041 增补 1 明文接受这个代价）。

**文件怎么搬进机器**：真机上是 U 盘或 `scp`；演练台上是 `docker cp <本地文件> qbs-host-source:/root/dist/`。
下面一律把它们当作已经躺在 `/root/dist/` 下。

```sh
mkdir -p /root/dist
ls /root/dist
# 期望看到：db-qbs-source  db-qbs-source-run  preflight-source.sh
#           instantclient-basic-linux.x64-19.32.0.0.0dbru.zip
#           stunnel-sink.conf  source.crt  source.key  target.crt
```

---

## 第 1 步：上机第一件事——跑自检，让它先红

**别先装东西。** 先让自检把这台机器缺什么一次列全，再照着清。

```sh
chmod +x /root/dist/preflight-source.sh
QBS_ORACLE_HOST=<客户给的 Oracle 地址> /root/dist/preflight-source.sh
```

`QBS_ORACLE_HOST` 的值取客户那条 Oracle 连接串里的主机（`//主机:1521/服务名` 中间那截）。
**不给它，S5 记「未判定」并算 FAIL**——自检刻意不猜 `127.0.0.1`（猜出来的绿是假绿）。

干净机器上这一趟的期望形状（**不是「一片红」**）：

| 项 | 干净机器上 | 为什么 |
|---|---|---|
| S1 glibc ≥ 2.17 | **PASS** | CentOS 7 的 glibc 本来就是 2.17 |
| S2–S4 Instant Client | FAIL | 还没解包 |
| S5 Oracle 监听口 | PASS（给了 `QBS_ORACLE_HOST` 且这一跳通） | 源端与 Oracle 同内网 |
| S6–S8 隧道 | FAIL | stunnel 还没装 |

> **S1 就红**：这台机器比 CentOS 7 还老，带来的二进制启动即 `GLIBC_2.xx not found`。
> 就地停下，别往下装——处置是换机器或回 `packaging/centos7/` 按这台机器的 glibc 重编。

> **S5 就红**：源端到 Oracle 这一跳不通。这是客户网络的事，先找客户，别自己往下装——
> 后面全装完了它还是红。

---

## 第 2 步：换 yum 源到 vault 存档（**两端共同的第 0 步**）

CentOS 7 已 EOL（2024-06-30），`mirrorlist.centos.org` 已停服。**不换源，`yum install` 第一条就 404。**
演练台在这一点上不比真机宽松，两边一模一样。

```sh
rm -f /etc/yum.repos.d/*.repo
keys=$(ls /etc/pki/rpm-gpg/RPM-GPG-KEY-CentOS-7*)
test -n "$keys"
rpm --import $keys
gpgkey_line=$(printf 'file://%s ' $keys)
VAULT_BASE=http://vault.centos.org/7.9.2009
VAULT_MIRRORS="https://linuxsoft.cern.ch/centos-vault/7.9.2009 https://archive.kernel.org/centos-vault/7.9.2009"
for repo in os:base:Base updates:updates:Updates extras:extras:Extras; do
  dir=${repo%%:*}; rest=${repo#*:}; id=${rest%%:*}; label=${rest#*:}
  urls="$VAULT_BASE/$dir/\$basearch/"
  for m in $VAULT_MIRRORS; do urls="$urls $m/$dir/\$basearch/"; done
  {
    echo "[$id]"
    echo "name=CentOS-7.9.2009 - $label - vault"
    echo "baseurl=$urls"
    echo 'failovermethod=priority'
    echo 'gpgcheck=1'
    echo "gpgkey=$gpgkey_line"
    echo 'enabled=1'
    echo
  } >> /etc/yum.repos.d/CentOS-Vault.repo
done
yum -y makecache fast >/dev/null && echo "yum 源就位"
```

**三个源不是冗余。** 2026-08-20 下午 `vault.centos.org` 前面那层 CDN 对 `*.sqlite.bz2`
回了几个小时 403（同一批 `*.xml.gz` 照常 200），而 yum 优先取 sqlite 元数据——单源时那几个小时
**装不上任何包**。`failovermethod=priority` 让它按上面的顺序退，yum 默认的 `roundrobin` 是随机挑起点，
「vault 优先」不成立。

**别关 `gpgcheck`。** 这是装到客户机上的东西的来源凭证。

> ⚠ **真机差异 ①：机器上可能已经有能用的源。**
> 客户内网常自建 CentOS 镜像站，`/etc/yum.repos.d/` 里那份是活的。
> 先跑一句 `yum -y makecache fast`：**能成就整步跳过**，别把客户配好的源删了。
> 上面第一条 `rm -f /etc/yum.repos.d/*.repo` 是不可逆的，动手前先 `cp -a /etc/yum.repos.d /root/yum.repos.d.bak`。

> ⚠ **真机差异 ②：机器可能上不了外网。**
> 那就走行李清单第 8 项的离线 rpm：`yum -y localinstall /root/dist/rpm/*.rpm`，第 3 步的 `yum install` 换成它。
> 出发前把 `libaio`、`stunnel`、`openssl`、`unzip`、`curl`、`iproute` 连同依赖下全
> （`yumdownloader --resolve --destdir=... libaio stunnel openssl unzip curl iproute`，在一台能上网的 CentOS 7 上跑）。

---

## 第 3 步：装那几个包

```sh
yum -y install libaio stunnel openssl unzip curl iproute
rpm -q libaio stunnel openssl unzip curl iproute
```

| 包 | 干什么用 |
|---|---|
| `libaio` | **Instant Client 的硬依赖**。缺它 S4 红在 `libaio.so.1 not found`，而报错只在连库那一刻才出现 |
| `stunnel` | 第 6 步的隧道客户端。CentOS 7 给的是 4.56 |
| `openssl` | 核对证书指纹用 |
| `unzip` | 解 Instant Client 的 zip |
| `curl` | 第 10 步没有浏览器时的排障命令。base 镜像里通常已经有，**但「通常已经有」不是干净机器该依赖的东西** |
| `iproute` | 给 `ss`。**干净的 CentOS 7 上没有它**（演练台实测 `ss: command not found`），而下面好几处都要 `ss -ltnp` 看端口 |

> ⚠ **真机差异 ③：这些包可能已经装过、且版本不同。**
> `rpm -q` 报 already installed 就算过，别 `yum reinstall`——客户机上别的东西可能正靠着它。
> 只有 `stunnel` 值得看一眼版本：`stunnel -version 2>&1 | head -2`
> （**它以 1 退出**，别在 `set -e` 的脚本里直接调）。

---

## 第 4 步：铺 Oracle Instant Client 19c

```sh
uname -m        # 必须是 x86_64；与你带来的 Instant Client 包的架构对上
mkdir -p /opt/oracle
unzip -oq /root/dist/instantclient-basic-linux.x64-19.32.0.0.0dbru.zip -d /opt/oracle
ln -sfn /opt/oracle/instantclient_19_32 /opt/oracle/instantclient
ls -l /opt/oracle/instantclient/libclntsh.so*
```

**接着把这个目录注册进动态链接器——这一步不能省：**

```sh
echo /opt/oracle/instantclient > /etc/ld.so.conf.d/oracle-instantclient.conf
ldconfig
ldconfig -p | grep -c 'libclntsh.so'                          # 期望：≥ 1
ldd /opt/oracle/instantclient/libclntsh.so | grep -c 'not found'   # 期望：0
```

**为什么单解包不够**：产品（ODPI-C）按全路径 `dlopen` 的只有 `libclntsh.so` 一个，
它自己还要拉起同一个包里的 `libnnz19.so` / `libclntshcore.so`，而那几个由**动态链接器按它自己的
搜索路径**去找——目录没进 `ldconfig`，产品就会在「测试连接」那一刻报
`DPI-1047 ... libnnz19.so: cannot open shared object file`。
**2026-08-20 的源端演练上就是这么撞到的**：自检 S1–S8 全绿之后，测试连接当场炸在这一条。
最后那句 `ldd` 是判它的：**别加 `LD_LIBRARY_PATH`**——加了就查不出这件事，
查的也不是产品会遇到的那条搜索路径。

`unzip` 一定带 `-o`：包里含 `META-INF/MANIFEST.MF`，不加会在无 tty 环境下卡在覆盖确认并以 1 退出。

**为什么是软链而不是直接用 `instantclient_19_32`**：自检与 `source.toml` 都指 `/opt/oracle/instantclient`，
换小版本时只改这一根软链（`ld.so.conf.d` 那一行也跟着不用改）。

> **手工建软链是这台机器上最容易手滑的一步**：指错了 `ls` 照样列得出来，但 `dlopen` 打不开。
> 自检 S2 专门判「软链解得开」，别跳过第 9 步。

---

## 第 5 步：铺两个二进制

源端要两个：`db-qbs-source`（Web UI + 任务编排）与 `db-qbs-source-run`（由前者拉起，跑一趟导入）。
**少带 `db-qbs-source-run`，界面全都在、但一趟搬运都跑不成**——它默认在 `db-qbs-source` 旁边找。

```sh
mkdir -p /opt/db-qbs/bin
install -m 0755 /root/dist/db-qbs-source /root/dist/db-qbs-source-run /opt/db-qbs/bin/
/opt/db-qbs/bin/db-qbs-source; echo "exit=$?"
# 期望：打一行用法（--config 没给），exit=1。
# 这就是「它跑得起来」的证据 —— 报 GLIBC_2.xx not found 或 Exec format error 才是坏的。
```

| 看到什么 | 是什么事 |
|---|---|
| 用法 + `exit=1` | 正常，进下一步 |
| `GLIBC_2.28 not found` | 二进制不是在 `centos:7` 里编的。回 `packaging/centos7/build.sh` 重编，别在现场想办法 |
| `Exec format error` | 架构拿错了。`uname -m` 对一眼，换 `out/bin/linux-<arch>/` 下的那一套 |

---

## 第 6 步：装 stunnel 客户端（隧道的源端那一头）

装法的权威是 [`packaging/stunnel/README.md`](../../packaging/stunnel/README.md)，这里是它的源端摘要。

```sh
mkdir -p /etc/stunnel/db-qbs
cp /root/dist/source.crt /root/dist/source.key /root/dist/target.crt /etc/stunnel/db-qbs/
cp /root/dist/stunnel-sink.conf /etc/stunnel/db-qbs/stunnel-sink.conf
chmod 600 /etc/stunnel/db-qbs/*.key
```

填掉模板里的三个占位符（**一个都不能留**）：

| 占位符 | 填什么 |
|---|---|
| `@@SINK_LOCAL_PORT@@` | `8080` —— 必须与第 10.5 步注册 agent 时填的地址端口**同一个值** |
| `@@TARGET_HOST@@` | 客户给的目标端**公网 IP / 域名** |
| `@@TARGET_PORT@@` | 客户开的白名单端口，与目标端那份配置里的 `@@WHITELIST_PORT@@` 同一个 |

> ⚠ **真机差异 ④：目标端地址。**
> `@@TARGET_HOST@@` 填的是**客户给的公网 IP / 域名**，演练台上那个 `host.docker.internal`
> 是 Docker Desktop 才有的东西。**拿不到这个地址就是装机当天彻底停摆**——
> 它是第二版唯一的外部阻塞项（ADR-0041「风险」），出发前就要跟客户要到。

```sh
sed -i 's/@@SINK_LOCAL_PORT@@/8080/; s|@@TARGET_HOST@@|203.0.113.10|; s/@@TARGET_PORT@@/15443/' \
  /etc/stunnel/db-qbs/stunnel-sink.conf
# 填完查一遍：注释行（; 开头）里那句提示自己就带 @@ 字样，所以要先把它滤掉
grep -vE '^[[:space:]]*;' /etc/stunnel/db-qbs/stunnel-sink.conf | grep -nE '@@[A-Z_]+@@' \
  && echo '还有没填的！' || echo '占位符已填完'
grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf
```

证书对不对，用指纹核一眼（与目标端那台上的同一条命令输出必须一致）：

```sh
openssl x509 -in /etc/stunnel/db-qbs/target.crt -noout -fingerprint -sha256
```

起它：

```sh
stunnel /etc/stunnel/db-qbs/stunnel-sink.conf     # 配置里 foreground = no，它自己转后台
sleep 1; cat /var/run/db-qbs-stunnel-sink.pid
tail -5 /var/log/db-qbs-stunnel-sink.log
```

> ⚠ **真机差异 ⑤：真机上走 systemd，不这么起。**
> ```sh
> cp /root/dist/db-qbs-stunnel.service /etc/systemd/system/
> systemctl daemon-reload && systemctl enable --now db-qbs-stunnel
> systemctl status db-qbs-stunnel
> ```
> `enable` 那半截是**重启后还在**，容器里没有 systemd，这一条演练台上验不到。

> ⚠ **真机差异 ⑥：SELinux。**
> `enforcing` 下 stunnel 绑非标准端口可能被拦：`getenforce` 看一眼，
> 被拦时 `sealert -a /var/log/audit/audit.log` 找那条 AVC，
> 实在不行 `semanage port -a -t http_port_t -p tcp 8080`。**别直接关 SELinux。**

> ⚠ **真机差异 ⑦：`8080` 可能已经有人在听。**
> `ss -ltnp | grep 8080` 先看。被占了就整体换一个端口，
> **源端 `accept`、注册 agent 时填的地址、目标端的 `connect` 三处要一起改**——
> 只改一处的话，报出来的是「连不上 sink」，而不是「端口配错了」。

> ⚠ **真机差异 ⑧：防火墙。**
> **源端不需要开任何入向口**——它只往外连；要 `firewall-cmd --permanent --add-port=15443/tcp`
> 的是目标端那台（见 `target-centos7.md`）。源端这边只要确认**出向**没被拦：
> ```sh
> systemctl is-active firewalld            # 容器里没有 firewalld，这一步演练台上撞不到
> firewall-cmd --list-all 2>/dev/null | grep -i 'egress\|rich rule'
> ```
> 客户内网若对出向做白名单，要放行的是**目标端公网 IP 的那个白名单端口**，不是 8080。

---

## 第 7 步：写 `source.toml`

```sh
mkdir -p /etc/db-qbs /var/lib/db-qbs-source
cat > /etc/db-qbs/source.toml <<'EOF'
oracle_client_lib_dir = "/opt/oracle/instantclient"
listen = "127.0.0.1:8088"
data_dir = "/var/lib/db-qbs-source"
EOF
chmod 0600 /etc/db-qbs/source.toml
```

三行，逐行的账：

- `oracle_client_lib_dir` —— ODPI-C 的 client 库**一个进程只初始化一次**，所以它是进程级配置、
  不是数据源级字段（ADR-0037 §6）。指第 4 步那根软链。
- `listen` —— 只绑回环（ADR-0024：两处 `listen` 都无鉴权，兜底就是只绑回环）。
- `data_dir` —— 数据源、agent 注册表、任务、运行历史都落这儿的 SQLite，口令加密落盘。
  **这是要备份的那个目录。**

**不要写 `sink_base_url`。** 它已退役（ADR-0044 §5）：目标端地址不再是进程级配置，
而是**逐条数据源绑定的 agent**，在第 10.5 步用界面注册。写了它，首次启动会凭空多出一条
名叫「默认」的 agent——那条路径是给**升级**的老部署准备的，新装的机器不该走。

**也不要写 `oracle_connect_string` / `oracle_username` / `oracle_password`。**
这三个字段已退役（ADR-0037 §10），只在首次启动且数据源表为空时被迁成一条叫「默认」的数据源——
写了就会凭空多出一条你没建过的数据源。Oracle 连接信息在第 10 步用界面建。

> ⚠ **真机差异 ⑨：你要从自己的笔记本看那个界面。**
> `listen` 只绑回环是故意的，别改成 `0.0.0.0`（能连上这个端口 = 持有该 source 的 Oracle 凭据
> 与目标端写权限）。从笔记本开一条 SSH 端口转发就够了：
> ```sh
> ssh -L 8088:127.0.0.1:8088 root@<源端主机>
> ```
> 然后本机浏览器开 `http://127.0.0.1:8088`。

---

## 第 8 步：起 source

```sh
nohup /opt/db-qbs/bin/db-qbs-source --config /etc/db-qbs/source.toml \
  >> /var/log/db-qbs-source.log 2>&1 &
sleep 2; tail -20 /var/log/db-qbs-source.log
```

> ⚠ **真机差异 ⑩：真机上做成 systemd 服务，别 `nohup`。**
> ```sh
> cat > /etc/systemd/system/db-qbs-source.service <<'EOF'
> [Unit]
> Description=db-qbs source
> After=network-online.target db-qbs-stunnel.service
> Wants=network-online.target
>
> [Service]
> ExecStart=/opt/db-qbs/bin/db-qbs-source --config /etc/db-qbs/source.toml
> Restart=on-failure
> RestartSec=5
>
> [Install]
> WantedBy=multi-user.target
> EOF
> systemctl daemon-reload && systemctl enable --now db-qbs-source
> systemctl status db-qbs-source
> journalctl -u db-qbs-source -n 50 --no-pager
> ```
> 容器里没有 systemd，**这一段演练台上没验过**，真机上第一次用要盯着 `systemctl status`。

---

## 第 9 步：再跑一遍自检——这次要全绿

```sh
QBS_ORACLE_HOST=<客户给的 Oracle 地址> /root/dist/preflight-source.sh; echo "exit=$?"
```

八条各查什么，红了才好知道该往哪儿看：

| | 查什么 | 装在第几步 |
|---|---|---|
| S1 | glibc ≥ 2.17 | —（机器自带） |
| S2 | Instant Client 目录里有 `libclntsh.so` 且软链解得开 | 4 |
| S3 | Instant Client 架构与本机一致 | 4 |
| S4 | 动态依赖全解析得开（`libaio`、`ldconfig`） | 3 / 4 |
| S5 | Oracle 监听口可达 | —（客户网络） |
| S6 | stunnel 客户端进程在跑 | 6 |
| S7 | 本机隧道入口在听 | 6 |
| S8 | 经隧道摸得到目标端的 **sink**（认它的 `RUN_UNKNOWN`） | 6 + 目标端那台 |

期望 `S1–S8 全 PASS`、`exit=0`。**红一条就在这儿停下**，每条 FAIL 自带一行处置，清完重跑。
别带着红往下走——「装到一半炸出下一个缺口」正是这支脚本要消灭的东西。

几条常见的红与它们真正的成因：

| 红在哪 | 十有八九是 |
|---|---|
| S4 缺 `libaio.so.1` | 第 3 步的 `libaio` 没装上 |
| S4 缺 `libnnz19.so`（说「加上目录就都在」） | 第 4 步的 `ldconfig` 那一段没做 |
| S6 pid 文件指不到活进程 | stunnel 没起来。看 `/var/log/db-qbs-stunnel-sink.log`；配置里还留着占位符是最常见的一种 |
| S7 隧道入口不通 | `accept` 的端口与注册 agent 时填的端口对不上 |
| S8「隧道口连得上但没有应答」 | **目标端那一头的事**：先去目标端跑 `preflight-target.sh` |
| S8「应答不是 sink」 | 隧道通到了别的服务：核对源端 `connect` 与目标端 `accept` / `connect` 三个端口 |

**自检证不了的那一档**：Oracle 的账号 / 口令 / 服务名对不对。Basic 包不带 `sqlplus`，
也不给客户机加装它——那一档在第 10 步用产品自己的连接路径证。

---

## 第 10 步：连上 Oracle（数据源 + 测试连接）

按第 7 步那条 SSH 转发开 `http://127.0.0.1:8088` → **数据源** → 新建 Oracle 数据源
→ 填连接串 / 用户名 / 口令 → **测试连接**。走的是产品自己的 Oracle 连接路径（ADR-0037 §9），
它绿了才算「这台机器连得上客户的库」。

没有浏览器时的等价命令（现场排障用）：

```sh
# 只测不存
curl -sS -X POST http://127.0.0.1:8088/api/datasources/test-connection \
  -H 'Content-Type: application/json' \
  -d '{"name":"生产 Oracle","kind":"oracle","connect_string":"//<主机>:1521/<服务名>","username":"<用户>","password":"<口令>"}'

# 建一条（口令加密落进 data_dir 的 SQLite，接口回参里没有口令、连密文都没有）
curl -sS -X POST http://127.0.0.1:8088/api/datasources \
  -H 'Content-Type: application/json' \
  -d '{"name":"生产 Oracle","kind":"oracle","connect_string":"//<主机>:1521/<服务名>","username":"<用户>","password":"<口令>"}'
```

`connect_string` 是 Easy Connect 写法：`//主机:端口/服务名`。

| 报什么 | 是什么事 |
|---|---|
| `ORA-12541 TNS:no listener` | 端口对了、监听没起或端口填错（S5 只判 TCP 通不通，不判监听在不在） |
| `ORA-12514` | 服务名不对 |
| `ORA-01017` | 用户名 / 口令不对 |
| `DPI-1047` 找不到 Oracle Client 库 | `oracle_client_lib_dir` 指错、`libaio` 没装，或**第 4 步的 `ldconfig` 那一段没做**（报的是 `libnnz19.so`） |

---

## 第 10.5 步：注册目标端 Agent（ADR-0044）

**目标库只能经 agent 访问**，所以这一步不做，后面建 MySQL 数据源时会无处可选。

界面 → **目标端 Agent** → 注册 Agent：

| 填什么 | 填成什么 |
|---|---|
| 名称 | 随便起，认得出是哪台目标端主机就行（留空则用 agent 自报的主机名） |
| 地址 | **`http://127.0.0.1:8080`——本机隧道入口，不是目标端的公网地址**。明文只在回环上走一小段，出机器之前已经进了 TLS。协议只收 `http`，填 `https://` 会被当场拒 |

**保存即连接**：source 当场打一次 `GET /v1/agent/info`，探通才落库；探不通就报「连不上这个地址上的
目标端 agent」，库里不留痕。所以这一步转绿，等于隧道 + 目标端 agent 两段一起验过了。

没有浏览器时的等价命令：

```sh
curl -sS -X POST http://127.0.0.1:8088/api/agents \
  -H 'Content-Type: application/json' \
  -d '{"name":"目标端 A","base_url":"http://127.0.0.1:8080"}'
```

回参里的 `instance_id` 就是那台 agent 自报的身份，source 已经把它钉在这条记录上了：
日后同一个地址上换了另一台 agent 应答，界面会显示「身份不符」并停掉所有目标端链路，
**而不是照常放行**。

| 报什么 | 是什么事 |
|---|---|
| `连不上 agent：…connection refused` | 隧道没起（第 6 步）、或目标端主机上的 `db-qbs-sink` 没起 |
| `这个地址回了 HTTP 4xx/5xx，它多半不是 db-qbs 的目标端 agent` | 8080 上听着的是别的东西；核对 stunnel 的 `accept` 端口 |
| `agent 地址必须是 http://` | 填成了 `https://`——TLS 由隧道给，产品这一侧不做 |

建 MySQL 数据源时，「目标端 Agent」是**必选项**；只注册了一台时界面会直接预选它。

---

## 收尾核对

```sh
QBS_ORACLE_HOST=<Oracle 地址> /root/dist/preflight-source.sh   # S1–S8 全 PASS
ss -ltnp | grep -E '8080|8088'                                 # 两个口都只在 127.0.0.1 上
ls -l /etc/db-qbs/source.toml /etc/stunnel/db-qbs/source.key   # 都是 0600
```

- [ ] 自检 S1–S8 全绿、退出码 0
- [ ] 界面「目标端 Agent」里那台是**在线**（第 10.5 步）
- [ ] 界面「测试连接」对客户的 Oracle 绿了
- [ ] `8088` 与 `8080` 都只绑回环
- [ ] `source.toml` 与私钥都是 `0600`
- [ ] 真机上：`db-qbs-stunnel` 与 `db-qbs-source` 两个 unit 都 `enable` 了（重启后还在）

---

## 真机差异一览（演练台上撞不到的十处）

按手册里出现的先后排；每一条在正文里都有一个 `⚠ 真机差异 <编号>` 的标记。

| # | 差异 | 真机上要做什么 | 在第几步 |
|---|---|---|---|
| ① | 已有可用的 yum 源 | 先 `yum makecache`，能成就整步跳过；动手前备份 `/etc/yum.repos.d` | 2 |
| ② | 机器上不了外网 | 走离线 rpm（行李清单第 8 项），`yum localinstall` | 2 / 3 |
| ③ | 包已经装过、版本不同 | `rpm -q` 报 installed 就算过，别 reinstall | 3 |
| ④ | 目标端地址 | `@@TARGET_HOST@@` 是客户给的公网 IP，不是 `host.docker.internal`。**拿不到它就是装机当天彻底停摆**（ADR-0041「风险」） | 6 |
| ⑤ | systemd（stunnel） | 做成 unit 并 `enable --now`；容器里没有 systemd，这一段没验过 | 6 |
| ⑥ | SELinux | `getenforce`；被拦时按 AVC 处置，**别关 SELinux** | 6 |
| ⑦ | `8080` 已被占 | 换端口，源端 `accept` / agent 注册地址 / 目标端 `connect` 三处一起改 | 6 |
| ⑧ | 防火墙 | 源端只需确认**出向**放行到目标端那个口；入向口在目标端那台 | 6 |
| ⑨ | 界面要从笔记本看 | `ssh -L 8088:127.0.0.1:8088`，**不改 `listen`** | 7 |
| ⑩ | systemd（source） | 同 ⑤，另加 `Restart=on-failure`；容器里没有 systemd，这一段没验过 | 8 |

**「重启后还在不在」全靠 ⑤ 与 ⑩ 里的 `enable`**——那是演练台上唯一验不到、
而现场一定会被用到的一件事（客户机迟早要重启一次）。

---

## 演练实录

本手册的每一行都在演练台的源端主机容器上走过，实录在 [`records/`](records/)。
**手册是走过的记录，不是照着想象写的**（ADR-0041 §6：任何一次「手册没写、临场解决」都算判据未达成，
回写手册、重走）。

演练台怎么起、怎么复现，见
[`docs/spikes/fixtures/local-rig/README.md`](../spikes/fixtures/local-rig/README.md) 的「装机演练台」一节。
