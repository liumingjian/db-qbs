# db-qbs 客户内网离线安装手册

本文用于把 db-qbs 安装到客户内网的两台服务器：

- **源端服务器**：部署 `db-qbs-source`，提供 Web 界面，连接 Oracle，发起迁移。
- **目标端服务器**：部署 `db-qbs-sink`，接收源端推送，连接 MySQL。

安装顺序：**先装目标端 sink，再装源端 source**。

## 1. 安装包

交付物至少包含两个压缩包：

```text
db-qbs-sink-linux-amd64-<版本>.tar.gz
db-qbs-source-linux-amd64-<版本>.tar.gz
SHA256SUMS
```

把 `db-qbs-sink-...tar.gz` 放到目标端服务器，把 `db-qbs-source-...tar.gz` 放到源端服务器。

在每台服务器上校验文件：

```sh
sha256sum -c SHA256SUMS
```

如果只拷贝了本端的一个压缩包，也可以单独校验：

```sh
sha256sum db-qbs-sink-linux-amd64-<版本>.tar.gz
sha256sum db-qbs-source-linux-amd64-<版本>.tar.gz
```

## 2. 服务器要求

两台服务器都需要：

- Linux x86_64，glibc 版本不低于 2.17，CentOS 7 及以上可用。
- root 权限，或具备写入 `/opt/tools/db-qbs`、安装 systemd 服务的权限。
- 源端服务器能访问 Oracle。
- 目标端服务器能访问 MySQL。
- 源端服务器能访问目标端服务器的 sink 端口，默认本文使用 `18080`。

源端还需要 Oracle Instant Client Basic，建议 19c x86_64。该组件受 Oracle 许可约束，通常由客户 DBA 或运维提供。

## 3. 目标端安装 sink

以下命令在**目标端服务器**执行。

### 3.1 解压

```sh
mkdir -p /tmp/db-qbs-install
tar -xzf db-qbs-sink-linux-amd64-<版本>.tar.gz -C /tmp/db-qbs-install
cd /tmp/db-qbs-install/db-qbs-sink-linux-amd64-<版本>
```

### 3.2 安装文件

```sh
chmod +x scripts/*.sh
./scripts/install.sh
```

安装后文件在：

```text
/opt/tools/db-qbs/bin/db-qbs-sink
/opt/tools/db-qbs/conf/sink.toml
/opt/tools/db-qbs/logs/
```

### 3.3 修改配置

编辑：

```sh
vi /opt/tools/db-qbs/conf/sink.toml
```

推荐配置：

```toml
listen = "0.0.0.0:18080"
```

说明：

- `18080` 是目标端 sink 端口，源端要能访问这个端口。
- sink 没有登录认证，请在客户防火墙上限制：**只允许源端服务器 IP 访问 `18080`**。
- sink 不需要配置 MySQL 账号密码。MySQL 数据源在源端 Web 界面里配置。

如果客户要求 sink 只监听某张网卡，可以写：

```toml
listen = "<目标端服务器内网IP>:18080"
```

### 3.4 启动

```sh
./scripts/start.sh
./scripts/status.sh
```

检查端口：

```sh
ss -ltnp | grep 18080
```

本机探测：

```sh
curl -sS http://127.0.0.1:18080/v1/runs/__probe__
```

看到包含 `RUN_UNKNOWN` 的 JSON，说明 sink 已正常响应。

### 3.5 放行防火墙

如果目标端开启了 firewalld：

```sh
firewall-cmd --permanent --add-port=18080/tcp
firewall-cmd --reload
```

同时在客户网络侧确认：只允许源端服务器访问该端口。

## 4. 源端安装 source

以下命令在**源端服务器**执行。

### 4.1 安装 Oracle Instant Client

如果客户已安装 Oracle Instant Client，确认目录即可。

如果未安装，示例：

```sh
mkdir -p /opt/tools/db-qbs/oracle
unzip instantclient-basic-linux.x64-19*.zip -d /opt/tools/db-qbs/oracle
ln -sfn /opt/tools/db-qbs/oracle/instantclient_19_* /opt/tools/db-qbs/oracle/instantclient
```

把 Instant Client 注册给动态链接器：

```sh
echo /opt/tools/db-qbs/oracle/instantclient > /etc/ld.so.conf.d/db-qbs-oracle.conf
ldconfig
ldconfig -p | grep libclntsh
```

如果 `ldconfig -p | grep libclntsh` 没有输出，先不要继续，说明 Oracle Client 没装好。

CentOS/RHEL 系统如果缺 `libaio`，需要由客户内网 yum 源或离线 rpm 安装：

```sh
yum install -y libaio unzip
```

### 4.2 解压 source 包

```sh
mkdir -p /tmp/db-qbs-install
tar -xzf db-qbs-source-linux-amd64-<版本>.tar.gz -C /tmp/db-qbs-install
cd /tmp/db-qbs-install/db-qbs-source-linux-amd64-<版本>
```

### 4.3 安装文件

```sh
chmod +x scripts/*.sh
./scripts/install.sh
```

安装后文件在：

```text
/opt/tools/db-qbs/bin/db-qbs-source
/opt/tools/db-qbs/bin/db-qbs-source-run
/opt/tools/db-qbs/conf/source.toml
/opt/tools/db-qbs/data/source/
/opt/tools/db-qbs/logs/
```

### 4.4 修改配置

编辑：

```sh
vi /opt/tools/db-qbs/conf/source.toml
```

示例：

```toml
listen = "0.0.0.0:18088"
sink_base_url = "http://<目标端服务器IP>:18080"
oracle_client_lib_dir = "/opt/tools/db-qbs/oracle/instantclient"
data_dir = "/opt/tools/db-qbs/data/source"
run_executable = "/opt/tools/db-qbs/bin/db-qbs-source-run"
history_retention_days = 90
```

需要改的地方：

- `<目标端服务器IP>`：填写目标端服务器 IP。
- `listen`：Web 界面端口，默认 `18088`。请只开放给管理员电脑。
- `oracle_client_lib_dir`：填写 Oracle Instant Client 目录。

### 4.5 启动

```sh
./scripts/start.sh
./scripts/status.sh
```

检查端口：

```sh
ss -ltnp | grep 18088
```

本机探测：

```sh
curl -sS http://127.0.0.1:18088/api/tasks
```

看到 `[]` 或任务 JSON 列表，说明 source 已正常响应。

## 5. 浏览器配置数据源

在管理员电脑打开：

```text
http://<源端服务器IP>:18088/
```

进入“数据源”，添加两条数据源。

### 5.1 Oracle 源端数据源

填写客户提供的 Oracle 信息：

```text
名称：自定义，例如 POC Oracle 11g
连接串：//<Oracle IP>:<端口>/<服务名或 SID>
用户名：Oracle 用户
密码：Oracle 密码
```

保存后点击“测试连接”。

### 5.2 MySQL 目标端数据源

填写客户提供的 MySQL 信息：

```text
名称：自定义，例如 POC MySQL 8
主机：<MySQL IP>
端口：3306
用户名：MySQL 用户
密码：MySQL 密码
数据库：目标库名
```

保存后点击“测试连接”。

MySQL 用户至少需要目标库上的 `SELECT`、`INSERT`、`UPDATE`、`CREATE`、`DROP` 权限。生产环境请按客户权限规范收敛授权范围。

## 6. 创建并运行任务

1. 进入“作业中心”，点击“新建任务”。
2. 选择源端 Oracle 数据源、目标端 MySQL 数据源。
3. 在“源表”区域点击“读取表”，搜索并选择源表。
4. 点击“读取列”，勾选要同步的字段，必要时可点“全选”。
5. 在“目标表”区域点击“读取表”，搜索并选择目标表。
6. 点击“读取列”。
7. 在“字段映射”区域点击“同名填充”，检查源字段和目标字段是否对应。
8. 选择主键字段。主键用于 upsert 去重，必须选择。
9. 保存任务。
10. 点击“发起运行”。

注意：

- v1 不负责自动创建目标表。目标表需要客户 DBA 事先在 MySQL 创建好。
- 如果目标表字段和源字段类型不匹配，运行前会失败并显示原因。
- 作业中心会显示迁移进度，进度分母来自开跑前源端 `COUNT(*)`。

## 7. 停止、重启、查看日志

目标端：

```sh
cd /tmp/db-qbs-install/db-qbs-sink-linux-amd64-<版本>
./scripts/status.sh
./scripts/stop.sh
./scripts/start.sh
```

源端：

```sh
cd /tmp/db-qbs-install/db-qbs-source-linux-amd64-<版本>
./scripts/status.sh
./scripts/stop.sh
./scripts/start.sh
```

如果使用 systemd，也可以直接：

```sh
systemctl status db-qbs-sink --no-pager
systemctl restart db-qbs-sink

systemctl status db-qbs-source --no-pager
systemctl restart db-qbs-source
```

日志：

```sh
journalctl -u db-qbs-sink -n 100 --no-pager
journalctl -u db-qbs-source -n 100 --no-pager
```

非 systemd 启动时看：

```sh
tail -100 /opt/tools/db-qbs/logs/sink.log
tail -100 /opt/tools/db-qbs/logs/source.log
```

## 8. 常见问题

### 8.1 source 启动后 Oracle 测试连接报 DPI-1047

通常是 Oracle Instant Client 没注册到动态链接器。

处理：

```sh
echo /opt/tools/db-qbs/oracle/instantclient > /etc/ld.so.conf.d/db-qbs-oracle.conf
ldconfig
ldconfig -p | grep libclntsh
systemctl restart db-qbs-source
```

如果提示 `libaio.so.1 not found`，安装 `libaio`。

### 8.2 源端访问 sink 失败

在源端执行：

```sh
curl -sS http://<目标端服务器IP>:18080/v1/runs/__probe__
```

如果不通，检查：

- 目标端 `db-qbs-sink` 是否启动。
- 目标端防火墙是否放行 `18080`。
- `source.toml` 的 `sink_base_url` 是否写对。
- 客户网络是否允许源端访问目标端该端口。

### 8.3 页面打不开

在源端检查：

```sh
ss -ltnp | grep 18088
curl -sS http://127.0.0.1:18088/api/tasks
```

如果本机能访问、管理员电脑不能访问，检查源端防火墙和客户网络 ACL。

### 8.4 运行失败提示目标表不存在

v1 不自动建表。请 DBA 在 MySQL 目标库创建目标表，再回页面重新读取目标表和目标列。

### 8.5 启动时报 GLIBC_x.xx not found

拿错安装包或服务器系统太老。请确认：

```sh
uname -m
ldd --version | head -1
```

本文提供的 `linux-amd64` 包要求 x86_64，glibc 不低于 2.17。

## 9. 卸载

源端：

```sh
systemctl stop db-qbs-source 2>/dev/null || true
systemctl disable db-qbs-source 2>/dev/null || true
rm -f /etc/systemd/system/db-qbs-source.service
systemctl daemon-reload 2>/dev/null || true
```

目标端：

```sh
systemctl stop db-qbs-sink 2>/dev/null || true
systemctl disable db-qbs-sink 2>/dev/null || true
rm -f /etc/systemd/system/db-qbs-sink.service
systemctl daemon-reload 2>/dev/null || true
```

如需删除程序和数据：

```sh
rm -rf /opt/tools/db-qbs
```

删除前请确认不再需要运行历史和数据源配置。
