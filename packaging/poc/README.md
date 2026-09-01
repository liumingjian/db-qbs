# POC 环境规范

本文档是项目 POC 部署与验收的唯一规范。除非任务明确覆盖，POC 使用下面的固定拓扑、目录和账号。

## 固定拓扑

| 角色 | 主机 | 部署目录 | 部署组件 |
| --- | --- | --- | --- |
| source | `10.250.0.24` | `/opt/tools/db-qbs` | `db-qbs-source`、`db-qbs-source-run` |
| sink | `10.250.0.202` | `/opt/tools/db-qbs` | `db-qbs-sink` |

两台主机均使用部署账号 `root`，POC 密码为 `ATT@2022`。部署时优先使用交互式 SSH/SCP 或密码管理工具，
不要把密码写进命令行、脚本、systemd unit 或运行日志。

数据库连接配置统一来自仓库根目录的 `config/database.toml`：

- Oracle 11g：`10.250.0.222:1522`，SID `xe`。
- MySQL 8：`10.250.0.24:3306`，数据库 `mysql`。
- MySQL 5：`10.250.0.24:33307`，数据库 `mysql`。

这三组连接值只在 `config/database.toml` 维护。部署包需要使用该文件时，将同一份文件复制到
`/opt/tools/db-qbs/conf/database.toml`，并设置为 `0600`；不要在 source.toml、sink.toml 或脚本里再写一份。

## 服务约定

POC 沿用仓库 customer packaging 的默认端口：

- source HTTP：`0.0.0.0:18088`。
- sink HTTP：`0.0.0.0:18080`，只允许 source 主机 `10.250.0.24` 访问；sink 没有登录层，防火墙白名单是必要条件。

source 端只部署 source 服务及其 `db-qbs-source-run` 子进程；sink 端只部署 sink 服务。两端都从各自的
`/opt/tools/db-qbs/conf/` 读取服务配置，运行数据和日志留在该目录下的持久化子目录中。

目标端 agent 的地址不写进全局数据库配置。启动 sink 后，在 source 的「目标端 Agent」界面注册
`http://10.250.0.202:18080`，再为 MySQL 数据源绑定这台 agent。

## 部署步骤

1. 使用 `packaging/centos7/build.sh` 构建并验证目标平台产物。source 包必须带上
   `db-qbs-source`、`db-qbs-source-run` 和 Oracle Instant Client；sink 包只带 `db-qbs-sink`。
2. 将 source 包部署到 `root@10.250.0.24:/opt/tools/db-qbs`，将 sink 包部署到
   `root@10.250.0.202:/opt/tools/db-qbs`，保持组件分配不变。
3. 在两端安装对应的 customer packaging，并确认配置文件权限为 `0600`：
   `source.toml`、`sink.toml`、`database.toml` 以及包含凭据的其他文件均按此处理。
4. source 端确认 `oracle_client_lib_dir` 指向已安装的 Oracle Instant Client；sink 端确认
   `sink.toml` 的监听端口与本规范一致，并在防火墙上只放行 `10.250.0.24`。
5. 启动 sink，再启动 source。登录 source 后注册 sink agent，使用 `database.toml` 的三组值创建
   Oracle、MySQL 8 和 MySQL 5 数据源，并分别执行「测试连接」。
6. 至少完成一次 Oracle 到 MySQL 8 的小数据量导入，再完成一次 Oracle 到 MySQL 5 的小数据量导入。
   两个 MySQL 目标都必须通过同一台已注册的 sink agent 访问。

## 验收门槛

在 source 端执行：

```sh
QBS_ORACLE_HOST=10.250.0.222 QBS_ORACLE_PORT=1522 \
QBS_SINK_BASE_URL=http://10.250.0.202:18080 \
QBS_SOURCE_CONFIG=/opt/tools/db-qbs/conf/source.toml \
./preflight-source.sh
```

在 sink 端针对两个 MySQL 端口分别执行（密码文件中放对应目标的口令）：

```sh
QBS_SINK_CONFIG=/opt/tools/db-qbs/conf/sink.toml \
QBS_SINK_LISTEN=0.0.0.0:18080 QBS_MYSQL_HOST=10.250.0.24 \
QBS_MYSQL_PORT=3306 QBS_MYSQL_USER=root QBS_MYSQL_DATABASE=mysql \
QBS_MYSQL_PASSWORD_FILE=/root/.qbs-mysql-pass ./preflight-target.sh

# 第二次执行时仅将 QBS_MYSQL_PORT 改为 33307，并更换密码文件内容。
```

MySQL 口令使用受限权限的密码文件传入，不放进 shell 历史。

POC 通过的条件是：source 与 sink 进程正常运行、Oracle 测试连接成功、两个 MySQL 端口均通过目标端
开连接仪式、agent 在线且身份匹配、两种目标版本各完成一次导入，且运行日志中没有明文数据库口令。
