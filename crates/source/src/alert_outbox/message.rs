use crate::RunHistory;

pub(super) fn is_alertable(history: &RunHistory) -> bool {
    history.outcome.as_deref() == Some("FAILED")
        && history.unknown_reason.as_deref() != Some("STOPPED_BY_USER")
        && (history.failure_kind.as_deref() != Some("SKIPPED")
            || history.scheduled_refusal_reason.is_some())
}

pub(super) fn safe_explanation(kind: Option<&str>, unknown_reason: Option<&str>) -> &'static str {
    match (kind, unknown_reason) {
        (Some("UNKNOWN"), Some("SERVICE_RESTARTED")) => {
            "服务重启期间运行未留下终态，结局无法确认，请在系统中查看运行详情。"
        }
        (Some("UNKNOWN"), Some("PROCESS_DISAPPEARED")) => {
            "运行进程消失且未留下终态，结局无法确认，请在系统中查看运行详情。"
        }
        (kind, _) => safe_failure_explanation(kind),
    }
}

fn safe_failure_explanation(kind: Option<&str>) -> &'static str {
    match kind {
        Some("CONFIG") => "运行配置未通过检查，请在系统中查看运行详情。",
        Some("ORCHESTRATOR") => "运行未能正常启动，请在系统中查看运行详情。",
        Some("SOURCE_CONNECT") => "源端数据库连接失败，请在系统中查看运行详情。",
        Some("SOURCE_DBLINK") => "源端数据库链路不可用，请在系统中查看运行详情。",
        Some("SOURCE_QUERY") => "源端查询执行失败，请在系统中查看运行详情。",
        Some("SOURCE_VALUE") => "源端数据值无法转换，请在系统中查看运行详情。",
        Some("MAPPING_PRECHECK") => "字段映射检查未通过，请在系统中查看运行详情。",
        Some("NETWORK") => "运行期间网络通信失败，请在系统中查看运行详情。",
        Some("SINK_WRITE") => "目标端写入失败，请在系统中查看运行详情。",
        Some("DATA_REJECTED") => "目标端拒绝了部分数据，请在系统中查看运行详情。",
        Some("SINK_ENVIRONMENT") => "目标端环境不满足运行要求，请在系统中查看运行详情。",
        Some("TARGET_BUSY") => "目标端当前忙碌，请在系统中查看运行详情。",
        Some("VERIFY_FAILED") => "写入后的校验未通过，请在系统中查看运行详情。",
        Some("DEFECT") => "运行遇到内部一致性错误，请在系统中查看运行详情。",
        Some("UNKNOWN") => "运行结局无法确认，请在系统中查看运行详情。",
        Some("PREVIOUS_RUN_ACTIVE") => "调度触发时上一次运行仍未结束，请在系统中查看运行详情。",
        Some("PREVIOUS_RUN_STOPPING") => "调度触发时上一次运行仍在停止，请在系统中查看运行详情。",
        Some("SOURCE_DATASOURCE_UNAVAILABLE") => {
            "调度触发时源端数据源不可用，请在系统中查看运行详情。"
        }
        Some("TARGET_DATASOURCE_UNAVAILABLE") => {
            "调度触发时目标端数据源不可用，请在系统中查看运行详情。"
        }
        Some("TARGET_AGENT_UNAVAILABLE") => "调度触发时目标端代理不可用，请在系统中查看运行详情。",
        Some("TARGET_HELD") => "调度触发时目标表仍被占用，请在系统中查看运行详情。",
        _ => "运行失败，请在系统中查看运行详情。",
    }
}

pub(super) struct PendingDelivery {
    pub delivery_id: String,
    pub recipient: String,
    pub alert_id: String,
    pub run_record_id: String,
    pub run_id: Option<String>,
    pub task_id: String,
    pub task_name: String,
    pub trigger: String,
    pub failed_at: String,
    pub failure_category: String,
    pub safe_explanation: String,
}

pub(super) fn render_message(
    delivery: &PendingDelivery,
    instance_name: &str,
    base_url: Option<&str>,
) -> (String, String, String) {
    let subject = format!("[db-qbs][{instance_name}][告警] {}", delivery.task_name);
    let run_id = delivery.run_id.as_deref().unwrap_or("未分配");
    let link = base_url.map(|base| format!("{base}/#runs/{}", delivery.run_record_id));
    let link_plain = link
        .as_deref()
        .map(|value| format!("\n运行详情：{value}"))
        .unwrap_or_default();
    let link_html = link
        .as_deref()
        .map(|value| format!("<p><a href=\"{}\">打开运行详情</a></p>", html_escape(value)))
        .unwrap_or_default();
    let plain = format!(
        "db-qbs 运行失败告警\n告警 ID：{}\n任务：{}（{}）\n运行记录 ID：{}\n目标端运行 ID：{}\n触发方式：{}\n失败时间：{}\n失败分类：{}\n说明：{}{}",
        delivery.alert_id, delivery.task_name, delivery.task_id, delivery.run_record_id,
        run_id, delivery.trigger, delivery.failed_at, delivery.failure_category,
        delivery.safe_explanation, link_plain
    );
    let html = format!(
        "<!doctype html><html><body><h1>db-qbs 运行失败告警</h1><dl><dt>告警 ID</dt><dd>{}</dd><dt>任务</dt><dd>{}（{}）</dd><dt>运行记录 ID</dt><dd>{}</dd><dt>目标端运行 ID</dt><dd>{}</dd><dt>触发方式</dt><dd>{}</dd><dt>失败时间</dt><dd>{}</dd><dt>失败分类</dt><dd>{}</dd><dt>说明</dt><dd>{}</dd></dl>{}</body></html>",
        html_escape(&delivery.alert_id), html_escape(&delivery.task_name), html_escape(&delivery.task_id),
        html_escape(&delivery.run_record_id), html_escape(run_id), html_escape(&delivery.trigger),
        html_escape(&delivery.failed_at), html_escape(&delivery.failure_category),
        html_escape(&delivery.safe_explanation), link_html
    );
    (subject, plain, html)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
