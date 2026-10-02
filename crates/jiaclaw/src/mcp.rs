// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Reviewed, read-only `StateKnot` Streamable HTTP MCP bindings.

use crate::{Tool, ToolRegistry};
use async_trait::async_trait;
use jiaclaw_core::{JiaClawError, McpConfig, McpServerConfig};
use serde_json::{json, Value};
use stateknot_integrations::{
    mcp_tool_descriptor_digest, AnonymousMcpAuthorization, ApiKey, McpClient,
    McpClientAuthorizationProvider, McpClientIdentity, McpClientOptions, McpTool, McpToolCall,
    ProviderEndpoint, ProviderHttpOptions, StaticMcpBearerAuthorization,
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_SCHEMA_BYTES: usize = 32 * 1024;

fn configuration(message: &str) -> JiaClawError {
    JiaClawError::Configuration(format!("MCP: {message}"))
}

fn execution(message: &str) -> JiaClawError {
    JiaClawError::ToolExecution(format!("MCP: {message}"))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 24
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn endpoint(value: &str) -> Result<ProviderEndpoint, JiaClawError> {
    if value.len() > 2048 {
        return Err(configuration("endpoint is too long"));
    }
    let parsed = if value.starts_with("https://") {
        ProviderEndpoint::https(value)
    } else {
        ProviderEndpoint::loopback_http(value)
    };
    // Do not echo an invalid URL: it may contain an embedded credential.
    parsed.map_err(|_| {
        configuration(
            "endpoint requires HTTPS or literal-loopback HTTP, without credentials/query/fragment",
        )
    })
}

fn validate_server(server: &McpServerConfig) -> Result<(), JiaClawError> {
    if !identifier(&server.name)
        || !(1..=120).contains(&server.timeout_secs)
        || !(1024..=1024 * 1024).contains(&server.max_response_bytes)
        || !(1..=16).contains(&server.max_concurrent_calls)
        || server.tools.is_empty()
        || server.tools.len() > 32
    {
        return Err(configuration(
            "invalid server name, limits or empty/oversized tool allowlist",
        ));
    }
    endpoint(&server.endpoint)?;
    let mut aliases = HashSet::new();
    let mut names = HashSet::new();
    for tool in &server.tools {
        if !identifier(&tool.alias)
            || !aliases.insert(&tool.alias)
            || tool.name.is_empty()
            || tool.name.len() > 128
            || tool.name.chars().any(char::is_control)
            || !names.insert(&tool.name)
            || tool.descriptor_sha256.len() != 71
            || !tool.descriptor_sha256.starts_with("sha256:")
            || !tool.descriptor_sha256[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(configuration(
                "invalid/duplicate tool name, alias or reviewed descriptor pin",
            ));
        }
    }
    Ok(())
}

fn authorization(
    variable: Option<&str>,
) -> Result<Arc<dyn McpClientAuthorizationProvider>, JiaClawError> {
    match variable {
        None => Ok(Arc::new(AnonymousMcpAuthorization)),
        Some(name) => {
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                || !name.as_bytes()[0].is_ascii_uppercase()
            {
                return Err(configuration(
                    "invalid bearer token environment variable name",
                ));
            }
            let token = std::env::var(name).map_err(|_| {
                configuration("required MCP bearer token environment variable is missing")
            })?;
            let key = ApiKey::new(token).map_err(|_| configuration("invalid MCP bearer token"))?;
            Ok(Arc::new(StaticMcpBearerAuthorization::new(key)))
        }
    }
}

async fn connect(server: &McpServerConfig) -> Result<McpClient, JiaClawError> {
    let transport = ProviderHttpOptions::new(
        Duration::from_secs(server.timeout_secs.min(10)),
        Duration::from_secs(30),
        MAX_ARGUMENT_BYTES + 4096,
        server.max_response_bytes,
        server.max_response_bytes,
        server.max_response_bytes,
        server.max_response_bytes,
    )
    .map_err(|_| configuration("invalid transport limits"))?;
    let options = McpClientOptions::new(
        transport,
        Duration::from_secs(server.timeout_secs),
        server.max_concurrent_calls,
        4,
        128,
        32,
    )
    .and_then(|o| o.with_maximum_authorization_retries(0))
    .map_err(|_| configuration("invalid client limits"))?;
    let identity = McpClientIdentity::new("JiaClaw", env!("CARGO_PKG_VERSION"))
        .map_err(|_| configuration("invalid client identity"))?;
    McpClient::connect(
        endpoint(&server.endpoint)?,
        identity,
        authorization(server.bearer_token_env.as_deref())?,
        options,
    )
    .await
    .map_err(|_| configuration("server discovery failed (transport, authentication or protocol)"))
}

fn bounded_tree(value: &Value, max_bytes: usize) -> bool {
    fn walk(value: &Value, depth: usize, remaining: &mut usize) -> bool {
        if depth > 24 || *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        match value {
            Value::Array(v) => v.iter().all(|v| walk(v, depth + 1, remaining)),
            Value::Object(v) => v.values().all(|v| walk(v, depth + 1, remaining)),
            _ => true,
        }
    }
    walk(value, 0, &mut 2048) && serde_json::to_vec(value).is_ok_and(|b| b.len() <= max_bytes)
}

fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, JiaClawError> {
    if !bounded_tree(schema, MAX_SCHEMA_BYTES) || !schema.is_object() {
        return Err(configuration("invalid or over-limit tool schema"));
    }
    // The offline retriever rejects both filesystem and network references.
    jsonschema::options()
        .should_validate_formats(true)
        .with_pattern_options(
            jsonschema::PatternOptions::fancy_regex()
                .backtrack_limit(10_000)
                .size_limit(256 * 1024)
                .dfa_size_limit(256 * 1024),
        )
        .offline()
        .build(schema)
        .map_err(|_| {
            configuration("tool schema compilation failed; external references are disabled")
        })
}

/// Build all bindings before atomically advertising any of them.
pub(crate) async fn initialize(
    config: &McpConfig,
    registry: &mut ToolRegistry,
) -> Result<(), JiaClawError> {
    if config.servers.len() > 8 {
        return Err(configuration("at most eight servers are allowed"));
    }
    let mut names = HashSet::new();
    // Validate the entire local policy before making the first network request.
    for server in &config.servers {
        validate_server(server)?;
        authorization(server.bearer_token_env.as_deref())?;
        if !names.insert(&server.name) {
            return Err(configuration("duplicate server name"));
        }
        for tool in &server.tools {
            if registry
                .get(&format!("mcp_{}_{}", server.name, tool.alias))
                .is_some()
            {
                return Err(configuration("local tool name collision"));
            }
        }
    }
    let mut pending: Vec<Box<dyn Tool>> = Vec::new();
    for server in &config.servers {
        let tools = tokio::time::timeout(Duration::from_secs(server.timeout_secs), async {
            let client = connect(server).await?;
            let catalog = client
                .list_tools()
                .await
                .map_err(|_| configuration("tool catalog discovery failed"))?;
            let concurrency = Arc::new(Semaphore::new(server.max_concurrent_calls));
            let mut tools: Vec<Box<dyn Tool>> = Vec::new();
            for policy in &server.tools {
                let tool = catalog
                    .find(&policy.name)
                    .ok_or_else(|| configuration("an approved tool is missing or was rejected"))?;
                let digest = mcp_tool_descriptor_digest(tool.raw())
                    .map_err(|_| configuration("invalid tool descriptor"))?;
                if digest.to_string() != policy.descriptor_sha256 {
                    return Err(configuration("reviewed tool descriptor has changed"));
                }
                let input = compile_schema(tool.input_schema())?;
                let output = tool.output_schema().map(compile_schema).transpose()?;
                let description = tool.description().unwrap_or("Reviewed read-only MCP tool");
                if description.len() > 4096 {
                    return Err(configuration("tool description exceeds limit"));
                }
                tools.push(Box::new(RemoteTool {
                    name: format!("mcp_{}_{}", server.name, policy.alias),
                    description: description.to_owned(),
                    client: client.clone(),
                    tool: tool.clone(),
                    input,
                    output,
                    concurrency: concurrency.clone(),
                    deadline: Duration::from_secs(server.timeout_secs),
                    maximum_result_bytes: server.max_response_bytes,
                }));
            }
            Ok::<_, JiaClawError>(tools)
        })
        .await
        .map_err(|_| configuration("server initialization deadline exceeded"))??;
        pending.extend(tools);
    }
    for tool in pending {
        registry.register(tool);
    }
    Ok(())
}

/// Retrieve review material and pins, without registering or invoking any tools.
/// The operator must independently review the descriptors before approving them.
///
/// # Errors
/// Returns an error on invalid endpoint, missing credentials or bounded discovery failure.
pub async fn inspect_mcp_server(
    url: &str,
    bearer_token_env: Option<String>,
) -> Result<Value, JiaClawError> {
    let server = McpServerConfig {
        name: "inspect".into(),
        endpoint: url.into(),
        bearer_token_env,
        timeout_secs: 30,
        max_response_bytes: 256 * 1024,
        max_concurrent_calls: 1,
        tools: Vec::new(),
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        let client = connect(&server).await?;
        let catalog = client
            .list_tools()
            .await
            .map_err(|_| configuration("tool catalog discovery failed"))?;
        let mut tools = Vec::new();
        for tool in catalog.tools() {
            let digest = mcp_tool_descriptor_digest(tool.raw())
                .map_err(|_| configuration("invalid tool descriptor"))?;
            tools.push(json!({"descriptor_sha256": digest.to_string(), "descriptor": tool.raw()}));
        }
        Ok(json!({"protocol": "2026-07-28", "tools": tools,
            "rejected_tools": catalog.rejected_tools().len()}))
    })
    .await
    .map_err(|_| configuration("inspection deadline exceeded"))?
}

struct RemoteTool {
    name: String,
    description: String,
    client: McpClient,
    tool: McpTool,
    input: jsonschema::Validator,
    output: Option<jsonschema::Validator>,
    concurrency: Arc<Semaphore>,
    deadline: Duration,
    maximum_result_bytes: usize,
}

#[async_trait]
impl Tool for RemoteTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters_schema(&self) -> Value {
        self.tool.input_schema().clone()
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| execution("server is busy; request was not submitted"))?;
        if !args.is_object()
            || !bounded_tree(&args, MAX_ARGUMENT_BYTES)
            || !self.input.is_valid(&args)
        {
            return Err(execution(
                "arguments do not match the approved schema or size/depth limits",
            ));
        }
        // Exactly one HTTP submission. No protocol/auth/transport replay or automatic MRTR resume.
        // Cancellation drops the HTTP future; it cannot undo a request already received remotely.
        let response =
            tokio::time::timeout(self.deadline, self.client.call_tool_once(&self.tool, args))
                .await
                .map_err(|_| execution("call deadline exceeded; remote outcome is unknown"))?
                .map_err(|_| {
                    execution(
                        "call failed (transport, authentication or protocol); no automatic retry",
                    )
                })?;
        let result = match response.into_outcome() {
            McpToolCall::Complete(result) => result,
            McpToolCall::InputRequired(_) => {
                return Err(execution(
                    "interactive input is required; no input was approved or submitted",
                ))
            }
            _ => return Err(execution("unsupported tool outcome")),
        };
        if result.is_error() {
            return Err(execution("remote tool reported an execution error"));
        }
        if result
            .content()
            .iter()
            .any(|block| block["type"] != "text" || !block["text"].is_string())
        {
            return Err(execution(
                "only text content is supported; no resource was fetched",
            ));
        }
        if let Some(schema) = &self.output {
            if result.structured_content().is_none_or(|value| {
                !bounded_tree(value, self.maximum_result_bytes) || !schema.is_valid(value)
            }) {
                return Err(execution(
                    "structured output does not match the approved schema",
                ));
            }
        }
        let encoded = serde_json::to_string(&json!({
            "content": result.content(), "structuredContent": result.structured_content(),
        }))
        .map_err(|_| execution("could not encode tool result"))?;
        if encoded.len() > self.maximum_result_bytes {
            return Err(execution("encoded result exceeds limit"));
        }
        Ok(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentConfig, JiaClawAgent};
    use jiaclaw_core::{McpToolConfig, McpToolEffect};
    use std::sync::Mutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    #[derive(Clone, Copy)]
    enum Mode {
        Json,
        Sse,
        Error,
        BadId,
        Oversize,
        Image,
        BadOutput,
        InputRequired,
        Delay,
        Unauthorized,
        Redirect,
    }

    struct Fixture {
        url: String,
        descriptor: Value,
        requests: Arc<Mutex<Vec<(Value, String)>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Fixture {
        async fn start(mode: Mode, external_schema: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/mcp/", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let mut descriptor = json!({
                "name": "lookup", "description": "Look up a reviewed inventory record.",
                "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "maxLength": 100}},
                    "required": ["query"], "additionalProperties": false},
                "outputSchema": {"type": "object", "properties": {"found": {"type": "boolean"}},
                    "required": ["found"]}
            });
            if external_schema {
                descriptor["inputSchema"] = json!({"$ref": "file:///etc/passwd"});
            }
            let tool = descriptor.clone();
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let header_end;
                    let length;
                    loop {
                        let mut chunk = [0_u8; 4096];
                        let count = socket.read(&mut chunk).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&chunk[..count]);
                        assert!(bytes.len() < 128 * 1024);
                        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                            header_end = end + 4;
                            let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                            length = headers
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length: "))
                                .unwrap()
                                .parse::<usize>()
                                .unwrap();
                            break;
                        }
                    }
                    while bytes.len() < header_end + length {
                        let mut chunk = [0_u8; 4096];
                        let count = socket.read(&mut chunk).await.unwrap();
                        assert!(count > 0);
                        bytes.extend_from_slice(&chunk[..count]);
                    }
                    let request: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    recorded.lock().unwrap().push((
                        request.clone(),
                        String::from_utf8_lossy(&bytes[..header_end]).into_owned(),
                    ));
                    let is_call = request["method"] == "tools/call";
                    if is_call && matches!(mode, Mode::Delay) {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    if is_call && matches!(mode, Mode::Unauthorized | Mode::Redirect) {
                        let status = if matches!(mode, Mode::Unauthorized) {
                            "401 Unauthorized"
                        } else {
                            "307 Temporary Redirect"
                        };
                        let response = format!("HTTP/1.1 {status}\r\nLocation: /other\r\nContent-Length: 15\r\nConnection: close\r\n\r\nSECRET-DONT-LOG");
                        let _ = socket.write_all(response.as_bytes()).await;
                        continue;
                    }
                    let result = match request["method"].as_str().unwrap() {
                        "server/discover" => {
                            json!({"resultType": "complete", "supportedVersions": ["2026-07-28"],
                            "capabilities": {"tools": {}}, "ttlMs": 0, "cacheScope": "private"})
                        }
                        "tools/list" => json!({"resultType": "complete", "tools": [tool.clone(), {
                            "name": "unapproved_write", "description": "Must never be exposed.",
                            "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}
                        }]}),
                        "tools/call" if matches!(mode, Mode::InputRequired) => json!({
                            "resultType": "input_required", "requestState": "SECRET-DONT-LOG",
                            "inputRequests": {"ask": {"method": "elicitation/create", "params": {
                                "mode": "form", "message": "Approve me", "requestedSchema": {"type": "object", "properties": {}}
                            }}}
                        }),
                        "tools/call" => {
                            let text = if matches!(mode, Mode::Oversize) {
                                "x".repeat(32 * 1024)
                            } else {
                                "record found".into()
                            };
                            let content = if matches!(mode, Mode::Image) {
                                json!([{"type": "image", "data": "AAAA", "mimeType": "image/png"}])
                            } else {
                                json!([{"type": "text", "text": text}])
                            };
                            json!({"resultType": "complete", "content": content,
                                "structuredContent": {"found": if matches!(mode, Mode::BadOutput) { json!("invalid") } else { json!(true) }},
                                "isError": matches!(mode, Mode::Error)})
                        }
                        other => panic!("unexpected method {other}"),
                    };
                    let id = if is_call && matches!(mode, Mode::BadId) {
                        json!("wrong-id")
                    } else {
                        request["id"].clone()
                    };
                    let response = json!({"jsonrpc": "2.0", "id": id, "result": result});
                    let body = if is_call && matches!(mode, Mode::Sse) {
                        format!(
                            "data: {}\r\n\r\ndata: {}\r\n\r\n",
                            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}}),
                            response
                        )
                    } else {
                        response.to_string()
                    };
                    let content_type = if is_call && matches!(mode, Mode::Sse) {
                        "text/event-stream"
                    } else {
                        "application/json"
                    };
                    let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    if socket.write_all(headers.as_bytes()).await.is_ok() {
                        for chunk in body.as_bytes().chunks(17) {
                            if socket.write_all(chunk).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            Self {
                url,
                descriptor,
                requests,
                task,
            }
        }

        fn config(&self) -> McpConfig {
            McpConfig {
                servers: vec![McpServerConfig {
                    name: "inventory".into(),
                    endpoint: self.url.clone(),
                    bearer_token_env: None,
                    timeout_secs: 5,
                    max_response_bytes: 1024,
                    max_concurrent_calls: 1,
                    tools: vec![McpToolConfig {
                        name: "lookup".into(),
                        alias: "lookup".into(),
                        descriptor_sha256: mcp_tool_descriptor_digest(&self.descriptor)
                            .unwrap()
                            .to_string(),
                        effect: McpToolEffect::ReadOnly,
                    }],
                }],
            }
        }

        fn call_count(&self) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(r, _)| r["method"] == "tools/call")
                .count()
        }

        async fn wait_for_call(&self) {
            tokio::time::timeout(Duration::from_secs(2), async {
                while self.call_count() == 0 {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn json_and_fragmented_sse_use_real_stateknot_transport() {
        for mode in [Mode::Json, Mode::Sse] {
            let fixture = Fixture::start(mode, false).await;
            let mut registry = ToolRegistry::new();
            initialize(&fixture.config(), &mut registry).await.unwrap();
            assert_eq!(registry.list(), ["mcp_inventory_lookup"]);
            let result = registry
                .get("mcp_inventory_lookup")
                .unwrap()
                .execute(json!({"query": "abc"}))
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&result).unwrap()["structuredContent"]["found"],
                true
            );
            assert_eq!(fixture.call_count(), 1);
            let requests = fixture.requests.lock().unwrap();
            assert_eq!(
                requests
                    .iter()
                    .map(|(r, _)| r["method"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["server/discover", "tools/list", "tools/call"]
            );
            assert_eq!(requests[2].0["params"]["name"], "lookup");
            assert!(!requests[2].1.to_lowercase().contains("authorization:"));
        }
    }

    #[tokio::test]
    async fn startup_rejects_drift_missing_tools_and_external_schema() {
        for scenario in 0..3 {
            let fixture = Fixture::start(Mode::Json, scenario == 2).await;
            let mut policy = fixture.config();
            if scenario == 0 {
                policy.servers[0].tools[0].descriptor_sha256 = format!("sha256:{}", "0".repeat(64));
            }
            if scenario == 1 {
                policy.servers[0].tools[0].name = "missing".into();
            }
            let mut registry = ToolRegistry::new();
            assert!(initialize(&policy, &mut registry).await.is_err());
            assert!(registry.list().is_empty());
            assert_eq!(fixture.call_count(), 0);
        }
    }

    #[tokio::test]
    async fn arguments_are_checked_before_network_submission() {
        let fixture = Fixture::start(Mode::Json, false).await;
        let mut registry = ToolRegistry::new();
        initialize(&fixture.config(), &mut registry).await.unwrap();
        for args in [
            json!({}),
            json!({"query": 10}),
            json!({"query": "abc", "extra": true}),
            json!({"query": "x".repeat(32 * 1024)}),
            json!([]),
        ] {
            assert!(registry
                .get("mcp_inventory_lookup")
                .unwrap()
                .execute(args)
                .await
                .is_err());
        }
        assert_eq!(fixture.call_count(), 0);
    }

    #[tokio::test]
    async fn remote_errors_and_unsupported_results_fail_without_replay() {
        for mode in [
            Mode::Error,
            Mode::BadId,
            Mode::Oversize,
            Mode::Image,
            Mode::BadOutput,
            Mode::InputRequired,
            Mode::Unauthorized,
            Mode::Redirect,
        ] {
            let fixture = Fixture::start(mode, false).await;
            let mut registry = ToolRegistry::new();
            initialize(&fixture.config(), &mut registry).await.unwrap();
            let error = registry
                .get("mcp_inventory_lookup")
                .unwrap()
                .execute(json!({"query": "abc"}))
                .await
                .unwrap_err();
            assert!(!error.to_string().contains("SECRET-DONT-LOG"));
            if matches!(mode, Mode::InputRequired) {
                assert!(error.to_string().contains("interactive input"));
            }
            assert_eq!(fixture.call_count(), 1);
        }
    }

    #[tokio::test]
    async fn deadline_is_finite_and_not_retried() {
        let fixture = Fixture::start(Mode::Delay, false).await;
        let mut config = fixture.config();
        config.servers[0].timeout_secs = 1;
        let mut registry = ToolRegistry::new();
        initialize(&config, &mut registry).await.unwrap();
        let start = std::time::Instant::now();
        assert!(registry
            .get("mcp_inventory_lookup")
            .unwrap()
            .execute(json!({"query": "abc"}))
            .await
            .is_err());
        assert!(start.elapsed() < Duration::from_millis(1800));
        assert_eq!(fixture.call_count(), 1);
    }

    #[tokio::test]
    async fn busy_calls_are_not_submitted_and_cancellation_releases_capacity() {
        let fixture = Fixture::start(Mode::Delay, false).await;
        let mut registry = ToolRegistry::new();
        initialize(&fixture.config(), &mut registry).await.unwrap();
        let registry = Arc::new(registry);
        let running = registry.clone();
        let task = tokio::spawn(async move {
            running
                .get("mcp_inventory_lookup")
                .unwrap()
                .execute(json!({"query": "abc"}))
                .await
        });
        fixture.wait_for_call().await;
        let busy = registry
            .get("mcp_inventory_lookup")
            .unwrap()
            .execute(json!({"query": "abc"}))
            .await
            .unwrap_err();
        assert!(busy.to_string().contains("not submitted"));
        assert_eq!(fixture.call_count(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let invalid = registry
            .get("mcp_inventory_lookup")
            .unwrap()
            .execute(json!({}))
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("arguments"));
        assert_eq!(fixture.call_count(), 1);
    }

    #[tokio::test]
    async fn all_server_bindings_are_published_atomically() {
        let fixture = Fixture::start(Mode::Json, false).await;
        let mut config = fixture.config();
        let mut second = config.servers[0].clone();
        second.name = "other".into();
        second.tools[0].descriptor_sha256 = format!("sha256:{}", "0".repeat(64));
        config.servers.push(second);
        let mut registry = ToolRegistry::new();
        assert!(initialize(&config, &mut registry).await.is_err());
        assert!(registry.list().is_empty());
        assert_eq!(fixture.call_count(), 0);
    }

    #[tokio::test]
    async fn invalid_local_policy_never_contacts_a_server() {
        let fixture = Fixture::start(Mode::Json, false).await;
        for scenario in 0..6 {
            let mut config = fixture.config();
            match scenario {
                0 => config.servers[0].endpoint = "https://user:secret@example.com/mcp".into(),
                1 => config.servers[0].timeout_secs = 0,
                2 => config.servers.push(config.servers[0].clone()),
                3 => config.servers[0].tools.clear(),
                4 => config.servers[0].tools[0].alias = "bad-alias".into(),
                _ => config.servers[0].bearer_token_env = Some("INVALID-MISSING".into()),
            }
            assert!(initialize(&config, &mut ToolRegistry::new()).await.is_err());
        }
        assert!(fixture.requests.lock().unwrap().is_empty());
        let config = AgentConfig {
            mcp: fixture.config(),
            ..AgentConfig::default()
        };
        assert!(JiaClawAgent::new(config).is_err());
    }

    #[tokio::test]
    async fn inspection_exports_review_material_without_invoking_tools() {
        let fixture = Fixture::start(Mode::Json, false).await;
        let review = inspect_mcp_server(&fixture.url, None).await.unwrap();
        assert_eq!(review["tools"].as_array().unwrap().len(), 2);
        assert_eq!(review["tools"][0]["descriptor"], fixture.descriptor);
        assert_eq!(
            review["tools"][0]["descriptor_sha256"],
            mcp_tool_descriptor_digest(&fixture.descriptor)
                .unwrap()
                .to_string()
        );
        assert_eq!(fixture.call_count(), 0);
    }

    #[test]
    fn pathological_pattern_is_bounded() {
        let validator =
            compile_schema(&json!({"type": "string", "pattern": "^(a+)+(?=b)b$"})).unwrap();
        let started = std::time::Instant::now();
        assert!(!validator.is_valid(&json!(format!("{}c", "a".repeat(1024)))));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
