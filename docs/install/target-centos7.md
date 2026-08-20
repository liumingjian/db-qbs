# 目标端装机手册 —— CentOS 7 上从零装出 sink（只绑回环）+ stunnel 服务端，连上 MySQL 8.0

**票**：[#156](https://github.com/liumingjian/db-qbs/issues/156)（规格 [#149](https://github.com/liumingjian/db-qbs/issues/149) E.16/E.17，判定 [ADR-0041](../adr/0041-v2-scope-trial-readiness.md) §6）
**读者**：所有者本人。命令为主，不解释产品是什么。
**源端那一份**：[`source-centos7.md`](source-centos7.md)（[#155](https://github.com/liumingjian/db-qbs/issues/155)）。

> **出发前先过一遍 [`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)。**
> 本手册假设清单上第 2、5、6、7、8、9、11 项已经在你手上（目标端要用的那几样），
> 装机现场多半没有可用的外网，**「到时候下一个」在这份手册里一次都不成立**。

## 这份手册的前提

- 机器是**干净的 CentOS 7 x86_64**，有 root，什么都没装过。凡是要用到的东西，下面每一步自己装。
- **先装这一台，再装源端**：源端自检的 S8 要摸到这台机器上的 sink，顺序反过来源端那一份的最后一步转不了绿。
  这台机器自己的自检（D1–D9）**不依赖源端**，装完就能全绿。
- 这台机器**够得着客户的 MySQL 8.0**（同一内网），并且你手上有目标库的**账号 / 口令 / 库名**。
- 全程 `root`。
- **每一处标了 `⚠ 真机差异` 的地方，演练台上撞不到，真机上要现场处置**——
  演练台是 mac Docker 里的 `centos:7` 容器（ADR-0041 增补 1 明文接受这个代价）。

**文件怎么搬进机器**：真机上是 U 盘或 `scp`；演练台上是 `docker cp <本地文件> qbs-host-target:/root/dist/`。
下面一律把它们当作已经躺在 `/root/dist/` 下。

```sh
mkdir -p /root/dist
ls /root/dist
# 期望看到：db-qbs-sink  preflight-target.sh
#           stunnel-sink.conf  db-qbs-stunnel.service  target.crt  target.key  source.crt
```

`stunnel-sink.conf` 是 **`packaging/stunnel/target-side/`** 下那一份（服务端模板）——两端的模板同名不同内容，
拷错了第 8 步的占位符对不上、填不进去。

---

## 第 1 步：上机第一件事——跑自检，让它先红

**别先装东西。** 先让自检把这台机器缺什么一次列全，再照着清。

```sh
chmod +x /root/dist/preflight-target.sh
umask 077; printf '%s\n' '<MySQL 口令>' > /root/.qbs-mysql-pass     # 口令走文件，不落 shell 历史
QBS_MYSQL_HOST=<客户给的 MySQL 地址> QBS_MYSQL_USER=<账号> QBS_MYSQL_DATABASE=<库名> \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass /root/dist/preflight-target.sh
```

四个变量**一个都别省**：`QBS_MYSQL_HOST` 不给它默认猜 `127.0.0.1`（MySQL 多半不在这台机器上，D1 会红在一个假地址上）；
账号 / 库名 / 口令不给，D4–D7 记「未判定」并算 FAIL——未判定不许记 PASS，自检刻意不替环境作保证。

干净机器上这一趟的期望形状（**不是「一片红」**）：

| 项 | 干净机器上 | 为什么 |
|---|---|---|
| D1 MySQL 监听口可达 | **PASS** | 目标端与 MySQL 同内网，这一跳本来就通 |
| D2–D3 sink | FAIL | 还没装 |
| D4–D7 开连接仪式三前提 | FAIL（未判定） | 三项是**问 sink 要的**，sink 没起就判不了 |
| D8–D9 stunnel 服务端 | FAIL | 还没装 |

> **D1 就红**：这台机器到 MySQL 这一跳不通。这是客户网络 / MySQL `bind-address` 的事，先找客户 DBA，
> 别自己往下装——后面全装完了 D4–D7 还是红。

---

## 第 2 步：换 yum 源到 vault 存档（**两端共同的第 0 步**）

CentOS 7 已 EOL（2024-06-30），`mirrorlist.centos.org` 已停服。**不换源，`yum install` 第一条就 404。**
演练台在这一点上不比真机宽松，两边一模一样。这一段与源端手册第 2 步**一字不差**。

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
> 目标端要的是六个里的四个（`stunnel` `openssl` `curl` `iproute`），但清单是两端一份、一起带。

---

## 第 3 步：装那几个包

```sh
yum -y install stunnel openssl curl iproute
rpm -q stunnel openssl curl iproute
```

| 包 | 干什么用 |
|---|---|
| `stunnel` | 第 8 步的隧道服务端。CentOS 7 给的是 4.56 |
| `openssl` | 核对证书指纹；第 10 步从公网侧握手排障也靠它 |
| `curl` | 本机排障 sink 用（第 6 步、第 9 步）。base 镜像里通常已经有，**但「通常已经有」不是干净机器该依赖的东西** |
| `iproute` | 给 `ss`。**干净的 CentOS 7 上没有它**（源端演练实测 `ss: command not found`），收尾核对要靠 `ss -ltnp` 看端口 |

**不装 MySQL 客户端，也别装。** CentOS 7 base 源里的 `mariadb` 客户端是 5.5 那一代，对 MySQL 8.0 默认的
`caching_sha2_password` 认不了——装上去连不上，红出来的是一条假故障。三项开连接仪式前提（第 7 步）
**由 sink 自己去问**：同一个驱动、同一套会话设置，自检问到的与搬运时用到的是同一件事。

> ⚠ **真机差异 ③：这些包可能已经装过、且版本不同。**
> `rpm -q` 报 already installed 就算过，别 `yum reinstall`——客户机上别的东西可能正靠着它。
> 只有 `stunnel` 值得看一眼版本：`stunnel -version 2>&1 | head -2`
> （**它以 1 退出**，别在 `set -e` 的脚本里直接调）。

---

## 第 4 步：铺 sink 二进制

目标端只有一个二进制：`db-qbs-sink`。

```sh
uname -m        # 必须是 x86_64；与你带来的二进制的架构对上
mkdir -p /opt/db-qbs/bin
install -m 0755 /root/dist/db-qbs-sink /opt/db-qbs/bin/
/opt/db-qbs/bin/db-qbs-sink; echo "exit=$?"
# 期望：打一行用法（--config 没给），exit=1。
# 这就是「它跑得起来」的证据 —— 报 GLIBC_2.xx not found 或 Exec format error 才是坏的。
```

| 看到什么 | 是什么事 |
|---|---|
| `{"level":"error","event":"sink_unavailable","message":"用法：db-qbs-sink --config <sink.toml>"}` + `exit=1` | 正常，进下一步 |
| `GLIBC_2.28 not found` | 二进制不是在 `centos:7` 里编的。回 `packaging/centos7/build.sh` 重编，别在现场想办法 |
| `Exec format error` | 架构拿错了。`uname -m` 对一眼，换 `out/bin/linux-<arch>/` 下的那一套 |

---

## 第 5 步：写 `sink.toml`

```sh
mkdir -p /etc/db-qbs
cat > /etc/db-qbs/sink.toml <<'EOF'
listen = "127.0.0.1:8080"
EOF
chmod 0600 /etc/db-qbs/sink.toml
```

就一行，但这一行是整台机器的安全形态：

- `listen` **只绑回环**（ADR-0024：sink 不做鉴权，能连上它的人可用调用方给的凭据清空并重写任意暂存表与目标表，
  兜底就是只绑回环）。**别改成 `0.0.0.0`**，也别绑内网网卡的地址——公网上的 source 不是直连它，
  是经第 8 步的 stunnel 服务端落到回环上来的。自检 D3 专门判这一条。
- `8080` 必须与第 8 步 stunnel 配置里的 `@@SINK_PORT@@` **同一个值**。

**不要写 `mysql_dsn` / `database`。** 这两个字段已退役（ADR-0037 §2）：目标库的凭据随每个 run 的请求
从 source 过线，sink 自己不持有任何一份。写了不报错，但 sink 启动时会多打一条 warn，现场看到的人会以为配置出了问题。
MySQL 的地址、账号、口令、库名都在**源端**的界面上填（`source-centos7.md` 的数据源那一步），这台机器上一个都不用配。

> ⚠ **真机差异 ④：`8080` 可能已经有人在听。**
> `ss -ltnp | grep 8080` 先看。被占了就整体换一个端口，
> **这里的 `listen`、第 8 步的 `connect`（`@@SINK_PORT@@`）两处一起改**；源端那边的 `8080` 是它自己回环上的另一个口，
> 与这里无关、不必跟着改。只改一处的话 D2 报「没有应答」，看着像 sink 没起。

---

## 第 6 步：起 sink

```sh
nohup /opt/db-qbs/bin/db-qbs-sink --config /etc/db-qbs/sink.toml >> /var/log/db-qbs-sink.log 2>&1 &
sleep 1; tail -5 /var/log/db-qbs-sink.log
curl -sS http://127.0.0.1:8080/v1/runs/__probe__; echo
```

期望：日志里一条 `"event":"sink_started"`、`"listen":"127.0.0.1:8080"`（它自己把「本服务无鉴权」那句警告打出来，**这是正常的**）；
`curl` 回一段 404 的 JSON，`"code":"RUN_UNKNOWN"`——**这就是「那头是 sink」的指纹**，
自检 D2、源端自检 S8 认的都是它（「有人应答」不算，隧道通到别的服务上也会有人应答）。

sink 启动**不连 MySQL**：连接按 run 建，连不上的失败点在第 9 步的 D4（或发起运行那一刻）。
所以这里起得来不等于库连得上，别把这一步的绿当成第 7 步的绿。

> ⚠ **真机差异 ⑤：真机上做成 systemd 服务，别 `nohup`。**
> ```sh
> cat > /etc/systemd/system/db-qbs-sink.service <<'EOF'
> [Unit]
> Description=db-qbs sink
> After=network-online.target
> Wants=network-online.target
>
> [Service]
> ExecStart=/opt/db-qbs/bin/db-qbs-sink --config /etc/db-qbs/sink.toml
> Restart=on-failure
> RestartSec=5
>
> [Install]
> WantedBy=multi-user.target
> EOF
> systemctl daemon-reload && systemctl enable --now db-qbs-sink
> systemctl status db-qbs-sink
> journalctl -u db-qbs-sink -n 50 --no-pager
> ```
> 容器里没有 systemd，**这一段演练台上没验过**，真机上第一次用要盯着 `systemctl status`。
> sink 的日志走 stdout，systemd 下在 `journalctl` 里，不在 `/var/log/db-qbs-sink.log`。

---

## 第 7 步：MySQL 那一头的三前提（客户 DBA 的活，这一步是给他的纸条）

这一步**在这台机器上没有命令可敲**——MySQL 客户端不装（第 3 步说了为什么），三前提由 sink 在第 9 步的 D4–D7 里去问。
这里写的是：**那几条红了，该让 DBA 改什么。** 出发前把这张纸条先发给 DBA，比现场再约窗口省一天。

sink 每开一条连接都走同一套开连接仪式：连上 → `SET NAMES utf8mb4` → `SET SESSION sql_mode = 'STRICT_ALL_TABLES'`
→ 回读三项会话变量逐项判（`crates/sink/src/mysql_destination.rs` 的 `run_connection_ritual`）。三项任一不合格，
**整个 sink 不可用**，不是「这一趟慢一点」。

给 DBA 的三件事：

1. **账号。** 给 sink 用的那个账号要能在目标库上建暂存表、写目标表、删暂存表：
   ```sql
   GRANT SELECT, INSERT, UPDATE, CREATE, DROP ON `<库名>`.* TO '<账号>'@'<目标端主机地址或 %>';
   ```
   （`SELECT` 是 `information_schema` 元数据与 `INSERT … SELECT` 切换段要的；没有 `DELETE`——主键 upsert 不删行，ADR-0035。）
2. **`my.cnf` 的两行**（改完要**重启 MySQL**，那是 DBA 的窗口，早点约）：
   ```ini
   [mysqld]
   character-set-server = utf8mb4
   max_allowed_packet   = 64M        # 即 67108864 字节；sink 判的是「≥ 这个数」
   ```
   MySQL 8.0 的 `max_allowed_packet` **默认就是 64M**，没人动过就刚好够；`character-set-server` 默认也是 `utf8mb4`。
   红的多半是有人按旧习惯改小 / 改成 `utf8`。
3. **别有东西改写会话变量。** `init_connect`、ProxySQL 之类的中间层若在连上时改 `sql_mode`，
   sink 的 `SET SESSION sql_mode` 回读回来就不是 `STRICT_ALL_TABLES`，D6 红。DBA 那边 `SHOW VARIABLES LIKE 'init_connect'` 看一眼。

DBA 侧自己核对的 SQL（**在他的客户端上跑**，不在这台机器上）：

```sql
SHOW VARIABLES WHERE Variable_name IN ('character_set_server','max_allowed_packet','init_connect','sql_mode');
```

> ⚠ **真机差异 ⑥：MySQL 是客户的库，三前提没有一项由我们掌控。**
> 演练台上的 MySQL 是 compose 起的（`--character-set-server=utf8mb4`、`max_allowed_packet` 是 8.0 的默认 64M、
> 账号 `spike` 对库 `qbs` 全权），三项天然满足、D4–D7 一次就绿。真机上这三项每一项都可能不是，
> 而且改 `max_allowed_packet` 要重启 MySQL——**这是行李清单「外部依赖」里那条，出发前就要约 DBA 的窗口**。

---

## 第 8 步：装 stunnel 服务端（隧道的目标端那一头）

装法的权威是 [`packaging/stunnel/README.md`](../../packaging/stunnel/README.md)，这里是它的目标端摘要。

```sh
mkdir -p /etc/stunnel/db-qbs
cp /root/dist/target.crt /root/dist/target.key /root/dist/source.crt /etc/stunnel/db-qbs/
cp /root/dist/stunnel-sink.conf /etc/stunnel/db-qbs/stunnel-sink.conf
chmod 600 /etc/stunnel/db-qbs/*.key
```

填掉模板里的两个占位符（**一个都不能留**）：

| 占位符 | 填什么 |
|---|---|
| `@@WHITELIST_PORT@@` | 客户开的**白名单端口**——这台机器对公网唯一露出来的口；源端那份配置里的 `@@TARGET_PORT@@` 填的是同一个数 |
| `@@SINK_PORT@@` | `8080` —— 必须与第 5 步 `sink.toml` 的 `listen` 端口**同一个值** |

> ⚠ **真机差异 ⑦：白名单端口由客户给，公网 IP 也是。**
> 演练台上白名单口是 `15443`、「公网」落点是 `host.docker.internal`（Docker Desktop 才有的东西）。
> 真机上这两个数都是客户给的——**拿不到就是装机当天彻底停摆**，它是第二版唯一的外部阻塞项（ADR-0041「风险」），
> 出发前就要跟客户要到。这台机器**不需要知道自己的公网 IP**（`accept` 绑 `0.0.0.0`），
> 要知道它的是源端那台（`source-centos7.md` 真机差异 ④）。

```sh
sed -i 's/@@WHITELIST_PORT@@/15443/; s/@@SINK_PORT@@/8080/' /etc/stunnel/db-qbs/stunnel-sink.conf
# 填完查一遍：注释行（; 开头）里那句提示自己就带 @@ 字样，所以要先把它滤掉
grep -vE '^[[:space:]]*;' /etc/stunnel/db-qbs/stunnel-sink.conf | grep -nE '@@[A-Z_]+@@' \
  && echo '还有没填的！' || echo '占位符已填完'
grep -E '^(accept|connect)' /etc/stunnel/db-qbs/stunnel-sink.conf
# 期望：accept  = 0.0.0.0:15443
#       connect = 127.0.0.1:8080
```

证书对不对，用指纹核一眼（**两条都要与源端那台上的同一条命令输出一致**——两端各钉住对方那一张，拷错任意一张隧道就握不上手）：

```sh
openssl x509 -in /etc/stunnel/db-qbs/target.crt -noout -fingerprint -sha256
openssl x509 -in /etc/stunnel/db-qbs/source.crt -noout -fingerprint -sha256
```

起它（**sink 先起、stunnel 后起**——反过来也起得来，stunnel 不预连，但第一次搬运会以连接被拒收场）：

```sh
stunnel /etc/stunnel/db-qbs/stunnel-sink.conf     # 配置里 foreground = no，它自己转后台
sleep 1; cat /var/run/db-qbs-stunnel-sink.pid
tail -5 /var/log/db-qbs-stunnel-sink.log
```

> ⚠ **真机差异 ⑧：真机上走 systemd，不这么起。**
> ```sh
> cp /root/dist/db-qbs-stunnel.service /etc/systemd/system/
> systemctl daemon-reload && systemctl enable --now db-qbs-stunnel
> systemctl status db-qbs-stunnel
> ```
> `enable` 那半截是**重启后还在**，容器里没有 systemd，这一条演练台上验不到。
> 那份 unit 是 `Type=forking` + `PIDFile`，对的是配置里 `foreground = no`——两边照抄即可、不必改。

> ⚠ **真机差异 ⑨：SELinux。**
> `enforcing` 下 stunnel 绑非标准端口（白名单口）可能被拦：`getenforce` 看一眼，
> 被拦时 `sealert -a /var/log/audit/audit.log` 找那条 AVC、照它给的处置做，
> 实在不行 `semanage port -a -t http_port_t -p tcp 15443`（端口换成客户给的那个）。**别直接关 SELinux。**

> ⚠ **真机差异 ⑩：防火墙。**
> **这一台要开入向口**——白名单端口那一个，别的都不开：
> ```sh
> systemctl is-active firewalld            # 容器里没有 firewalld，这一步演练台上撞不到
> firewall-cmd --permanent --add-port=15443/tcp && firewall-cmd --reload
> firewall-cmd --list-ports
> ```
> `8080` **不开**——sink 只在回环上，本来就不该从外面进得来。客户网络层的白名单（裁定 9）是第二道，
> 这里是机器自己那一道，两道都要通。

---

## 第 9 步：再跑一遍自检——这次要全绿

```sh
QBS_MYSQL_HOST=<MySQL 地址> QBS_MYSQL_USER=<账号> QBS_MYSQL_DATABASE=<库名> \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass /root/dist/preflight-target.sh; echo "exit=$?"
```

九条各查什么，红了才好知道该往哪儿看：

| | 查什么 | 装在第几步 |
|---|---|---|
| D1 | MySQL 监听口可达 | —（客户网络） |
| D2 | sink 在回环上应答（认 `RUN_UNKNOWN`） | 5 / 6 |
| D3 | sink 没越出回环（`listen` 是回环，且从本机非回环地址摸不到它） | 5 |
| D4 | sink 用给定凭据连得上目标库 | 7（账号）|
| D5 | 会话字符集三项都是 `utf8mb4` | 7（`character-set-server`） |
| D6 | `sql_mode` 设得成 `STRICT_ALL_TABLES` | 7（`init_connect` / 中间层） |
| D7 | `max_allowed_packet` ≥ 64 MiB | 7（`my.cnf` + 重启） |
| D8 | stunnel 服务端进程在跑 | 8 |
| D9 | 白名单口在听（端口从配置里读，不写死） | 8 |

期望 `D1–D9 全 PASS`、`exit=0`。**红一条就在这儿停下**，每条 FAIL 自带一行处置，清完重跑。
别带着红往下走——「装到一半炸出下一个缺口」正是这支脚本要消灭的东西。

几条常见的红与它们真正的成因：

| 红在哪 | 十有八九是 |
|---|---|
| D2「没有应答」 | 第 6 步的 sink 没起，或 `listen` 的端口与你以为的不是一个。看 `/var/log/db-qbs-sink.log` |
| D2「应答的不是 sink」 | `8080` 上是别的服务（真机差异 ④） |
| D3「`listen` 绑的不是回环」 | 第 5 步把 `listen` 改成了 `0.0.0.0` 或内网地址。改回来 |
| D4「连接 MySQL 失败」 | 账号 / 口令 / 库名 / 授权对不上，或 MySQL 没放行这台机器的地址（`'<账号>'@'%'` 那一截） |
| D5 | 库的 `character-set-server` 不是 `utf8mb4` → DBA 改 `my.cnf` 重启 |
| D6 | `init_connect` / 中间层改写了 `sql_mode` → DBA 查 |
| D7 | `max_allowed_packet` 被改小了 → DBA 改 `my.cnf`（至少 67108864）重启 |
| D8 pid 文件指不到活进程 | stunnel 没起来。看 `/var/log/db-qbs-stunnel-sink.log`；配置里还留着占位符是最常见的一种 |
| D9 白名单口不通 | stunnel 没 bind 上：端口被占、证书路径不对，或 SELinux 拦了（真机差异 ⑨） |

D4–D7 的等价命令（排障时想看 sink 的原话）：

```sh
curl -sS -X POST http://127.0.0.1:8080/v1/target/test-connection -H 'Content-Type: application/json' \
  -d "{\"host\":\"<MySQL 地址>\",\"port\":3306,\"username\":\"<账号>\",\"password\":\"$(head -1 /root/.qbs-mysql-pass)\",\"database\":\"<库名>\"}"
# 合格：{"ok":true,...}；不合格时 message 里是 sink 自己那句「环境配置错误：…」，与搬运时的报错一字不差
```

**自检证不了的那一档**：隧道的加密与认人。自检只判「通不通、那头是不是 sink」；加密取证是演练台的判据
（`rehearsal-tunnel-check.sh --sink real` 的 T6–T8），第 10 步从公网侧再核一眼。

---

## 第 10 步：从「公网」侧核一眼——只有经隧道才到得了 sink

这一步的命令**在源端那台机器上敲**（或在任何一台带着源端那套证书材料的机器上，比如你的笔记本），
`<目标端公网 IP>` 与 `<白名单端口>` 换成客户给的那两个。这四条合起来就是票面那句
「sink 只绑回环；从公网侧只有经 stunnel 服务端能到达它」。

```sh
# 1) 明文 HTTP 打白名单口：拿不到任何东西（TLS 服务端对明文要么闭嘴要么回一条 alert）
curl -sS --max-time 8 http://<目标端公网 IP>:<白名单端口>/v1/runs/__probe__; echo "exit=$?"
# 期望：没有 JSON；curl 以 52（Empty reply）或 56 退出

# 2) 带源端的客户端证书握手同一地址：拿得到 sink 的 RUN_UNKNOWN（1 的正对照——没有它，「拿不到」可能只是那儿没人听）
printf 'GET /v1/runs/__probe__ HTTP/1.0\r\n\r\n' | openssl s_client -connect <目标端公网 IP>:<白名单端口> \
  -CAfile /etc/stunnel/db-qbs/target.crt -cert /etc/stunnel/db-qbs/source.crt -key /etc/stunnel/db-qbs/source.key \
  -quiet 2>/dev/null | grep -o RUN_UNKNOWN
# 期望：RUN_UNKNOWN

# 3) 不带客户端证书握手：被拒（verify = 2 双向认证在生效，换一张自签证书的人进不来）
printf 'GET /v1/runs/__probe__ HTTP/1.0\r\n\r\n' | openssl s_client -connect <目标端公网 IP>:<白名单端口> \
  -CAfile /etc/stunnel/db-qbs/target.crt -quiet 2>/dev/null | grep -c RUN_UNKNOWN
# 期望：0

# 4) 源端的 stunnel 客户端装完之后（source-centos7.md 第 6 步）：经源端本机的隧道入口到达 sink
curl -sS http://127.0.0.1:8080/v1/runs/__probe__; echo
# 期望：RUN_UNKNOWN 的那段 JSON —— 这就是源端自检 S8 判的事
```

而「回环之外摸不到 sink」这一半在**本机**就能判：自检 D3 从这台机器的非回环地址反向摸 `8080` 必须不通，
下面收尾核对的 `ss -ltnp` 必须只看到 `127.0.0.1:8080`。源端那台按目标端的内网 IP 直连 `8080` 同样不通
——那是客户网络层的事（裁定 9），演练台上由拓扑判据 R6 / T4 / T10 替它作证。

---

## 收尾核对

```sh
QBS_MYSQL_HOST=<MySQL 地址> QBS_MYSQL_USER=<账号> QBS_MYSQL_DATABASE=<库名> \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass /root/dist/preflight-target.sh   # D1–D9 全 PASS
ss -ltnp | grep -E '8080|15443'                          # 8080 只在 127.0.0.1 上；15443 在 0.0.0.0（或 *）上
ls -l /etc/db-qbs/sink.toml /etc/stunnel/db-qbs/target.key /root/.qbs-mysql-pass   # 都是 0600
```

- [ ] 自检 D1–D9 全绿、退出码 0
- [ ] `8080` 只绑回环；对外露出来的只有白名单那一个口
- [ ] 第 10 步四条：明文拿不到、带证书拿到 `RUN_UNKNOWN`、不带证书被拒、经源端隧道入口拿到 `RUN_UNKNOWN`
- [ ] `sink.toml`、私钥、口令文件都是 `0600`
- [ ] 真机上：`db-qbs-sink` 与 `db-qbs-stunnel` 两个 unit 都 `enable` 了（重启后还在）；`firewalld` 放行了白名单口
- [ ] DBA 那张纸条（第 7 步）已经发出去、窗口约好了

---

## 真机差异一览（演练台上撞不到的十处）

按手册里出现的先后排；每一条在正文里都有一个 `⚠ 真机差异 <编号>` 的标记。

| # | 差异 | 真机上要做什么 | 在第几步 |
|---|---|---|---|
| ① | 已有可用的 yum 源 | 先 `yum makecache`，能成就整步跳过；动手前备份 `/etc/yum.repos.d` | 2 |
| ② | 机器上不了外网 | 走离线 rpm（行李清单第 8 项），`yum localinstall` | 2 / 3 |
| ③ | 包已经装过、版本不同 | `rpm -q` 报 installed 就算过，别 reinstall | 3 |
| ④ | `8080` 已被占 | 换端口，`sink.toml` 的 `listen` 与 stunnel 的 `connect` 两处一起改 | 5 |
| ⑤ | systemd（sink） | 做成 unit 并 `enable --now`；日志在 `journalctl`；容器里没有 systemd，这一段没验过 | 6 |
| ⑥ | MySQL 是客户的库 | 三前提 + 账号权限都要问 DBA；改 `max_allowed_packet` 要重启 MySQL，出发前约窗口 | 7 |
| ⑦ | 白名单端口 / 公网 IP 由客户给 | `@@WHITELIST_PORT@@` 填客户给的端口，不是 `15443`。**拿不到它就是装机当天彻底停摆**（ADR-0041「风险」） | 8 |
| ⑧ | systemd（stunnel） | 用 `packaging/stunnel/target-side/db-qbs-stunnel.service`，`enable --now`；容器里没有 systemd，这一段没验过 | 8 |
| ⑨ | SELinux | `getenforce`；被拦时按 AVC 处置，**别关 SELinux** | 8 |
| ⑩ | 防火墙 | `firewall-cmd --permanent --add-port=<白名单端口>/tcp`，只开这一个口；`8080` 不开 | 8 |

**「重启后还在不在」全靠 ⑤ 与 ⑧ 里的 `enable`**——那是演练台上唯一验不到、
而现场一定会被用到的一件事（客户机迟早要重启一次）。

---

## 演练实录

本手册的每一行都在演练台的目标端主机容器上走过，实录在 [`records/`](records/)。
**手册是走过的记录，不是照着想象写的**（ADR-0041 §6：任何一次「手册没写、临场解决」都算判据未达成，
回写手册、重走）。

演练台怎么起、怎么复现，见
[`docs/spikes/fixtures/local-rig/README.md`](../spikes/fixtures/local-rig/README.md) 的「装机演练台」一节：
可执行回放是 `scripts/rehearsal-target-install.sh`，不起台架的静态自检是 `scripts/test-rehearsal-target-install.sh`，
第 10 步那四条在演练台上的完整版是 `scripts/rehearsal-tunnel-check.sh --sink real`（T0–T11）。
