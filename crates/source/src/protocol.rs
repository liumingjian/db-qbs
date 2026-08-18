use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

// 报文形状的唯一定义在 `db-qbs-shared`（#124）。本文件只留 source 侧自己的
// 客户端错误模型（`SinkError` 一族）与 HTTP 客户端实现。
use db_qbs_shared::{
    AbortResponse, BatchPayload, BatchResponse, CommitRequest, CommitResponse, ErrorEnvelope,
    OpenRunRequest, OpenRunResponse, PrecheckIssue, RunResponse,
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

pub trait SinkClient {
    fn open(&mut self, request: &OpenRunRequest) -> Result<OpenRunResponse, SinkError>;
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
    fn open(&mut self, request: &OpenRunRequest) -> Result<OpenRunResponse, SinkError> {
        self.post(
            "/v1/runs",
            serde_json::to_value(request).expect("open request must serialize"),
            None,
        )
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
