// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bounded memory-file I/O through directory capabilities. The workspace root
//! is administrator-owned. Never reopen a checked leaf by an ambient path.

use cap_std::{
    ambient_authority,
    fs::{Dir, File, Metadata, OpenOptions},
};
use jiaclaw_core::JiaClawError;
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_READ_BYTES: usize = 512 * 1024;
pub(crate) const MAX_PATH_BYTES: usize = 1024;
static IO_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

fn failure(e: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("memory file: 安全/IO 错误: {e}"))
}

/// Admission stays owned by the blocking job if its async caller is cancelled.
pub(crate) async fn run_blocking<T, F>(work: F) -> Result<T, JiaClawError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, JiaClawError> + Send + 'static,
{
    run_blocking_with_slots(&IO_SLOTS, work).await
}

async fn run_blocking_with_slots<T, F>(
    slots: &'static tokio::sync::Semaphore,
    work: F,
) -> Result<T, JiaClawError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, JiaClawError> + Send + 'static,
{
    let permit = slots
        .try_acquire()
        .map_err(|_| failure("I/O capacity busy"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| failure("I/O task failed"))?
}

pub(crate) fn relative(raw: &str) -> Result<&Path, JiaClawError> {
    let raw = raw.trim();
    let path = Path::new(raw);
    if raw.is_empty()
        || raw.len() > MAX_PATH_BYTES
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        || !path.components().any(|c| matches!(c, Component::Normal(_)))
        || path.components().count() > 64
    {
        return Err(failure(
            "路径必须相对于工作空间，禁止绝对路径、穿越和空路径；最多 1024 字节/64 个组件",
        ));
    }
    Ok(path)
}

fn regular(meta: &Metadata) -> Result<(), JiaClawError> {
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(failure("只允许常规文件，禁止目录、符号链接和特殊文件"));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(failure("禁止硬链接"));
        }
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if meta.number_of_links() != Some(1) {
            return Err(failure("禁止硬链接或未知链接计数"));
        }
    }
    Ok(())
}

fn read_options(directory: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(
            libc::O_NOFOLLOW | libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 },
        );
    }
    #[cfg(not(unix))]
    let _ = directory;
    options
}

// Open each parent separately with O_NOFOLLOW on supported Linux/macOS hosts.
// cap-std additionally prevents escapes through substituted path components.
fn parent(root: &Dir, path: &Path, create: bool) -> Result<Option<Dir>, JiaClawError> {
    let mut dir = root.try_clone().map_err(failure)?;
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        match dir.symlink_metadata(name) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => (),
            Ok(_) => return Err(failure("父目录不是普通目录，禁止符号链接")),
            Err(e) if e.kind() == io::ErrorKind::NotFound && create => match dir.create_dir(name) {
                Ok(()) => {
                    sync_dir(&dir)?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(failure(e)),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failure(e)),
        }
        #[cfg(unix)]
        {
            let opened = dir.open_with(name, &read_options(true)).map_err(failure)?;
            dir = Dir::from_std_file(opened.into_std());
        }
        #[cfg(not(unix))]
        {
            dir = dir.open_dir(name).map_err(failure)?;
        }
    }
    Ok(Some(dir))
}

fn open_regular(dir: &Dir, name: &OsStr) -> Result<Option<File>, JiaClawError> {
    match dir.symlink_metadata(name) {
        Ok(meta) => regular(&meta)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(failure(e)),
    }
    let file = dir.open_with(name, &read_options(false)).map_err(failure)?;
    regular(&file.metadata().map_err(failure)?)?;
    Ok(Some(file))
}

pub(crate) struct TextFile {
    pub text: String,
    pub truncated: bool,
    pub size_bytes: u64,
}

fn read_open(file: File, limit: usize, truncate: bool) -> Result<TextFile, JiaClawError> {
    if limit > MAX_READ_BYTES {
        return Err(failure("读取上限超过 512 KiB"));
    }
    let metadata = file.metadata().map_err(failure)?;
    regular(&metadata)?;
    if metadata.len() > limit as u64 && !truncate {
        return Err(failure(format!("文件超过上限 {limit} 字节")));
    }
    let mut bytes = Vec::with_capacity(limit.min(8192));
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    let truncated = bytes.len() > limit;
    if truncated {
        if !truncate {
            return Err(failure(format!("文件超过上限 {limit} 字节")));
        }
        bytes.truncate(limit);
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) if truncated && e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).map_err(|_| failure("无效 UTF-8"))?
        }
        Err(_) => return Err(failure("无效 UTF-8")),
    };
    Ok(TextFile {
        text,
        truncated,
        size_bytes: metadata.len(),
    })
}

pub(crate) fn read_text(
    workspace: &Path,
    configured: &str,
    limit: usize,
    truncate: bool,
) -> Result<Option<TextFile>, JiaClawError> {
    let path = relative(configured)?;
    let root = match Dir::open_ambient_dir(workspace, ambient_authority()) {
        Ok(root) => root,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(failure(e)),
    };
    let Some(dir) = parent(&root, path, false)? else {
        return Ok(None);
    };
    open_regular(&dir, path.file_name().ok_or_else(|| failure("无效文件名"))?)?
        .map(|file| read_open(file, limit, truncate))
        .transpose()
}

pub(crate) fn inspect(workspace: &Path, configured: &str) -> Result<Option<u64>, JiaClawError> {
    let path = relative(configured)?;
    let root = match Dir::open_ambient_dir(workspace, ambient_authority()) {
        Ok(root) => root,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(failure(e)),
    };
    let Some(dir) = parent(&root, path, false)? else {
        return Ok(None);
    };
    open_regular(&dir, path.file_name().ok_or_else(|| failure("无效文件名"))?)?
        .map(|file| file.metadata().map(|m| m.len()).map_err(failure))
        .transpose()
}

// No mutable lock path: all cooperating writers lock the workspace directory
// inode for the whole read/modify/publish operation. A busy workspace fails
// immediately. File editors that do not acquire this lock are not serialized.
fn writer_lock(root: &Dir) -> Result<std::fs::File, JiaClawError> {
    let lock = root.open(".").map_err(failure)?.into_std();
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|e| failure(format!("workspace write lock busy or unsupported: {e}")))?;
    Ok(lock)
}

fn sync_dir(dir: &Dir) -> Result<(), JiaClawError> {
    #[cfg(unix)]
    dir.open(".")
        .map_err(failure)?
        .sync_all()
        .map_err(failure)?;
    Ok(())
}

struct Staged<'a> {
    dir: &'a Dir,
    name: String,
}
impl Drop for Staged<'_> {
    fn drop(&mut self) {
        let _ = self.dir.remove_file(&self.name);
    }
}

fn publish(dir: &Dir, name: &OsStr, bytes: &[u8], overwrite: bool) -> Result<bool, JiaClawError> {
    let staged = Staged {
        dir,
        name: format!(".jiaclaw-memory-{}", uuid::Uuid::new_v4()),
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = dir.open_with(&staged.name, &options).map_err(failure)?;
    file.write_all(bytes).map_err(failure)?;
    file.sync_all().map_err(failure)?;
    drop(file);
    // Revalidate the directory entry; a final race cannot follow the leaf:
    // rename replaces the entry, and hard_link is atomic create-if-absent.
    match dir.symlink_metadata(name) {
        Ok(meta) => {
            regular(&meta)?;
            if !overwrite {
                return Ok(false);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(failure(e)),
    }
    if overwrite {
        dir.rename(&staged.name, dir, name).map_err(failure)?;
    } else {
        match dir.hard_link(&staged.name, dir, name) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                regular(&dir.symlink_metadata(name).map_err(failure)?)?;
                return Ok(false);
            }
            Err(e) => return Err(failure(e)),
        }
        dir.remove_file(&staged.name).map_err(|e| {
            failure(format!(
                "文件已创建但暂存链接清理失败，请核对文件后再决定是否重试: {e}"
            ))
        })?;
    }
    drop(staged);
    sync_dir(dir).map_err(|e| {
        failure(format!(
            "文件已提交但目录同步失败，请核对文件后再决定是否重试: {e}"
        ))
    })?;
    Ok(true)
}

pub(crate) fn write_text(
    workspace: &Path,
    configured: &str,
    content: &str,
    replace: bool,
    limit: usize,
) -> Result<(PathBuf, usize), JiaClawError> {
    // Public APIs may ask for a lower limit, never lift the memory-write cap.
    let limit = limit.min(jiaclaw_core::MEMORY_PROMPT_MAX_BYTES);
    if content.len() > limit {
        return Err(failure(format!("记忆文件超过上限 {limit} 字节")));
    }
    let path = relative(configured)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let dir = parent(&root, path, true)?.ok_or_else(|| failure("缺少父目录"))?;
    let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
    let existing = open_regular(&dir, name)?;
    let contents = if replace {
        content.to_owned()
    } else {
        match existing {
            Some(file) => {
                crate::memory::join_memory_append(&read_open(file, limit, false)?.text, content)
            }
            None => content.to_owned(),
        }
    };
    if contents.len() > limit {
        return Err(failure(format!("记忆文件超过上限 {limit} 字节")));
    }
    publish(&dir, name, contents.as_bytes(), true)?;
    Ok((workspace.join(path), contents.len()))
}

pub(crate) fn create_text_if_missing(
    workspace: &Path,
    configured: &str,
    content: &str,
) -> Result<bool, JiaClawError> {
    if content.len() > jiaclaw_core::MEMORY_PROMPT_MAX_BYTES {
        return Err(failure("初始文件超过 32 KiB"));
    }
    let path = relative(configured)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let dir = parent(&root, path, true)?.ok_or_else(|| failure("缺少父目录"))?;
    let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
    if open_regular(&dir, name)?.is_some() {
        return Ok(false);
    }
    publish(&dir, name, content.as_bytes(), false)
}

// Compatibility helper for existing general file tools. Their callers own path
// authorization; this fixes unique staging/durability, not their path policy.
pub(crate) fn atomic_replace_ambient(path: &Path, bytes: &[u8]) -> Result<(), JiaClawError> {
    let parent = path.parent().ok_or_else(|| failure("无效父目录"))?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority()).map_err(failure)?;
    publish(
        &dir,
        path.file_name().ok_or_else(|| failure("无效文件名"))?,
        bytes,
        true,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_retains_capacity_until_blocking_work_completes() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use std::time::Duration;
        // Independent capacity keeps parallel memory tests out of this protocol.
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_blocking_with_slots(&SLOTS, move || {
            let _ = started_tx.send(());
            release_rx.recv().map_err(failure)?;
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let called = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&called);
        let denied = run_blocking_with_slots(&SLOTS, move || {
            observed.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(denied.unwrap_err().to_string().contains("capacity busy"));
        assert!(!called.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        // Await the permit itself, not a sleep or a signal before the worker returns.
        let permit = tokio::time::timeout(Duration::from_secs(5), SLOTS.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
        assert_eq!(run_blocking_with_slots(&SLOTS, || Ok(7)).await.unwrap(), 7);
    }

    #[test]
    fn bounded_utf8_and_invalid_input() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a"), "abc中tail").unwrap();
        let text = read_text(ws.path(), "a", 5, true).unwrap().unwrap();
        assert_eq!(text.text, "abc");
        assert!(text.truncated);
        assert!(read_text(ws.path(), "a", 5, false).is_err());
        std::fs::write(ws.path().join("a"), [0xff, b'a']).unwrap();
        assert!(read_text(ws.path(), "a", 8, true).is_err());
        assert!(read_text(ws.path(), ".", 8, true).is_err());
    }
    #[test]
    fn write_limit_and_nested_create_preserve_previous_bytes() {
        let ws = tempfile::tempdir().unwrap();
        write_text(ws.path(), "nested/a", "original", true, 32).unwrap();
        assert!(write_text(ws.path(), "nested/a", &"x".repeat(33), false, 32).is_err());
        assert_eq!(
            std::fs::read_to_string(ws.path().join("nested/a")).unwrap(),
            "original"
        );
        assert!(!create_text_if_missing(ws.path(), "nested/a", "other").unwrap());
        assert!(write_text(ws.path(), "huge", &"x".repeat(32769), true, usize::MAX).is_err());
        assert!(!ws.path().join("huge").exists());
    }
    #[cfg(unix)]
    #[test]
    fn links_special_files_and_predictable_tmp_cannot_escape() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "untouched").unwrap();
        symlink(outside.path(), ws.path().join("linked")).unwrap();
        assert!(write_text(ws.path(), "linked/new", "bad", true, 32).is_err());
        assert!(!outside.path().join("new").exists());
        symlink(outside.path().join("secret"), ws.path().join("a")).unwrap();
        assert!(read_text(ws.path(), "a", 32, true).is_err());
        assert!(write_text(ws.path(), "a", "bad", true, 32).is_err());
        std::fs::remove_file(ws.path().join("a")).unwrap();
        std::fs::hard_link(outside.path().join("secret"), ws.path().join("a")).unwrap();
        assert!(read_text(ws.path(), "a", 32, true).is_err());
        assert!(write_text(ws.path(), "a", "bad", true, 32).is_err());
        std::fs::remove_file(ws.path().join("a")).unwrap();
        symlink(outside.path().join("secret"), ws.path().join("a.tmp")).unwrap();
        write_text(ws.path(), "a", "safe", true, 32).unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret")).unwrap(),
            "untouched"
        );
        assert_eq!(
            std::fs::metadata(ws.path().join("a"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let socket = std::os::unix::net::UnixListener::bind(ws.path().join("socket")).unwrap();
        assert!(read_text(ws.path(), "socket", 32, true).is_err());
        drop(socket);
        assert!(!std::fs::read_dir(ws.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jiaclaw-memory-")));
    }
    #[cfg(unix)]
    #[test]
    fn concurrent_appends_commit_without_lost_updates_and_lock_is_released() {
        let ws = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for i in 0..12 {
                let path = ws.path();
                scope.spawn(move || {
                    let text = format!("record-{i:02}");
                    for _ in 0..200 {
                        match write_text(path, "a", &text, false, 32768) {
                            Ok(_) => return,
                            Err(e) if e.to_string().contains("lock busy") => {
                                std::thread::sleep(std::time::Duration::from_millis(2))
                            }
                            Err(e) => panic!("{e}"),
                        }
                    }
                    panic!("lock did not release");
                });
            }
        });
        let text = std::fs::read_to_string(ws.path().join("a")).unwrap();
        assert_eq!(text.lines().filter(|s| !s.is_empty()).count(), 12);
        for i in 0..12 {
            assert_eq!(text.matches(&format!("record-{i:02}")).count(), 1);
        }
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let lock = writer_lock(&root).unwrap();
        assert!(write_text(ws.path(), "a", "denied", false, 32768).is_err());
        drop(lock);
        write_text(ws.path(), "a", "after", false, 32768).unwrap();
    }
}
