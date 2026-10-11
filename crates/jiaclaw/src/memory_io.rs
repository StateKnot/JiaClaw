// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Bounded memory and workspace-file I/O through directory capabilities. The workspace root
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
    descend(root, path.parent().unwrap_or_else(|| Path::new("")), create)
}

fn descend(root: &Dir, path: &Path, create: bool) -> Result<Option<Dir>, JiaClawError> {
    let mut dir = root.try_clone().map_err(failure)?;
    for component in path.components() {
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
struct WorkspaceWriteLock {
    file: std::fs::File,
    owner_pid: u32,
}

impl Drop for WorkspaceWriteLock {
    fn drop(&mut self) {
        // A forked child has no ownership of its parent's active mutation.
        // The completing owner explicitly unlocks even if an unrelated process
        // still holds a duplicated descriptor until exec/exit.
        if self.owner_pid == std::process::id() {
            if let Err(error) = fs2::FileExt::unlock(&self.file) {
                tracing::error!(%error, "workspace writer unlock failed; closing descriptor");
            }
        }
    }
}

fn writer_lock(root: &Dir) -> Result<WorkspaceWriteLock, JiaClawError> {
    let lock = root.open(".").map_err(failure)?.into_std();
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|e| failure(format!("workspace write lock busy or unsupported: {e}")))?;
    Ok(WorkspaceWriteLock {
        file: lock,
        owner_pid: std::process::id(),
    })
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

pub(crate) fn initialize_workspace_file(
    workspace: &Path,
    configured: &str,
    content: &str,
    overwrite: bool,
) -> Result<bool, JiaClawError> {
    if content.len() > jiaclaw_core::MEMORY_PROMPT_MAX_BYTES {
        return Err(failure("初始文件超过 32 KiB"));
    }
    let path = relative(configured)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    // Policy presence and publication share the actual nonqueueing writer.
    // Initialization does not parse, rewrite or bypass an operator source lock.
    if path.starts_with("skills") && open_regular(&root, OsStr::new("skills.lock.json"))?.is_some()
    {
        // Preserve parent-link refusal, without opening any managed skill leaf.
        parent(&root, path, false)?;
        return Ok(false);
    }
    let dir = parent(&root, path, true)?.ok_or_else(|| failure("缺少父目录"))?;
    let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
    if open_regular(&dir, name)?.is_some() && !overwrite {
        return Ok(false);
    }
    publish(&dir, name, content.as_bytes(), overwrite)
}

// General file tools share the directory capabilities and writer lock with
// memory files, but retain their own byte limits and exact append semantics.
const FILE_LIMIT: usize = crate::files::READ_FILE_MAX_BYTES;
const DIRECTORY_SCAN_LIMIT: usize = 2000;
const DIRECTORY_DEPTH_LIMIT: usize = 32;
const DIRECTORY_JSON_BUDGET: usize = 64 * 1024;

fn read_bytes(file: File, limit: usize) -> Result<Vec<u8>, JiaClawError> {
    let limit = limit.min(FILE_LIMIT);
    let metadata = file.metadata().map_err(failure)?;
    regular(&metadata)?;
    if metadata.len() > limit as u64 {
        return Err(failure(format!("文件超过上限 {limit} 字节")));
    }
    read_bounded_bytes(file, limit)
}

fn read_bounded_bytes(reader: impl Read, limit: usize) -> Result<Vec<u8>, JiaClawError> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() > limit {
        return Err(failure(format!("读取期间文件超过上限 {limit} 字节")));
    }
    Ok(bytes)
}

pub(crate) fn read_file_bytes(
    workspace: &Path,
    raw: &str,
    limit: usize,
) -> Result<Vec<u8>, JiaClawError> {
    let path = relative(raw)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let dir = parent(&root, path, false)?.ok_or_else(|| failure("文件不存在"))?;
    let file = open_regular(&dir, path.file_name().ok_or_else(|| failure("无效文件名"))?)?
        .ok_or_else(|| failure("文件不存在"))?;
    read_bytes(file, limit)
}

fn edit_file<T>(
    workspace: &Path,
    raw: &str,
    create_parents: bool,
    limit: usize,
    edit: impl FnOnce(Option<File>) -> Result<(Vec<u8>, T), JiaClawError>,
) -> Result<T, JiaClawError> {
    let path = relative(raw)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let dir = parent(&root, path, create_parents)?.ok_or_else(|| failure("文件不存在"))?;
    let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
    let (bytes, result) = edit(open_regular(&dir, name)?)?;
    if bytes.len() > limit.min(FILE_LIMIT) {
        return Err(failure(format!(
            "文件超过上限 {} 字节",
            limit.min(FILE_LIMIT)
        )));
    }
    publish(&dir, name, &bytes, true)?;
    Ok(result)
}

// The edit and its validation run under the same cooperative workspace writer
// ownership as publication; callers never receive or reopen an ambient leaf.
pub(crate) fn update_existing_text<T>(
    workspace: &Path,
    raw: &str,
    limit: usize,
    update: impl FnOnce(&str) -> Result<(String, T), JiaClawError>,
) -> Result<T, JiaClawError> {
    edit_file(workspace, raw, false, limit, |existing| {
        let text = read_open(existing.ok_or_else(|| failure("文件不存在"))?, limit, false)?.text;
        let (next, result) = update(&text)?;
        Ok((next.into_bytes(), result))
    })
}

pub(crate) fn write_file_bytes(
    workspace: &Path,
    raw: &str,
    content: &str,
    append: bool,
    limit: usize,
) -> Result<usize, JiaClawError> {
    let limit = limit.min(FILE_LIMIT);
    if content.len() > limit {
        return Err(failure(format!("文件超过上限 {limit} 字节")));
    }
    edit_file(workspace, raw, true, limit, |existing| {
        let mut bytes = if append {
            existing
                .map(|file| read_bytes(file, limit))
                .transpose()?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if bytes.len().saturating_add(content.len()) > limit {
            return Err(failure(format!("文件超过上限 {limit} 字节")));
        }
        bytes.extend_from_slice(content.as_bytes());
        let length = bytes.len();
        Ok((bytes, length))
    })
}

pub(crate) fn replace_file_text(
    workspace: &Path,
    raw: &str,
    old: &str,
    new: &str,
    all: bool,
    limit: usize,
) -> Result<(usize, usize), JiaClawError> {
    let limit = limit.min(FILE_LIMIT);
    if old.is_empty() {
        return Err(failure("参数 'old_str' 不能为空"));
    }
    // A huge replacement is rejected before reading or creating any file.
    if new.len() > limit {
        return Err(failure(format!("文件超过上限 {limit} 字节")));
    }
    edit_file(workspace, raw, false, limit, |existing| {
        let bytes = read_bytes(existing.ok_or_else(|| failure("文件不存在"))?, limit)?;
        let text = String::from_utf8(bytes).map_err(|_| failure("二进制文件，拒绝替换"))?;
        if text.contains('\0') {
            return Err(failure("二进制文件，拒绝替换"));
        }
        let count = text.matches(old).count();
        if count == 0 {
            return Err(failure("old_str 在文件中未找到（匹配 0 次）"));
        }
        if !all && count != 1 {
            return Err(failure(format!(
                "old_str 在文件中匹配 {count} 次，默认必须恰好 1 次"
            )));
        }
        let removed = old
            .len()
            .checked_mul(count)
            .ok_or_else(|| failure("替换大小溢出"))?;
        let inserted = new
            .len()
            .checked_mul(count)
            .ok_or_else(|| failure("替换大小溢出"))?;
        let size = text
            .len()
            .checked_sub(removed)
            .and_then(|n| n.checked_add(inserted))
            .ok_or_else(|| failure("替换大小溢出"))?;
        if size > limit {
            return Err(failure(format!("文件超过上限 {limit} 字节")));
        }
        let result = text.replace(old, new).into_bytes();
        Ok((result, (count, size)))
    })
}

pub(crate) fn delete_file(workspace: &Path, raw: &str) -> Result<u64, JiaClawError> {
    let path = relative(raw)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let dir = parent(&root, path, false)?.ok_or_else(|| failure("文件不存在"))?;
    let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
    let file = open_regular(&dir, name)?.ok_or_else(|| failure("文件不存在"))?;
    let size = file.metadata().map_err(failure)?.len();
    drop(file);
    // Unlink only this directory entry. A replaced leaf can never redirect the
    // unlink to its symlink target; cooperating mutations retain the root lock.
    dir.remove_file(name).map_err(failure)?;
    sync_dir(&dir).map_err(|e| {
        failure(format!(
            "文件已删除但目录同步失败，请核对后再决定是否重试: {e}"
        ))
    })?;
    Ok(size)
}

/// Stream a bounded binary copy under the same directory lock as other writers.
pub(crate) fn copy_file(
    workspace: &Path,
    from: &str,
    to: &str,
    overwrite: bool,
) -> Result<(u64, bool), JiaClawError> {
    copy_file_with(workspace, from, to, overwrite, |input, output| {
        io::copy(input, output)
    })
}

fn copy_file_with(
    workspace: &Path,
    from: &str,
    to: &str,
    overwrite: bool,
    transfer: impl FnOnce(&mut io::Take<File>, &mut File) -> io::Result<u64>,
) -> Result<(u64, bool), JiaClawError> {
    let source = relative(from)?;
    let destination = relative(to)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let source_dir = parent(&root, source, false)?.ok_or_else(|| failure("源父目录不存在"))?;
    let input = open_regular(
        &source_dir,
        source.file_name().ok_or_else(|| failure("无效源文件名"))?,
    )?
    .ok_or_else(|| failure("源文件不存在"))?;
    let source_metadata = input.metadata().map_err(failure)?;
    if source_metadata.len() > crate::copy::COPY_MAX_BYTES {
        return Err(failure("复制源超过 64 MiB"));
    }
    let destination_dir =
        parent(&root, destination, false)?.ok_or_else(|| failure("目标父目录不存在"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| failure("无效目标文件名"))?;
    let overwritten = match destination_dir.symlink_metadata(name) {
        Ok(metadata) => {
            regular(&metadata)?;
            if same_entry(&source_metadata, &metadata) {
                return Err(failure("源与目标是同一文件"));
            }
            if !overwrite {
                return Err(failure("目标已存在（overwrite=false）"));
            }
            true
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(failure(e)),
    };
    let staged = Staged {
        dir: &destination_dir,
        name: format!(".jiaclaw-copy-{}", uuid::Uuid::new_v4()),
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = destination_dir
        .open_with(&staged.name, &options)
        .map_err(failure)?;
    // The extra byte detects growth after the metadata check, without copying
    // an unbounded source into either memory or the destination filesystem.
    let bytes = transfer(
        &mut input.take(crate::copy::COPY_MAX_BYTES + 1),
        &mut output,
    )
    .map_err(failure)?;
    if bytes > crate::copy::COPY_MAX_BYTES {
        return Err(failure("复制期间源文件超过 64 MiB"));
    }
    output.sync_all().map_err(failure)?;
    drop(output);
    // Cooperating writers retain the root lock. Recheck external substitutions;
    // publication operates on the held parent and never follows the final leaf.
    match destination_dir.symlink_metadata(name) {
        Ok(metadata) => {
            regular(&metadata)?;
            if same_entry(&source_metadata, &metadata) {
                return Err(failure("源与目标是同一文件"));
            }
            if !overwrite {
                return Err(failure("目标已存在（overwrite=false）"));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(failure(e)),
    }
    if overwrite {
        destination_dir
            .rename(&staged.name, &destination_dir, name)
            .map_err(failure)?;
    } else {
        destination_dir
            .hard_link(&staged.name, &destination_dir, name)
            .map_err(failure)?;
        destination_dir.remove_file(&staged.name).map_err(|e| {
            failure(format!(
                "复制已提交但暂存链接清理失败，请核对后再决定是否重试: {e}"
            ))
        })?;
    }
    drop(staged);
    sync_dir(&destination_dir).map_err(|e| {
        failure(format!(
            "复制已提交但目录同步失败，请核对后再决定是否重试: {e}"
        ))
    })?;
    Ok((bytes, overwritten))
}

/// Idempotent directory creation under the same lock as file mutations.
pub(crate) fn mkdir_directory(
    workspace: &Path,
    raw: &str,
    recursive: bool,
) -> Result<bool, JiaClawError> {
    if raw.len() > MAX_PATH_BYTES {
        return Err(failure("目录路径最多 1024 字节"));
    }
    let trimmed = raw.trim();
    let candidate = Path::new(trimmed);
    let is_root =
        !trimmed.is_empty() && candidate.components().all(|part| part == Component::CurDir);
    let path = if is_root { candidate } else { relative(raw)? };
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    if is_root {
        return Ok(false);
    }
    let dir = parent(&root, path, recursive)
        .map_err(|e| {
            if recursive {
                failure(format!("创建父目录失败，可能已创建部分目录，请核对: {e}"))
            } else {
                e
            }
        })?
        .ok_or_else(|| failure("父目录不存在（recursive/parents=false）"))?;
    let name = path.file_name().ok_or_else(|| failure("无效目录名"))?;
    let created = match dir.create_dir(name) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = dir.symlink_metadata(name).map_err(failure)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(failure("路径已存在且不是目录，禁止符号链接和特殊文件"));
            }
            false
        }
        Err(e) => return Err(failure(e)),
    };
    // This also rejects a static leaf symlink, including an internal one. An
    // O_NOFOLLOW directory open rejects substitution before acquiring the fd.
    let opened = descend(&dir, Path::new(name), false)?
        .ok_or_else(|| failure("创建期间目录已消失，请核对"))?;
    if created {
        sync_dir(&opened)
            .and_then(|()| sync_dir(&dir))
            .map_err(|e| failure(format!("目录已创建但同步失败，请核对后决定是否重试: {e}")))?;
    }
    Ok(created)
}

fn normalized_entry_path(raw: &str) -> Result<PathBuf, JiaClawError> {
    Ok(relative(raw)?
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect())
}

fn entry_kind(metadata: &Metadata) -> Result<crate::files::MoveKind, JiaClawError> {
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(crate::files::MoveKind::Dir)
    } else {
        regular(metadata)?;
        Ok(crate::files::MoveKind::File)
    }
}

fn same_entry(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false // move is unsupported on these targets, before any mutation.
    }
}

// Names must be individual validated path components. These safe OS wrappers
// use the held parents directly; never downgrade NOREPLACE to check + rename.
fn rename_entry(
    source: &Dir,
    source_name: &OsStr,
    destination: &Dir,
    destination_name: &OsStr,
    overwrite: bool,
) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if overwrite {
            rustix::fs::renameat(source, source_name, destination, destination_name)
        } else {
            rustix::fs::renameat_with(
                source,
                source_name,
                destination,
                destination_name,
                rustix::fs::RenameFlags::NOREPLACE,
            )
        }
        .map_err(io::Error::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (
            source,
            source_name,
            destination,
            destination_name,
            overwrite,
        );
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "原子移动仅支持 Linux/macOS",
        ))
    }
}

fn move_error(error: io::Error) -> JiaClawError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EXDEV) {
        return failure("跨文件系统移动被拒绝（EXDEV）；未复制或删除源/目标");
    }
    if error.kind() == io::ErrorKind::AlreadyExists {
        return failure("目标已存在或目录非空，原子移动被拒绝");
    }
    if error.kind() == io::ErrorKind::Unsupported
        || error.raw_os_error() == Some(libc::ENOSYS)
        || error.raw_os_error() == Some(libc::EINVAL)
    {
        return failure(format!(
            "原子移动不受支持或参数被系统拒绝；未使用复制/删除回退: {error}"
        ));
    }
    failure(format!("原子移动失败，未预先删除目标: {error}"))
}

/// Single-filesystem atomic rename; no destination removal or copy fallback.
pub(crate) fn move_entry(
    workspace: &Path,
    from: &str,
    to: &str,
    overwrite: bool,
) -> Result<(crate::files::MoveKind, bool), JiaClawError> {
    move_entry_with(workspace, from, to, overwrite, rename_entry)
}

fn move_entry_with(
    workspace: &Path,
    from: &str,
    to: &str,
    overwrite: bool,
    rename: impl FnOnce(&Dir, &OsStr, &Dir, &OsStr, bool) -> io::Result<()>,
) -> Result<(crate::files::MoveKind, bool), JiaClawError> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return Err(failure("原子移动仅支持 Linux/macOS；不使用非原子回退"));
    }
    let from = normalized_entry_path(from)?;
    let to = normalized_entry_path(to)?;
    if from == to {
        return Err(failure("源与目标是同一路径"));
    }
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let _lock = writer_lock(&root)?;
    let source =
        parent(&root, &from, false)?.ok_or_else(|| failure("源路径不存在（父目录不存在）"))?;
    let source_name = from.file_name().ok_or_else(|| failure("无效源文件名"))?;
    let source_metadata = source.symlink_metadata(source_name).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            failure("源路径不存在")
        } else {
            failure(e)
        }
    })?;
    let kind = entry_kind(&source_metadata)?;
    if kind == crate::files::MoveKind::Dir && to.starts_with(&from) {
        return Err(failure("拒绝将目录移动到自身子路径"));
    }
    // Compare every destination ancestor's actual identity, also covering
    // case-insensitive filesystem aliases that lexical prefixes cannot detect.
    let mut destination = root.try_clone().map_err(failure)?;
    for component in to.parent().unwrap_or_else(|| Path::new("")).components() {
        let Component::Normal(name) = component else {
            continue;
        };
        destination = descend(&destination, Path::new(name), false)?
            .ok_or_else(|| failure("目标父目录不存在"))?;
        if kind == crate::files::MoveKind::Dir
            && same_entry(
                &source_metadata,
                &destination.dir_metadata().map_err(failure)?,
            )
        {
            return Err(failure("拒绝将目录移动到自身子路径"));
        }
    }
    let destination_name = to.file_name().ok_or_else(|| failure("无效目标文件名"))?;
    let destination_metadata = match destination.symlink_metadata(destination_name) {
        Ok(metadata) => Some(metadata),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(failure(e)),
    };
    if let Some(metadata) = &destination_metadata {
        let destination_kind = entry_kind(metadata)?;
        if same_entry(&source_metadata, metadata) {
            return Err(failure("源与目标是同一路径或同一 inode"));
        }
        if !overwrite {
            return Err(failure("目标已存在（overwrite=false，拒绝覆盖）"));
        }
        if kind != destination_kind {
            return Err(failure("源与目标类型不一致，拒绝覆盖"));
        }
        // The kernel, not a traversal, decides whether a destination dir is empty.
    }
    let overwritten = destination_metadata.is_some();
    rename(
        &source,
        source_name,
        &destination,
        destination_name,
        overwrite,
    )
    .map_err(move_error)?;
    // The namespace change has committed. Sync both parents even if the first
    // sync fails, then report uncertainty without undoing or retrying the move.
    let source_sync = sync_dir(&source);
    let destination_sync = sync_dir(&destination);
    source_sync.and(destination_sync).map_err(|e| {
        failure(format!(
            "移动已提交但父目录同步失败，源/目标可能已改变，请核对后决定是否重试: {e}"
        ))
    })?;
    Ok((kind, overwritten))
}

// Metadata operations accept the workspace root but never a blank input.
pub(crate) fn info_relative(raw: &str) -> Result<PathBuf, JiaClawError> {
    if raw.len() > MAX_PATH_BYTES || raw.trim().is_empty() {
        return Err(failure("路径必须非空白，且最多 1024 字节"));
    }
    let path = Path::new(raw.trim());
    if path.components().all(|part| part == Component::CurDir) {
        return Ok(PathBuf::from("."));
    }
    normalized_entry_path(raw)
}

fn metadata_kind(metadata: &Metadata) -> &'static str {
    if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "dir"
    } else if metadata.is_file() && regular(metadata).is_ok() {
        "file"
    } else {
        "unsupported"
    }
}

fn unix_milliseconds(time: std::time::SystemTime) -> Option<i64> {
    use std::time::UNIX_EPOCH;
    let millis = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_millis()).ok()?,
        Err(error) => {
            let duration = error.duration();
            // Floor fractional milliseconds on both sides of the epoch.
            -i128::try_from(duration.as_millis()).ok()?
                - i128::from(duration.subsec_nanos() % 1_000_000 != 0)
        }
    };
    i64::try_from(millis).ok()
}

fn stat_metadata(path: String, metadata: Metadata) -> crate::filesystem_info::StatOutput {
    let kind = metadata_kind(&metadata);
    let modified_unix_ms = if kind == "file" || kind == "dir" {
        metadata
            .modified()
            .ok()
            .and_then(|time| unix_milliseconds(time.into_std()))
    } else {
        None
    };
    crate::filesystem_info::StatOutput {
        path,
        kind: kind.into(),
        size_bytes: (kind == "file").then_some(metadata.len()),
        modified_unix_ms,
    }
}

fn stat_leaf(
    dir: &Dir,
    name: &OsStr,
    path: String,
) -> Result<crate::filesystem_info::StatOutput, JiaClawError> {
    // lstat only: even a FIFO, dangling link, or large binary needs no open/read.
    Ok(stat_metadata(
        path,
        dir.symlink_metadata(name).map_err(failure)?,
    ))
}

pub(crate) fn stat_entry(
    workspace: &Path,
    raw: &str,
) -> Result<crate::filesystem_info::StatOutput, JiaClawError> {
    let path = info_relative(raw)?;
    let display = path.to_string_lossy().into_owned();
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    if path == Path::new(".") {
        return Ok(stat_metadata(
            display,
            root.dir_metadata().map_err(failure)?,
        ));
    }
    let dir = parent(&root, &path, false)?.ok_or_else(|| failure("父目录不存在"))?;
    stat_leaf(
        &dir,
        path.file_name().ok_or_else(|| failure("路径无效"))?,
        display,
    )
}

struct Listing {
    entries: Vec<crate::files::DirEntryInfo>,
    scanned: usize,
    bytes: usize,
    limit: usize,
    base: PathBuf,
    max_descent: usize,
    truncated: bool,
    deadline: std::time::Instant,
}
impl Listing {
    fn exhausted(&mut self) -> bool {
        if self.scanned >= DIRECTORY_SCAN_LIMIT
            || self.entries.len() >= self.limit
            || self.deadline <= std::time::Instant::now()
        {
            self.truncated = true;
            true
        } else {
            false
        }
    }
    fn walk(
        &mut self,
        dir: &Dir,
        prefix: &str,
        recursive: bool,
        depth: usize,
    ) -> Result<(), JiaClawError> {
        let mut children = Vec::new();
        for entry in dir.entries().map_err(failure)? {
            if self.exhausted() {
                break;
            }
            self.scanned += 1;
            let entry = entry.map_err(failure)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| failure("目录含非 UTF-8 文件名，无法无损表示"))?;
            children.push(name);
        }
        children.sort();
        for name in children {
            if self.entries.len() >= self.limit || self.deadline <= std::time::Instant::now() {
                self.truncated = true;
                break;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let complete_path = self.base.join(&rel);
            if complete_path.as_os_str().len() > MAX_PATH_BYTES
                || complete_path.components().count() > 64
            {
                self.truncated = true;
                continue;
            }
            let meta = dir.symlink_metadata(&name).map_err(failure)?;
            let kind = metadata_kind(&meta);
            let item = crate::files::DirEntryInfo {
                name: rel.clone(),
                kind: kind.into(),
                size: (kind == "file").then_some(meta.len()),
            };
            // The final response pretty-prints each item at four-space indent.
            // Include separators; the envelope reserves its extra array lines.
            let json = serde_json::to_string_pretty(&item).map_err(failure)?;
            let encoded = json.len() + json.lines().count() * 4 + 2;
            if self.bytes.saturating_add(encoded) > DIRECTORY_JSON_BUDGET {
                self.truncated = true;
                break;
            }
            self.bytes += encoded;
            self.entries.push(item);
            if recursive && kind == "dir" {
                if depth >= self.max_descent || self.exhausted() {
                    self.truncated = true;
                    continue;
                }
                let child = descend(dir, Path::new(&name), false)?
                    .ok_or_else(|| failure("扫描期间目录已消失"))?;
                self.walk(&child, &rel, true, depth + 1)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn list_directory(
    workspace: &Path,
    raw: &str,
    maximum: usize,
    recursive: bool,
) -> Result<(Vec<crate::files::DirEntryInfo>, bool), JiaClawError> {
    if raw.len() > MAX_PATH_BYTES {
        return Err(failure("目录路径最多 1024 字节"));
    }
    let path = if raw == "." || raw.is_empty() {
        Path::new(".")
    } else {
        relative(raw)?
    };
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let dir = descend(&root, path, false)?.ok_or_else(|| failure("目录不存在"))?;
    let envelope = crate::files::ListDirOutput {
        path: raw.into(),
        recursive,
        truncated: false,
        entries: Vec::new(),
    };
    let mut listing = Listing {
        entries: Vec::new(),
        scanned: 0,
        bytes: serde_json::to_string_pretty(&envelope)
            .map_err(failure)?
            .len()
            + 8,
        limit: maximum.clamp(1, crate::files::LIST_DIR_MAX_ENTRIES),
        base: path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect(),
        max_descent: DIRECTORY_DEPTH_LIMIT,
        truncated: false,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    listing.walk(&dir, "", recursive, 0)?;
    listing.entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((listing.entries, listing.truncated))
}

pub(crate) fn tree_directory(
    workspace: &Path,
    raw: &str,
    max_depth: usize,
    max_entries: usize,
) -> Result<crate::filesystem_info::TreeOutput, JiaClawError> {
    if !(1..=32).contains(&max_depth) || !(1..=1000).contains(&max_entries) {
        return Err(failure(
            "max_depth 必须为 1..=32；max_entries 必须为 1..=1000",
        ));
    }
    let path = info_relative(raw)?;
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let dir = descend(&root, &path, false)?.ok_or_else(|| failure("目录不存在"))?;
    let mut output = crate::filesystem_info::TreeOutput {
        path: path.to_string_lossy().into_owned(),
        max_depth,
        max_entries,
        truncated: false,
        entries: Vec::new(),
    };
    let mut listing = Listing {
        entries: Vec::new(),
        scanned: 0,
        bytes: serde_json::to_string_pretty(&output)
            .map_err(failure)?
            .len()
            + 8,
        limit: max_entries,
        base: if path == Path::new(".") {
            PathBuf::new()
        } else {
            path
        },
        max_descent: max_depth - 1,
        truncated: false,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    listing.walk(&dir, "", true, 0)?;
    output.entries = listing.entries;
    output.truncated = listing.truncated;
    Ok(output)
}

// Search has its own walker: listing's output/entry limits must not silently
// remove candidates before grep/glob can examine them.
pub(crate) const SEARCH_OUTPUT_BYTES: usize = 64 * 1024;
const SEARCH_READ_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct SearchBudget {
    scanned: usize,
    read_bytes: usize,
    deadline: std::time::Instant,
    truncated: bool,
}
impl SearchBudget {
    pub(crate) fn expired(&mut self) -> bool {
        if self.deadline <= std::time::Instant::now() {
            self.truncated = true;
            true
        } else {
            false
        }
    }
}

pub(crate) enum SearchText {
    Text(String),
    Skipped,
    Exhausted,
}

pub(crate) struct SearchFile<'a> {
    dir: &'a Dir,
    name: &'a OsStr,
    pub(crate) path: &'a str,
    explicit: bool,
}
impl SearchFile<'_> {
    fn skipped(&self, reason: &str) -> Result<SearchText, JiaClawError> {
        if self.explicit {
            Err(failure(reason))
        } else {
            Ok(SearchText::Skipped)
        }
    }

    pub(crate) fn read_text(
        &self,
        budget: &mut SearchBudget,
        limit: usize,
    ) -> Result<SearchText, JiaClawError> {
        let limit = limit.min(FILE_LIMIT);
        let file =
            open_regular(self.dir, self.name)?.ok_or_else(|| failure("扫描期间文件已消失"))?;
        if file.metadata().map_err(failure)?.len() > limit as u64 {
            return self.skipped(&format!("文件超过上限 {limit} 字节"));
        }
        self.read_admitted(file, budget, limit)
    }

    // Metadata admission is separate from this bounded stream read so a file
    // growing immediately after its size check cannot evade either budget.
    fn read_admitted(
        &self,
        mut file: impl Read,
        budget: &mut SearchBudget,
        limit: usize,
    ) -> Result<SearchText, JiaClawError> {
        let limit = limit.min(FILE_LIMIT);
        let mut bytes = Vec::with_capacity(limit.min(8192));
        let mut chunk = [0_u8; 8192];
        loop {
            if budget.expired() || budget.read_bytes >= SEARCH_READ_BYTES {
                budget.truncated = true;
                return Ok(SearchText::Exhausted);
            }
            let size = chunk
                .len()
                .min(limit + 1 - bytes.len())
                .min(SEARCH_READ_BYTES - budget.read_bytes);
            let n = file.read(&mut chunk[..size]).map_err(failure)?;
            budget.read_bytes += n;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.len() > limit {
                return self.skipped(&format!("读取期间文件超过上限 {limit} 字节"));
            }
        }
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => Ok(SearchText::Text(text)),
            _ => self.skipped("二进制文件，跳过搜索（NUL 或非 UTF-8）"),
        }
    }
}

fn walk_search(
    dir: &Dir,
    prefix: &str,
    depth: usize,
    budget: &mut SearchBudget,
    visit: &mut impl FnMut(SearchFile<'_>, &mut SearchBudget) -> Result<bool, JiaClawError>,
) -> Result<bool, JiaClawError> {
    let mut children = Vec::new();
    for entry in dir.entries().map_err(failure)? {
        if budget.expired() || budget.scanned >= DIRECTORY_SCAN_LIMIT {
            budget.truncated = true;
            break;
        }
        budget.scanned += 1;
        let name = entry
            .map_err(failure)?
            .file_name()
            .into_string()
            .map_err(|_| failure("目录含非 UTF-8 文件名，无法无损表示"))?;
        if name != ".git" {
            children.push(name);
        }
    }
    children.sort();
    for name in children {
        if budget.expired() {
            return Ok(true);
        }
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if rel.len() > MAX_PATH_BYTES || Path::new(&rel).components().count() > 64 {
            budget.truncated = true;
            continue;
        }
        let metadata = dir.symlink_metadata(&name).map_err(failure)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if depth >= DIRECTORY_DEPTH_LIMIT || budget.scanned >= DIRECTORY_SCAN_LIMIT {
                budget.truncated = true;
                continue;
            }
            let child = descend(dir, Path::new(&name), false)?
                .ok_or_else(|| failure("扫描期间目录已消失"))?;
            if walk_search(&child, &rel, depth + 1, budget, visit)? {
                return Ok(true);
            }
        } else if regular(&metadata).is_ok()
            && visit(
                SearchFile {
                    dir,
                    name: OsStr::new(&name),
                    path: &rel,
                    explicit: false,
                },
                budget,
            )?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn search_files(
    workspace: &Path,
    raw: &str,
    mut visit: impl FnMut(SearchFile<'_>, &mut SearchBudget) -> Result<bool, JiaClawError>,
) -> Result<bool, JiaClawError> {
    if raw.len() > MAX_PATH_BYTES {
        return Err(failure("搜索路径最多 1024 字节"));
    }
    let path = if raw.trim().is_empty() || raw.trim() == "." {
        Path::new(".")
    } else {
        relative(raw)?
    };
    let root = Dir::open_ambient_dir(workspace, ambient_authority()).map_err(failure)?;
    let mut budget = SearchBudget {
        scanned: 0,
        read_bytes: 0,
        truncated: false,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
    };
    let stopped = if path == Path::new(".") {
        walk_search(&root, "", 0, &mut budget, &mut visit)?
    } else {
        let dir = parent(&root, path, false)?.ok_or_else(|| failure("路径不存在"))?;
        let name = path.file_name().ok_or_else(|| failure("无效文件名"))?;
        let metadata = dir.symlink_metadata(name).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                failure("路径不存在")
            } else {
                failure(e)
            }
        })?;
        let rel = path
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let child =
                descend(&dir, Path::new(name), false)?.ok_or_else(|| failure("目录不存在"))?;
            walk_search(&child, &rel, 0, &mut budget, &mut visit)?
        } else {
            regular(&metadata)?;
            visit(
                SearchFile {
                    dir: &dir,
                    name,
                    path: &rel,
                    explicit: true,
                },
                &mut budget,
            )?
        }
    };
    Ok(stopped || budget.truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn workspace_lock_busy<T>(result: &Result<T, JiaClawError>) -> bool {
        matches!(result, Err(JiaClawError::ToolExecution(message))
            if message.starts_with("memory file: 安全/IO 错误: workspace write lock busy or unsupported: ")
                && message.ends_with(&format!("(os error {})", libc::EWOULDBLOCK)))
    }

    #[cfg(unix)]
    fn after_workspace_contention<T>(
        mut operation: impl FnMut() -> Result<T, JiaClawError>,
    ) -> Result<T, JiaClawError> {
        // Use the concurrent writer test's existing 200 x 2ms budget for
        // contention between live owners. Completing owners explicitly unlock.
        // The tool error already rendered its io::Error as text. Retry only the
        // fixed workspace-flock context and EWOULDBLOCK errno suffix; unsupported
        // locks and other I/O failures return immediately. Production remains nonblocking.
        let mut result = operation();
        for _ in 1..200 {
            if !workspace_lock_busy(&result) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
            result = operation();
        }
        result
    }
    #[cfg(unix)]
    fn settled_concurrent_writes<T: Send>(
        operation: impl Fn(usize) -> Result<T, JiaClawError> + Sync,
    ) -> Vec<(usize, Result<T, JiaClawError>)> {
        let start = std::sync::Barrier::new(13);
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for i in 0..12 {
                let start = &start;
                let operation = &operation;
                handles.push(scope.spawn(move || {
                    start.wait();
                    (i, operation(i))
                }));
            }
            start.wait();
            // Observe actual owner completion before any explicit retry. Busy
            // does not promise a peer's fsync will finish within a poll budget.
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        })
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_results_wait_for_the_actual_original_writer() {
        let ws = tempfile::tempdir().unwrap();
        write_file_bytes(ws.path(), "a", "original", false, 256).unwrap();
        let entered = std::sync::Barrier::new(12);
        let (rejected_tx, rejected_rx) = std::sync::mpsc::channel();
        let rejected_rx = std::sync::Mutex::new(rejected_rx);
        let outcomes = settled_concurrent_writes(|i| {
            if i == 0 {
                update_existing_text(ws.path(), "a", 256, |original| {
                    // The editor owns the real workspace lock until all other
                    // original callers have observed definite busy rejection.
                    entered.wait();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    for _ in 1..12 {
                        rejected_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(
                                deadline.saturating_duration_since(std::time::Instant::now()),
                            )
                            .unwrap();
                    }
                    Ok((format!("{original}[00]"), original.len() + 4))
                })
            } else {
                entered.wait();
                let result = write_file_bytes(ws.path(), "a", &format!("[{i:02}]"), true, 256);
                assert!(workspace_lock_busy(&result));
                rejected_tx.send(()).unwrap();
                result
            }
        });
        assert_eq!(outcomes.len(), 12);
        assert!(outcomes[0].1.is_ok());
        assert!(outcomes[1..]
            .iter()
            .all(|(_, result)| workspace_lock_busy(result)));
        assert_eq!(std::fs::read(ws.path().join("a")).unwrap(), b"original[00]");
        for (i, result) in outcomes {
            if result.is_err() {
                write_file_bytes(ws.path(), "a", &format!("[{i:02}]"), true, 256).unwrap();
            }
        }
        let text = std::fs::read_to_string(ws.path().join("a")).unwrap();
        assert_eq!(text.len(), 8 + 12 * 4);
        for i in 0..12 {
            assert_eq!(text.matches(&format!("[{i:02}]")).count(), 1);
        }
    }

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
        assert!(!initialize_workspace_file(ws.path(), "nested/a", "other", false).unwrap());
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
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let owner = writer_lock(&root).unwrap();
        // Busy is a permitted rejection before mutation, not a promise that
        // another writer's fsync will finish within a polling interval.
        let start = std::sync::Barrier::new(13);
        std::thread::scope(|scope| {
            for i in 0..12 {
                let path = ws.path();
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    let result = write_text(path, "a", &format!("record-{i:02}"), false, 32768);
                    assert!(workspace_lock_busy(&result));
                });
            }
            start.wait();
        });
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 0);
        drop(owner);

        // All twelve original callers compete on the actual kernel lock.
        // Retry only identified busy rejections, after every original owner
        // has joined; never repeat a successful or uncertain mutation.
        let outcomes = settled_concurrent_writes(|i| {
            write_text(ws.path(), "a", &format!("record-{i:02}"), false, 32768)
        });
        let mut pending = Vec::new();
        let committed = std::fs::read_to_string(ws.path().join("a")).unwrap();
        for (i, outcome) in outcomes {
            let occurrences = committed.matches(&format!("record-{i:02}")).count();
            match outcome {
                Ok(_) => assert_eq!(occurrences, 1),
                Err(error) => {
                    assert!(workspace_lock_busy(&Err::<(), _>(error)));
                    assert_eq!(occurrences, 0);
                    pending.push(i);
                }
            }
        }
        assert_eq!(
            committed.lines().filter(|line| !line.is_empty()).count(),
            12 - pending.len()
        );
        for i in pending {
            write_text(ws.path(), "a", &format!("record-{i:02}"), false, 32768).unwrap();
        }
        let text = std::fs::read_to_string(ws.path().join("a")).unwrap();
        assert_eq!(text.lines().filter(|s| !s.is_empty()).count(), 12);
        for i in 0..12 {
            assert_eq!(text.matches(&format!("record-{i:02}")).count(), 1);
        }
        let lock = after_workspace_contention(|| writer_lock(&root)).unwrap();
        assert!(workspace_lock_busy(&write_text(
            ws.path(),
            "a",
            "denied",
            false,
            32768,
        )));
        drop(lock);
        after_workspace_contention(|| write_text(ws.path(), "a", "after", false, 32768)).unwrap();
        let text = std::fs::read_to_string(ws.path().join("a")).unwrap();
        assert!(!text.contains("denied"));
        assert_eq!(text.lines().filter(|line| !line.is_empty()).count(), 13);
        assert_eq!(text.matches("after").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn writer_ownership_releases_lock_while_duplicated_descriptor_remains_open() {
        let ws = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let lock = after_workspace_contention(|| writer_lock(&root)).unwrap();
        // Real duplicated kernel descriptors model references retained by a
        // forked child before exec. A non-owner guard cannot unlock the owner.
        let retained = lock.file.try_clone().unwrap();
        let foreign_guard = WorkspaceWriteLock {
            file: retained.try_clone().unwrap(),
            owner_pid: lock.owner_pid.wrapping_add(1),
        };
        drop(foreign_guard);
        assert!(workspace_lock_busy(&writer_lock(&root)));
        drop(lock);
        assert!(retained.metadata().unwrap().is_dir());
        let lock = writer_lock(&root).unwrap();
        // Closing the old descriptor must not release the new owner's lock.
        drop(retained);
        assert!(workspace_lock_busy(&write_text(
            ws.path(),
            "a",
            "denied",
            false,
            32768,
        )));
        drop(lock);
        after_workspace_contention(|| write_text(ws.path(), "a", "committed", false, 32768))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.path().join("a")).unwrap(),
            "committed"
        );
        let unsupported: Result<(), JiaClawError> = Err(failure(format!(
            "workspace write lock busy or unsupported: {}",
            std::io::Error::from_raw_os_error(libc::ENOTSUP),
        )));
        assert!(!workspace_lock_busy(&unsupported));
        let other: Result<(), JiaClawError> = Err(failure(std::io::Error::from_raw_os_error(
            libc::EWOULDBLOCK,
        )));
        assert!(!workspace_lock_busy(&other));
        for error in [unsupported, other] {
            let Err(JiaClawError::ToolExecution(message)) = error else {
                panic!("expected the existing rendered tool error");
            };
            let mut calls = 0;
            let result = after_workspace_contention::<()>(|| {
                calls += 1;
                Err(JiaClawError::ToolExecution(message.clone()))
            });
            assert!(result.is_err());
            assert_eq!(calls, 1, "non-contention failure must not be retried");
        }
    }

    #[test]
    fn workspace_read_growth_consumes_only_limit_plus_one_bytes() {
        struct GrowingReader<'a> {
            file: std::fs::File,
            path: PathBuf,
            consumed: &'a mut usize,
        }
        impl Read for GrowingReader<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if *self.consumed == 0 {
                    // Growth occurs after metadata admission, before the first read.
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(&self.path)?
                        .write_all(&vec![b'x'; 4096])?;
                }
                let n = self.file.read(bytes)?;
                *self.consumed += n;
                Ok(n)
            }
        }
        let ws = tempfile::tempdir().unwrap();
        let path = ws.path().join("growing");
        std::fs::write(&path, b"seed").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 4);
        let mut consumed = 0;
        let error = read_bounded_bytes(
            GrowingReader {
                file,
                path,
                consumed: &mut consumed,
            },
            8,
        )
        .unwrap_err();
        assert!(error.to_string().contains("读取期间"));
        assert_eq!(consumed, 9);
    }

    #[test]
    fn workspace_listing_budgets_cover_escaped_json_scan_depth_and_deadline() {
        let ws = tempfile::tempdir().unwrap();
        for i in 0..400 {
            std::fs::write(
                ws.path().join(format!("{i:04}-{}", "\"\n\\".repeat(60))),
                b"x",
            )
            .unwrap();
        }
        let output = crate::files::list_workspace_dir(ws.path(), ".", 1000, false).unwrap();
        assert!(output.truncated);
        assert!(!output.entries.is_empty());
        assert!(serde_json::to_string_pretty(&output).unwrap().len() <= DIRECTORY_JSON_BUDGET);
        assert!(list_directory(ws.path(), &" ".repeat(1025), 1000, false).is_err());

        let flat = tempfile::tempdir().unwrap();
        for i in 0..DIRECTORY_SCAN_LIMIT + 1 {
            std::fs::write(flat.path().join(format!("f{i:04}")), b"").unwrap();
        }
        let dir = Dir::open_ambient_dir(flat.path(), ambient_authority()).unwrap();
        let mut listing = Listing {
            entries: Vec::new(),
            scanned: 0,
            bytes: 0,
            limit: 1000,
            base: PathBuf::new(),
            max_descent: DIRECTORY_DEPTH_LIMIT,
            truncated: false,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
        };
        listing.walk(&dir, "", false, 0).unwrap();
        assert_eq!(listing.scanned, DIRECTORY_SCAN_LIMIT);
        assert!(listing.truncated);
        assert!(listing.entries.len() <= 1000);
        let mut expired = Listing {
            entries: Vec::new(),
            scanned: 0,
            bytes: 0,
            limit: 1000,
            base: PathBuf::new(),
            max_descent: DIRECTORY_DEPTH_LIMIT,
            truncated: false,
            deadline: std::time::Instant::now(),
        };
        expired.walk(&dir, "", false, 0).unwrap();
        assert!(expired.truncated);
        assert_eq!(expired.scanned, 0);
        assert!(expired.entries.is_empty());

        let deep = tempfile::tempdir().unwrap();
        let mut path = deep.path().to_path_buf();
        for _ in 0..DIRECTORY_DEPTH_LIMIT + 3 {
            path.push("d");
            std::fs::create_dir(&path).unwrap();
        }
        std::fs::write(path.join("not-scanned"), b"x").unwrap();
        let (entries, truncated) = list_directory(deep.path(), ".", 1000, true).unwrap();
        assert!(truncated);
        assert_eq!(entries.len(), DIRECTORY_DEPTH_LIMIT + 1);
        assert!(!entries.iter().any(|e| e.name.contains("not-scanned")));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_files_reject_links_and_special_files_without_following_targets() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, b"untouched").unwrap();
        symlink(&secret, ws.path().join("symlink")).unwrap();
        std::fs::hard_link(&secret, ws.path().join("hardlink")).unwrap();
        symlink(outside.path(), ws.path().join("linked-dir")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(ws.path().join("fifo"))
            .status()
            .unwrap()
            .success());
        for name in ["symlink", "hardlink", "fifo"] {
            assert!(read_file_bytes(ws.path(), name, 32).is_err(), "{name}");
            assert!(
                write_file_bytes(ws.path(), name, "bad", false, 32).is_err(),
                "{name}"
            );
            assert!(
                write_file_bytes(ws.path(), name, "bad", true, 32).is_err(),
                "{name}"
            );
            assert!(
                replace_file_text(ws.path(), name, "untouched", "bad", false, 32).is_err(),
                "{name}"
            );
            assert!(delete_file(ws.path(), name).is_err(), "{name}");
            assert!(list_directory(ws.path(), name, 10, true).is_err(), "{name}");
        }
        assert!(write_file_bytes(ws.path(), "linked-dir/missing/leaf", "bad", false, 32).is_err());
        assert!(!outside.path().join("missing").exists());
        let (entries, truncated) = list_directory(ws.path(), ".", 20, true).unwrap();
        assert!(!truncated);
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().all(|e| e.size.is_none()));
        assert!(entries
            .iter()
            .all(|e| e.kind == "symlink" || e.kind == "unsupported"));
        assert_eq!(std::fs::read(&secret).unwrap(), b"untouched");
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn workspace_mutations_share_memory_lock_and_append_without_lost_bytes() {
        let ws = tempfile::tempdir().unwrap();
        write_file_bytes(ws.path(), "a", "original", false, 256).unwrap();
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let lock = writer_lock(&root).unwrap();
        assert!(workspace_lock_busy(&write_file_bytes(
            ws.path(),
            "a",
            "bad",
            false,
            256
        )));
        assert!(workspace_lock_busy(&write_file_bytes(
            ws.path(),
            "a",
            "bad",
            true,
            256
        )));
        assert!(workspace_lock_busy(&replace_file_text(
            ws.path(),
            "a",
            "original",
            "bad",
            false,
            256
        )));
        assert!(workspace_lock_busy(&delete_file(ws.path(), "a")));
        assert_eq!(std::fs::read(ws.path().join("a")).unwrap(), b"original");
        drop(lock);

        let outcomes = settled_concurrent_writes(|i| {
            write_file_bytes(ws.path(), "a", &format!("[{i:02}]"), true, 256)
        });
        let committed = std::fs::read_to_string(ws.path().join("a")).unwrap();
        let mut pending = Vec::new();
        for (i, outcome) in outcomes {
            let occurrences = committed.matches(&format!("[{i:02}]")).count();
            match outcome {
                Ok(_) => assert_eq!(occurrences, 1),
                Err(error) => {
                    assert!(workspace_lock_busy(&Err::<(), _>(error)));
                    assert_eq!(occurrences, 0);
                    pending.push(i);
                }
            }
        }
        assert_eq!(committed.len(), 8 + (12 - pending.len()) * 4);
        for i in pending {
            // Only definite pre-effect lock rejections are separate requests.
            // A successful or uncertain mutation is never replayed.
            write_file_bytes(ws.path(), "a", &format!("[{i:02}]"), true, 256).unwrap();
        }
        let text = std::fs::read_to_string(ws.path().join("a")).unwrap();
        assert_eq!(text.len(), 8 + 12 * 4);
        assert!(!text.contains('\n'));
        for i in 0..12 {
            assert_eq!(text.matches(&format!("[{i:02}]")).count(), 1);
        }
        assert!(replace_file_text(ws.path(), "a", "[", &"x".repeat(32), true, 256).is_err());
        assert_eq!(std::fs::read_to_string(ws.path().join("a")).unwrap(), text);
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 1);
    }

    fn search_budget() -> SearchBudget {
        SearchBudget {
            scanned: 0,
            read_bytes: 0,
            truncated: false,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
        }
    }

    #[test]
    fn search_counts_directories_and_skipped_entries_before_collecting() {
        let ws = tempfile::tempdir().unwrap();
        for i in 0..2001 {
            std::fs::create_dir(ws.path().join(format!("d{i:04}"))).unwrap();
        }
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let mut budget = search_budget();
        let mut visited = 0;
        walk_search(&dir, "", 0, &mut budget, &mut |_, _| {
            visited += 1;
            Ok(false)
        })
        .unwrap();
        assert_eq!(budget.scanned, DIRECTORY_SCAN_LIMIT);
        assert!(budget.truncated);
        assert_eq!(visited, 0);

        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        std::fs::write(ws.path().join(".git/hidden"), "needle").unwrap();
        std::fs::write(ws.path().join("visible"), "needle").unwrap();
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let mut budget = search_budget();
        let mut paths = Vec::new();
        walk_search(&dir, "", 0, &mut budget, &mut |f, _| {
            paths.push(f.path.to_string());
            Ok(false)
        })
        .unwrap();
        assert_eq!(budget.scanned, 2); // .git still consumes an entry.
        assert_eq!(paths, ["visible"]);
    }

    #[test]
    fn search_depth_path_and_deadline_are_checked_before_more_work() {
        let ws = tempfile::tempdir().unwrap();
        let mut path = ws.path().to_path_buf();
        for _ in 0..35 {
            path.push("d");
            std::fs::create_dir(&path).unwrap();
        }
        std::fs::write(path.join("beyond"), b"needle").unwrap();
        let mut visited = 0;
        assert!(search_files(ws.path(), ".", |_, _| {
            visited += 1;
            Ok(false)
        })
        .unwrap());
        assert_eq!(visited, 0);
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let mut budget = search_budget();
        budget.deadline = std::time::Instant::now();
        walk_search(&dir, "", 0, &mut budget, &mut |_, _| {
            panic!("expired walk called visitor")
        })
        .unwrap();
        assert_eq!(budget.scanned, 0);
        assert!(budget.truncated);

        let ws = tempfile::tempdir().unwrap();
        let mut dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        for _ in 0..4 {
            let name = "d".repeat(250);
            dir.create_dir(&name).unwrap();
            dir = dir.open_dir(&name).unwrap();
        }
        dir.create("x".repeat(30))
            .unwrap()
            .write_all(b"needle")
            .unwrap();
        assert!(search_files(ws.path(), ".", |_, _| panic!("overlong candidate visited")).unwrap());
    }

    #[test]
    fn search_reads_charge_actual_bytes_and_never_search_partial_files() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a"), b"needle").unwrap();
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let file = SearchFile {
            dir: &dir,
            name: OsStr::new("a"),
            path: "a",
            explicit: false,
        };
        let mut budget = search_budget();
        assert!(
            matches!(file.read_text(&mut budget, FILE_LIMIT).unwrap(), SearchText::Text(s) if s == "needle")
        );
        assert_eq!(budget.read_bytes, 6);
        budget.read_bytes = SEARCH_READ_BYTES - 3;
        assert!(matches!(
            file.read_text(&mut budget, FILE_LIMIT).unwrap(),
            SearchText::Exhausted
        ));
        assert_eq!(budget.read_bytes, SEARCH_READ_BYTES);
        assert!(budget.truncated);

        let mut budget = search_budget();
        std::fs::write(ws.path().join("a"), [0_u8, 255, 1]).unwrap();
        assert!(matches!(
            file.read_text(&mut budget, FILE_LIMIT).unwrap(),
            SearchText::Skipped
        ));
        assert_eq!(budget.read_bytes, 3); // Rejected binary bytes count too.
        std::fs::write(ws.path().join("a"), b"oversized").unwrap();
        assert!(matches!(
            file.read_text(&mut budget, 2).unwrap(),
            SearchText::Skipped
        ));
        assert_eq!(budget.read_bytes, 3); // Oversize metadata needs no body read.
        budget.deadline = std::time::Instant::now();
        assert!(matches!(
            file.read_text(&mut budget, FILE_LIMIT).unwrap(),
            SearchText::Exhausted
        ));
        assert_eq!(budget.read_bytes, 3);
    }

    #[cfg(unix)]
    #[test]
    fn search_retains_parent_capability_and_rejects_leaf_replacement() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("a"), b"outside secret").unwrap();
        std::fs::create_dir(ws.path().join("nested")).unwrap();
        std::fs::write(ws.path().join("nested/a"), b"inside").unwrap();
        let truncated = search_files(ws.path(), "nested", |file, budget| {
            std::fs::rename(ws.path().join("nested"), ws.path().join("retained")).unwrap();
            symlink(outside.path(), ws.path().join("nested")).unwrap();
            assert!(
                matches!(file.read_text(budget, FILE_LIMIT)?, SearchText::Text(s) if s == "inside")
            );
            Ok(false)
        })
        .unwrap();
        assert!(!truncated);
        let result = search_files(ws.path(), "retained/a", |file, budget| {
            std::fs::remove_file(ws.path().join("retained/a")).unwrap();
            symlink(outside.path().join("a"), ws.path().join("retained/a")).unwrap();
            file.read_text(budget, FILE_LIMIT)?;
            panic!("replaced leaf must not be followed");
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(outside.path().join("a")).unwrap(),
            b"outside secret"
        );
    }

    // APFS rejects such names at creation; Linux exercises the rejection path.
    #[cfg(target_os = "linux")]
    #[test]
    fn search_never_lossily_rewrites_invalid_names() {
        use std::os::unix::ffi::OsStringExt;
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(
            ws.path().join(std::ffi::OsString::from_vec(vec![0xff])),
            b"x",
        )
        .unwrap();
        let error =
            search_files(ws.path(), ".", |_, _| panic!("invalid name visited")).unwrap_err();
        assert!(error.to_string().contains("UTF-8"));
    }
    #[test]
    fn search_growth_after_metadata_is_bounded_and_charged() {
        struct GrowOnRead {
            file: File,
            writer: std::fs::File,
            first: bool,
        }
        impl Read for GrowOnRead {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.first {
                    self.writer.write_all(&[b'x'; 4096])?;
                    self.first = false;
                }
                self.file.read(bytes)
            }
        }
        let ws = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        for explicit in [false, true] {
            std::fs::write(ws.path().join("a"), b"seed").unwrap();
            let opened = open_regular(&dir, OsStr::new("a")).unwrap().unwrap();
            assert_eq!(opened.metadata().unwrap().len(), 4);
            let reader = GrowOnRead {
                file: opened,
                writer: std::fs::OpenOptions::new()
                    .append(true)
                    .open(ws.path().join("a"))
                    .unwrap(),
                first: true,
            };
            let file = SearchFile {
                dir: &dir,
                name: OsStr::new("a"),
                path: "a",
                explicit,
            };
            let mut budget = search_budget();
            let result = file.read_admitted(reader, &mut budget, 8);
            if explicit {
                assert!(result.err().unwrap().to_string().contains("读取期间"));
            } else {
                assert!(matches!(result.unwrap(), SearchText::Skipped));
            }
            assert_eq!(budget.read_bytes, 9);
        }
    }

    #[test]
    fn mkdir_mutation_is_idempotent_and_bounded() {
        let ws = tempfile::tempdir().unwrap();
        assert!(!mkdir_directory(ws.path(), ".", false).unwrap());
        assert!(!mkdir_directory(ws.path(), "./", true).unwrap());
        assert!(mkdir_directory(ws.path(), "parent/leaf", false)
            .unwrap_err()
            .to_string()
            .contains("父目录不存在"));
        assert!(!ws.path().join("parent").exists());
        assert!(mkdir_directory(ws.path(), "parent/leaf", true).unwrap());
        assert!(!mkdir_directory(ws.path(), "./parent/./leaf", true).unwrap());
        std::fs::write(ws.path().join("file"), b"preserved").unwrap();
        assert!(mkdir_directory(ws.path(), "file", false)
            .unwrap_err()
            .to_string()
            .contains("不是目录"));
        assert_eq!(std::fs::read(ws.path().join("file")).unwrap(), b"preserved");
        for raw in ["", "..", "../escape", "/absolute"] {
            assert!(mkdir_directory(ws.path(), raw, true).is_err(), "{raw}");
        }
        assert!(mkdir_directory(ws.path(), &"x".repeat(MAX_PATH_BYTES + 1), true).is_err());
        assert!(mkdir_directory(ws.path(), &vec!["a"; 65].join("/"), true).is_err());
        assert!(!ws.path().join("a").exists());
        // A valid bounded relative path may still exceed the filesystem's
        // single-name limit after ordinary parents have already been created.
        let partial = format!("partial/{}", "x".repeat(256));
        assert!(mkdir_directory(ws.path(), &partial, true).is_err());
        assert!(ws.path().join("partial").is_dir());
        assert_eq!(
            std::fs::read_dir(ws.path().join("partial"))
                .unwrap()
                .count(),
            0
        );
        assert!(mkdir_directory(ws.path(), "partial/reconciled", false).unwrap());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn move_mutation_preserves_entries_on_rejection_and_renames_whole_directories() {
        use crate::files::MoveKind;
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"new").unwrap();
        std::fs::write(ws.path().join("target"), b"old").unwrap();
        for (from, to, overwrite) in [
            ("source", "target", false),
            ("source", "./source", true),
            ("source", "missing/target", true),
            (".", "target", true),
        ] {
            assert!(move_entry(ws.path(), from, to, overwrite).is_err());
            assert_eq!(std::fs::read(ws.path().join("source")).unwrap(), b"new");
            assert_eq!(std::fs::read(ws.path().join("target")).unwrap(), b"old");
        }
        assert!(move_entry(ws.path(), "missing", "new", false)
            .unwrap_err()
            .to_string()
            .contains("源路径不存在"));
        let before = std::fs::metadata(ws.path().join("source")).unwrap();
        assert_eq!(
            move_entry(ws.path(), "source", "target", true).unwrap(),
            (MoveKind::File, true)
        );
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(ws.path().join("target")).unwrap().ino(),
            before.ino()
        );
        assert!(!ws.path().join("source").exists());
        assert_eq!(std::fs::read(ws.path().join("target")).unwrap(), b"new");
        std::fs::create_dir_all(ws.path().join("tree/nested")).unwrap();
        std::fs::write(ws.path().join("tree/nested/a"), b"tree content").unwrap();
        std::fs::create_dir(ws.path().join("empty")).unwrap();
        std::fs::create_dir(ws.path().join("nonempty")).unwrap();
        std::fs::write(ws.path().join("nonempty/b"), b"retain").unwrap();
        for target in ["nonempty", "target", "tree/nested/dest"] {
            assert!(move_entry(ws.path(), "tree", target, true).is_err());
        }
        assert_eq!(
            std::fs::read(ws.path().join("tree/nested/a")).unwrap(),
            b"tree content"
        );
        assert_eq!(
            std::fs::read(ws.path().join("nonempty/b")).unwrap(),
            b"retain"
        );
        // Moving a directory does not read or interpret its descendants.
        std::os::unix::fs::symlink("nested/a", ws.path().join("tree/link")).unwrap();
        assert_eq!(
            move_entry(ws.path(), "tree", "empty", true).unwrap(),
            (MoveKind::Dir, true)
        );
        assert!(!ws.path().join("tree").exists());
        assert_eq!(
            std::fs::read_link(ws.path().join("empty/link")).unwrap(),
            Path::new("nested/a")
        );
        assert_eq!(
            std::fs::read(ws.path().join("empty/nested/a")).unwrap(),
            b"tree content"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn mutations_reject_static_links_hardlinks_and_special_entries() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"outside").unwrap();
        std::fs::write(ws.path().join("source"), b"inside").unwrap();
        std::fs::create_dir(ws.path().join("directory")).unwrap();
        symlink(outside.path(), ws.path().join("parent_link")).unwrap();
        symlink("directory", ws.path().join("internal_link")).unwrap();
        symlink(outside.path().join("secret"), ws.path().join("leaf_link")).unwrap();
        symlink("absent", ws.path().join("dangling")).unwrap();
        std::fs::hard_link(outside.path().join("secret"), ws.path().join("hard")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(ws.path().join("fifo"))
            .status()
            .unwrap()
            .success());
        for path in [
            "parent_link/new",
            "internal_link/new",
            "leaf_link",
            "dangling",
            "hard",
            "fifo",
        ] {
            assert!(
                mkdir_directory(ws.path(), path, true).is_err(),
                "mkdir {path}"
            );
            assert!(
                move_entry(ws.path(), "source", path, true).is_err(),
                "dest {path}"
            );
            assert!(
                move_entry(ws.path(), path, "new", true).is_err(),
                "source {path}"
            );
        }
        assert_eq!(std::fs::read(ws.path().join("source")).unwrap(), b"inside");
        assert_eq!(
            std::fs::read(outside.path().join("secret")).unwrap(),
            b"outside"
        );
        assert!(!outside.path().join("new").exists());
        assert!(!ws.path().join("directory/new").exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn directory_mutations_share_the_file_writer_lock() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("a"), b"source").unwrap();
        std::fs::write(ws.path().join("b"), b"target").unwrap();
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let lock = writer_lock(&dir).unwrap();
        assert!(mkdir_directory(ws.path(), "new/leaf", true)
            .unwrap_err()
            .to_string()
            .contains("write lock busy"));
        assert!(move_entry(ws.path(), "a", "b", true)
            .unwrap_err()
            .to_string()
            .contains("write lock busy"));
        assert!(!ws.path().join("new").exists());
        assert_eq!(std::fs::read(ws.path().join("a")).unwrap(), b"source");
        assert_eq!(std::fs::read(ws.path().join("b")).unwrap(), b"target");
        drop(lock);
        assert!(mkdir_directory(ws.path(), "new/leaf", true).unwrap());
        assert!(move_entry(ws.path(), "a", "b", true).unwrap().1);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn move_noreplace_rejects_a_destination_created_after_preflight() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"source").unwrap();
        let result = move_entry_with(
            ws.path(),
            "source",
            "target",
            false,
            |src, from, dst, to, overwrite| {
                std::fs::write(ws.path().join("target"), b"competitor").unwrap();
                rename_entry(src, from, dst, to, overwrite)
            },
        );
        assert!(result.unwrap_err().to_string().contains("目标已存在"));
        assert_eq!(std::fs::read(ws.path().join("source")).unwrap(), b"source");
        assert_eq!(
            std::fs::read(ws.path().join("target")).unwrap(),
            b"competitor"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn atomic_noreplace_syscall_has_one_winner_and_preserves_loser() {
        let ws = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        std::fs::write(ws.path().join("a"), b"a").unwrap();
        std::fs::write(ws.path().join("b"), b"b").unwrap();
        // Exercise the kernel race without our cooperative workspace lock
        // serializing the contestants and concealing a check/rename bug.
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let barrier = barrier.clone();
                let dir = dir.try_clone().unwrap();
                std::thread::spawn(move || {
                    barrier.wait();
                    (
                        name,
                        rename_entry(&dir, OsStr::new(name), &dir, OsStr::new("winner"), false),
                    )
                })
            })
            .collect();
        let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        let winner = outcomes
            .iter()
            .find(|(_, result)| result.is_ok())
            .unwrap()
            .0;
        let (loser, error) = outcomes.iter().find(|(_, result)| result.is_err()).unwrap();
        assert_eq!(
            error.as_ref().unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(ws.path().join("winner")).unwrap(),
            winner.as_bytes()
        );
        assert!(!ws.path().join(winner).exists());
        assert_eq!(
            std::fs::read(ws.path().join(loser)).unwrap(),
            loser.as_bytes()
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn move_syscall_failures_never_delete_source_or_destination() {
        for (error, expected) in [
            (io::Error::from_raw_os_error(libc::EXDEV), "EXDEV"),
            (
                io::Error::new(io::ErrorKind::Unsupported, "fixture"),
                "不受支持",
            ),
            (io::Error::from_raw_os_error(libc::ENOSYS), "不受支持"),
            (io::Error::from_raw_os_error(libc::EACCES), "未预先删除"),
        ] {
            let ws = tempfile::tempdir().unwrap();
            std::fs::write(ws.path().join("a"), b"source").unwrap();
            std::fs::write(ws.path().join("b"), b"destination").unwrap();
            let mut calls = 0;
            let result = move_entry_with(ws.path(), "a", "b", true, |_, _, _, _, overwrite| {
                assert!(overwrite);
                calls += 1;
                Err(error)
            });
            assert_eq!(calls, 1);
            assert!(result.unwrap_err().to_string().contains(expected));
            assert_eq!(std::fs::read(ws.path().join("a")).unwrap(), b"source");
            assert_eq!(std::fs::read(ws.path().join("b")).unwrap(), b"destination");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn move_retains_parent_capabilities_when_ambient_parents_are_replaced() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join("src")).unwrap();
        std::fs::create_dir(ws.path().join("dst")).unwrap();
        std::fs::write(ws.path().join("src/a"), b"inside source").unwrap();
        std::fs::write(ws.path().join("dst/b"), b"inside destination").unwrap();
        std::fs::write(outside.path().join("a"), b"outside a").unwrap();
        std::fs::write(outside.path().join("b"), b"outside b").unwrap();
        let result = move_entry_with(
            ws.path(),
            "src/a",
            "dst/b",
            true,
            |src, from, dst, to, overwrite| {
                std::fs::rename(ws.path().join("src"), ws.path().join("held_src")).unwrap();
                std::fs::rename(ws.path().join("dst"), ws.path().join("held_dst")).unwrap();
                symlink(outside.path(), ws.path().join("src")).unwrap();
                symlink(outside.path(), ws.path().join("dst")).unwrap();
                rename_entry(src, from, dst, to, overwrite)
            },
        );
        assert!(result.unwrap().1);
        assert!(!ws.path().join("held_src/a").exists());
        assert_eq!(
            std::fs::read(ws.path().join("held_dst/b")).unwrap(),
            b"inside source"
        );
        assert_eq!(
            std::fs::read(outside.path().join("a")).unwrap(),
            b"outside a"
        );
        assert_eq!(
            std::fs::read(outside.path().join("b")).unwrap(),
            b"outside b"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn move_leaf_replacement_race_does_not_follow_a_symlink() {
        use std::os::unix::fs::symlink;
        for replace_source in [false, true] {
            let ws = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(ws.path().join("a"), b"source").unwrap();
            std::fs::write(ws.path().join("b"), b"destination").unwrap();
            std::fs::write(outside.path().join("secret"), b"outside").unwrap();
            move_entry_with(
                ws.path(),
                "a",
                "b",
                true,
                |src, from, dst, to, overwrite| {
                    let name = if replace_source { "a" } else { "b" };
                    std::fs::remove_file(ws.path().join(name)).unwrap();
                    symlink(outside.path().join("secret"), ws.path().join(name)).unwrap();
                    rename_entry(src, from, dst, to, overwrite)
                },
            )
            .unwrap();
            assert_eq!(
                std::fs::read(outside.path().join("secret")).unwrap(),
                b"outside"
            );
            assert!(!ws.path().join("a").exists());
            if replace_source {
                assert_eq!(
                    std::fs::read_link(ws.path().join("b")).unwrap(),
                    outside.path().join("secret")
                );
            } else {
                assert_eq!(std::fs::read(ws.path().join("b")).unwrap(), b"source");
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn move_rejects_case_aliases_of_same_inode_and_descendant() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join("Mixed/child")).unwrap();
        if !ws.path().join("mixed").exists() {
            eprintln!("case-sensitive filesystem: alias cases not applicable");
            return;
        }
        assert!(move_entry(ws.path(), "Mixed", "mixed", true)
            .unwrap_err()
            .to_string()
            .contains("同一"));
        assert!(move_entry(ws.path(), "Mixed", "mixed/child/new", true)
            .unwrap_err()
            .to_string()
            .contains("子路径"));
        assert!(ws.path().join("Mixed/child").is_dir());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_move_real_cross_volume_error_preserves_both_entries() {
        use cap_std::fs::MetadataExt;
        let source_tmp = tempfile::tempdir().unwrap();
        let Ok(destination_tmp) = tempfile::tempdir_in("/dev/shm") else {
            assert!(
                std::env::var_os("CI").is_none(),
                "CI requires writable /dev/shm for real EXDEV evidence"
            );
            eprintln!("cross-volume fixture skipped: /dev/shm unavailable");
            return;
        };
        let source = Dir::open_ambient_dir(source_tmp.path(), ambient_authority()).unwrap();
        let destination =
            Dir::open_ambient_dir(destination_tmp.path(), ambient_authority()).unwrap();
        if source.dir_metadata().unwrap().dev() == destination.dir_metadata().unwrap().dev() {
            assert!(
                std::env::var_os("CI").is_none(),
                "CI requires /dev/shm on a different filesystem for real EXDEV evidence"
            );
            eprintln!("cross-volume fixture skipped: /dev/shm shares temporary filesystem");
            return;
        }
        std::fs::write(source_tmp.path().join("a"), b"source").unwrap();
        std::fs::write(destination_tmp.path().join("b"), b"destination").unwrap();
        let error = rename_entry(
            &source,
            OsStr::new("a"),
            &destination,
            OsStr::new("b"),
            true,
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
        assert!(move_error(error).to_string().contains("EXDEV"));
        assert_eq!(
            std::fs::read(source_tmp.path().join("a")).unwrap(),
            b"source"
        );
        assert_eq!(
            std::fs::read(destination_tmp.path().join("b")).unwrap(),
            b"destination"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stat_is_metadata_only_for_large_binary_links_and_fifo_without_a_write_lock() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = ws.path().join("binary");
        std::fs::write(&path, [0xff, 0, 0x80]).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(32 * 1024 * 1024).unwrap();
        let before_epoch = std::time::UNIX_EPOCH - std::time::Duration::from_millis(1234);
        file.set_times(std::fs::FileTimes::new().set_modified(before_epoch))
            .unwrap();
        std::fs::write(outside.path().join("secret"), b"private").unwrap();
        symlink(outside.path().join("secret"), ws.path().join("link")).unwrap();
        symlink("missing", ws.path().join("dangling")).unwrap();
        symlink(outside.path(), ws.path().join("parent_link")).unwrap();
        std::fs::hard_link(outside.path().join("secret"), ws.path().join("hard")).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(ws.path().join("fifo"))
            .status()
            .unwrap()
            .success());
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let _writer = writer_lock(&root).unwrap();
        let output = stat_entry(ws.path(), "./binary").unwrap();
        assert_eq!(output.kind, "file");
        assert_eq!(output.size_bytes, Some(32 * 1024 * 1024));
        assert_eq!(output.modified_unix_ms, Some(-1234));
        for (name, kind) in [
            ("link", "symlink"),
            ("dangling", "symlink"),
            ("hard", "unsupported"),
            ("fifo", "unsupported"),
        ] {
            let output = serde_json::to_value(stat_entry(ws.path(), name).unwrap()).unwrap();
            assert_eq!(output, serde_json::json!({"path":name,"type":kind}));
        }
        let root_stat = stat_entry(ws.path(), ".").unwrap();
        assert_eq!(root_stat.kind, "dir");
        assert!(root_stat.size_bytes.is_none());
        assert!(root_stat.modified_unix_ms.is_some());
        assert!(stat_entry(ws.path(), "parent_link/secret").is_err());
        assert!(stat_entry(ws.path(), "missing").is_err());
        let tree = tree_directory(ws.path(), ".", 3, 200).unwrap();
        assert!(!tree.truncated);
        assert_eq!(tree.entries.len(), 6);
        assert_eq!(
            std::fs::read(outside.path().join("secret")).unwrap(),
            b"private"
        );
    }

    #[test]
    fn stat_timestamps_floor_pre_epoch_and_reject_unrepresentable_milliseconds() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(unix_milliseconds(UNIX_EPOCH), Some(0));
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH + Duration::from_nanos(1)),
            Some(0)
        );
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH - Duration::from_nanos(1)),
            Some(-1)
        );
        assert_eq!(
            unix_milliseconds(UNIX_EPOCH - Duration::from_micros(1500)),
            Some(-2)
        );
        if let Some(too_late) = UNIX_EPOCH.checked_add(Duration::from_millis(i64::MAX as u64 + 1)) {
            assert_eq!(unix_milliseconds(too_late), None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn metadata_and_tree_use_held_parent_descriptors_and_never_follow_replaced_leaves() {
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join("nested")).unwrap();
        std::fs::write(ws.path().join("nested/file"), b"inside").unwrap();
        std::fs::write(outside.path().join("file"), b"outside secret").unwrap();
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let held = descend(&root, Path::new("nested"), false).unwrap().unwrap();
        std::fs::rename(ws.path().join("nested"), ws.path().join("retained")).unwrap();
        symlink(outside.path(), ws.path().join("nested")).unwrap();
        assert_eq!(
            stat_leaf(&held, OsStr::new("file"), "nested/file".into())
                .unwrap()
                .size_bytes,
            Some(6)
        );
        let mut listing = Listing {
            entries: Vec::new(),
            scanned: 0,
            bytes: 0,
            limit: 100,
            base: PathBuf::from("nested"),
            max_descent: 2,
            truncated: false,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        };
        listing.walk(&held, "", true, 0).unwrap();
        assert_eq!(listing.entries[0].size, Some(6));
        std::fs::remove_file(ws.path().join("retained/file")).unwrap();
        symlink(outside.path().join("file"), ws.path().join("retained/file")).unwrap();
        let replaced = stat_leaf(&held, OsStr::new("file"), "nested/file".into()).unwrap();
        assert_eq!(replaced.kind, "symlink");
        assert!(replaced.size_bytes.is_none() && replaced.modified_unix_ms.is_none());
        assert!(stat_entry(ws.path(), "nested/file").is_err());
        assert!(tree_directory(ws.path(), "nested", 2, 100).is_err());
        assert_eq!(
            std::fs::read(outside.path().join("file")).unwrap(),
            b"outside secret"
        );
    }

    #[test]
    fn tree_preserves_dfs_and_applies_depth_and_entry_boundaries() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".git")).unwrap();
        std::fs::write(ws.path().join(".git/config"), b"").unwrap();
        std::fs::write(ws.path().join(".hidden"), b"").unwrap();
        std::fs::create_dir_all(ws.path().join("a/child/grandchild")).unwrap();
        std::fs::write(ws.path().join("a/z"), b"").unwrap();
        std::fs::write(ws.path().join("a.rs"), b"").unwrap();
        let full = tree_directory(ws.path(), ".", 4, 200).unwrap();
        let names: Vec<_> = full
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                ".git",
                ".git/config",
                ".hidden",
                "a",
                "a/child",
                "a/child/grandchild",
                "a/z",
                "a.rs"
            ]
        );
        assert!(!full.truncated);
        let limited = tree_directory(ws.path(), ".", 3, 200).unwrap();
        assert_eq!(limited.entries.len(), full.entries.len());
        assert!(
            limited.truncated,
            "depth-limited empty directory is conservatively incomplete"
        );
        let first = tree_directory(ws.path(), ".", 4, 1).unwrap();
        assert_eq!(first.entries.len(), 1);
        assert!(first.truncated);
        let flat = tree_directory(ws.path(), ".", 1, 200).unwrap();
        assert!(flat.truncated);
        assert!(flat.entries.iter().all(|entry| !entry.name.contains('/')));
        assert!(tree_directory(ws.path(), "a.rs", 3, 200).is_err());
        for (depth, entries) in [(0, 1), (33, 1), (1, 0), (1, 1001)] {
            assert!(tree_directory(ws.path(), ".", depth, entries).is_err());
        }
    }

    #[test]
    fn tree_budget_includes_escaped_envelope_and_full_workspace_paths_before_lstat() {
        let ws = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let selected = "\"\\".repeat(90);
        let dir = descend(&root, Path::new(&selected), true).unwrap().unwrap();
        for index in 0..400 {
            dir.write(format!("{index:04}{}", "\"\n\\".repeat(60)), b"")
                .unwrap();
        }
        let tree = tree_directory(ws.path(), &selected, 3, 1000).unwrap();
        assert!(tree.truncated);
        assert!(!tree.entries.is_empty());
        assert!(serde_json::to_string_pretty(&tree).unwrap().len() <= DIRECTORY_JSON_BUDGET);
        for base in [vec!["d"; 64].join("/"), vec!["b".repeat(250); 4].join("/")] {
            let dir = descend(&root, Path::new(&base), true).unwrap().unwrap();
            let name = "x".repeat(32);
            dir.write(&name, b"").unwrap();
            // Both listings omit entries that other workspace tools cannot
            // address within their full path byte/component limits.
            let tree = tree_directory(ws.path(), &base, 3, 1000).unwrap();
            assert!(tree.truncated);
            assert!(tree.entries.is_empty());
            let (entries, truncated) = list_directory(ws.path(), &base, 1000, false).unwrap();
            assert!(truncated && entries.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_refuses_hardlinks_and_special_entries_without_publishing() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"original").unwrap();
        std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        std::fs::hard_link(outside.path().join("sentinel"), ws.path().join("hardlink")).unwrap();
        for (from, to) in [("hardlink", "new"), ("source", "hardlink")] {
            assert!(copy_file(ws.path(), from, to, true).is_err());
        }
        assert!(!ws.path().join("new").exists());
        assert_eq!(
            std::fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
        std::os::unix::fs::symlink(outside.path(), ws.path().join("parent")).unwrap();
        assert!(copy_file(ws.path(), "source", "parent/new", true).is_err());
        assert!(copy_file(ws.path(), "parent/sentinel", "new", false).is_err());
        let fifo = ws.path().join("fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(copy_file(ws.path(), "fifo", "new", false).is_err());
        assert!(copy_file(ws.path(), "source", "fifo", true).is_err());
        assert!(!outside.path().join("new").exists());
        assert!(!std::fs::read_dir(ws.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".jiaclaw-copy-")
        }));
    }

    #[test]
    fn copy_full_path_byte_and_component_boundaries_apply_to_both_ends() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"bytes").unwrap();
        let parent = vec!["d"; 63].join("/");
        std::fs::create_dir_all(ws.path().join(&parent)).unwrap();
        let exact = format!("{parent}/s");
        assert_eq!(copy_file(ws.path(), "source", &exact, false).unwrap().0, 5);
        assert_eq!(copy_file(ws.path(), &exact, "again", false).unwrap().0, 5);
        let too_deep = format!("{parent}/d/s");
        std::fs::create_dir_all(ws.path().join(&parent).join("d")).unwrap();
        std::fs::write(ws.path().join(&too_deep), b"too deep").unwrap();
        assert!(copy_file(ws.path(), &too_deep, "new", false).is_err());
        assert!(copy_file(ws.path(), "source", &too_deep, true).is_err());
        assert_eq!(
            std::fs::read(ws.path().join(&too_deep)).unwrap(),
            b"too deep"
        );
        let parent = vec!["b".repeat(250); 4].join("/");
        let root = Dir::open_ambient_dir(ws.path(), ambient_authority()).unwrap();
        let dir = descend(&root, Path::new(&parent), true).unwrap().unwrap();
        let exact = format!("{parent}/{}", "s".repeat(20));
        assert_eq!(exact.len(), 1024);
        assert_eq!(copy_file(ws.path(), "source", &exact, false).unwrap().0, 5);
        assert_eq!(
            copy_file(ws.path(), &exact, "at-limit", false).unwrap().0,
            5
        );
        let beyond = format!("{exact}x");
        dir.write(format!("{}x", "s".repeat(20)), b"beyond")
            .unwrap();
        assert!(copy_file(ws.path(), &beyond, "new", false).is_err());
        assert!(copy_file(ws.path(), "source", &beyond, true).is_err());
        assert_eq!(dir.read(format!("{}x", "s".repeat(20))).unwrap(), b"beyond");
        assert!(!ws.path().join("new").exists());
    }

    #[test]
    fn copy_growth_reads_at_most_limit_plus_one_and_preserves_existing_destination() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"small").unwrap();
        std::fs::write(ws.path().join("destination"), b"preserved").unwrap();
        let mut observed = 0;
        let result = copy_file_with(ws.path(), "source", "destination", true, |input, output| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(ws.path().join("source"))?
                .set_len(crate::copy::COPY_MAX_BYTES + 100)?;
            observed = io::copy(input, output)?;
            Ok(observed)
        });
        assert!(result.unwrap_err().to_string().contains("复制期间"));
        assert_eq!(observed, crate::copy::COPY_MAX_BYTES + 1);
        assert_eq!(
            std::fs::read(ws.path().join("destination")).unwrap(),
            b"preserved"
        );
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn copy_retains_open_parent_capabilities_and_rejects_substituted_leaf() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for parent in ["src", "dst"] {
            std::fs::create_dir(ws.path().join(parent)).unwrap();
        }
        std::fs::write(ws.path().join("src/source"), b"held source").unwrap();
        std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        let copied = copy_file_with(
            ws.path(),
            "src/source",
            "dst/new",
            false,
            |input, output| {
                for parent in ["src", "dst"] {
                    std::fs::rename(
                        ws.path().join(parent),
                        ws.path().join(format!("{parent}-held")),
                    )?;
                    std::os::unix::fs::symlink(outside.path(), ws.path().join(parent))?;
                }
                io::copy(input, output)
            },
        )
        .unwrap();
        assert_eq!(copied, (11, false));
        assert_eq!(
            std::fs::read(ws.path().join("dst-held/new")).unwrap(),
            b"held source"
        );
        assert!(!outside.path().join("new").exists());
        let result = copy_file_with(
            ws.path(),
            "src-held/source",
            "dst-held/new",
            true,
            |input, output| {
                std::fs::remove_file(ws.path().join("dst-held/new"))?;
                std::os::unix::fs::symlink(
                    outside.path().join("sentinel"),
                    ws.path().join("dst-held/new"),
                )?;
                io::copy(input, output)
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
        assert_eq!(
            std::fs::read_dir(ws.path().join("dst-held"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn copy_no_clobber_race_and_transfer_failure_leave_no_staged_file() {
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"source").unwrap();
        let result = copy_file_with(
            ws.path(),
            "source",
            "destination",
            false,
            |input, output| {
                std::fs::write(ws.path().join("destination"), b"external winner")?;
                io::copy(input, output)
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(ws.path().join("destination")).unwrap(),
            b"external winner"
        );
        let result = copy_file_with(
            ws.path(),
            "source",
            "destination",
            true,
            |_input, output| {
                output.write_all(b"partial")?;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected transfer failure",
                ))
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(ws.path().join("destination")).unwrap(),
            b"external winner"
        );
        assert_eq!(std::fs::read_dir(ws.path()).unwrap().count(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_copy_keeps_blocking_capacity_and_writer_lock_until_publication() {
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source"), b"copy").unwrap();
        let work_path = ws.path().to_path_buf();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(run_blocking_with_slots(&SLOTS, move || {
            let result = copy_file_with(
                &work_path,
                "source",
                "destination",
                false,
                |input, output| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    io::copy(input, output)
                },
            );
            finished_tx.send(result).unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let denied = executed.clone();
        assert!(run_blocking_with_slots(&SLOTS, move || {
            denied.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await
        .is_err());
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(copy_file(ws.path(), "source", "second", false).is_err());
        assert!(write_file_bytes(ws.path(), "other", "blocked", false, FILE_LIMIT).is_err());
        assert!(!ws.path().join("destination").exists());
        assert!(!ws.path().join("second").exists());
        assert!(!ws.path().join("other").exists());
        release_tx.send(()).unwrap();
        assert_eq!(finished_rx.await.unwrap().unwrap(), (4, false));
        let permit = tokio::time::timeout(std::time::Duration::from_secs(2), SLOTS.acquire())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(ws.path().join("destination")).unwrap(),
            b"copy"
        );
        drop(permit);
        assert_eq!(
            run_blocking_with_slots(&SLOTS, move || copy_file(
                ws.path(),
                "source",
                "second",
                false
            ))
            .await
            .unwrap(),
            (4, false)
        );
    }
}
