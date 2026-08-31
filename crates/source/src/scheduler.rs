//! 到点派活——**source 常驻父进程里的那条调度线程**（#266）。
//!
//! 它与代理探针循环（`server::spawn_agent_probe_loop`）同层、同形状：一条后台线程、
//! 睡成小段、每一段都回头看一眼 `terminated`，SIGTERM 之后立刻走人。区别只有一个：
//! 探针只写 agent 注册表，而这条线程要发起真的运行，所以它借的是整个 [`crate::http::Api`]，
//! 走的是**和「立即运行」那颗按钮完全同一条**派发路径——两条路只在「谁按的」这一个
//! 字段上不同（[`crate::RunTrigger`]）。
//!
//! 四条规则，每一条都是为了**那个触发时刻永远有答案**：
//!
//! 1. **到点且开关开着才发**。开关关掉的任务一次都不会被触发；表达式解析不了的同样不触发
//!    （保存时那道校验早就拦过一次，这里只是不再重复报警）。
//! 2. **上一次还没结束就跳过本次**，并且**写一行没有运行标识的历史**说明原因。静默丢弃
//!    会让「月末那次到底跑没跑」变成没人答得上来的问题。
//! 3. **停机期间错过的时间点不补跑**。判据就在 [`ScheduleState::observe`] 里：一个任务
//!    第一次被看到时（进程刚起、任务刚建、开关刚打开、表达式刚改）只**算出下一个**触发
//!    时刻，从不回头看过去。重启因此绝不会触发一串意料之外的写入。
//! 4. **派发遵守目标端的并发额度**，超出的**在 source 侧排队**。额度是 agent 自报的那一份
//!    （[`db_qbs_shared::AgentInfo::max_concurrent_runs`]），不是 source 侧另配的数值——
//!    同一条限额有两个真相源时，配大了照旧被拒、配小了白白空着额度。**没自报的旧 agent
//!    按一次一个派发**：那是唯一一个绝不会撞上 `RUN_QUOTA_EXCEEDED` 的取值，拿 sink 的
//!    默认值 4 顶上就是猜。
//!
//! 时区只有一个答案：**运行 `source` 的那台机器的本地时区**。cron 表达式里那行文本没有
//! 时区（[`crate::CronSchedule`] 从头到尾只认挂钟时间），换算发生在这里和 HTTP 边界那一处
//! 预览上，两处读的是同一个 `Local`。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use db_qbs_shared::{LogEvent, LogLevel};
use serde::Serialize;
use serde_json::json;

use crate::http::{dispatch_scheduled_run, emit, Api, DispatchOutcome};
use crate::{RunHistory, RunTrigger, ScheduledRefusalReason, Task};

/// 两次评估之间隔多久。cron 是分钟分辨率，5 秒足够按时触发，
/// 而排队中的那些也靠它重试——额度腾出来之后最多 5 秒就会被派出去。
const EVALUATE_INTERVAL: Duration = Duration::from_secs(5);

/// 睡多小一段才回头看一眼 `terminated`。取值与 `accept_loop` 的 `recv_timeout` 一样是
/// 100 毫秒，理由也一样：这条线程活在 `thread::scope` 里、退出时会被 join，
/// 所以它对 SIGTERM 的反应速度就是优雅退出的下限。
const TERMINATION_POLL: Duration = Duration::from_millis(100);

/// 界面上那个触发时刻长什么样。**秒永远是 0**，写出来只会让人以为它有意义。
///
/// 调度这件事上「时刻」只有一种写法，所以这份常量也只有一份：HTTP 那一层的
/// `/api/schedule` 与 cron 预览读的就是这里（`crate::scheduler::SCHEDULE_TIME_FORMAT`）。
pub(crate) const SCHEDULE_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// 一个任务的调度状态：表达式原文 + 下一个触发时刻。
///
/// 存表达式原文是为了认出「人把这一行改了」：改了就从**现在**重新起算，
/// 而不是拿旧表达式算出来的那个时刻去触发一次没人配过的运行。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduleEntry {
    expression: String,
    /// `None` = 这条表达式永远不会触发（`0 30 2 * *` 那种）。
    next_fire: Option<NaiveDateTime>,
}

/// 一个已经到点、但还没派出去的触发时刻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueuedOccurrence {
    pub task_id: String,
    pub task_name: String,
    /// 本该触发的那一刻（本地挂钟时间）。
    pub due_at: String,
    /// 上一次尝试派发时它为什么还没走成。空串表示还没试过。
    pub waiting_reason: String,
}

/// 一个到点了的触发时刻，[`ScheduleState::observe`] 的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueOccurrence {
    pub task_id: String,
    pub due_at: NaiveDateTime,
}

/// 调度器此刻的全部可见状态。**它被 HTTP 面读**（`GET /api/schedule`），
/// 所以排队中的任务在界面上看得见，而不是只活在一条后台线程的脑子里。
#[derive(Debug, Default)]
pub struct ScheduleState {
    entries: BTreeMap<String, ScheduleEntry>,
    queue: Vec<QueuedOccurrence>,
}

/// 调度器状态的共享句柄。形状与 `AgentRegistry` 一样，理由也一样：
/// 一条后台线程写、若干条 HTTP 工作线程读。
pub type ScheduleRegistry = Arc<Mutex<ScheduleState>>;

impl ScheduleState {
    /// 把任务清单对一遍时钟，吐出本轮到点的那些。**纯函数**（只动自己那份表），
    /// 不碰运行历史、不碰网络——「不补跑」这条规则就钉在这里，可以单独用例守。
    pub fn observe(&mut self, tasks: &[Task], now: NaiveDateTime) -> Vec<DueOccurrence> {
        let mut due = Vec::new();
        let mut alive = Vec::new();
        for task in tasks {
            let Some(expression) = task.spec.schedule_expression() else {
                continue;
            };
            let Some(schedule) = task.spec.active_schedule() else {
                continue;
            };
            alive.push(task.task_id.clone());
            let entry = self.entries.entry(task.task_id.clone());
            let entry = entry.or_insert_with(|| ScheduleEntry {
                expression: expression.to_owned(),
                // **第一次看到它就只算下一个**：进程刚起、任务刚建、开关刚打开都走这里。
                // 停机期间错过的时间点因此不补跑（规则 3）。
                next_fire: schedule.next_after(now),
            });
            if entry.expression != expression {
                // 表达式被改了：旧的那个时刻作废，从现在重新起算。
                entry.expression = expression.to_owned();
                entry.next_fire = schedule.next_after(now);
                continue;
            }
            let Some(next_fire) = entry.next_fire else {
                continue;
            };
            if next_fire > now {
                continue;
            }
            due.push(DueOccurrence {
                task_id: task.task_id.clone(),
                due_at: next_fire,
            });
            // 从 `now` 往后算，不是从 `next_fire` 往后算：否则一次长时间的停顿
            // 之后会连着吐出一串过去的时刻，那正是「不补跑」要挡的东西。
            entry.next_fire = schedule.next_after(now);
        }
        // 任务被删了、开关被关了、表达式被清空了——它们的下一次触发时刻一并作废。
        // 重新打开时会当成「第一次看到」，从那一刻往后算。
        self.entries.retain(|task_id, _| alive.contains(task_id));
        due
    }

    /// 这个任务此刻有没有一个还没派出去的触发时刻在队里。
    pub fn is_queued(&self, task_id: &str) -> bool {
        self.queue.iter().any(|queued| queued.task_id == task_id)
    }

    pub fn enqueue(&mut self, occurrence: QueuedOccurrence) {
        self.queue.push(occurrence);
    }

    pub fn queue(&self) -> &[QueuedOccurrence] {
        &self.queue
    }

    /// 排队中的那些，供界面显示（`GET /api/schedule`）。
    pub fn queue_view(&self) -> Vec<QueuedOccurrence> {
        self.queue.clone()
    }

    /// 每个任务的下一次触发时刻，供界面显示。永不触发的那些在这里是 `null`。
    pub fn next_fires(&self) -> Vec<(String, Option<String>)> {
        self.entries
            .iter()
            .map(|(task_id, entry)| {
                (
                    task_id.clone(),
                    entry
                        .next_fire
                        .map(|fire| fire.format(SCHEDULE_TIME_FORMAT).to_string()),
                )
            })
            .collect()
    }

    fn remove_queued(&mut self, task_id: &str) {
        self.queue.retain(|queued| queued.task_id != task_id);
    }

    fn note_waiting(&mut self, task_id: &str, reason: &str) {
        for queued in &mut self.queue {
            if queued.task_id == task_id {
                queued.waiting_reason = reason.to_owned();
            }
        }
    }
}

/// 把调度线程挂到常驻进程上。与 `spawn_agent_probe_loop` 同层、同退出方式：
/// 每秒醒一次只为看 `terminated`，因此 SIGTERM 之后最多再拖一秒，优雅退出的时限没变。
///
/// 它是 `thread::scope` 里的一条作用域线程（和 HTTP 工作线程同一个作用域），
/// 所以能直接借栈上那几个 store，不必把每一份状态都搬进 `Arc`。
pub fn scheduler_loop(state: &Api<'_>, schedule: &ScheduleRegistry, terminated: &AtomicBool) {
    let slices = EVALUATE_INTERVAL.as_millis() / TERMINATION_POLL.as_millis();
    while !terminated.load(Ordering::Relaxed) {
        evaluate(state, schedule, Local::now().naive_local(), terminated);
        for _ in 0..slices {
            if terminated.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(TERMINATION_POLL);
        }
    }
}

/// 一轮：对一遍时钟，把到点的收进队列（或跳过），再把队列尽量派出去。
///
/// **`now` 是参数不是 `Local::now()`**：这一轮的全部行为——补不补跑、跳不跳过、
/// 排不排队——都由它决定，而这些恰恰是最该被用例钉死的。真跑起来时由
/// [`scheduler_loop`] 传本地挂钟时间。
pub fn evaluate(
    state: &Api<'_>,
    schedule: &ScheduleRegistry,
    now: NaiveDateTime,
    terminated: &AtomicBool,
) {
    let tasks = match state.tasks.list() {
        Ok(tasks) => tasks,
        Err(error) => {
            emit(
                LogLevel::Error,
                LogEvent::SourceStarted,
                json!({ "message": format!("调度器读任务清单失败：{error}") }),
            );
            return;
        }
    };
    let due = match schedule.lock() {
        Ok(mut state) => state.observe(&tasks, now),
        Err(_) => return,
    };
    for occurrence in due {
        let Some(task) = tasks.iter().find(|task| task.task_id == occurrence.task_id) else {
            continue;
        };
        // 规则 2：上一次还没结束就跳过本次。「还没结束」有两种形态——真的在跑，
        // 或者上一个触发时刻还堵在队里连派都没派出去。两者都还没有结局，处置相同。
        let busy = task_has_active_run(state, &task.task_id)
            || schedule
                .lock()
                .map(|state| state.is_queued(&task.task_id))
                .unwrap_or(true);
        if busy {
            record_skipped(state, task);
            continue;
        }
        if let Ok(mut state) = schedule.lock() {
            state.enqueue(QueuedOccurrence {
                task_id: task.task_id.clone(),
                task_name: task.name.clone(),
                due_at: occurrence.due_at.format(SCHEDULE_TIME_FORMAT).to_string(),
                waiting_reason: String::new(),
            });
        }
    }
    drain_queue(state, schedule, &tasks, terminated);
}

/// 把队列尽量派出去。**先进先出**：排在前面的那个触发时刻先走。
///
/// 额度满的那一条留在队里、只记下原因（界面上看得见），下一轮再试；
/// 解不开的那一条（数据源没了、agent 不在线、这个任务又被人手动跑起来了）
/// 落一行历史说明原因，然后出队——把它无限期留在队里只会让某个时刻突然冒出一次运行。
fn drain_queue(
    state: &Api<'_>,
    schedule: &ScheduleRegistry,
    tasks: &[Task],
    terminated: &AtomicBool,
) {
    let queued = match schedule.lock() {
        Ok(state) => state.queue_view(),
        Err(_) => return,
    };
    for occurrence in queued {
        // 派一次活里头有秒级的阻塞 IO（解连接、当场探一次 agent）。SIGTERM 已经来了
        // 就一个都不再发：队里那些留着，下次起来当成「第一次看到」——不补跑那条规则
        // 本来就说了，停机期间错过的时刻作废。
        if terminated.load(Ordering::Relaxed) {
            return;
        }
        let Some(task) = tasks.iter().find(|task| task.task_id == occurrence.task_id) else {
            // 任务被删了：它的运行历史也已经没有归属，队里这一条直接作废。
            if let Ok(mut state) = schedule.lock() {
                state.remove_queued(&occurrence.task_id);
            }
            continue;
        };
        if task_has_active_run(state, &task.task_id) {
            record_skipped(state, task);
            if let Ok(mut state) = schedule.lock() {
                state.remove_queued(&task.task_id);
            }
            continue;
        }
        // 派发本身走的是和「立即运行」同一条路，**锁一把都不攥着**：
        // 它里头要解两端连接、还要当场探一次 agent，都是秒级的阻塞 IO。
        match dispatch_scheduled_run(state, task) {
            DispatchOutcome::Started(run_record_id) => {
                emit(
                    LogLevel::Info,
                    LogEvent::SourceStarted,
                    json!({
                        "task_id": task.task_id,
                        "run_record_id": run_record_id,
                        "due_at": occurrence.due_at,
                        "message": "调度到点，已发起运行",
                    }),
                );
                if let Ok(mut state) = schedule.lock() {
                    state.remove_queued(&task.task_id);
                }
            }
            DispatchOutcome::Waiting(reason) => {
                if let Ok(mut state) = schedule.lock() {
                    state.note_waiting(&task.task_id, &reason);
                }
            }
            DispatchOutcome::Refused(reason, message) => {
                record_history(state, task, reason, message);
                if let Ok(mut state) = schedule.lock() {
                    state.remove_queued(&task.task_id);
                }
            }
        }
    }
}

fn task_has_active_run(state: &Api<'_>, task_id: &str) -> bool {
    match state.runs.lock() {
        Ok(runs) => runs.has_active_run(task_id),
        // 锁坏了就当它在跑：宁可跳过一次并留下记录，也不要并发跑同一个任务。
        Err(_) => true,
    }
}

fn record_skipped(state: &Api<'_>, task: &Task) {
    record_history(
        state,
        task,
        ScheduledRefusalReason::PreviousRunActive,
        "上次尚未结束，本次跳过".to_owned(),
    );
}

/// 落一行「到点了但没发起」的历史。**没有运行标识**（`run_id` 为空）：这一次
/// 根本没走到向 sink 发请求那一步，与「预检拒绝、从未到达代理」同构。
fn record_history(
    state: &Api<'_>,
    task: &Task,
    reason: ScheduledRefusalReason,
    message: String,
) {
    let now = state.clock.now();
    let mut history = RunHistory::accepted(
        &crate::http::generate_run_record_id(),
        &task.task_id,
        &task.spec.source_sql(),
        now,
    );
    history.task_name = task.name.clone();
    history.trigger = RunTrigger::Scheduled.as_str().to_owned();
    history.mark_scheduled_refusal(reason, message.clone(), now);
    if let Err(error) = state
        .history
        .finalize(&history, now, state.config.history_retention_days)
    {
        emit(
            LogLevel::Error,
            LogEvent::SourceStarted,
            json!({ "message": format!("调度器写跳过记录失败：{error}") }),
        );
        return;
    }
    emit(
        LogLevel::Warn,
        LogEvent::SourceStarted,
        json!({
            "task_id": task.task_id,
            "run_record_id": history.run_record_id,
            "message": format!("调度未发起本次运行：{message}"),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColumnMapping, TaskSpec, WriteMode};
    use chrono::{NaiveDate, TimeDelta};

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    fn task(task_id: &str, cron: Option<&str>, enabled: bool) -> Task {
        Task {
            task_id: task_id.to_owned(),
            name: format!("任务 {task_id}"),
            source_datasource_id: "src".to_owned(),
            target_datasource_id: "tgt".to_owned(),
            spec: TaskSpec {
                source_sql: None,
                dblink: None,
                owner: "APP".to_owned(),
                table: "T".to_owned(),
                target_table: "t".to_owned(),
                where_clause: None,
                write_mode: WriteMode::Append,
                schedule_cron: cron.map(str::to_owned),
                schedule_enabled: enabled,
                primary_key: vec!["id".to_owned()],
                columns: vec![ColumnMapping {
                    source: "ID".to_owned(),
                    target: "id".to_owned(),
                }],
            },
        }
    }

    #[test]
    fn a_task_first_seen_is_never_fired_for_a_moment_that_already_passed() {
        // 服务在 01:00 起来，任务配的是每天 02:00——起来那一刻不该有任何触发，
        // 哪怕它昨天 02:00 那次没跑成。这就是「停机期间不补跑」。
        let mut state = ScheduleState::default();
        let tasks = vec![task("t1", Some("0 2 * * *"), true)];
        assert!(state.observe(&tasks, at("2026-08-28 03:00")).is_empty());
        assert!(state.observe(&tasks, at("2026-08-29 01:59")).is_empty());
        let due = state.observe(&tasks, at("2026-08-29 02:00"));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].due_at, at("2026-08-29 02:00"));
    }

    #[test]
    fn a_long_outage_yields_one_occurrence_not_a_burst() {
        // 03:00 起、每小时一次；再回来已是三天后。补跑会是 72 次写入，实际只该有一次。
        let mut state = ScheduleState::default();
        let tasks = vec![task("t1", Some("0 * * * *"), true)];
        assert!(state.observe(&tasks, at("2026-08-28 03:00")).is_empty());
        let due = state.observe(&tasks, at("2026-08-31 03:30"));
        assert_eq!(due.len(), 1);
        let next = state.next_fires();
        assert_eq!(next[0].1.as_deref(), Some("2026-08-31 04:00"));
    }

    #[test]
    fn the_switch_being_off_means_it_never_fires() {
        let mut state = ScheduleState::default();
        let tasks = vec![task("t1", Some("0 * * * *"), false)];
        assert!(state.observe(&tasks, at("2026-08-28 03:00")).is_empty());
        assert!(state.observe(&tasks, at("2026-08-28 04:00")).is_empty());
        assert!(state.next_fires().is_empty());
    }

    #[test]
    fn turning_the_switch_back_on_starts_from_the_next_occurrence() {
        let mut state = ScheduleState::default();
        let off = vec![task("t1", Some("0 * * * *"), false)];
        state.observe(&off, at("2026-08-28 03:00"));
        let on = vec![task("t1", Some("0 * * * *"), true)];
        // 打开开关那一刻正好是整点，也不许拿「刚才那一刻」当触发时刻。
        assert!(state.observe(&on, at("2026-08-28 04:00")).is_empty());
        assert_eq!(state.next_fires()[0].1.as_deref(), Some("2026-08-28 05:00"));
    }

    #[test]
    fn rewriting_the_expression_restarts_the_clock() {
        let mut state = ScheduleState::default();
        let hourly = vec![task("t1", Some("0 * * * *"), true)];
        state.observe(&hourly, at("2026-08-28 03:10"));
        let daily = vec![task("t1", Some("0 2 * * *"), true)];
        assert!(state.observe(&daily, at("2026-08-28 03:20")).is_empty());
        assert_eq!(state.next_fires()[0].1.as_deref(), Some("2026-08-29 02:00"));
    }

    #[test]
    fn an_expression_that_never_fires_stays_quiet() {
        let mut state = ScheduleState::default();
        // 2 月 30 号：合法的五个字段，永远不会到。
        let tasks = vec![task("t1", Some("0 0 30 2 *"), true)];
        assert!(state.observe(&tasks, at("2026-08-28 03:00")).is_empty());
        assert_eq!(state.next_fires()[0].1, None);
    }

    #[test]
    fn a_deleted_task_takes_its_next_fire_time_with_it() {
        let mut state = ScheduleState::default();
        let tasks = vec![task("t1", Some("0 * * * *"), true)];
        state.observe(&tasks, at("2026-08-28 03:10"));
        assert_eq!(state.next_fires().len(), 1);
        state.observe(&[], at("2026-08-28 03:20"));
        assert!(state.next_fires().is_empty());
    }

    #[test]
    fn the_queue_answers_whether_a_task_is_waiting() {
        let mut state = ScheduleState::default();
        assert!(!state.is_queued("t1"));
        state.enqueue(QueuedOccurrence {
            task_id: "t1".to_owned(),
            task_name: "任务 t1".to_owned(),
            due_at: "2026-08-28 02:00".to_owned(),
            waiting_reason: String::new(),
        });
        assert!(state.is_queued("t1"));
        state.note_waiting("t1", "额度满");
        assert_eq!(state.queue()[0].waiting_reason, "额度满");
        state.remove_queued("t1");
        assert!(!state.is_queued("t1"));
    }

    #[test]
    fn one_fire_time_is_the_next_minute_after_now_not_now_itself() {
        // `next_after` 是**严格之后**，所以同一分钟内评估两次不会触发两回。
        let mut state = ScheduleState::default();
        let tasks = vec![task("t1", Some("* * * * *"), true)];
        let now = NaiveDate::from_ymd_opt(2026, 8, 28)
            .unwrap()
            .and_hms_opt(3, 10, 0)
            .unwrap();
        assert!(state.observe(&tasks, now).is_empty());
        assert!(state.observe(&tasks, now).is_empty());
        let a_minute_later = now + TimeDelta::minutes(1);
        assert_eq!(state.observe(&tasks, a_minute_later).len(), 1);
    }
}
