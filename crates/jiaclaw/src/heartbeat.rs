// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工作区 `HEARTBEAT.md`：周期性自检提示文件的路径校验与读取。
//!
//! 路径解析复用 [`crate::memory::resolve_workspace_relative_path`]（禁 `..`、绝对路径、symlink 逃逸）。

#![allow(clippy::module_name_repetitions)]

use crate::memory::{
    ensure_existing_within_workspace, inspect_workspace_file, resolve_workspace_relative_path,
    MemoryFileStatus,
};
use jiaclaw_core::JiaClawError;
use std::path::{Path, PathBuf};

/// 将配置中的相对路径解析为工作区内的心跳文件路径。
///
/// # Errors
///
/// 路径为空、绝对路径、包含 `..`，或不落在工作空间内时返回错误。
pub fn resolve_heartbeat_path(workspace: &Path, configured: &str) -> Result<PathBuf, JiaClawError> {
    resolve_workspace_relative_path(workspace, configured)
}

/// 检查心跳文件是否存在及其大小（不读取全文）。
///
/// # Errors
///
/// 配置路径非法时返回错误。
pub fn inspect_heartbeat_file(
    workspace: &Path,
    configured: &str,
) -> Result<MemoryFileStatus, JiaClawError> {
    inspect_workspace_file(workspace, configured)
}

/// 读取心跳文件全文，作为一轮 chat 的 user 消息。
///
/// 文件不存在或（trim 后）为空时返回 `Ok(None)`，不报错。
/// 不截断；超过 32KiB 时仅 `warn` 仍返回全文。
///
/// # Errors
///
/// 路径非法、symlink 逃逸，或文件存在但无法读取时返回错误。
pub fn load_heartbeat_message(
    workspace: &Path,
    configured: &str,
) -> Result<Option<String>, JiaClawError> {
    let path = resolve_heartbeat_path(workspace, configured)?;
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        tracing::debug!(
            path = %path.display(),
            "HEARTBEAT 路径存在但不是文件，跳过本轮"
        );
        return Ok(None);
    }

    ensure_existing_within_workspace(workspace, &path)?;

    let raw = std::fs::read_to_string(&path).map_err(|e| {
        JiaClawError::Configuration(format!("无法读取HEARTBEAT文件 {}: {e}", path.display()))
    })?;

    if raw.trim().is_empty() {
        return Ok(None);
    }

    if raw.len() > jiaclaw_core::MEMORY_PROMPT_MAX_BYTES {
        tracing::warn!(
            path = %path.display(),
            size_bytes = raw.len(),
            "HEARTBEAT 文件较大，仍将全文作为 user 消息"
        );
    }

    Ok(Some(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiaclaw_core::DEFAULT_HEARTBEAT_PATH;
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
    fn resolve_default_path() {
        let ws = unique_temp("jiaclaw_hb_resolve");
        let path = resolve_heartbeat_path(&ws, DEFAULT_HEARTBEAT_PATH).unwrap();
        assert_eq!(path, ws.canonicalize().unwrap().join("HEARTBEAT.md"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn custom_relative_path_within_workspace() {
        let ws = unique_temp("jiaclaw_hb_custom");
        fs::create_dir_all(ws.join("ops")).unwrap();
        fs::write(ws.join("ops/HEARTBEAT.md"), "check calendar").unwrap();
        let loaded = load_heartbeat_message(&ws, "ops/HEARTBEAT.md")
            .unwrap()
            .expect("content");
        assert_eq!(loaded, "check calendar");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_parent_dir() {
        let ws = unique_temp("jiaclaw_hb_parent");
        let err = resolve_heartbeat_path(&ws, "../secret.md").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let err = resolve_heartbeat_path(&ws, "foo/../../etc/passwd").unwrap_err();
        assert!(err.to_string().contains("穿越"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let ws = unique_temp("jiaclaw_hb_abs");
        let err = resolve_heartbeat_path(&ws, "/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("绝对路径"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_missing_file_is_none() {
        let ws = unique_temp("jiaclaw_hb_missing");
        let loaded = load_heartbeat_message(&ws, DEFAULT_HEARTBEAT_PATH).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_empty_file_is_none() {
        let ws = unique_temp("jiaclaw_hb_empty");
        fs::write(ws.join("HEARTBEAT.md"), "   \n\t\n").unwrap();
        let loaded = load_heartbeat_message(&ws, DEFAULT_HEARTBEAT_PATH).unwrap();
        assert!(loaded.is_none());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn load_returns_content_verbatim() {
        let ws = unique_temp("jiaclaw_hb_load");
        let body = "# Heartbeat\n\n- remind me to stretch\n";
        fs::write(ws.join("HEARTBEAT.md"), body).unwrap();
        let loaded = load_heartbeat_message(&ws, DEFAULT_HEARTBEAT_PATH)
            .unwrap()
            .expect("content");
        assert_eq!(loaded, body);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn inspect_reports_size_without_reading_body() {
        let ws = unique_temp("jiaclaw_hb_inspect");
        let missing = inspect_heartbeat_file(&ws, DEFAULT_HEARTBEAT_PATH).unwrap();
        assert!(!missing.exists);
        assert_eq!(missing.size_bytes, 0);

        fs::write(ws.join("HEARTBEAT.md"), "abcd").unwrap();
        let present = inspect_heartbeat_file(&ws, DEFAULT_HEARTBEAT_PATH).unwrap();
        assert!(present.exists);
        assert_eq!(present.size_bytes, 4);
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected_on_read() {
        let ws = unique_temp("jiaclaw_hb_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_hb_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("HEARTBEAT.md")).unwrap();

        let result = load_heartbeat_message(&ws, DEFAULT_HEARTBEAT_PATH);
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }
}
