/**
 * 系统设置——导航最后一项（ADR-0043 §2 的三项之一；ADR-0044 §6 在最前面加了「目标端 Agent」）。
 *
 * **本版它不可写，而且是有意的**：这一版能改的东西一共三类，都已经各有落点——
 * 连接信息在「数据源」屏（ADR-0037 §5），目标端 agent 在「目标端 Agent」屏（ADR-0044 §6），
 * 任务定义在作业中心的构建器里。
 * 剩下的是服务端进程级配置（Oracle 客户端库、监听地址、数据目录、
 * 历史保留期），它们改一次要重启进程，而且其中两项**决定这台服务本身怎么起**——
 * 做成网页上可改的表单，等于给一个未鉴权的界面（ADR-0024「端口即凭据」）
 * 一条改自己启动参数的路。
 *
 * 所以这一屏摆的是**事实与落点**，不是空表单：让人知道该去哪儿改，比给一个
 * 点了不生效的输入框诚实。要真做可写设置，得先有鉴权，那是另一票。
 */
export function SettingsScreen() {
  return (
    <section className="card" id="settings" aria-labelledby="settings-title">
      <header className="card-header">
        <div>
          <h1 id="settings-title">系统设置</h1>
          <span className="card-subtitle">本版只读</span>
        </div>
      </header>
      <div className="modal-body settings-copy">
        <p>
          这一版没有可以在界面上改的设置。能改的三类东西各有落点：
          <strong>连接信息</strong>在「数据源」屏增删改，
          <strong>目标端 agent</strong> 在「目标端 Agent」屏注册与增删改，
          <strong>任务定义</strong>在作业中心的任务构建器里改。
        </p>
        <p>
          其余是服务端的进程级配置，写在 <code>source.toml</code> 里，改完需要重启服务：
        </p>
        <dl>
          <dt>Oracle 客户端库</dt>
          <dd>oracle_client_lib_dir</dd>
          <dt>本服务监听地址</dt>
          <dd>listen</dd>
          <dt>数据目录（任务、数据源、运行历史都在这里）</dt>
          <dd>data_dir</dd>
          <dt>运行历史保留期</dt>
          <dd>history_retention_days（默认 90 天）</dd>
        </dl>
        <p className="drawer-note">
          <code>sink_base_url</code> 已退役（ADR-0044 §5）：目标端地址不再是一个全局配置，
          而是逐条数据源绑定的 agent。老配置里若还留着它，首次启动会把它迁成一台名为「默认」的
          agent，随后就可以从配置文件里删掉。
        </p>
        <p className="drawer-note">
          口令类字段不在这里显示，也永远不回读——界面拿不到它们，连密文都拿不到（ADR-0037 §5）。
        </p>
      </div>
    </section>
  );
}
