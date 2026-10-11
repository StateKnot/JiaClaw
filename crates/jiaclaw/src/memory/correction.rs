// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operator-only correction of the configured memory, using the original writer.

use crate::memory_io;
use cap_std::fs::Dir;
use jiaclaw_core::JiaClawError;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// A complete bounded snapshot, including private memory contents.
#[derive(Debug, Serialize)]
pub struct MemorySnapshot {
    /// Configured workspace-relative target.
    pub path: String,
    /// Complete private UTF-8 contents; never truncated.
    pub content: String,
    /// SHA256 of the exact original bytes, including line endings.
    pub content_sha256: String,
    /// Complete byte count.
    pub bytes: usize,
}

/// A disk publication receipt; it does not attest to runtime or index contents.
#[derive(Debug, Serialize)]
pub struct MemoryReplacement {
    /// Configured workspace-relative target.
    pub path: String,
    /// Reviewed original byte digest.
    pub previous_sha256: String,
    /// Published replacement byte digest.
    pub content_sha256: String,
    /// Replacement bytes published.
    pub bytes_written: usize,
}

/// Full memory review and replacement under two explicit content conditions.
pub struct MemoryEditor {
    workspace: PathBuf,
    configured: String,
}

impl MemoryEditor {
    /// Does not create files, open a store, or initialize an Agent.
    #[must_use]
    pub fn new(workspace: &Path, configured: impl Into<String>) -> Self {
        Self {
            workspace: workspace.to_owned(),
            configured: configured.into(),
        }
    }

    /// Read all original bytes, never a truncated prompt excerpt.
    ///
    /// # Errors
    /// Missing, unsafe, non-UTF8 or over-32KiB memory is rejected.
    pub fn inspect(&self) -> Result<MemorySnapshot, JiaClawError> {
        let content = memory_io::read_text(
            &self.workspace,
            &self.configured,
            super::MEMORY_WRITE_MAX_BYTES,
            false,
        )?
        .ok_or_else(|| invalid("memory does not exist"))?
        .text;
        Ok(MemorySnapshot {
            path: self.configured.clone(),
            bytes: content.len(),
            content_sha256: digest(&content),
            content,
        })
    }

    /// Replace only the reviewed original with the reviewed candidate bytes.
    ///
    /// # Errors
    /// Invalid hashes, stale or unsafe inputs, no-op, writer contention and I/O
    /// errors are rejected. A post-rename sync error can already be committed.
    pub fn replace_from(
        &self,
        candidate: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<MemoryReplacement, JiaClawError> {
        let candidate = memory_io::relative(candidate)?;
        self.replace_with(expected, replacement, |root| {
            read(root, candidate).map(|(_, text)| text)
        })
    }

    /// Publish explicitly reviewed UTF-8 text using the same writer as file inputs.
    /// An empty string explicitly clears the current file; history and indexes remain.
    ///
    /// # Errors
    /// Over-32KiB text, invalid hashes, stale or unsafe targets, no-op, contention
    /// and I/O errors reject. A post-rename sync error can already be committed.
    pub fn replace_text(
        &self,
        content: String,
        expected: &str,
        replacement: &str,
    ) -> Result<MemoryReplacement, JiaClawError> {
        if content.len() > super::MEMORY_WRITE_MAX_BYTES {
            return Err(invalid("replacement exceeds 32KiB"));
        }
        self.replace_with(expected, replacement, |_| Ok(content))
    }

    fn replace_with(
        &self,
        expected: &str,
        replacement: &str,
        capture: impl FnOnce(&Dir) -> Result<String, JiaClawError>,
    ) -> Result<MemoryReplacement, JiaClawError> {
        if !valid_hash(expected) || !valid_hash(replacement) {
            return Err(invalid(
                "both conditions require 64 lowercase SHA256 digits",
            ));
        }
        let target = memory_io::relative(&self.configured)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let (dir, original) = read(root, target)?;
            if digest(&original) != expected {
                return Err(invalid("original memory hash changed; review again"));
            }
            let next = capture(root)?;
            if digest(&next) != replacement {
                return Err(invalid("replacement hash changed; review again"));
            }
            if original == next {
                return Err(invalid("replacement is unchanged; no publication"));
            }
            memory_io::publish(
                &dir,
                target
                    .file_name()
                    .ok_or_else(|| invalid("invalid target"))?,
                next.as_bytes(),
                true,
            )?;
            Ok(MemoryReplacement {
                path: self.configured.clone(),
                previous_sha256: expected.to_owned(),
                content_sha256: replacement.to_owned(),
                bytes_written: next.len(),
            })
        })
    }
}

fn read(root: &Dir, path: &Path) -> Result<(Dir, String), JiaClawError> {
    let dir =
        memory_io::parent(root, path, false)?.ok_or_else(|| invalid("parent does not exist"))?;
    let file = memory_io::open_regular(
        &dir,
        path.file_name().ok_or_else(|| invalid("invalid path"))?,
    )?
    .ok_or_else(|| invalid("file does not exist"))?;
    let text = memory_io::read_open(file, super::MEMORY_WRITE_MAX_BYTES, false)?.text;
    Ok((dir, text))
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn invalid(message: &str) -> JiaClawError {
    JiaClawError::ToolExecution(format!("memory correction: {message}"))
}

#[cfg(test)]
mod tests {
    use super::MemoryEditor;
    use sha2::{Digest, Sha256};
    use std::fs;

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn memory_text_correction_uses_raw_conditions_and_existing_target_only() {
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("memory"), "old\r\n").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        let old = editor.inspect().unwrap();
        let new = "new 茶\r\n";
        assert!(editor
            .replace_text(new.to_owned(), &old.content_sha256, &hash(b"different"))
            .is_err());
        assert_eq!(fs::read(ws.path().join("memory")).unwrap(), b"old\r\n");
        let receipt = editor
            .replace_text(new.to_owned(), &old.content_sha256, &hash(new.as_bytes()))
            .unwrap();
        assert_eq!(receipt.bytes_written, new.len());
        assert_eq!(editor.inspect().unwrap().content, new);
        assert!(editor
            .replace_text("later".to_owned(), &old.content_sha256, &hash(b"later"))
            .is_err());
        assert!(editor
            .replace_text(
                new.to_owned(),
                &receipt.content_sha256,
                &receipt.content_sha256
            )
            .is_err());
        fs::remove_file(ws.path().join("memory")).unwrap();
        assert!(editor
            .replace_text("new".to_owned(), &receipt.content_sha256, &hash(b"new"))
            .is_err());
        assert!(!ws.path().join("memory").exists());
    }

    #[test]
    fn memory_text_correction_accepts_full_budget_and_explicit_empty_text() {
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("memory"), "old").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        let text = "\0".repeat(32768);
        assert!(editor
            .replace_text("x".repeat(32769), &hash(b"old"), &hash(b"bad"))
            .is_err());
        editor
            .replace_text(text.clone(), &hash(b"old"), &hash(text.as_bytes()))
            .unwrap();
        assert_eq!(editor.inspect().unwrap().bytes, 32768);
        let receipt = editor
            .replace_text(String::new(), &hash(text.as_bytes()), &hash(b""))
            .unwrap();
        assert_eq!(receipt.bytes_written, 0);
        assert_eq!(fs::read(ws.path().join("memory")).unwrap(), b"");
    }

    #[test]
    fn memory_correction_preserves_later_edits_and_rejects_noop() {
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("memory"), "hello").unwrap();
        fs::write(ws.path().join("reviewed"), "world").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        let original = editor.inspect().unwrap();
        assert_eq!(original.content, "hello");
        assert_eq!(
            original.content_sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(editor
            .replace_from("reviewed", &original.content_sha256, &hash(b"drift"))
            .is_err());
        fs::write(ws.path().join("memory"), "later correction").unwrap();
        assert!(editor
            .replace_from("reviewed", &original.content_sha256, &hash(b"world"))
            .is_err());
        assert_eq!(
            fs::read(ws.path().join("memory")).unwrap(),
            b"later correction"
        );
        let current = editor.inspect().unwrap();
        let saved = editor
            .replace_from("reviewed", &current.content_sha256, &hash(b"world"))
            .unwrap();
        assert_eq!(saved.bytes_written, 5);
        assert_eq!(saved.previous_sha256, current.content_sha256);
        assert_eq!(editor.inspect().unwrap().content, "world");
        let inode = fs::metadata(ws.path().join("memory")).unwrap();
        assert!(editor
            .replace_from("reviewed", &hash(b"world"), &hash(b"world"))
            .is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                inode.ino(),
                fs::metadata(ws.path().join("memory")).unwrap().ino()
            );
        }
    }

    #[test]
    fn memory_correction_reads_full_bounded_utf8_and_allows_reviewed_empty() {
        let ws = tempfile::tempdir().unwrap();
        let text = "汉".repeat(10922) + "ab";
        fs::write(ws.path().join("memory"), &text).unwrap();
        fs::write(ws.path().join("empty"), "").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        assert_eq!(editor.inspect().unwrap().bytes, 32768);
        assert_eq!(editor.inspect().unwrap().content, text);
        editor
            .replace_from("empty", &hash(text.as_bytes()), &hash(b""))
            .unwrap();
        assert_eq!(editor.inspect().unwrap().content, "");
        fs::write(ws.path().join("large"), vec![b'x'; 32769]).unwrap();
        assert!(editor
            .replace_from("large", &hash(b""), &hash(&vec![b'x'; 32769]))
            .is_err());
        fs::write(ws.path().join("memory"), [0xff]).unwrap();
        assert!(editor.inspect().is_err());
        assert!(editor
            .replace_from("empty", &hash(&[0xff]), &hash(b""))
            .is_err());
    }

    #[test]
    fn memory_correction_requires_existing_paths_and_both_explicit_hashes() {
        let ws = tempfile::tempdir().unwrap();
        let editor = MemoryEditor::new(ws.path(), "nested/memory");
        assert!(editor.inspect().is_err());
        assert!(editor
            .replace_from("missing", &hash(b""), &hash(b"x"))
            .is_err());
        assert!(!ws.path().join("nested").exists());
        fs::write(ws.path().join("memory"), "hello").unwrap();
        fs::write(ws.path().join("reviewed"), "world").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        assert!(editor
            .replace_from("reviewed", "bad", &hash(b"world"))
            .is_err());
        assert!(editor
            .replace_from("reviewed", &hash(b"hello"), &hash(b"world").to_uppercase())
            .is_err());
        assert!(editor
            .replace_from("../outside", &hash(b"hello"), &hash(b"world"))
            .is_err());
        assert_eq!(fs::read(ws.path().join("memory")).unwrap(), b"hello");
    }

    #[cfg(unix)]
    #[test]
    fn memory_correction_rejects_linked_inputs_and_actual_writer_contention() {
        use fs2::FileExt;
        use std::os::unix::fs::symlink;
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("memory"), "hello").unwrap();
        fs::write(ws.path().join("reviewed"), "world").unwrap();
        let editor = MemoryEditor::new(ws.path(), "memory");
        let owner = fs::File::open(ws.path()).unwrap();
        FileExt::try_lock_exclusive(&owner).unwrap();
        assert!(editor
            .replace_from("reviewed", &hash(b"hello"), &hash(b"world"))
            .is_err());
        assert_eq!(editor.inspect().unwrap().content, "hello");
        FileExt::unlock(&owner).unwrap();
        fs::remove_file(ws.path().join("reviewed")).unwrap();
        symlink("memory", ws.path().join("reviewed")).unwrap();
        assert!(editor
            .replace_from("reviewed", &hash(b"hello"), &hash(b"hello"))
            .is_err());
        fs::remove_file(ws.path().join("reviewed")).unwrap();
        fs::hard_link(ws.path().join("memory"), ws.path().join("reviewed")).unwrap();
        assert!(editor.inspect().is_err());
        assert!(editor
            .replace_from("reviewed", &hash(b"hello"), &hash(b"hello"))
            .is_err());
        assert_eq!(fs::read(ws.path().join("memory")).unwrap(), b"hello");
    }
}
