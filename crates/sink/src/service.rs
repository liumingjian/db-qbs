use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use db_qbs_shared::{MysqlServerInfo, OpenOutcome, RowCounts, Verdict};
use regex::Regex;
use serde_json::json;

use crate::precheck::{
    precheck_with_primary_key, range_check_columns, range_check_issue, target_check_findings,
};
use crate::{
    AbortResponse, ActiveRun, ApiError, AtomicSwapError, AtomicSwapRequest, BatchPayload,
    BatchResponse, CommitResponse, CreateStagingError, Destination, DestinationFactory,
    DropStagingError, FixedDestination, OpenRunRequest, PrecheckIssue, RangeCheckColumn,
    RangeCheckResult, RunAdmission, RunResponse, SinkService, SourceColumn, TargetCheckRequest,
    TargetCheckResult, TargetColumn, Terminal, WriteBatchError, DEFAULT_MAX_CONCURRENT_RUNS,
    MAX_PREPARED_STATEMENT_PLACEHOLDERS, TOMBSTONE_LIMIT,
};

static RUN_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{14}_[0-9a-f]{6}$").expect("run id regex must compile"));

impl<D: Destination> SinkService<FixedDestination<D>> {
    /// 固定一份目的地的服务：请求里的连接信息被忽略。测试与夹具走这条。
    pub fn new(database: impl Into<String>, destination: Arc<D>) -> Self {
        Self::with_factory(FixedDestination::new(database, destination))
    }
}

impl<F: DestinationFactory> SinkService<F> {
    /// 生产路径：每个 run 按请求里的连接信息现建目的地（ADR-0037 §2）。
    /// 并发额度取 [`DEFAULT_MAX_CONCURRENT_RUNS`]，配置里写了另一个值就再链一次
    /// [`SinkService::with_max_concurrent_runs`]。
    pub fn with_factory(factory: F) -> Self {
        Self {
            factory,
            max_concurrent_runs: DEFAULT_MAX_CONCURRENT_RUNS,
            active_runs: Mutex::new(HashMap::new()),
            admitting: Mutex::new(HashMap::new()),
            tombstones: Mutex::new(VecDeque::new()),
            observed_mysql: Mutex::new(None),
        }
    }

    /// 把并发额度换成 `sink.toml` 里那一份（#260）。
    ///
    /// **0 当 1 用**：配置里写 0 的意思几乎肯定是「不限」而不是「一个都不许跑」，
    /// 但「不限」正是本票要撤掉的那件事；把它收成 1 既不静默放行、也不让 agent 变砖。
    pub fn with_max_concurrent_runs(mut self, max_concurrent_runs: usize) -> Self {
        self.max_concurrent_runs = max_concurrent_runs.max(1);
        self
    }

    /// 最近一次连上目标端时读到的 MySQL 自述，从没连上过就是 `None`（#257）。
    /// `GET /v1/agent/info` 读的就是这里。
    pub fn observed_mysql(&self) -> Option<MysqlServerInfo> {
        self.observed_mysql
            .lock()
            .expect("observed mysql mutex poisoned")
            .clone()
    }

    /// 记下这条连接背后那台 MySQL 的自述。
    ///
    /// **读不到就不动缓存**：一次读失败不该把上一次读到的真值抹成「未知」，
    /// 那会让界面上的版本一列莫名其妙地闪回横杠。
    fn remember_mysql<D: Destination>(&self, destination: &D) {
        let Some(observed) = destination.server_info() else {
            return;
        };
        *self
            .observed_mysql
            .lock()
            .expect("observed mysql mutex poisoned") = Some(observed);
    }

    pub fn check_target(&self, request: TargetCheckRequest) -> Result<TargetCheckResult, ApiError> {
        if request.target_table.trim().is_empty() {
            return Err(ApiError {
                status: 400,
                code: "BAD_REQUEST",
                message: "target_table 不能为空".to_owned(),
                run_id: None,
                details: json!({}),
            });
        }
        let connected = self
            .factory
            .connect(&request.target)
            .map_err(target_check_environment)?;
        // 手上有凭据的路径顺手把版本记一笔（#257）——信息接口本身连不了 MySQL。
        self.remember_mysql(connected.destination.as_ref());
        let columns = connected
            .destination
            .target_columns(&request.target_table)
            .map_err(target_check_environment)?;
        let keys = connected
            .destination
            .target_keys(&request.target_table)
            .map_err(target_check_environment)?;
        let findings = target_check_findings(
            &request.target_table,
            &request.primary_key,
            &request.source_columns,
            &columns,
            &keys,
        );
        Ok(TargetCheckResult {
            ok: findings.is_empty(),
            findings,
            suggested_ddl: None,
        })
    }

    /// 受理一个 run：并发额度与目标表互斥两道判定，在同一把锁下一起做（#260）。
    ///
    /// **互斥键是「（agent、库、目标表）」，不是「任务」**。任务那一层的互斥在 source 侧，
    /// 但 source 可能有好几台，而且没有一台掌握目标库的真相——两个不同任务完全可以
    /// 指向同一张目标表（目标表就是个字符串，没有唯一性约束）。串行时代这不出事；
    /// 并发之后，一个任务正在整表改写、另一个刚把行 upsert 进去，那些行会被静默盖掉，
    /// **两次运行都报成功，数据已经没了**。所以这道判定必须落在写库的这一端。
    ///
    /// 表名按**忽略大小写**比：MySQL 的表名大小写敏感与否随 `lower_case_table_names`
    /// 和文件系统变，判互斥宁可多拦一次，也不能因为大小写差一位就放两个 run 同时进去。
    ///
    /// 返回的是一张**位子的收据**：它一 drop，位子就还回去，`open` 后半程的每一条
    /// 出错分支都不必记得手工归还。
    fn admit(
        &self,
        run_id: &str,
        database: &str,
        target_table: &str,
    ) -> Result<AdmissionGuard<'_, F>, ApiError> {
        // 两把锁的次序在整个 crate 里只有这一处成对出现：先 `active_runs` 再 `admitting`。
        let active_runs = self.active_runs.lock().expect("active run mutex poisoned");
        let mut admitting = self.admitting.lock().expect("admission mutex poisoned");
        // 同一个 run id 重开自己不算冲突，也不占第二份额度。
        let holder = active_runs
            .values()
            .filter(|run| run.run_id != run_id)
            .find(|run| {
                run.database == database && run.target_table.eq_ignore_ascii_case(target_table)
            })
            .map(|run| run.run_id.clone())
            .or_else(|| {
                admitting
                    .iter()
                    .filter(|(id, _)| id.as_str() != run_id)
                    .find(|(_, admission)| {
                        admission.database == database
                            && admission.target_table.eq_ignore_ascii_case(target_table)
                    })
                    .map(|(id, _)| id.clone())
            });
        if let Some(holder) = holder {
            return Err(target_table_busy_error(
                run_id,
                database,
                target_table,
                &holder,
            ));
        }

        let mut in_flight = active_runs
            .keys()
            .chain(admitting.keys())
            .filter(|id| id.as_str() != run_id)
            .cloned()
            .collect::<Vec<_>>();
        in_flight.sort();
        in_flight.dedup();
        if in_flight.len() >= self.max_concurrent_runs {
            return Err(run_quota_exceeded_error(
                run_id,
                self.max_concurrent_runs,
                &in_flight,
            ));
        }

        admitting.insert(
            run_id.to_owned(),
            RunAdmission {
                database: database.to_owned(),
                target_table: target_table.to_owned(),
            },
        );
        Ok(AdmissionGuard {
            service: self,
            run_id: run_id.to_owned(),
        })
    }

    /// 开一个 run。**回 [`OpenOutcome::RangeCheckNeeded`] 时什么都没建、什么都没存**——
    /// 预检的 1–3 步过了，但 3.5 步值域校核要源端去数真实数据，本次调用到此为止。
    /// 那不是「开成了一半」，`active_runs` 里不会有条目，此时推批次只会拿到 404。
    pub fn open(&self, request: OpenRunRequest) -> Result<OpenOutcome, ApiError> {
        validate_open_request(&request)?;

        // 连接先建起来：目标端元数据、暂存表 DDL、写批次、切换全在这一条连接上跑。
        // 连不上是目标端环境故障，复用 `SINK_ENVIRONMENT`（ADR-0037 §9，码闭集不增）。
        let connected = self
            .factory
            .connect(&request.target)
            .map_err(|message| target_connect_error(&request.run_id, message))?;
        let destination = connected.destination;
        self.remember_mysql(destination.as_ref());
        // 受理点在**连上之后**：库名是工厂给的那一份（`ConnectedDestination::database`），
        // 请求里的那个字段只是它的原料。一次连接的代价换互斥键与运行时用的是同一个库名。
        let _admission = self.admit(&request.run_id, &connected.database, &request.target_table)?;

        let target_columns =
            destination
                .target_columns(&request.target_table)
                .map_err(|message| ApiError {
                    status: 500,
                    code: "STAGING_CREATE_FAILED",
                    message: format!(
                        "读取目标表元数据失败：{message}。这是目标端环境故障，目标表未被改动"
                    ),
                    run_id: Some(request.run_id.clone()),
                    details: json!({ "kind": "OTHER" }),
                })?;
        let target_keys = destination
            .target_keys(&request.target_table)
            .map_err(|message| ApiError {
                status: 500,
                code: "STAGING_CREATE_FAILED",
                message: format!(
                    "读取目标表唯一约束失败：{message}。这是目标端环境故障，目标表未被改动"
                ),
                run_id: Some(request.run_id.clone()),
                details: json!({ "kind": "OTHER" }),
            })?;
        let mut issues = precheck_with_primary_key(
            &request.target_table,
            &request.primary_key,
            &request.source_columns,
            &target_columns,
            &target_keys,
        );
        let range_columns = range_check_columns(&request.source_columns, &target_columns);
        if !range_columns.is_empty() {
            let Some(results) = request.range_check_results.as_deref() else {
                return Ok(OpenOutcome::RangeCheckNeeded {
                    run_id: request.run_id,
                    columns_checked: target_columns.len(),
                    columns: range_columns,
                });
            };
            append_range_check_issues(
                &request.run_id,
                &request.source_columns,
                &target_columns,
                &range_columns,
                results,
                &mut issues,
            )?;
        }
        if !issues.is_empty() {
            let total = issues.len();
            return Err(ApiError {
                status: 422,
                code: "PRECHECK_FAILED",
                message: format!("映射预检未通过：一次发现 {total} 项问题，未创建暂存表"),
                run_id: Some(request.run_id),
                details: json!({ "issues": issues, "total": total }),
            });
        }

        let staging_table = format!("{}__stg_{}", request.target_table, request.run_id);
        let ddl = build_staging_ddl(&connected.database, &staging_table, &target_columns);
        destination
            .create_staging(&staging_table, &ddl)
            .map_err(|error| create_staging_api_error(&request.run_id, &staging_table, error))?;

        let source_columns = request
            .source_columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        // 切换语句**只覆盖被映射到的列**。铺到目标表全部列的话，未映射列会被暂存表里那个
        // NULL 一路写进目标表：预检按 ADR-0038 §5 第 3 分支放行了「未映射但有默认值」的列，
        // 运行时却给它写 NULL，撞 `ERROR 1048`——预检与切换两套语义打架。
        // 收窄之后，未映射列在 INSERT 时走它自己的 DEFAULT，在 UPDATE 时保持原值。
        let mut ordered_target_columns = target_columns
            .iter()
            .filter(|column| {
                source_columns
                    .iter()
                    .any(|mapped| mapped.eq_ignore_ascii_case(&column.name))
            })
            .collect::<Vec<_>>();
        ordered_target_columns.sort_by_key(|column| column.ordinal);
        let swap_columns = ordered_target_columns
            .into_iter()
            .map(|column| column.name.clone())
            .collect();
        let active_run = ActiveRun {
            run_id: request.run_id.clone(),
            database: connected.database,
            staging_table: staging_table.clone(),
            max_rows_per_insert: MAX_PREPARED_STATEMENT_PLACEHOLDERS / source_columns.len(),
            source_columns,
            swap_columns,
            target_table: request.target_table,
            primary_key: request.primary_key,
            next_seq: 1,
            rows_written: 0,
            sealed: false,
            destination,
        };
        self.active_runs
            .lock()
            .expect("active run mutex poisoned")
            .insert(request.run_id.clone(), active_run);

        Ok(OpenOutcome::Opened {
            run_id: request.run_id,
            staging_table,
            columns_checked: target_columns.len(),
        })
    }

    pub fn write_batch(
        &self,
        run_id: &str,
        payload: BatchPayload,
    ) -> Result<BatchResponse, ApiError> {
        // 锁**只圈住取句柄与占号那几行**，MySQL 写在锁外做（#260）。
        //
        // 从前这把锁一路攥到 `write_batch` 返回，于是任意一个 run 的一次批次写入
        // 会把这台 agent 上所有其他 run 的推送全堵住——一次切换时的整表改写
        // 能让别的任务停在那里干等。提交那条路早就是这个写法（取出 `run.clone()`、
        // 放锁、再 `atomic_swap`），本票只是把剩下的一条统一过来。
        //
        // 序号是**先占后写**的：占号与「期望第几批」的判定在同一把锁下完成，
        // 所以两条同 run 的推送不会双双认为自己是第 n 批；写失败再把号退回去，
        // 对调用方而言 `next_seq` 与从前一个字节不差。
        let run = {
            let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
            let Some(run) = active_runs.get_mut(run_id) else {
                drop(active_runs);
                return Err(self.inactive_run_error(run_id));
            };

            if run.sealed {
                return Err(run_sealed_error(run_id, active_run_response(run_id, run)));
            }

            if payload.seq != run.next_seq {
                return Err(ApiError {
                    status: 409,
                    code: "SEQ_MISMATCH",
                    message: format!(
                        "run {run_id} 批次顺序不符：期望第 {} 批，实际第 {} 批",
                        run.next_seq, payload.seq
                    ),
                    run_id: Some(run_id.to_owned()),
                    details: json!({ "expected": run.next_seq, "got": payload.seq }),
                });
            }

            let column_count = run.source_columns.len();
            if let Some((row_index, row)) = payload
                .rows
                .iter()
                .enumerate()
                .find(|(_, row)| row.len() != column_count)
            {
                return Err(ApiError {
                    status: 400,
                    code: "BAD_REQUEST",
                    message: format!(
                        "第 {} 批第 {} 行有 {} 个值，source_columns 要求 {column_count} 个",
                        payload.seq,
                        row_index + 1,
                        row.len()
                    ),
                    run_id: Some(run_id.to_owned()),
                    details: json!({
                        "seq": payload.seq,
                        "row": row_index + 1,
                        "expected": column_count,
                        "got": row.len(),
                    }),
                });
            }

            run.next_seq += 1;
            run.clone()
        };

        let rows_written = match run.destination.write_batch(
            &run.staging_table,
            &run.source_columns,
            &payload.rows,
            run.max_rows_per_insert,
        ) {
            Ok(rows_written) => rows_written,
            Err(error) => {
                self.release_seq(run_id, payload.seq);
                return Err(write_batch_api_error(run_id, payload.seq, error));
            }
        };
        let row_count = payload.rows.len();
        if rows_written != row_count as u64 {
            self.release_seq(run_id, payload.seq);
            return Err(ApiError {
                status: 500,
                code: "INTERNAL_ASSERTION_FAILED",
                message: format!(
                    "第 {} 批写入行数断言失败：发送 {} 行，MySQL affected_rows 合计为 {rows_written}；这是程序缺陷，不是数据或环境问题，请报 issue",
                    payload.seq,
                    row_count
                ),
                run_id: Some(run_id.to_owned()),
                details: json!({
                    "seq": payload.seq,
                    "expected": row_count,
                    "written": rows_written,
                }),
            });
        }

        // 落账要回到锁里：这中间这条 run 可能已经被 abort 掉了，那就没有账可落。
        let next_seq = {
            let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
            match active_runs.get_mut(run_id) {
                Some(run) => {
                    run.rows_written += rows_written;
                    run.next_seq
                }
                None => payload.seq + 1,
            }
        };
        Ok(BatchResponse {
            seq: payload.seq,
            rows_written,
            next_seq,
        })
    }

    /// 本批次写失败，把先前占下的序号退回去（#260）。
    ///
    /// 只在号还是自己占的那一格时才退：run 已经被 abort 掉、或后一批已经占过号，
    /// 都不该由这条失败路径去改。
    fn release_seq(&self, run_id: &str, seq: u64) {
        let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
        if let Some(run) = active_runs.get_mut(run_id) {
            if run.next_seq == seq + 1 {
                run.next_seq = seq;
            }
        }
    }

    pub fn commit(
        &self,
        run_id: &str,
        total_batches: u64,
        total_rows: u64,
    ) -> Result<CommitResponse, ApiError> {
        let run = {
            let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
            let Some(run) = active_runs.get_mut(run_id) else {
                drop(active_runs);
                return Err(self.inactive_run_error(run_id));
            };
            if run.sealed {
                return Err(run_sealed_error(run_id, active_run_response(run_id, run)));
            }
            run.sealed = true;
            run.clone()
        };

        let received_batches = run.next_seq - 1;
        let swap_request = AtomicSwapRequest {
            run_id: run.run_id.clone(),
            staging_table: run.staging_table.clone(),
            target_table: run.target_table.clone(),
            primary_key: run.primary_key.clone(),
            columns: run.swap_columns.clone(),
            source_rows: total_rows,
            source_batches: total_batches,
            received_batches,
        };

        match run.destination.atomic_swap(&swap_request) {
            Ok(result) => {
                self.finish_run(
                    run_id,
                    &run,
                    Terminal::Swapped,
                    result.purged_rows,
                    result.swapped_rows,
                );
                self.drop_after_swap(run.destination.as_ref(), run_id, &run.staging_table)?;
                Ok(CommitResponse {
                    source_rows: total_rows,
                    staged_rows: result.staged_rows,
                    purged_rows: result.purged_rows,
                    swapped_rows: result.swapped_rows,
                    count_ms: result.count_ms,
                })
            }
            Err(AtomicSwapError::VerifyFailed {
                staged_rows,
                count_ms,
            }) => {
                self.finish_run(run_id, &run, Terminal::Discarded, 0, 0);
                let mut error = verify_failed_error(
                    run_id,
                    &swap_request,
                    staged_rows,
                    run.rows_written,
                    count_ms,
                );
                self.drop_after_discard(run.destination.as_ref(), &run.staging_table, &mut error);
                Err(error)
            }
            Err(AtomicSwapError::TargetBusy { errno }) => {
                self.finish_run(run_id, &run, Terminal::Discarded, 0, 0);
                let mut error = ApiError {
                    status: 409,
                    code: "SWAP_TARGET_BUSY",
                    message: format!(
                        "目标表 {} 当前被另一个切换事务占用，通常是同一张表上有另一个 run 正在切换；这不是数据错误，重跑即可",
                        run.target_table
                    ),
                    run_id: Some(run_id.to_owned()),
                    details: json!({
                        "target_table": &run.target_table,
                        "errno": errno,
                    }),
                };
                self.drop_after_discard(run.destination.as_ref(), &run.staging_table, &mut error);
                Err(error)
            }
            Err(AtomicSwapError::Other(message)) => {
                self.finish_run(run_id, &run, Terminal::Discarded, 0, 0);
                let mut error = ApiError {
                    status: 500,
                    code: "SWAP_FAILED",
                    message: format!(
                        "切换目标表失败，事务已回滚：{message}。目标表未被触碰，可直接重跑"
                    ),
                    run_id: Some(run_id.to_owned()),
                    details: json!({}),
                };
                self.drop_after_discard(run.destination.as_ref(), &run.staging_table, &mut error);
                Err(error)
            }
        }
    }

    pub fn get(&self, run_id: &str) -> Result<RunResponse, ApiError> {
        if let Some(run) = self
            .active_runs
            .lock()
            .expect("active run mutex poisoned")
            .get(run_id)
        {
            return Ok(active_run_response(run_id, run));
        }
        self.tombstones
            .lock()
            .expect("tombstone mutex poisoned")
            .iter()
            .find(|tombstone| tombstone.run_id == run_id)
            .cloned()
            .ok_or_else(|| run_unknown_error(run_id))
    }

    pub fn abort(&self, run_id: &str) -> Result<AbortResponse, ApiError> {
        let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
        let Some(run) = active_runs.get(run_id).cloned() else {
            return Ok(AbortResponse {
                run_id: run_id.to_owned(),
                staging_dropped: false,
            });
        };
        if run.sealed {
            return Err(run_sealed_error(run_id, active_run_response(run_id, &run)));
        }

        run.destination
            .drop_staging(&run.staging_table)
            .map_err(|error| drop_staging_api_error(run_id, &run.staging_table, error))?;
        self.push_tombstone(terminal_response(run_id, &run, Terminal::Discarded, 0, 0));
        active_runs.remove(run_id);
        drop(active_runs);

        Ok(AbortResponse {
            run_id: run_id.to_owned(),
            staging_dropped: true,
        })
    }

    fn inactive_run_error(&self, run_id: &str) -> ApiError {
        let tombstone = self
            .tombstones
            .lock()
            .expect("tombstone mutex poisoned")
            .iter()
            .find(|tombstone| tombstone.run_id == run_id && tombstone.sealed)
            .cloned();
        match tombstone {
            Some(tombstone) => run_sealed_error(run_id, tombstone),
            None => run_unknown_error(run_id),
        }
    }

    fn drop_after_swap(
        &self,
        destination: &F::Dest,
        run_id: &str,
        staging_table: &str,
    ) -> Result<(), ApiError> {
        destination.drop_staging(staging_table).map_err(|error| {
            let (message, details) = match error {
                DropStagingError::PermissionDenied => (
                    format!(
                        "目标表已完成切换，但暂存表 {staging_table} 清理失败：sink 的 MySQL 账号缺少 DROP 权限"
                    ),
                    json!({ "kind": "PERMISSION_DENIED", "operation": "DROP" }),
                ),
                DropStagingError::Other(message) => (
                    format!(
                        "目标表已完成切换，但暂存表 {staging_table} 清理失败：{message}"
                    ),
                    json!({ "kind": "OTHER" }),
                ),
            };
            ApiError {
                status: 500,
                code: "SWAP_FAILED",
                message,
                run_id: Some(run_id.to_owned()),
                details,
            }
        })
    }

    // 失败路径上的 DROP 失败不得顶替主错误（VERIFY_FAILED 的五个门禁数、SWAP_FAILED 的
    // 回滚措辞都比清理失败重要）；残留暂存表按 ADR-0010 §6 的口径留给手工清。
    fn drop_after_discard(&self, destination: &F::Dest, staging_table: &str, error: &mut ApiError) {
        if let Err(drop_error) = destination.drop_staging(staging_table) {
            let reason = match drop_error {
                DropStagingError::PermissionDenied => "sink 的 MySQL 账号缺少 DROP 权限".to_owned(),
                DropStagingError::Other(message) => message,
            };
            let separator = if error.message.ends_with('。') {
                ""
            } else {
                "；"
            };
            error.message = format!(
                "{}{separator}另外，暂存表 {staging_table} 清理失败：{reason}，需手工 DROP。",
                error.message
            );
            if let Some(details) = error.details.as_object_mut() {
                details.insert(
                    "staging_drop_failed".to_owned(),
                    json!({ "staging_table": staging_table }),
                );
            }
        }
    }

    fn finish_run(
        &self,
        run_id: &str,
        run: &ActiveRun<F::Dest>,
        terminal: Terminal,
        purged_rows: u64,
        swapped_rows: u64,
    ) {
        let mut active_runs = self.active_runs.lock().expect("active run mutex poisoned");
        self.push_tombstone(terminal_response(
            run_id,
            run,
            terminal,
            purged_rows,
            swapped_rows,
        ));
        active_runs.remove(run_id);
    }

    fn push_tombstone(&self, tombstone: RunResponse) {
        let mut tombstones = self.tombstones.lock().expect("tombstone mutex poisoned");
        if tombstones.len() == TOMBSTONE_LIMIT {
            tombstones.pop_front();
        }
        tombstones.push_back(tombstone);
    }
}

fn append_range_check_issues(
    run_id: &str,
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
    range_columns: &[RangeCheckColumn],
    results: &[RangeCheckResult],
    issues: &mut Vec<PrecheckIssue>,
) -> Result<(), ApiError> {
    let expected: HashMap<String, &RangeCheckColumn> = range_columns
        .iter()
        .map(|column| (column.column.to_uppercase(), column))
        .collect();
    let sources: HashMap<String, &SourceColumn> = source_columns
        .iter()
        .map(|column| (column.name.to_uppercase(), column))
        .collect();
    let targets: HashMap<String, &TargetColumn> = target_columns
        .iter()
        .map(|column| (column.name.to_uppercase(), column))
        .collect();
    let mut seen = HashSet::new();

    for result in results {
        let key = result.column.to_uppercase();
        let Some(range_column) = expected.get(&key).copied() else {
            return Err(range_check_results_error(
                run_id,
                format!("值域校核结果包含未请求的列 {}", result.column),
            ));
        };
        if !seen.insert(key.clone()) {
            return Err(range_check_results_error(
                run_id,
                format!("值域校核结果重复包含列 {}", result.column),
            ));
        }
        if result.invalid_rows == 0 {
            continue;
        }
        let source = sources.get(&key).copied().expect("range column has source");
        let target = targets.get(&key).copied().expect("range column has target");
        issues.push(range_check_issue(
            source,
            target,
            range_column,
            result.invalid_rows,
        ));
    }

    if seen.len() != expected.len() {
        return Err(range_check_results_error(
            run_id,
            "值域校核结果没有逐列覆盖 sink 请求的列".to_owned(),
        ));
    }
    Ok(())
}

fn range_check_results_error(run_id: &str, message: String) -> ApiError {
    ApiError {
        status: 400,
        code: "BAD_REQUEST",
        message,
        run_id: Some(run_id.to_owned()),
        details: json!({}),
    }
}

fn active_run_response<D>(run_id: &str, run: &ActiveRun<D>) -> RunResponse {
    RunResponse {
        run_id: run_id.to_owned(),
        staging_table: run.staging_table.clone(),
        batches_received: run.next_seq - 1,
        rows_written: run.rows_written,
        sealed: run.sealed,
        terminal: None,
        purged_rows: None,
        swapped_rows: None,
    }
}

fn terminal_response<D>(
    run_id: &str,
    run: &ActiveRun<D>,
    terminal: Terminal,
    purged_rows: u64,
    swapped_rows: u64,
) -> RunResponse {
    RunResponse {
        terminal: Some(terminal),
        purged_rows: Some(purged_rows),
        swapped_rows: Some(swapped_rows),
        ..active_run_response(run_id, run)
    }
}

/// 连不上目标端。**不增错误码**（ADR-0037 §9）：`SINK_ENVIRONMENT` 的本义就是
/// 「目标端环境配置错误，不要排查业务数据」，连不上正属这一类。
/// `open` 受理成功后拿到的位子收据（#260）。drop 即归还，成功或失败都一样。
struct AdmissionGuard<'a, F: DestinationFactory> {
    service: &'a SinkService<F>,
    run_id: String,
}

impl<F: DestinationFactory> Drop for AdmissionGuard<'_, F> {
    fn drop(&mut self) {
        // 锁坏了就不动：那时进程已经在往下崩，硬 panic 只会把真正的第一现场盖掉。
        if let Ok(mut admitting) = self.service.admitting.lock() {
            admitting.remove(&self.run_id);
        }
    }
}

/// 并发额度满了（#260）。**当场拒，不排队**：排队等于让 source 那边挂着一条
/// 看不出为什么不动的连接，而「额度满了」是句能读懂、能处置的话。
fn run_quota_exceeded_error(run_id: &str, max: usize, in_flight: &[String]) -> ApiError {
    ApiError {
        status: 429,
        code: "RUN_QUOTA_EXCEEDED",
        message: format!(
            "目标端 agent 并发额度已满：同时最多允许 {max} 次运行，当前在飞的是 {}；本次未受理，目标表未被改动，等其中一次结束后重跑，或把 sink.toml 的 max_concurrent_runs 调大",
            in_flight.join("、")
        ),
        run_id: Some(run_id.to_owned()),
        details: json!({
            "max_concurrent_runs": max,
            "in_flight": in_flight,
        }),
    }
}

/// 目标表被另一次运行占着（#260）。措辞里**点名是哪张表、被哪次运行占着**——
/// 只说「冲突了」的话，现场那个人还得自己去猜是哪两个任务撞在了一起。
fn target_table_busy_error(
    run_id: &str,
    database: &str,
    target_table: &str,
    holder: &str,
) -> ApiError {
    ApiError {
        status: 409,
        code: "TARGET_TABLE_BUSY",
        message: format!(
            "目标表 {database}.{target_table} 正被运行 {holder} 占用：同一台 agent 上同一张目标表同时只允许一次运行；本次未受理，目标表未被改动，等 {holder} 结束后重跑"
        ),
        run_id: Some(run_id.to_owned()),
        details: json!({
            "database": database,
            "target_table": target_table,
            "holder_run_id": holder,
        }),
    }
}

fn target_connect_error(run_id: &str, message: String) -> ApiError {
    ApiError {
        status: 500,
        code: "SINK_ENVIRONMENT",
        message: format!(
            "连接目标端失败：{message}。这是目标端环境或数据源配置错误，目标表未被改动"
        ),
        run_id: Some(run_id.to_owned()),
        details: json!({ "kind": "OTHER" }),
    }
}

fn target_check_environment(message: String) -> ApiError {
    ApiError {
        status: 500,
        code: "SINK_ENVIRONMENT",
        message: format!("读取目标端元数据失败：{message}"),
        run_id: None,
        details: json!({ "kind": "OTHER" }),
    }
}

fn run_unknown_error(run_id: &str) -> ApiError {
    ApiError {
        status: 404,
        code: "RUN_UNKNOWN",
        message: format!("未知 run {run_id}"),
        run_id: Some(run_id.to_owned()),
        details: json!({}),
    }
}

fn run_sealed_error(run_id: &str, status: RunResponse) -> ApiError {
    ApiError {
        status: 409,
        code: "RUN_SEALED",
        message: format!("run {run_id} 已封口，不能再接收批次或重复 commit"),
        run_id: Some(run_id.to_owned()),
        details: serde_json::to_value(status).expect("run response must serialize"),
    }
}

fn verify_failed_error(
    run_id: &str,
    request: &AtomicSwapRequest,
    staged_rows: u64,
    sink_reported_rows: u64,
    count_ms: u64,
) -> ApiError {
    let source_rows = request.source_rows;
    let source_batches = request.source_batches;
    let received_batches = request.received_batches;
    // 哪条腿垮了由 `shared::verification` 说，这里只挑话说：`Verdict::RowsDiffer` 意味着
    // 批次全到齐了，所以「sink 逐批报告写了这么多、暂存表里却少了」才是**写入过程丢数据**；
    // 其余情形一律落到「有批次未送达」。`sink_reported_rows` 不参与门禁，只参与措辞——
    // 它是 sink 自报的中间数，正是门禁要怀疑的那一环。
    let counts = RowCounts {
        source_rows,
        staged_rows,
        source_batches,
        received_batches,
    };
    let rows_lost_after_an_accepted_write = matches!(counts.verdict(), Verdict::RowsDiffer { .. })
        && sink_reported_rows == source_rows
        && staged_rows < sink_reported_rows;
    let message = if rows_lost_after_an_accepted_write {
        format!(
            "校验未通过：源端读出 {source_rows} 行，sink 逐批报告已写入 {sink_reported_rows} 行，但暂存表实际只有 {staged_rows} 行——数据在写入 MySQL 的过程中丢失。目标表未被触碰，可直接重跑；重跑仍失败请报 issue。"
        )
    } else {
        format!(
            "校验未通过：源端读出 {source_rows} 行 / {source_batches} 批，sink 只收到 {sink_reported_rows} 行 / {received_batches} 批——有批次未送达。目标表未被触碰，可直接重跑。"
        )
    };
    ApiError {
        status: 409,
        code: "VERIFY_FAILED",
        message,
        run_id: Some(run_id.to_owned()),
        details: json!({
            "source_rows": source_rows,
            "staged_rows": staged_rows,
            "source_batches": source_batches,
            "received_batches": received_batches,
            "sink_reported_rows": sink_reported_rows,
            "count_ms": count_ms,
        }),
    }
}

/// 写批次失败的三个成因各占一个码（ADR-0010 闭集 12 → 15、ADR-0029 §1）。
///
/// 早先三者共用两个别处的码：数据被 MySQL 逐值拒绝报 `BAD_REQUEST`（本义是「请求不合法」），
/// 写暂存表失败与 `max_allowed_packet` 环境错都报 `SWAP_FAILED`（本义是「切换失败」）。
/// 人话虽然分得清，但**码上分不清**，排障与失败分类只能靠匹配文字。
/// 状态码与报文形状不动，只把 `code` 拆开。
fn write_batch_api_error(run_id: &str, seq: u64, error: WriteBatchError) -> ApiError {
    match error {
        WriteBatchError::DataValue {
            mysql_code,
            column,
            value,
        } => ApiError {
            status: 400,
            code: "DATA_REJECTED",
            message: mysql_data_value_message(seq, mysql_code, &column, value.as_deref()),
            run_id: Some(run_id.to_owned()),
            details: json!({
                "seq": seq,
                "mysql_code": mysql_code,
                "column": column,
                "value": value,
            }),
        },
        WriteBatchError::PrecheckEscape {
            mysql_code,
            column,
            value,
        } => ApiError {
            status: 500,
            code: "INTERNAL_PRECHECK_ESCAPE",
            message: format!(
                "第 {seq} 批出现 MySQL Note {mysql_code}：预检本应阻止的静默改值路径逃逸；这是 P0 程序缺陷，不是运行故障，请报 issue"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({
                "seq": seq,
                "mysql_code": mysql_code,
                "column": column,
                "value": value,
            }),
        },
        WriteBatchError::Environment { mysql_code } => ApiError {
            status: 500,
            code: "SINK_ENVIRONMENT",
            message: format!(
                "第 {seq} 批写入被 MySQL ERROR {mysql_code} 拒绝：max_allowed_packet 低于开连接仪式要求或运行期被改小；这是目标端环境配置错误，请恢复到至少 64 MiB，不要排查业务数据"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "seq": seq, "mysql_code": mysql_code }),
        },
        WriteBatchError::Other(message) => ApiError {
            status: 500,
            code: "BATCH_WRITE_FAILED",
            message: format!(
                "第 {seq} 批写入暂存表失败，整批事务已回滚：{message}；这是目标端故障，目标表未被改动"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "seq": seq }),
        },
    }
}

fn mysql_data_value_message(
    seq: u64,
    mysql_code: u16,
    column: &str,
    value: Option<&str>,
) -> String {
    let value = value.unwrap_or("<无法从 MySQL 错误定位原值>");
    let reason = match mysql_code {
        1264 => "超出目标数值范围，请修正该列数据",
        1292 => "不是目标日期列可接受的日期时间，请修正该列数据",
        1366 => "无法按目标列字符集写入，请检查该列字符集与这个值",
        _ => "未被目标列接受，请修正该列数据",
    };
    format!(
        "第 {seq} 批数据问题：列 {column} 的值 {value} {reason}；整批事务已回滚，目标表未被改动"
    )
}

fn validate_open_request(request: &OpenRunRequest) -> Result<(), ApiError> {
    let mut problems = Vec::new();
    if !RUN_ID_RE.is_match(&request.run_id) {
        problems.push("run_id 必须是 14 位 UTC 时间、下划线和 6 位小写 hex");
    }
    if request.target_table.is_empty() {
        problems.push("target_table 不能为空");
    }
    if request.primary_key.is_empty() {
        problems.push("primary_key 不能为空：upsert 的去重键必选");
    }
    if request.source_columns.is_empty() {
        problems.push("source_columns 不能为空");
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ApiError {
            status: 400,
            code: "BAD_REQUEST",
            message: problems.join("；"),
            run_id: Some(request.run_id.clone()),
            details: json!({ "problems": problems }),
        })
    }
}

pub fn build_staging_ddl(
    database: &str,
    staging_table: &str,
    target_columns: &[TargetColumn],
) -> String {
    let mut columns: Vec<_> = target_columns.iter().collect();
    columns.sort_by_key(|column| column.ordinal);
    let definitions = columns
        .iter()
        .map(|column| {
            let character_set = column
                .character_set
                .as_deref()
                .map(|value| format!(" CHARACTER SET {value}"))
                .unwrap_or_default();
            format!(
                "  {} {}{} NULL",
                quote_identifier(&column.name),
                column.column_type,
                character_set
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "CREATE TABLE {}.{} (\n{}\n)",
        quote_identifier(database),
        quote_identifier(staging_table),
        definitions
    )
}

pub(crate) fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn create_staging_api_error(
    run_id: &str,
    staging_table: &str,
    error: CreateStagingError,
) -> ApiError {
    match error {
        CreateStagingError::TableExists => ApiError {
            status: 409,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "暂存表 {staging_table} 已存在，绝不会自动 DROP 或重建；它可能是遗留暂存表，表名时间为 {}",
                run_id_time(run_id)
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "TABLE_EXISTS", "staging_table": staging_table }),
        },
        CreateStagingError::PermissionDenied => ApiError {
            status: 500,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "创建暂存表 {staging_table} 失败：sink 的 MySQL 账号缺少 CREATE 权限；目标表未被改动"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "PERMISSION_DENIED", "operation": "CREATE" }),
        },
        CreateStagingError::Other(message) => ApiError {
            status: 500,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "创建暂存表 {staging_table} 失败：{message}。这是目标端故障，目标表未被改动"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "OTHER" }),
        },
    }
}

fn drop_staging_api_error(run_id: &str, staging_table: &str, error: DropStagingError) -> ApiError {
    let (message, details) = match error {
        DropStagingError::PermissionDenied => (
            format!(
                "丢弃暂存表 {staging_table} 失败：sink 的 MySQL 账号缺少 DROP 权限；目标表未被改动"
            ),
            json!({ "kind": "PERMISSION_DENIED", "operation": "DROP" }),
        ),
        DropStagingError::Other(message) => (
            format!("丢弃暂存表 {staging_table} 失败：{message}。这是目标端故障，目标表未被改动"),
            json!({ "kind": "OTHER" }),
        ),
    };
    ApiError {
        status: 500,
        code: "STAGING_CREATE_FAILED",
        message,
        run_id: Some(run_id.to_owned()),
        details,
    }
}

fn run_id_time(run_id: &str) -> String {
    let timestamp = &run_id[..14];
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &timestamp[0..4],
        &timestamp[4..6],
        &timestamp[6..8],
        &timestamp[8..10],
        &timestamp[10..12],
        &timestamp[12..14]
    )
}
