# source 安装手册（CentOS 7 x86_64）

本手册适用于 `db-qbs-source-linux-amd64-<version>.tar.gz`。仓库交付前先核对 [`packaging/PACKING-LIST.md`](../../packaging/PACKING-LIST.md)；归档内的 `INSTALL.md` 是同一份内容。目标端先按 [`target-centos7.md`](target-centos7.md) 安装并启动 sink。

## 0. 前提与包内容

目标主机需要 CentOS 7 `x86_64`、glibc 至少 `2.17` 和 root 权限。完整 source 包包含：

```text
bin/db-qbs-source
bin/db-qbs-source-run
conf/source.toml.example
conf/database.toml
oracle/instantclient-basic-linux.x64-19*.zip
scripts/install.sh
scripts/start.sh
scripts/status.sh
scripts/stop.sh
systemd/db-qbs-source.service
preflight-source.sh
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
rpm -q unzip libaio curl iproute
```

缺包时使用客户 yum 源或离线 rpm：

```sh
yum -y install unzip libaio curl iproute
```

`libaio` 是 Oracle Instant Client 的运行时依赖，`unzip` 用于解 Oracle zip，`iproute` 提供 `ss`，`curl` 用于 HTTP 检查。不要安装 MySQL 客户端代替 sink 的连接仪式；旧客户端可能无法处理 MySQL 8 的 `caching_sha2_password`。

⚠ **真机差异：** 机器上可能已有不同版本的依赖。先用 `rpm -q` 检查，已安装就不要强制重装。

⚠ **真机差异：** SELinux 为 `Enforcing` 时，不要直接关闭它；遇到拒绝先查看审计日志并按客户安全规范放行。

## 2. 解包和安装

```sh
install -d -m 0700 /root/db-qbs-install
tar -xzf db-qbs-source-linux-amd64-<version>.tar.gz -C /root/db-qbs-install
cd /root/db-qbs-install/db-qbs-source-linux-amd64-<version>
chmod 0755 scripts/*.sh preflight-source.sh
./scripts/install.sh
```

安装脚本会安装两个 source 二进制、解包 Oracle Instant Client、创建 `/opt/tools/db-qbs/oracle/instantclient` 软链、执行 `ldconfig`、安装 systemd unit，并将包内 `conf/database.toml` 安装为 `/opt/tools/db-qbs/conf/database.toml`，权限为 `0600`。首次安装时才创建 `source.toml`；已有配置会保留。

安装后应满足：

```text
/opt/tools/db-qbs/conf/source.toml       0600
/opt/tools/db-qbs/conf/database.toml     0600
/opt/tools/db-qbs/data/source/           0700
/opt/tools/db-qbs/logs/                  0700
/opt/tools/db-qbs/oracle/instantclient/
```

## 3. 配置 source

编辑 `/opt/tools/db-qbs/conf/source.toml`：

```toml
listen = "0.0.0.0:18088"
oracle_client_lib_dir = "/opt/tools/db-qbs/oracle/instantclient"
data_dir = "/opt/tools/db-qbs/data/source"
run_executable = "/opt/tools/db-qbs/bin/db-qbs-source-run"
history_retention_days = 90
```

新装配置只写服务字段。**不要写 `oracle_connect_string`、`oracle_username`、`oracle_password`，也不要写 `sink_base_url`。** Oracle 连接和目标端 agent 在 source Web 界面中保存；数据源密码会加密落在 `data_dir` 中。

POC 中 source HTTP 是 `0.0.0.0:18088`，只允许受信任的管理终端访问。若客户要求只开放本机，将 `listen` 改为 `127.0.0.1:18088`，再使用 SSH 端口转发：

```sh
ssh -L 18088:127.0.0.1:18088 root@<source-host>
```

检查 Oracle 动态依赖：

```sh
ldconfig -p | grep libclntsh
ldd /opt/tools/db-qbs/oracle/instantclient/libclntsh.so | grep -c 'not found'
```

第二条应为 `0`。若出现 `libaio.so.1`，安装 `libaio`；若出现 `libnnz19.so`，重新检查 `/etc/ld.so.conf.d/db-qbs-oracle.conf` 和 `ldconfig`。

## 4. 启动和检查

```sh
systemctl enable --now db-qbs-source
systemctl status db-qbs-source --no-pager
ss -ltnp | grep ':18088'
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18088/
journalctl -u db-qbs-source -n 50 --no-pager
```

HTTP 状态应为 `200`。非 systemd 环境才使用包内脚本：

```sh
./scripts/start.sh
./scripts/status.sh
```

⚠ **真机差异：** 不要同时用 `nohup` 和 systemd 启动同一个 source；systemd 受客户基线限制时，以 `systemctl status` 和 `journalctl` 为准。

## 5. source preflight

POC 直接连接拓扑使用：

```sh
QBS_DIRECT_MODE=1 \
QBS_ORACLE_HOST=10.250.0.222 QBS_ORACLE_PORT=1522 \
QBS_SINK_BASE_URL=http://10.250.0.202:18080 \
QBS_SOURCE_CONFIG=/opt/tools/db-qbs/conf/source.toml \
./preflight-source.sh
```

S1-S8 含义如下：

| 项 | 检查 |
| --- | --- |
| S1 | glibc 满足 CentOS 7 二进制要求 |
| S2 | `libclntsh.so` 存在且可读 |
| S3 | Oracle Client 架构与主机一致 |
| S4 | Oracle Client 动态依赖完整且已进入 linker 搜索路径 |
| S5 | source 到 Oracle 监听口可达 |
| S6 | stunnel 客户端；直接模式下明确跳过 |
| S7 | sink 端口可达 |
| S8 | 响应含 `RUN_UNKNOWN`，证明对端是 db-qbs-sink |

`QBS_DIRECT_MODE=1` 只适用于 POC 的可信内网直连。跨不可信网络时，去掉该变量，使用归档内 `stunnel/source-side/` 模板，并与 target-side 证书和端口配置对齐。

⚠ **真机差异：** `firewall`、SELinux、已有端口和出向 ACL 都可能让 S5/S7 失败；不要把失败改成跳过，先处理对应网络问题。

## 6. 登录、注册 agent 和配置数据源

1. 以管理员身份打开 `http://<source-host>:18088/`。
2. 进入“目标端 Agent”，注册 `http://10.250.0.202:18080`，确认状态为在线。
3. 在“数据源”中创建 Oracle 数据源，使用包内 `conf/database.toml` 的 `[oracle11g]` 值，连接串按 `//<host>:<port>/<sid>` 填写，然后点击“测试连接”。
4. 创建 MySQL 8 数据源，使用 `[mysql8]` 值，目标 agent 选择同一台 sink，点击“测试连接”。
5. 创建 MySQL 5 数据源，使用 `[mysql5]` 值，端口是 `33307`，目标 agent 仍选择同一台 sink，点击“测试连接”。

source API 不回显数据源密码；`data/source` 和 `datasource.key` 必须纳入备份。不要把数据库密码写进 shell 命令、systemd unit 或日志。

## 7. 创建任务并完成验收

先让 DBA 在两个目标库创建好目标表；v1 不自动创建业务目标表。

1. 选择 Oracle 数据源和 MySQL 8 数据源，读取源表/列，配置字段映射和目标表，保存并执行小数据量导入。
2. 复用相同的 Oracle 源和目标表定义，目标数据源改为 MySQL 5 `33307`，执行第二次小数据量导入。
3. 在作业中心查看阶段、行数和最终结果。
4. 在两个目标库核对实际行和业务字段，不要只看 HTTP `202`。

## 8. 升级、回滚和日志

升级前备份二进制、配置、agent 注册数据和 source data：

```sh
install -d -m 0700 /opt/tools/db-qbs/backups/<timestamp>
cp -p /opt/tools/db-qbs/bin/db-qbs-source /opt/tools/db-qbs/bin/db-qbs-source-run /opt/tools/db-qbs/backups/<timestamp>/
cp -p /opt/tools/db-qbs/conf/source.toml /opt/tools/db-qbs/conf/database.toml /opt/tools/db-qbs/backups/<timestamp>/
cp -a /opt/tools/db-qbs/data/source /opt/tools/db-qbs/backups/<timestamp>/data-source
systemctl stop db-qbs-source
./scripts/install.sh
systemctl start db-qbs-source
```

新版本启动失败时，从备份恢复两个 source 二进制和配置，再执行 `systemctl daemon-reload && systemctl start db-qbs-source`。不要删除 `data/source`，否则会丢失 agent、数据源和运行历史。

日志只写 stdout；systemd 下查看：

```sh
journalctl -u db-qbs-source -n 100 --no-pager
```

运行日志可能包含业务列值，不能改成公共可读权限，也不要转发到未授权系统。

## 9. 常见故障

- `GLIBC_2.xx not found`：架构或构建环境错误，核对 `uname -m` 和 `ldd --version`。
- `DPI-1047`：检查 `oracle_client_lib_dir`、`libaio`、`ldconfig`，不要只在当前 shell 临时设置 `LD_LIBRARY_PATH`。
- agent offline：从 source 执行 `curl http://10.250.0.202:18080/v1/agent/info`，再检查 sink、firewall 和端口。
- `SINK_ENVIRONMENT`：在 sink 端分别运行两个 MySQL 目标的 preflight，修复字符集、`sql_mode` 或 `max_allowed_packet`。

## 10. 卸载

```sh
systemctl disable --now db-qbs-source
unlink /etc/systemd/system/db-qbs-source.service
systemctl daemon-reload
```

仅移除程序而保留数据时不要删除 `/opt/tools/db-qbs/data/source`。彻底清理前先取得备份并按客户变更流程执行。
