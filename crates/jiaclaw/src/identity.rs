// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工作区 `SOUL.md` / `USER.md`：人格与用户画像注入、路径校验、原子写入。
//!
//! 路径解析复用 [`crate::memory::resolve_workspace_relative_path`]（禁 `..`、绝对路径、symlink 逃逸）。

#![allow(clippy::module_name_repetitions)]

use crate::memory::{
    inspect_workspace_file, load_prompt_file, resolve_workspace_relative_path,
    write_workspace_file, MemoryFileStatus,
};
use crate::tools::Tool;
use async_trait::async_trait;
use jiaclaw_core::{JiaClawError, DEFAULT_SOUL_PATH, DEFAULT_USER_PATH};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 身份文件种类（人格或用户画像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// `SOUL.md` — Agent 人格
    Soul,
    /// `USER.md` — 用户画像
    User,
}

impl IdentityKind {
    /// 配置 / 工具用短名：`soul` 或 `user`。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Soul => "soul",
            Self::User => "user",
        }
    }

    /// 日志与错误信息中的文件标签。
    #[must_use]
    pub fn file_label(self) -> &'static str {
        match self {
            Self::Soul => "SOUL",
            Self::User => "USER",
        }
    }

    /// 系统提示区块标题。
    #[must_use]
    pub fn prompt_heading(self) -> &'static str {
        match self {
            Self::Soul => "Soul（人格）",
            Self::User => "User（用户画像）",
        }
    }

    /// 默认相对路径。
    #[must_use]
    pub fn default_path(self) -> &'static str {
        match self {
            Self::Soul => DEFAULT_SOUL_PATH,
            Self::User => DEFAULT_USER_PATH,
        }
    }
}

/// 将配置中的相对路径解析为工作区内的身份文件路径。
///
/// # Errors
///
/// 路径为空、绝对路径、包含 `..`，或不落在工作空间内时返回错误。
pub fn resolve_identity_path(workspace: &Path, configured: &str) -> Result<PathBuf, JiaClawError> {
    resolve_workspace_relative_path(workspace, configured)
}

/// 读取身份文件以注入系统提示。
///
/// 文件不存在或（trim 后）为空时返回 `Ok(None)`，不报错。
/// 超过 32KiB 时截断到 UTF-8 边界并 `warn`。
///
/// # Errors
///
/// 路径非法，或文件存在但无法读取时返回错误。
pub fn load_identity_for_prompt(
    workspace: &Path,
    configured: &str,
    kind: IdentityKind,
) -> Result<Option<String>, JiaClawError> {
    load_prompt_file(workspace, configured, kind.file_label())
}

/// 检查身份文件是否存在及其大小。
///
/// # Errors
///
/// 配置路径非法时返回错误。
pub fn inspect_identity_file(
    workspace: &Path,
    configured: &str,
) -> Result<MemoryFileStatus, JiaClawError> {
    inspect_workspace_file(workspace, configured)
}

/// 写入身份文件：默认覆盖整个文件；`replace = false` 时追加。
///
/// 只能写约定路径。
///
/// # Errors
///
/// 路径非法、越出工作空间，或 IO 失败时返回错误。
pub fn write_identity(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
) -> Result<PathBuf, JiaClawError> {
    write_workspace_file(workspace, configured, content, replace)
}

/// `soul_write` / `user_write`：向约定身份路径覆盖或追加 Markdown。
pub struct IdentityWriteTool {
    workspace_path: PathBuf,
    rel_path: String,
    kind: IdentityKind,
}

impl IdentityWriteTool {
    /// 创建工具；`rel_path` 相对工作空间。
    #[must_use]
    pub fn new(workspace_path: &Path, rel_path: impl Into<String>, kind: IdentityKind) -> Self {
        let canonical_workspace = workspace_path
            .canonicalize()
            .unwrap_or_else(|_| workspace_path.to_path_buf());
        Self {
            workspace_path: canonical_workspace,
            rel_path: rel_path.into(),
            kind,
        }
    }

    /// 写入人格文件（默认 `SOUL.md`）。
    #[must_use]
    pub fn soul(workspace_path: &Path, rel_path: impl Into<String>) -> Self {
        Self::new(workspace_path, rel_path, IdentityKind::Soul)
    }

    /// 写入用户画像文件（默认 `USER.md`）。
    #[must_use]
    pub fn user(workspace_path: &Path, rel_path: impl Into<String>) -> Self {
        Self::new(workspace_path, rel_path, IdentityKind::User)
    }
}

#[async_trait]
impl Tool for IdentityWriteTool {
    fn name(&self) -> &str {
        match self.kind {
            IdentityKind::Soul => "soul_write",
            IdentityKind::User => "user_write",
        }
    }

    fn description(&self) -> &str {
        match self.kind {
            IdentityKind::Soul => {
                "将内容写入工作区人格文件（默认 SOUL.md）。replace=true（默认）覆盖整个文件；replace=false 时追加。只能写约定路径。"
            }
            IdentityKind::User => {
                "将内容写入工作区用户画像文件（默认 USER.md）。replace=true（默认）覆盖整个文件；replace=false 时追加。只能写约定路径。"
            }
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "要写入的 Markdown 内容"
                },
                "replace": {
                    "type": "boolean",
                    "description": "true（默认）覆盖整个文件；false 追加",
                    "default": true
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| JiaClawError::ToolExecution("缺少参数 'content'".to_string()))?;

        let replace = args.get("replace").and_then(Value::as_bool).unwrap_or(true);

        let path = write_identity(&self.workspace_path, &self.rel_path, content, replace)?;

        let metadata = std::fs::metadata(&path).ok();
        let size = metadata.map_or(0, |m| m.len());
        let mode = if replace { "覆盖" } else { "追加" };
        let label = self.kind.file_label();

        Ok(format!(
            "✅ {label} 已{mode}: {}\n大小: {size} 字节\n约定路径: {}",
            path.display(),
            self.rel_path
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::MEMORY_PROMPT_MAX_BYTES;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_default_paths() {
        let ws = unique_temp("jiaclaw_id_resolve");
        let soul = resolve_identity_path(&ws, DEFAULT_SOUL_PATH).unwrap();
        let user = resolve_identity_path(&ws, DEFAULT_USER_PATH).unwrap();
        assert_eq!(soul, ws.canonicalize().unwrap().join("SOUL.md"));
        assert_eq!(user, ws.canonicalize().unwrap().join("USER.md"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_parent_dir() {
        let ws = unique_temp("jiaclaw_id_parent");
        let err = resolve_identity_path(&ws, "../secret.md").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let err = resolve_identity_path(&ws, "foo/../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let ws = unique_temp("jiaclaw_id_abs");
        let err = resolve_identity_path(&ws, "/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("绝对路径"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_missing_file_is_none() {
        let ws = unique_temp("jiaclaw_id_missing");
        let loaded = load_identity_for_prompt(&ws, DEFAULT_SOUL_PATH, IdentityKind::Soul).unwrap();
        assert!(loaded.is_none());
        let loaded = load_identity_for_prompt(&ws, DEFAULT_USER_PATH, IdentityKind::User).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_empty_file_is_none() {
        let ws = unique_temp("jiaclaw_id_empty");
        fs::write(ws.join("SOUL.md"), "   \n\t\n").unwrap();
        let loaded = load_identity_for_prompt(&ws, DEFAULT_SOUL_PATH, IdentityKind::Soul).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_returns_content_verbatim() {
        let ws = unique_temp("jiaclaw_id_load");
        let body = "# Soul\n\nconcise helper\n";
        fs::write(ws.join("SOUL.md"), body).unwrap();
        let loaded = load_identity_for_prompt(&ws, DEFAULT_SOUL_PATH, IdentityKind::Soul)
            .unwrap()
            .expect("content");
        assert_eq!(loaded, body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_truncates_oversize() {
        let ws = unique_temp("jiaclaw_id_trunc");
        let mut body = "héllo".repeat(MEMORY_PROMPT_MAX_BYTES);
        body.push('中');
        fs::write(ws.join("USER.md"), &body).unwrap();
        let loaded = load_identity_for_prompt(&ws, DEFAULT_USER_PATH, IdentityKind::User)
            .unwrap()
            .expect("truncated");
        assert!(loaded.len() <= MEMORY_PROMPT_MAX_BYTES);
        assert!(loaded.is_char_boundary(loaded.len()));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn replace_overwrites_by_default_semantics() {
        let ws = unique_temp("jiaclaw_id_replace");
        write_identity(&ws, DEFAULT_SOUL_PATH, "old", true).unwrap();
        write_identity(&ws, DEFAULT_SOUL_PATH, "new", true).unwrap();
        let text = fs::read_to_string(ws.join("SOUL.md")).unwrap();
        assert_eq!(text, "new");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn append_separates_when_replace_false() {
        let ws = unique_temp("jiaclaw_id_append");
        write_identity(&ws, DEFAULT_USER_PATH, "first", true).unwrap();
        write_identity(&ws, DEFAULT_USER_PATH, "second", false).unwrap();
        let text = fs::read_to_string(ws.join("USER.md")).unwrap();
        assert_eq!(text, "first\n\nsecond");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn write_rejects_path_traversal() {
        let ws = unique_temp("jiaclaw_id_write_trav");
        let err = write_identity(&ws, "../evil.md", "nope", true).unwrap_err();
        assert!(err.to_string().contains("穿越") || err.to_string().contains("安全"));
        assert!(!ws.parent().unwrap().join("evil.md").exists());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn soul_and_memory_do_not_overwrite_each_other() {
        let ws = unique_temp("jiaclaw_id_coexist");
        write_identity(&ws, DEFAULT_SOUL_PATH, "soul-token", true).unwrap();
        write_identity(&ws, DEFAULT_USER_PATH, "user-token", true).unwrap();
        crate::memory::write_memory(&ws, "MEMORY.md", "memory-token", true).unwrap();

        assert_eq!(
            fs::read_to_string(ws.join("SOUL.md")).unwrap(),
            "soul-token"
        );
        assert_eq!(
            fs::read_to_string(ws.join("USER.md")).unwrap(),
            "user-token"
        );
        assert_eq!(
            fs::read_to_string(ws.join("MEMORY.md")).unwrap(),
            "memory-token"
        );

        write_identity(&ws, DEFAULT_SOUL_PATH, "soul-updated", true).unwrap();
        assert_eq!(
            fs::read_to_string(ws.join("SOUL.md")).unwrap(),
            "soul-updated"
        );
        assert_eq!(
            fs::read_to_string(ws.join("USER.md")).unwrap(),
            "user-token"
        );
        assert_eq!(
            fs::read_to_string(ws.join("MEMORY.md")).unwrap(),
            "memory-token"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn inspect_reports_size() {
        let ws = unique_temp("jiaclaw_id_inspect");
        let missing = inspect_identity_file(&ws, DEFAULT_SOUL_PATH).unwrap();
        assert!(!missing.exists);
        assert_eq!(missing.size_bytes, 0);

        fs::write(ws.join("SOUL.md"), "abcd").unwrap();
        let present = inspect_identity_file(&ws, DEFAULT_SOUL_PATH).unwrap();
        assert!(present.exists);
        assert_eq!(present.size_bytes, 4);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn soul_write_tool_overwrites() {
        let ws = unique_temp("jiaclaw_id_tool_soul");
        let tool = IdentityWriteTool::soul(&ws, DEFAULT_SOUL_PATH);
        assert_eq!(tool.name(), "soul_write");

        tool.execute(serde_json::json!({"content": "v1"}))
            .await
            .unwrap();
        tool.execute(serde_json::json!({"content": "v2"}))
            .await
            .unwrap();
        let text = fs::read_to_string(ws.join("SOUL.md")).unwrap();
        assert_eq!(text, "v2");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn user_write_tool_ignores_path_argument() {
        let ws = unique_temp("jiaclaw_id_ignore_path");
        let tool = IdentityWriteTool::user(&ws, DEFAULT_USER_PATH);
        assert_eq!(tool.name(), "user_write");
        tool.execute(serde_json::json!({
            "content": "safe-user",
            "path": "../evil.md"
        }))
        .await
        .unwrap();
        assert!(ws.join("USER.md").exists());
        assert!(!ws.join("evil.md").exists());
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_on_read() {
        let ws = unique_temp("jiaclaw_id_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_id_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("SOUL.md")).unwrap();

        let result = load_identity_for_prompt(&ws, DEFAULT_SOUL_PATH, IdentityKind::Soul);
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }
}
