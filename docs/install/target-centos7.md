# sink 安装手册（CentOS 7 x86_64）

本手册适用于 `db-qbs-sink-linux-amd64-<version>.tar.gz`。仓库交付前先核对 [`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)；归档内的 `INSTALL.md` 是同一份内容。source 端手册见 [`source-centos7.md`](source-centos7.md)，部署顺序是先安装 sink、再安装 source。

## 0. 前提与包内容

目标主机需要 CentOS 7 `x86_64`、glibc 至少 `2.17` 和 root 权限。完整 sink 包包含：

```text
bin/db-qbs-sink
conf/sink.toml.example
scripts/install.sh
scripts/start.sh
scripts/status.sh
scripts/stop.sh
systemd/db-qbs-sink.service
preflight-target.sh
INSTALL.md
```

⚠ **真机差异：** 用 `uname -m` 选择对应架构；不能在客户机现场重新编译或混用 arm64/amd64 产物。

⚠ **真机差异：** CentOS 7 已 EOL。优先使用客户已有的内网 yum 源；没有内网源时，把依赖 rpm 放进包的 `rpm/` 目录，现场使用 `yum -y localinstall rpm/*.rpm`。

## 1. 校验和与依赖

在归档根目录执行：

```sh
sha256sum -c SHA256SUMS
uname -m
ldd --version | head -1
rpm -q curl iproute
```

缺包时使用客户 yum 源或离线 rpm：

```sh
yum -y install curl iproute
```

`curl` 用于 agent HTTP 探测，`iproute` 提供 `ss`。**不要安装 MySQL 客户端来做验收**：目标端连接仪式必须由 sink 自己执行，旧客户端可能无法处理 MySQL 8 的 `caching_sha2_password`。

⚠ **真机差异：** 机器上可能已有不同版本的依赖。先用 `rpm -q` 检查，已安装就不要强制重装。

⚠ **真机差异：** SELinux 为 `Enforcing` 时，不要直接关闭它；遇到拒绝先查看审计日志并按客户安全规范放行。

## 2. 解包和安装

```sh
install -d -m 0700 /root/db-qbs-install
tar -xzf db-qbs-sink-linux-amd64-<version>.tar.gz -C /root/db-qbs-install
cd /root/db-qbs-install/db-qbs-sink-linux-amd64-<version>
chmod 0755 scripts/*.sh preflight-target.sh
./scripts/install.sh
```

安装后：

```text
/opt/tools/db-qbs/bin/db-qbs-sink
/opt/tools/db-qbs/conf/sink.toml       0600
/opt/tools/db-qbs/conf/agent-id        首次启动自动生成，0600
/opt/tools/db-qbs/logs/                0700
```

sink 不携带 `database.toml`，也不保存 MySQL 密码。每次运行的目标连接由 source 通过已注册 agent 发送。

## 3. 配置 sink 和防火墙

POC 直接连接拓扑使用：

```toml
listen = "0.0.0.0:18080"
```

**sink 没有登录层。** 直接模式必须在 sink 主机限制 TCP `18080`，只允许 source 主机 `10.250.0.24`，并允许本机回环探测：

```sh
iptables -C INPUT -p tcp -s 10.250.0.24 --dport 18080 -j ACCEPT 2>/dev/null \
  || iptables -I INPUT 1 -p tcp -s 10.250.0.24 --dport 18080 -j ACCEPT
iptables -C INPUT -i lo -p tcp --dport 18080 -j ACCEPT 2>/dev/null \
  || iptables -I INPUT 2 -i lo -p tcp --dport 18080 -j ACCEPT
iptables -C INPUT -p tcp --dport 18080 -j DROP 2>/dev/null \
  || iptables -I INPUT 3 -p tcp --dport 18080 -j DROP
iptables-save > /etc/sysconfig/iptables
iptables -S INPUT | grep -- '--dport 18080'
```

如果客户使用 firewalld，按客户标准建立等价 rich rule；不要同时让 firewalld 和手工 iptables 对同一端口产生互相覆盖的规则。确认规则在重启后的恢复方式，CentOS 7 上 `iptables-services` 未必已安装。

如果采用 mTLS stunnel 形态，sink 改为只监听 `127.0.0.1:18080`，使用包内 `stunnel/` 的 target-side 模板，并只对外开放证书保护的白名单端口；此时不要使用上面的直接模式规则替代 stunnel 防火墙策略。

**不要写 `mysql_dsn` 或 `database`。** 这两个字段已退役，目标数据库配置属于 source 上的数据源。

## 4. 启动和检查

```sh
systemctl enable --now db-qbs-sink
systemctl status db-qbs-sink --no-pager
ss -ltnp | grep ':18080'
curl -fsS http://127.0.0.1:18080/v1/agent/info
curl -sS http://127.0.0.1:18080/v1/runs/__probe__
journalctl -u db-qbs-sink -n 50 --no-pager
```

`/v1/agent/info` 应返回 agent 信息，未知 run 应返回包含 `RUN_UNKNOWN` 的错误 JSON。首次启动会生成 `/opt/tools/db-qbs/conf/agent-id`，备份或迁移 sink 时必须保留该文件；删除它会让 source 判定 agent 身份变化。

非 systemd 环境才使用包内脚本：

```sh
./scripts/start.sh
./scripts/status.sh
```

⚠ **真机差异：** 不要同时用 `nohup` 和 systemd 启动同一个 sink；systemd 受客户基线限制时，以 `systemctl status` 和 `journalctl` 为准。

## 5. sink preflight

POC 直接连接拓扑要求分别检查 MySQL 8 和 MySQL 5。密码只通过受限文件传入，不出现在 shell 历史：

```sh
umask 077
read -r -s -p 'MySQL 8 password: ' QBS_MYSQL_PASSWORD
printf '\n'
printf '%s\n' "$QBS_MYSQL_PASSWORD" > /root/.qbs-mysql-pass
chmod 0600 /root/.qbs-mysql-pass
QBS_DIRECT_MODE=1 \
QBS_SINK_CONFIG=/opt/tools/db-qbs/conf/sink.toml \
QBS_SINK_LISTEN=0.0.0.0:18080 QBS_MYSQL_HOST=10.250.0.24 \
QBS_MYSQL_PORT=3306 QBS_MYSQL_USER=root QBS_MYSQL_DATABASE=mysql \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass ./preflight-target.sh

read -r -s -p 'MySQL 5 password: ' QBS_MYSQL_PASSWORD
printf '\n'
printf '%s\n' "$QBS_MYSQL_PASSWORD" > /root/.qbs-mysql-pass
QBS_DIRECT_MODE=1 \
QBS_SINK_CONFIG=/opt/tools/db-qbs/conf/sink.toml \
QBS_SINK_LISTEN=0.0.0.0:18080 QBS_MYSQL_HOST=10.250.0.24 \
QBS_MYSQL_PORT=33307 QBS_MYSQL_USER=root QBS_MYSQL_DATABASE=mysql \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass ./preflight-target.sh

unlink /root/.qbs-mysql-pass
unset QBS_MYSQL_PASSWORD
```

`QBS_DIRECT_MODE=1` 使 D3、D8、D9 按 POC 直连拓扑判定；D1、D2、D4-D7 仍必须通过。完整 mTLS 拓扑去掉该变量，并按 `stunnel/` 模板配置。

D1-D9 含义如下：

| 项 | 检查 |
| --- | --- |
| D1 | sink 到 MySQL 监听口可达 |
| D2 | sink HTTP 返回自己的 `RUN_UNKNOWN` |
| D3 | 直接模式下确认对外监听受 firewall 保护；mTLS 模式下要求 sink 只绑回环 |
| D4 | 使用给定凭据连接目标库 |
| D5 | 会话字符集三项为 `utf8mb4` |
| D6 | 会话 `sql_mode` 为 `STRICT_ALL_TABLES` |
| D7 | `max_allowed_packet >= 67108864` |
| D8 | stunnel 服务端；直接模式下明确跳过 |
| D9 | stunnel 白名单口；直接模式下明确跳过 |

MySQL 5.7 默认 `max_allowed_packet` 常为 4 MiB，必须由 DBA 将运行时值和 my.cnf 的 `[mysqld]` 配置都提高到至少 `64M`。目标账号至少需要目标表所需的 `SELECT, INSERT, UPDATE, CREATE, DROP` 权限。

## 6. source 注册与目标数据源

source 启动后，由管理员在 source 的“目标端 Agent”界面注册：

```text
http://10.250.0.202:18080
```

注册成功后确认 agent 在线且身份稳定。MySQL 8 和 MySQL 5 两个数据源都绑定同一台 agent；主机、端口、账号、库名和密码在 source 数据源界面填写，不在 sink 配置文件中重复维护。

## 7. 升级、回滚和日志

升级前备份 sink 二进制、`sink.toml`、`agent-id`：

```sh
install -d -m 0700 /opt/tools/db-qbs/backups/<timestamp>
cp -p /opt/tools/db-qbs/bin/db-qbs-sink /opt/tools/db-qbs/conf/sink.toml /opt/tools/db-qbs/conf/agent-id /opt/tools/db-qbs/backups/<timestamp>/
systemctl stop db-qbs-sink
./scripts/install.sh
systemctl start db-qbs-sink
```

新版本启动失败时，从备份恢复 sink 二进制和配置，再执行 `systemctl daemon-reload && systemctl start db-qbs-sink`。不要删除 `agent-id`。

日志只写 stdout；systemd 下查看：

```sh
journalctl -u db-qbs-sink -n 100 --no-pager
```

sink 日志不应出现明文 MySQL 密码。密码文件用完即删。

## 8. 卸载

先确认 source 已停止使用这台 agent，并保存 agent-id 及运行记录：

```sh
systemctl disable --now db-qbs-sink
unlink /etc/systemd/system/db-qbs-sink.service
systemctl daemon-reload
```

仅移除程序而保留 agent 身份和日志时不要删除 `/opt/tools/db-qbs/conf/agent-id` 或 `/opt/tools/db-qbs/logs`。彻底清理前先取得备份并按客户变更流程执行。
