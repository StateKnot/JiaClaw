// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Leaf-file checks for administrator-owned state directories.
//!
//! Callers validate their directory, map I/O errors into their own domain, and
//! retain ownership locks. Database identity, recovery and SQLite descriptor
//! lifetimes stay with each store; these functions run before SQLite opens.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::Path,
};

pub(crate) fn inspect(path: &Path) -> io::Result<Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state files must be ordinary files without symlinks",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.nlink() != 1 || metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "state files require mode 0600 and exactly one hard link",
            ));
        }
    }
    Ok(metadata)
}

pub(crate) fn open_or_create(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            inspect(path)?;
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            options.open(path)?
        }
        Err(error) => return Err(error),
    };
    let metadata = inspect(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() || opened.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "state file changed during open",
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn reopening_preserves_private_bytes_and_returns_the_original_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state");
        let mut file = open_or_create(&path).unwrap();
        file.write_all(b"private receipt").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut reopened = open_or_create(&path).unwrap();
        let mut bytes = Vec::new();
        reopened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private receipt");
        assert_eq!(inspect(&path).unwrap().len(), bytes.len() as u64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = reopened.metadata().unwrap();
            let named = inspect(&path).unwrap();
            assert_eq!((opened.dev(), opened.ino()), (named.dev(), named.ino()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_insecure_and_special_leaves_are_rejected_without_changes() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt, net::UnixListener};
        for kind in [
            "symlink",
            "dangling",
            "hardlink",
            "permissions",
            "directory",
            "socket",
            "fifo",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let outside = temp.path().join("outside");
            let mut original = open_or_create(&outside).unwrap();
            original.write_all(b"never touch").unwrap();
            drop(original);
            let path = temp.path().join("state");
            let mut socket = None;
            match kind {
                "symlink" => symlink(&outside, &path).unwrap(),
                "dangling" => symlink(temp.path().join("absent"), &path).unwrap(),
                "hardlink" => fs::hard_link(&outside, &path).unwrap(),
                "permissions" => {
                    drop(open_or_create(&path).unwrap());
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                }
                "directory" => fs::create_dir(&path).unwrap(),
                "socket" => socket = Some(UnixListener::bind(&path).unwrap()),
                "fifo" => {
                    assert!(std::process::Command::new("mkfifo")
                        .arg(&path)
                        .status()
                        .unwrap()
                        .success());
                }
                _ => unreachable!(),
            }
            let before = fs::symlink_metadata(&path).unwrap();
            assert!(inspect(&path).is_err(), "accepted inspection: {kind}");
            assert!(open_or_create(&path).is_err(), "accepted open: {kind}");
            let after = fs::symlink_metadata(&path).unwrap();
            assert_eq!(before.permissions().mode(), after.permissions().mode());
            assert_eq!(before.len(), after.len());
            assert_eq!(fs::read(&outside).unwrap(), b"never touch");
            assert!(!temp.path().join("absent").exists());
            drop(socket);
        }
    }
}
