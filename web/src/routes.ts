/**
 * Reading and writing hash addresses — the only copy of this.
 *
 * It does not live in `App.tsx` because the job centre's status chips build links
 * with it, and `App` already imports the job centre; importing back would close a
 * cycle. Cycles do run under ESM, but evaluation order then rides on luck, which is
 * not worth betting to save one file.
 */

/**
 * The address of a run detail (UX review P1-6).
 *
 * It used to have no address at all: the full-screen run detail was just an
 * `activeRun` state whose only entrance was the run you had just started. Reload the
 * page, send a colleague a link, or press Back, and it was gone.
 *
 * Keyed on `run_record_id`, not `task_id`: this screen shows one *run*, not a task.
 * Keyed by task, the same link would point at something else after the next start.
 */
const RUN_HASH_PREFIX = "#runs/";

/**
 * 「查看日志」的地址后缀（#263）。
 *
 * 日志不另开一屏——它是运行详情的一段。但它需要**自己的地址**：任务列表上那颗
 * 「查看日志」要求一步到位，落在运行详情顶部再让人自己往下找，等于把「不用猜是哪一条」
 * 换成了「不用猜是哪一条，但要自己找」。`run_record_id` 进地址前是编过码的，
 * 而 `/logs` 是编码之后才拼上去的，所以剥它不会剥到 id 头上。
 */
const RUN_LOGS_SUFFIX = "/logs";

export function runHash(runRecordId: string): string {
  return `${RUN_HASH_PREFIX}${encodeURIComponent(runRecordId)}`;
}

/** 直达这一次运行的日志。 */
export function runLogsHash(runRecordId: string): string {
  return `${runHash(runRecordId)}${RUN_LOGS_SUFFIX}`;
}

export function runRecordFromHash(hash: string): string | null {
  if (!hash.startsWith(RUN_HASH_PREFIX)) {
    return null;
  }
  const rest = hash.slice(RUN_HASH_PREFIX.length);
  const encoded = rest.endsWith(RUN_LOGS_SUFFIX)
    ? rest.slice(0, -RUN_LOGS_SUFFIX.length)
    : rest;
  return encoded === "" ? null : decodeURIComponent(encoded);
}

/** 这个地址点名的是日志那一段吗。 */
export function runLogsRequestedFromHash(hash: string): boolean {
  return runRecordFromHash(hash) !== null && hash.endsWith(RUN_LOGS_SUFFIX);
}
