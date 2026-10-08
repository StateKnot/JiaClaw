// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Descriptor-relative, bounded and atomically published workspace copies.

use crate::{parse_move_args, Tool};
use async_trait::async_trait;
use jiaclaw_core::JiaClawError;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Maximum accepted source size; enforced again while reading a growing file.
pub const COPY_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Result returned after the complete file has been committed.
#[derive(Debug, Serialize)]
pub struct CopyOutput {
    /// Workspace-relative source.
    pub from: String,
    /// Workspace-relative destination.
    pub to: String,
    /// Bytes copied.
    pub bytes: u64,
    /// Whether the destination existed before publication.
    pub overwritten: bool,
}

fn failure(e: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("copy: {e}"))
}

/// Copy one regular file without following workspace-escaping paths.
/// Destination parents must exist. Existing files are replaced only on explicit
/// overwrite; publication without overwrite is an atomic create-if-absent.
///
/// # Errors
/// Invalid paths, links, special files, size overflow and IO failures fail closed.
pub fn copy_workspace(
    workspace: &Path,
    from: &str,
    to: &str,
    overwrite: bool,
) -> Result<CopyOutput, JiaClawError> {
    let (bytes, overwritten) =
        crate::memory_io::copy_file(workspace, from, to, overwrite).map_err(failure)?;
    Ok(CopyOutput {
        from: from.into(),
        to: to.into(),
        bytes,
        overwritten,
    })
}

/// Model-visible copy tool. `file_copy` is retained as a compatibility alias.
pub struct WorkspaceCopyTool {
    workspace: PathBuf,
    name: &'static str,
}
impl WorkspaceCopyTool {
    /// Create the canonical copy tool.
    #[must_use]
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.into(),
            name: "copy",
        }
    }
    /// Create the old `file_copy` alias with the same safe implementation.
    #[must_use]
    pub fn legacy(workspace: &Path) -> Self {
        Self {
            workspace: workspace.into(),
            name: "file_copy",
        }
    }
}
#[async_trait]
impl Tool for WorkspaceCopyTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &'static str {
        "在工作区内原子复制单链接常规文件（上限 64 MiB）。默认不覆盖。父目录必须存在，禁止绝对路径、..、符号/硬链接；路径最多 1024 字节/64 组件。source/destination 是 from/to 的别名。"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"source":{"type":"string"},"destination":{"type":"string"},"overwrite":{"type":"boolean","default":false}},"allOf":[{"anyOf":[{"required":["from"]},{"required":["source"]}]},{"anyOf":[{"required":["to"]},{"required":["destination"]}]}],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_move_args(&args)?;
        let workspace = self.workspace.clone();
        let result = crate::memory_io::run_blocking(move || {
            copy_workspace(&workspace, &parsed.from, &parsed.to, parsed.overwrite)
        })
        .await
        .map_err(failure)?;
        serde_json::to_string(&result).map_err(failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_binary_copy_and_overwrite() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a"), [0, 255, 42]).unwrap();
        assert_eq!(copy_workspace(ws.path(), "a", "b", false).unwrap().bytes, 3);
        assert!(copy_workspace(ws.path(), "a", "b", false).is_err());
        std::fs::write(ws.path().join("a"), b"changed").unwrap();
        assert!(
            copy_workspace(ws.path(), "a", "b", true)
                .unwrap()
                .overwritten
        );
        assert_eq!(std::fs::read(ws.path().join("b")).unwrap(), b"changed");
        assert!(copy_workspace(ws.path(), "a", "a", true).is_err());
        assert!(copy_workspace(ws.path(), "a", "../escape", false).is_err());
        assert!(copy_workspace(ws.path(), "a", "missing/b", false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_and_oversize_fail_without_mutating_target() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"untouched").unwrap();
        std::fs::write(ws.path().join("a"), b"copy").unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), ws.path().join("b")).unwrap();
        assert!(copy_workspace(ws.path(), "a", "link/new/nested", true).is_err());
        assert!(copy_workspace(ws.path(), "a", "b", true).is_err());
        assert!(copy_workspace(ws.path(), "b", "c", false).is_err());
        assert_eq!(
            std::fs::read(outside.path().join("secret")).unwrap(),
            b"untouched"
        );
        std::fs::File::create(ws.path().join("huge"))
            .unwrap()
            .set_len(COPY_MAX_BYTES + 1)
            .unwrap();
        assert!(copy_workspace(ws.path(), "huge", "a", true).is_err());
        assert_eq!(std::fs::read(ws.path().join("a")).unwrap(), b"copy");
    }
    #[test]
    fn concurrent_no_clobber_has_exactly_one_winner() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a"), b"a").unwrap();
        let results = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| copy_workspace(ws.path(), "a", "b", false).is_ok()))
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .filter(|&ok| ok)
                .count()
        });
        assert_eq!(results, 1);
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 2);
    }
}
