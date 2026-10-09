// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 进程内 Prometheus 文本指标（不引入 telemetry SDK）。

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, MutexGuard,
    },
};

/// Prometheus 0.0.4 exposition `Content-Type`。
pub(crate) const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

type HttpCounters = BTreeMap<(String, String, u16), AtomicU64>;
type ToolCounters = BTreeMap<(String, String), AtomicU64>;

/// 进程内计数器。HTTP 与工具调用用 `AtomicU64`；会话数在 scrape 时读取。
#[derive(Debug, Default)]
pub(crate) struct Metrics {
    http: Mutex<HttpCounters>,
    tools: Mutex<ToolCounters>,
}

impl Metrics {
    fn lock_http(&self) -> MutexGuard<'_, HttpCounters> {
        self.http
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_tools(&self) -> MutexGuard<'_, ToolCounters> {
        self.tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 记录一次 HTTP 请求（按路由族 / 方法 / 状态码）。
    pub(crate) fn record_http(&self, family: &str, method: &str, status: u16) {
        let mut map = self.lock_http();
        map.entry((family.to_string(), method.to_string(), status))
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次工具执行结果（`ok` / `error`）。
    pub(crate) fn record_tool_call(&self, tool: &str, ok: bool) {
        let result = if ok { "ok" } else { "error" };
        let mut map = self.lock_tools();
        map.entry((tool.to_string(), result.to_string()))
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 渲染 Prometheus 文本。`/metrics` 自身的计数在中间件返回后才增加，不包含当次 scrape。
    pub(crate) fn render(&self, sessions_active: u64, version: &str) -> String {
        let mut out = String::new();
        out.push_str("# HELP jiaclaw_http_requests_total HTTP requests by route family, method, and status.\n");
        out.push_str("# TYPE jiaclaw_http_requests_total counter\n");
        {
            let http = self.lock_http();
            for ((family, method, status), counter) in http.iter() {
                let value = counter.load(Ordering::Relaxed);
                out.push_str(&format!(
                    "jiaclaw_http_requests_total{{path=\"{}\",method=\"{}\",status=\"{status}\"}} {value}\n",
                    escape_prom_label(family),
                    escape_prom_label(method),
                ));
            }
        }

        out.push_str("# HELP jiaclaw_sessions_active In-memory session map size.\n");
        out.push_str("# TYPE jiaclaw_sessions_active gauge\n");
        out.push_str(&format!("jiaclaw_sessions_active {sessions_active}\n"));

        out.push_str("# HELP jiaclaw_tool_calls_total Tool executions by name and result.\n");
        out.push_str("# TYPE jiaclaw_tool_calls_total counter\n");
        {
            let tools = self.lock_tools();
            for ((tool, result), counter) in tools.iter() {
                let value = counter.load(Ordering::Relaxed);
                out.push_str(&format!(
                    "jiaclaw_tool_calls_total{{tool=\"{}\",result=\"{}\"}} {value}\n",
                    escape_prom_label(tool),
                    escape_prom_label(result),
                ));
            }
        }

        out.push_str("# HELP jiaclaw_build_info Build information.\n");
        out.push_str("# TYPE jiaclaw_build_info gauge\n");
        out.push_str(&format!(
            "jiaclaw_build_info{{version=\"{}\"}} 1\n",
            escape_prom_label(version)
        ));
        out
    }
}

/// 将路径折叠为低基数路由族，避免按原始 path 爆炸。
#[must_use]
pub(crate) fn classify_http_path(path: &str) -> &'static str {
    match path {
        "/api/chat" => "api_chat",
        path if path.starts_with("/api/turns/") => "api_turns",
        "/hooks/telegram" => "hooks_telegram",
        "/hooks/slack" => "hooks_slack",
        "/hooks/discord" => "hooks_discord",
        "/hooks/inbound" => "hooks_inbound",
        "/health" => "health",
        "/metrics" => "metrics",
        _ => "other",
    }
}

fn escape_prom_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{classify_http_path, escape_prom_label, Metrics};

    #[test]
    fn classify_http_path_uses_route_families() {
        assert_eq!(classify_http_path("/api/chat"), "api_chat");
        assert_eq!(classify_http_path("/hooks/telegram"), "hooks_telegram");
        assert_eq!(classify_http_path("/hooks/slack"), "hooks_slack");
        assert_eq!(classify_http_path("/hooks/discord"), "hooks_discord");
        assert_eq!(classify_http_path("/hooks/inbound"), "hooks_inbound");
        assert_eq!(classify_http_path("/health"), "health");
        assert_eq!(classify_http_path("/metrics"), "metrics");
        assert_eq!(classify_http_path("/api/tools"), "other");
        assert_eq!(classify_http_path("/api/sessions"), "other");
        assert_eq!(classify_http_path("/unknown"), "other");
    }

    #[test]
    fn escape_prom_label_escapes_special_chars() {
        assert_eq!(escape_prom_label(r#"a\b"c"#), r#"a\\b\"c"#);
        assert_eq!(escape_prom_label("line\nbreak"), r"line\nbreak");
    }

    #[test]
    fn render_includes_counters_gauge_and_build_info() {
        let metrics = Metrics::default();
        metrics.record_http("health", "GET", 200);
        metrics.record_http("health", "GET", 200);
        metrics.record_http("other", "POST", 200);
        metrics.record_tool_call("workspace_list", true);
        metrics.record_tool_call("http_get", false);

        let body = metrics.render(3, "0.1.0");
        assert!(body.contains(
            "jiaclaw_http_requests_total{path=\"health\",method=\"GET\",status=\"200\"} 2"
        ));
        assert!(body.contains(
            "jiaclaw_http_requests_total{path=\"other\",method=\"POST\",status=\"200\"} 1"
        ));
        assert!(body.contains("jiaclaw_sessions_active 3"));
        assert!(body.contains("jiaclaw_tool_calls_total{tool=\"workspace_list\",result=\"ok\"} 1"));
        assert!(body.contains("jiaclaw_tool_calls_total{tool=\"http_get\",result=\"error\"} 1"));
        assert!(body.contains("jiaclaw_build_info{version=\"0.1.0\"} 1"));
        assert!(body.contains("# TYPE jiaclaw_http_requests_total counter"));
        assert!(body.contains("# TYPE jiaclaw_sessions_active gauge"));
        assert!(body.ends_with('\n'));
    }
}
