import type { Agent, Datasource, DatasourceInput, Task } from "./api";

/**
 * 数据源屏的纯判定（ADR-0039 §2/§3/§4）。
 *
 * 摆在组件外面是因为这几条是**规则**不是渲染：「测通才让存」的门槛、只改名称的例外、
 * 两类字段集各显各的连接摘要，错一条的后果都是用户看不出来的（存进去一条连不上的数据源、
 * 或者改个名字被逼着去连一个已经下线的库）。规则有用例守着，渲染归走查。
 */

/** 「连接」一栏：两类字段集不同，**各显各的**，不拼一个假的统一连接串（ADR-0039 §2）。 */
export function connectionSummary(datasource: Datasource): string {
  return datasource.kind === "oracle"
    ? datasource.connect_string
    : `${datasource.host}:${datasource.port} / ${datasource.database}`;
}

/**
 * 目标表的**全限定名**：`库.表`。
 *
 * 只在「要动生产数据了」那两处二次确认里用（清理本次写入、批量发起）——那种时刻
 * 一个光秃秃的 `ORDER_ITEM` 不够核对：同名表在几个库里都可能有一张，而确认框正是
 * 用来确认「我要动的是不是那一张」的（2026-08 UX 评审 P0-2）。
 *
 * **认不出目标端就只给表名**，不编一个库名出来：拼不出全名是显示问题，
 * 拼错一个库名会让人在确认框上核对一张根本不相干的表。
 */
export function qualifiedTargetTable(
  datasource: Datasource | undefined,
  targetTable: string,
): string {
  return datasource === undefined || datasource.kind !== "mysql"
    ? targetTable
    : `${datasource.database}.${targetTable}`;
}

/**
 * 「目标端 Agent」一栏（ADR-0044 §6）。
 *
 * Oracle 那半边**是空的，不是「无」**：源库由 source 直连，那一栏对它没有含义，
 * 写个「不适用」只会让人以为这里少配了一样东西。
 *
 * MySQL 那半边显示 agent 的名字；绑的 agent 已经不在注册表里时**点名说出来**——
 * 这条数据源此刻是不能用的，含糊成一个 id 片段会让人以为只是显示问题。
 */
export function agentLabel(datasource: Datasource, agents: Agent[]): string {
  if (datasource.kind === "oracle") {
    return "";
  }
  const agent = agents.find((candidate) => candidate.agent_id === datasource.agent_id);
  return agent === undefined ? "已失效的 agent" : agent.name;
}

/** 那一栏的状态标记：agent 不在线 / 身份不符时，这条数据源现在就是不能用的。 */
export function agentStatusOf(
  datasource: Datasource,
  agents: Agent[],
): Agent["status"] | null {
  if (datasource.kind === "oracle") {
    return null;
  }
  return (
    agents.find((candidate) => candidate.agent_id === datasource.agent_id)?.status ??
    "offline"
  );
}

/**
 * 表单里那组**连接字段**的指纹。
 *
 * **名称不在里面**——只改名称免测连是 ADR-0039 §3 的唯一例外，而它正是靠「名称不进指纹」
 * 实现的：改名不动指纹，指纹没动就还等于初始值，门槛自然放行。
 */
export function connectionFingerprint(input: DatasourceInput): string {
  return JSON.stringify(
    input.kind === "oracle"
      ? [input.kind, input.connect_string, input.username, input.password]
      : [
          input.kind,
          // agent 进指纹：换一台 agent 就是换了一条到目标库的路，上一次的测连结果
          // 与新路没关系（ADR-0044 §6）。不进指纹的话，改完 agent 能直接存，
          // 而那条路可能根本不通。
          input.agent_id,
          input.host,
          input.port,
          input.database,
          input.username,
          input.password,
        ],
  );
}

/**
 * 保存门槛（所有者 2026-08-19 裁定 2）：**当前这组连接字段必须有过一次成功测连**。
 *
 * 两条放行路径，只有两条：
 * 1. 刚测通的那组值就是现在表单里的这组（指纹相等）；
 * 2. 编辑态且连接字段一字未动——那组值没变，重测买不到任何新信息，
 *    却要求库在改名的那一刻恰好在线。
 *
 * 新建态没有第 2 条：库里还没有这条数据源，「没变」无从谈起。
 */
export function canSaveDatasource(
  draft: DatasourceInput,
  initial: DatasourceInput | null,
  testedFingerprint: string | null,
): boolean {
  const fingerprint = connectionFingerprint(draft);
  if (testedFingerprint === fingerprint) {
    return true;
  }
  return initial !== null && fingerprint === connectionFingerprint(initial);
}

/**
 * 编辑态的初值。**口令永远是空的**：界面不回读口令，连密文都不回（ADR-0037 §5）；
 * 空串在写入面上的含义是「不改」，与这里的「显示为空」是同一件事的两面。
 */
export function draftFrom(existing: Datasource | null): DatasourceInput {
  if (existing === null) {
    return {
      name: "",
      kind: "oracle",
      connect_string: "",
      username: "",
      password: "",
    };
  }
  return existing.kind === "oracle"
    ? {
        name: existing.name,
        kind: "oracle",
        connect_string: existing.connect_string,
        username: existing.username,
        password: "",
      }
    : {
        name: existing.name,
        kind: "mysql",
        agent_id: existing.agent_id,
        host: existing.host,
        port: existing.port,
        database: existing.database,
        username: existing.username,
        password: "",
      };
}

/**
 * 「被引用」列的计数（ADR-0039 §2）。
 *
 * 数的是**任务**不是绑定：一个任务两端都指着同一条数据源仍然只算一个——
 * 删除被拒时服务端点名的就是任务，两处口径必须一致，否则列上写「1 个任务」、
 * 删除时列出两条同名任务。
 */
export function referenceCounts(tasks: Task[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const task of tasks) {
    for (const id of new Set([
      task.source_datasource_id,
      task.target_datasource_id,
    ])) {
      if (id !== "") {
        counts.set(id, (counts.get(id) ?? 0) + 1);
      }
    }
  }
  return counts;
}

/**
 * 删除被拒时红底那段话该说什么（ADR-0039 §4）。
 *
 * 服务端的 409 报文把任务名连成一串写进 `message`，同一批名字又在 `error.tasks` 里，
 * 界面上是两处各列一遍：红底一大坨顿号串，紧接着又一份 `<li>` 列表。判据要的「点名列出」
 * 由列表那份买单——它随任务数变多仍然可扫，长句会在窄视口折成一坨——所以名字**只留列表**，
 * 这里只说数量与该做什么。拿不到 `tasks` 时（旧服务端、报文形状变了）原样退回服务端那句话，
 * 那时候一遍也没有比一遍都不点名强。
 */
export function deleteRefusalMessage(
  message: string,
  referencedTasks: string[],
): string {
  return referencedTasks.length === 0
    ? message
    : `数据源仍被 ${referencedTasks.length} 个任务引用；请先改这些任务的数据源`;
}
