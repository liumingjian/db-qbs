# db-qbs 安装手册

本目录对应两个可以分别交付的 CentOS 7 安装包：

- [`source-centos7.md`](source-centos7.md)：安装 `db-qbs-source`、`db-qbs-source-run` 和 Oracle Instant Client。
- [`target-centos7.md`](target-centos7.md)：安装 `db-qbs-sink`，生成并保护 target agent 身份。

部署顺序是先 sink、后 source。两个归档各自带有对应的 `INSTALL.md`，解包后不依赖本仓库目录。

## 生成归档

先用 `packaging/centos7/build.sh` 生成并验证目标平台二进制。完整 source 包需要 Oracle Instant Client 19c Basic zip 和显式提供的 `config/database.toml`；数据库配置含凭据，打包命令不会自动寻找或下载它。

```sh
packaging/centos7/build.sh --platform linux/amd64
packaging/centos7/package.sh \
  --platform linux/amd64 \
  --oracle-client-zip /path/to/instantclient-basic-linux.x64-19.32.0.0.0dbru.zip \
  --database-config config/database.toml
```

输出目录是 `packaging/centos7/out/packages/`：

```text
db-qbs-source-linux-amd64-<git-sha>.tar.gz
db-qbs-sink-linux-amd64-<git-sha>.tar.gz
SHA256SUMS-<git-sha>
```

每个归档内都包含自己的 `INSTALL.md` 和 `SHA256SUMS`。source 归档内的 `conf/database.toml` 和整个 source 归档都按敏感文件处理，交付、传输和服务器上均只允许授权人员读取。

## POC 固定拓扑

本项目 POC 的 canonical 参数见 [`packaging/poc/README.md`](../../packaging/poc/README.md)：source 是 `10.250.0.24`，sink 是 `10.250.0.202`，source 监听 `18088`，sink 监听 `18080`。sink 没有登录层，直接模式下必须在 sink 主机防火墙只允许 source 主机访问 `18080`。

## 交付前检查

```sh
tar tzf db-qbs-source-linux-amd64-<git-sha>.tar.gz
tar tzf db-qbs-sink-linux-amd64-<git-sha>.tar.gz
sha256sum -c SHA256SUMS-<git-sha>
```

不要把 `config/database.toml`、Oracle zip、归档内的密码或运行日志提交到公共制品库。安装完成后按两份角色手册执行服务检查、preflight、数据源测试和小数据量导入。
