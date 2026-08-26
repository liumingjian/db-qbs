/**
 * 哈希地址的读写，**只有这一份**。
 *
 * 它不住在 `App.tsx` 里，是因为作业中心那一列状态标签要拿它拼链接——而 `App` 本来就
 * 引着作业中心，反过来再引一次就成了环。环在 ESM 下能跑，但求值顺序从此要靠运气，
 * 不值得为省一个文件去赌。
 */

/**
 * 运行详情的地址（UX 评审 P1-6）。
 *
 * 它原来**没有地址**：整屏运行详情只是一个 `activeRun` state，唯一的入口是刚点完
 * 「发起运行」的那一次。刷新一页、发一条链接给同事、按一下浏览器后退，它就没了。
 *
 * 键是 `run_record_id` 而不是 `task_id`：这一屏摆的是**一次运行**，不是一个任务。
 * 按任务寻址的话，同一条链接在下一次运行之后指向的会是另一件事。
 */
const RUN_HASH_PREFIX = "#runs/";

export function runHash(runRecordId: string): string {
  return `${RUN_HASH_PREFIX}${encodeURIComponent(runRecordId)}`;
}

export function runRecordFromHash(hash: string): string | null {
  return hash.startsWith(RUN_HASH_PREFIX)
    ? decodeURIComponent(hash.slice(RUN_HASH_PREFIX.length))
    : null;
}
