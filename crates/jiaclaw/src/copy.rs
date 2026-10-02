// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Descriptor-relative, bounded and atomically published workspace copies.

use crate::{parse_move_args, Tool};
use async_trait::async_trait;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use jiaclaw_core::JiaClawError;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

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

fn relative(raw: &str) -> Result<&Path, JiaClawError> {
    let path = Path::new(raw);
    if raw.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(failure("路径必须相对于工作区，禁止 .. 和绝对路径"));
    }
    if !path.components().any(|c| matches!(c, Component::Normal(_))) {
        return Err(failure("不能复制工作区根目录"));
    }
    Ok(path)
}

// Reject links in every component. Dir operations enforce confinement even if an
// untrusted process replaces a component after this check.
fn reject_links(root: &Dir, path: &Path) -> Result<(), JiaClawError> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        match root.symlink_metadata(&prefix) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(failure("禁止符号链接")),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(failure(e)),
        }
    }
    Ok(())
}

struct StagedFile<'a> {
    dir: &'a Dir,
    name: String,
}
impl Drop for StagedFile<'_> {
    fn drop(&mut self) {
        let _ = self.dir.remove_file(&self.name);
    }
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
    let source = relative(from)?;
    let destination = relative(to)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    reject_links(&root, source)?;
    reject_links(&root, destination)?;
    if root.canonicalize(source).map_err(failure)?
        == root
            .canonicalize(destination)
            .unwrap_or_else(|_| destination.to_path_buf())
    {
        return Err(failure("源与目标是同一路径"));
    }
    if !root.symlink_metadata(source).map_err(failure)?.is_file() {
        return Err(failure("源必须是常规文件"));
    }
    let mut input_options = OpenOptions::new();
    input_options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        input_options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut input = root.open_with(source, &input_options).map_err(failure)?;
    let metadata = input.metadata().map_err(failure)?;
    if !metadata.is_file() || metadata.len() > COPY_MAX_BYTES {
        return Err(failure("源不是常规文件或超过 64 MiB"));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_dir = root.open_dir(parent).map_err(failure)?;
    let name = destination.file_name().ok_or_else(|| failure("无效目标"))?;
    let overwritten = match parent_dir.symlink_metadata(name) {
        Ok(meta) if meta.is_file() => true,
        Ok(_) => return Err(failure("目标必须是常规文件，禁止目录与符号链接")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(failure(e)),
    };
    if overwritten && !overwrite {
        return Err(failure("目标已存在（overwrite=false）"));
    }
    let staged = StagedFile {
        dir: &parent_dir,
        name: format!(".jiaclaw-copy-{}", uuid::Uuid::new_v4()),
    };
    let mut output_options = OpenOptions::new();
    output_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        output_options.mode(0o600);
    }
    let mut output = parent_dir
        .open_with(&staged.name, &output_options)
        .map_err(failure)?;
    let bytes =
        io::copy(&mut (&mut input).take(COPY_MAX_BYTES + 1), &mut output).map_err(failure)?;
    if bytes > COPY_MAX_BYTES {
        return Err(failure("复制期间源文件超过 64 MiB"));
    }
    output.sync_all().map_err(failure)?;
    drop(output);
    if overwrite {
        parent_dir
            .rename(&staged.name, &parent_dir, name)
            .map_err(failure)?;
    } else {
        // Hard link publication is no-clobber even under a competing writer.
        parent_dir
            .hard_link(&staged.name, &parent_dir, name)
            .map_err(failure)?;
    }
    // Sync the parent after publication on Unix for crash durability.
    #[cfg(unix)]
    parent_dir
        .try_clone()
        .map_err(failure)?
        .into_std_file()
        .sync_all()
        .map_err(failure)?;
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
        "在工作区内原子复制常规文件（上限 64 MiB）。默认不覆盖。父目录必须存在，禁止绝对路径、.. 和符号链接。source/destination 是 from/to 的别名。"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"},"source":{"type":"string"},"destination":{"type":"string"},"overwrite":{"type":"boolean","default":false}},"anyOf":[{"required":["from","to"]},{"required":["source","destination"]}],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_move_args(&args)?;
        let workspace = self.workspace.clone();
        let result = tokio::task::spawn_blocking(move || {
            copy_workspace(&workspace, &parsed.from, &parsed.to, parsed.overwrite)
        })
        .await
        .map_err(failure)??;
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
