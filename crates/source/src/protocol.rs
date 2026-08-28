use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

// 报文形状的唯一定义在 `db-qbs-shared`（#124）。本文件只留 source 侧自己的
// 客户端错误模型（`SinkError` 一族）与 HTTP 客户端实现。
use db_qbs_shared::{
    AbortResponse, BatchPayload, BatchResponse, CommitRequest, CommitResponse, ErrorEnvelope,
    OpenOutcome, OpenRunRequest, OpenRunResponse, PrecheckIssue, RangeCheckColumn,
    RangeCheckResult, RunResponse,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const NORMAL_READ_TIMEOUT: Duration = Duration::from_secs(60);
const ABORT_TIMEOUT: Duration = Duration::from_secs(30);
const COMMIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkErrorKind {
    Transport,
    Response,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkError {
    pub kind: SinkErrorKind,
    pub code: Option<String>,
    pub message: String,
    pub column: Option<String>,
    pub value: Option<String>,
    pub precheck_issues: Box<Vec<PrecheckIssue>>,
    pub gate: Option<Box<SinkGateDetails>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SinkGateDetails {
    pub source_rows: u64,
    pub staged_rows: u64,
    pub source_batches: u64,
    pub received_batches: u64,
    pub sink_reported_rows: u64,
    pub count_ms: u64,
}

impl SinkError {
    pub fn response(code: Option<String>, message: impl Into<String>) -> Self {
        Self {
            kind: SinkErrorKind::Response,
            code,
            message: message.into(),
            column: None,
            value: None,
            precheck_issues: Box::new(Vec::new()),
            gate: None,
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        Self {
            kind: SinkErrorKind::Transport,
            code: None,
            message: message.into(),
            column: None,
            value: None,
            precheck_issues: Box::new(Vec::new()),
            gate: None,
        }
    }
}

/// 一个开成了的 run。**没有第二种状态**——拿到它就意味着暂存表已建、可以推批次了。
///
/// 这正是它不是 [`OpenRunResponse`] 的原因：那个型别还带着「其实没开成」的可能，
/// 而调用方一旦漏读一个字段就会往一个不存在的 run 里推批次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedRun {
    pub staging_table: String,
    pub columns_checked: usize,
}

/// 开 run 没能开成的三种缘由。**三者的处置各不相同**，所以它们在型别上就是分开的：
/// 目标端拒绝要把预检问题逐条报给人看，值域校核失败是源端故障，而剩下那类是目标端有缺陷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenFailure<E> {
    /// 目标端拒绝或不可达。预检未过也走这条（`PRECHECK_FAILED`，`precheck_issues` 里逐条带着）。
    Sink(SinkError),
    /// 值域校核在源端没跑成。`E` 是调用方自己的源端错误型别。
    RangeCheck(E),
    /// 目标端答非所问。只可能在目标端有缺陷时出现，人话已经在里面了。
    Defect(&'static str),
}

pub trait SinkClient {
    /// 一次 `POST /v1/runs` 往返。**它可能回「还没开成」**（[`OpenOutcome::RangeCheckNeeded`]）——
    /// 两段式的第二段由 [`SinkClient::open`] 走完，实现这个 trait 只需管好一次往返。
    fn open_attempt(&mut self, request: &OpenRunRequest) -> Result<OpenOutcome, SinkError>;

    /// 开一个 run，**两段式在这里走完**。
    ///
    /// sink 跑完映射预检的 1–3 步后，可能回过头来要求源端跑 3.5 步值域校核：那一步要数真实
    /// 数据里有多少行超出目标列的值域，只有源端够得着。此时目标端**什么都没建、什么都没存**，
    /// 要拿着同一份请求、附上校核结果再问一次。`range_check` 就是「去把这几列数一遍」，
    /// 由调用方提供——数据从哪来是它的事，这里只负责把结果填回请求里续走。
    ///
    /// 调用方看不到这一切：要么拿到一个 [`OpenedRun`]，要么拿到一条说得出缘由的失败。
    fn open<E>(
        &mut self,
        mut request: OpenRunRequest,
        mut range_check: impl FnMut(&[RangeCheckColumn]) -> Result<Vec<RangeCheckResult>, E>,
    ) -> Result<OpenedRun, OpenFailure<E>> {
        let mut asked_already = false;
        loop {
            let outcome = self.open_attempt(&request).map_err(OpenFailure::Sink)?;
            // run_id 由源端生成、原样回显。对不上说明目标端认错了 run，两次往返各校验一次:
            // 第二次对不上意味着中途换了个 run，值得与第一次分开说。
            if outcome.run_id() != request.run_id {
                return Err(OpenFailure::Defect(if asked_already {
                    "目标端第二次开任务响应的 run_id 与请求不一致"
                } else {
                    "目标端开任务响应的 run_id 与请求不一致"
                }));
            }
            match outcome {
                OpenOutcome::Opened {
                    staging_table,
                    columns_checked,
                    ..
                } => {
                    return Ok(OpenedRun {
                        staging_table,
                        columns_checked,
                    })
                }
                OpenOutcome::RangeCheckNeeded { columns, .. } => {
                    // 附上结果之后目标端只可能开成或拒绝。再要一次就是它的状态机坏了，
                    // 这里必须停：接着推批次只会撞上一个不存在的 run。
                    if asked_already {
                        return Err(OpenFailure::Defect(
                            "目标端收到值域校核结果后仍要求值域校核",
                        ));
                    }
                    request.range_check_results =
                        Some(range_check(&columns).map_err(OpenFailure::RangeCheck)?);
                    asked_already = true;
                }
            }
        }
    }

    fn push_batch(
        &mut self,
        run_id: &str,
        payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError>;
    fn commit(
        &mut self,
        run_id: &str,
        total_batches: u64,
        total_rows: u64,
    ) -> Result<CommitResponse, SinkError>;
    fn get(&mut self, run_id: &str) -> Result<RunResponse, SinkError>;
    fn abort(&mut self, run_id: &str) -> Result<bool, SinkError>;
}

pub struct HttpSinkClient {
    base_url: String,
    agent: ureq::Agent,
}

impl HttpSinkClient {
    pub fn new(base_url: &str) -> Result<Self, String> {
        let url =
            Url::parse(base_url).map_err(|error| format!("invalid sink_base_url: {error}"))?;
        if url.scheme() != "http" {
            return Err("sink_base_url must use http for the M1 local endpoint".to_owned());
        }
        if url.host_str().is_none() {
            return Err("sink_base_url must include a host".to_owned());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("sink_base_url must not contain a query or fragment".to_owned());
        }

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            agent: normal_agent(),
        })
    }

    fn endpoint(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.base_url)
    }

    // Pooled keep-alive is safe for these POSTs despite ADR-0010 §5's retry=0: ureq 2.10.1
    // (unit.rs is_retryable) only ever resends idempotent methods with zero-length bodies,
    // so a stale pooled connection can never replay a POST with a JSON body.
    fn post<T: DeserializeOwned>(
        &self,
        suffix: &str,
        body: Value,
        timeout: Option<Duration>,
    ) -> Result<T, SinkError> {
        let mut request = self
            .agent
            .post(&self.endpoint(suffix))
            .set("Content-Type", "application/json");
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        decode_response(request.send_json(body))
    }
}

impl SinkClient for HttpSinkClient {
    fn open_attempt(&mut self, request: &OpenRunRequest) -> Result<OpenOutcome, SinkError> {
        let response: OpenRunResponse = self.post(
            "/v1/runs",
            serde_json::to_value(request).expect("open request must serialize"),
            None,
        )?;
        Ok(OpenOutcome::from_response(response))
    }

    fn push_batch(
        &mut self,
        run_id: &str,
        payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError> {
        self.post(
            &format!("/v1/runs/{run_id}/batches"),
            serde_json::to_value(payload).expect("batch payload must serialize"),
            None,
        )
    }

    fn commit(
        &mut self,
        run_id: &str,
        total_batches: u64,
        total_rows: u64,
    ) -> Result<CommitResponse, SinkError> {
        self.post(
            &format!("/v1/runs/{run_id}/commit"),
            serde_json::to_value(CommitRequest {
                total_batches,
                total_rows,
            })
            .expect("commit request must serialize"),
            Some(COMMIT_TIMEOUT),
        )
    }

    fn get(&mut self, run_id: &str) -> Result<RunResponse, SinkError> {
        // A fresh agent prevents ureq's stale pooled-connection retry. This call must happen once.
        decode_response(
            normal_agent()
                .get(&self.endpoint(&format!("/v1/runs/{run_id}")))
                .call(),
        )
    }

    fn abort(&mut self, run_id: &str) -> Result<bool, SinkError> {
        let response: AbortResponse = self.post(
            &format!("/v1/runs/{run_id}/abort"),
            json!({}),
            Some(ABORT_TIMEOUT),
        )?;
        if response.run_id != run_id {
            return Err(SinkError::response(
                None,
                "目标端 abort 响应的 run_id 与请求不一致",
            ));
        }
        Ok(response.staging_dropped)
    }
}

fn normal_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(NORMAL_READ_TIMEOUT)
        .redirects(0)
        .build()
}

fn decode_response<T: DeserializeOwned>(
    result: Result<ureq::Response, ureq::Error>,
) -> Result<T, SinkError> {
    match result {
        Ok(response) => response.into_json().map_err(|error| {
            SinkError::response(None, format!("目标端成功响应不是有效 JSON：{error}"))
        }),
        Err(ureq::Error::Status(_, response)) => {
            let error = response
                .into_json::<ErrorEnvelope>()
                .map_err(|error| {
                    SinkError::response(None, format!("目标端错误响应不是有效 JSON：{error}"))
                })?
                .error;
            let column = error
                .details
                .get("column")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let value = error
                .details
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let precheck_issues = error
                .details
                .get("issues")
                .cloned()
                .and_then(|issues| serde_json::from_value(issues).ok())
                .unwrap_or_default();
            let gate = serde_json::from_value(error.details.clone())
                .ok()
                .map(Box::new);
            Err(SinkError {
                kind: SinkErrorKind::Response,
                code: Some(error.code),
                message: error.message,
                column,
                value,
                precheck_issues: Box::new(precheck_issues),
                gate,
            })
        }
        Err(ureq::Error::Transport(error)) => Err(SinkError::transport(format!(
            "HTTP 请求失败或连接中断：{error}"
        ))),
    }
}
