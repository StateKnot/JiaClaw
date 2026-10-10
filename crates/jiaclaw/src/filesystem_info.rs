// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Read-only workspace metadata and bounded directory trees.

use crate::{memory::canonicalize_existing_or_clone, tools::Tool};
use async_trait::async_trait;
use jiaclaw_core::JiaClawError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Metadata for a workspace entry. Links and unsupported entries expose no target details.
#[derive(Debug, Serialize)]
pub struct StatOutput {
    /// Normalized workspace-relative path; `.` denotes the root.
    pub path: String,
    /// `file`, `dir`, `symlink`, or `unsupported`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Size only for an ordinary, single-link file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Modification time for ordinary files/directories, in signed Unix milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<i64>,
}

/// Flat tree entries in per-directory name-sorted depth-first order.
#[derive(Debug, Serialize)]
pub struct TreeOutput {
    /// Normalized workspace-relative directory path.
    pub path: String,
    /// Maximum entry depth; direct children have depth one.
    pub max_depth: usize,
    /// Maximum number of returned entries.
    pub max_entries: usize,
    /// True whenever a resource or depth boundary prevented a complete scan.
    pub truncated: bool,
    /// Paths relative to the selected directory. Links are never followed.
    pub entries: Vec<crate::files::DirEntryInfo>,
}

fn default_path() -> String {
    ".".into()
}
fn default_depth() -> usize {
    3
}
fn default_entries() -> usize {
    200
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatArgs {
    #[serde(default = "default_path")]
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeArgs {
    #[serde(default = "default_path")]
    path: String,
    #[serde(default = "default_depth")]
    max_depth: usize,
    #[serde(default = "default_entries")]
    max_entries: usize,
}

fn parse_error(e: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("文件信息工具参数无效: {e}"))
}

fn parse_args<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, JiaClawError> {
    if !args.is_object() {
        return Err(parse_error("参数必须是 JSON object"));
    }
    serde_json::from_value(args).map_err(parse_error)
}

fn validate_tree(args: &TreeArgs) -> Result<(), JiaClawError> {
    crate::memory_io::info_relative(&args.path)?;
    if !(1..=32).contains(&args.max_depth) || !(1..=1000).contains(&args.max_entries) {
        return Err(parse_error(
            "max_depth 必须为 1..=32；max_entries 必须为 1..=1000",
        ));
    }
    Ok(())
}

fn path_schema() -> Value {
    json!({"type":"string", "minLength":1, "maxLength":1024, "default":".",
        "description":"工作区相对路径，默认 .；非空白，最多 1024 UTF-8 字节和 64 个组件，禁止绝对路径及 .."})
}

fn encode(output: &impl Serialize) -> Result<String, JiaClawError> {
    let json = serde_json::to_string_pretty(output).map_err(parse_error)?;
    if json.len() > 64 * 1024 {
        return Err(JiaClawError::ToolExecution(
            "文件信息结果超过 64 KiB JSON 上限".into(),
        ));
    }
    Ok(json)
}

/// `stat`: inspect metadata without opening file contents or following a leaf link.
pub struct WorkspaceStatTool {
    workspace: PathBuf,
}

impl WorkspaceStatTool {
    /// Create a tool bound to the administrator-provisioned workspace.
    #[must_use]
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: canonicalize_existing_or_clone(workspace),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceStatTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "stat"
    }
    fn description(&self) -> &str {
        "只读取工作区路径元数据，不读文件正文。path 默认 .；父目录禁止链接；叶链接仅返回 symlink，硬链接和特殊文件返回 unsupported 且隐藏大小/时间。不跟随链接。返回 {path,type,size_bytes,modified_unix_ms}。"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object", "properties":{"path":path_schema()}, "additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let args: StatArgs = parse_args(args)?;
        crate::memory_io::info_relative(&args.path)?;
        let workspace = self.workspace.clone();
        let result = crate::memory_io::run_blocking(move || {
            crate::memory_io::stat_entry(&workspace, &args.path)
        })
        .await?;
        encode(&result)
    }
}

/// `tree`: a bounded, depth-first workspace directory listing.
pub struct WorkspaceTreeTool {
    workspace: PathBuf,
}

impl WorkspaceTreeTool {
    /// Create a tool bound to the administrator-provisioned workspace.
    #[must_use]
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: canonicalize_existing_or_clone(workspace),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceTreeTool {
    fn failure_effect(&self) -> crate::tools::ToolFailureEffect {
        crate::tools::ToolFailureEffect::NoEffect
    }

    fn name(&self) -> &str {
        "tree"
    }
    fn description(&self) -> &str {
        "按名称排序逐层深度优先列出工作区目录，包含隐藏项且不跟随链接。path 默认 .；max_depth 默认 3（1..32），max_entries 默认 200（1..1000），超范围报错。共享扫描 2000 项/2 秒协作期限/64 KiB 完整 JSON 上限，任何限制均 truncated=true；不是一致快照。entries 路径相对于选择的目录。"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object", "properties":{
            "path":path_schema(),
            "max_depth":{"type":"integer","minimum":1,"maximum":32,"default":3},
            "max_entries":{"type":"integer","minimum":1,"maximum":1000,"default":200}
        },"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let args: TreeArgs = parse_args(args)?;
        validate_tree(&args)?;
        let workspace = self.workspace.clone();
        let result = crate::memory_io::run_blocking(move || {
            crate::memory_io::tree_directory(
                &workspace,
                &args.path,
                args.max_depth,
                args.max_entries,
            )
        })
        .await?;
        encode(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schemas_and_strict_native_arguments_agree() {
        let ws = tempfile::tempdir().unwrap();
        let stat = WorkspaceStatTool::new(ws.path());
        let tree = WorkspaceTreeTool::new(ws.path());
        let stat_schema = stat.parameters_schema();
        let tree_schema = tree.parameters_schema();
        let stat_validator = jsonschema::validator_for(&stat_schema).unwrap();
        let tree_validator = jsonschema::validator_for(&tree_schema).unwrap();
        for args in [
            json!(null),
            json!([]),
            json!({"path":null}),
            json!({"path":7}),
            json!({"path":""}),
            json!({"extra":1}),
        ] {
            assert!(!stat_validator.is_valid(&args), "{args}");
            assert!(!tree_validator.is_valid(&args), "{args}");
            assert!(stat.execute(args.clone()).await.is_err(), "{args}");
            assert!(tree.execute(args.clone()).await.is_err(), "{args}");
        }
        for args in [
            json!({"max_depth":0}),
            json!({"max_depth":33}),
            json!({"max_depth":1.5}),
            json!({"max_depth":"2"}),
            json!({"max_entries":0}),
            json!({"max_entries":1001}),
            json!({"max_entries":false}),
            json!({"max_entries":null}),
        ] {
            assert!(!tree_validator.is_valid(&args), "{args}");
            assert!(tree.execute(args).await.is_err());
        }
        for args in [
            json!({}),
            json!({"path":"."}),
            json!({"path":"./dir", "max_depth":32,"max_entries":1000}),
        ] {
            assert!(tree_validator.is_valid(&args), "{args}");
            let parsed: TreeArgs = serde_json::from_value(args).unwrap();
            validate_tree(&parsed).unwrap();
        }
        // UTF-8 byte, path component, and traversal rules supplement JSON Schema.
        for path in [
            " ".into(),
            "../escape".into(),
            "/absolute".into(),
            "é".repeat(513),
            vec!["a"; 65].join("/"),
        ] {
            assert!(stat.execute(json!({"path":path})).await.is_err());
            assert!(tree.execute(json!({"path":path})).await.is_err());
        }
        let defaults: TreeArgs = serde_json::from_value(json!({})).unwrap();
        assert_eq!(
            (
                defaults.path.as_str(),
                defaults.max_depth,
                defaults.max_entries
            ),
            (".", 3, 200)
        );
    }

    #[tokio::test]
    async fn real_tools_return_pretty_json_with_the_declared_shape() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("file"), b"hello").unwrap();
        // Retry only explicit pre-admission refusal from unrelated parallel unit
        // tests, never an admitted I/O error or any production operation.
        async fn admitted(tool: &dyn Tool, args: Value) -> String {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match tool.execute(args.clone()).await {
                    Ok(result) => return result,
                    Err(JiaClawError::ToolExecution(error))
                        if error == "memory file: 安全/IO 错误: I/O capacity busy"
                            && std::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        }
        let stat = WorkspaceStatTool::new(ws.path());
        let result: Value =
            serde_json::from_str(&admitted(&stat, json!({"path":"./file"})).await).unwrap();
        assert_eq!(result["path"], "file");
        assert_eq!(result["type"], "file");
        assert_eq!(result["size_bytes"], 5);
        assert!(result["modified_unix_ms"].is_i64());
        let tree = WorkspaceTreeTool::new(ws.path());
        let result: Value = serde_json::from_str(&admitted(&tree, json!({})).await).unwrap();
        assert_eq!(
            result,
            json!({"path":".","max_depth":3,"max_entries":200,"truncated":false,
            "entries":[{"name":"file","type":"file","size":5}]})
        );
    }
}
