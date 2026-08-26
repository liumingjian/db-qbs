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

export function runHash(runRecordId: string): string {
  return `${RUN_HASH_PREFIX}${encodeURIComponent(runRecordId)}`;
}

export function runRecordFromHash(hash: string): string | null {
  return hash.startsWith(RUN_HASH_PREFIX)
    ? decodeURIComponent(hash.slice(RUN_HASH_PREFIX.length))
    : null;
}
