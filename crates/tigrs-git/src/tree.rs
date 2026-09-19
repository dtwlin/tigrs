// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Tree navigation and blob extraction via in-process `gix`.

use gix::ObjectId;
use gix::bstr::ByteSlice;
use std::path::Path;
use std::sync::Arc;
use tigrs_core::error::{Result, TigError};

pub use tigrs_core::ansi::strip_control_chars;

/// The type of entry in a Git tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeEntryKind {
    /// Directory / Subtree (`040000`)
    Tree,
    /// Regular file (`100644`)
    Blob,
    /// Executable file (`100755`)
    Executable,
    /// Symbolic link (`120000`)
    Link,
    /// Submodule / Gitlink (`160000`)
    Commit,
    /// Other or custom mode
    Other(u32),
}

impl TreeEntryKind {
    /// Returns true if this entry represents a directory/subtree.
    pub fn is_tree(&self) -> bool {
        matches!(self, Self::Tree)
    }

    /// Returns the standard Unix-style permission string (e.g., `drwxr-xr-x`).
    pub fn mode_str(&self) -> &'static str {
        match self {
            Self::Tree => "drwxr-xr-x",
            Self::Blob => "-rw-r--r--",
            Self::Executable => "-rwxr-xr-x",
            Self::Link => "lrwxrwxrwx",
            Self::Commit => "m---------",
            Self::Other(_) => "----------",
        }
    }
}

/// An entry in a Git tree directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Entry filename without path (e.g. `main.rs` or `view`).
    pub name: String,
    /// Full path from repository root (e.g. `crates/tigrs-ui/src/view`).
    pub path: String,
    /// Entry kind (tree, blob, executable, link, commit).
    pub kind: TreeEntryKind,
    /// Raw Git filemode (e.g., 0o100644).
    pub mode: u32,
    /// Object ID of the tree or blob.
    pub oid: ObjectId,
    /// File size in bytes for blobs, if known.
    pub size: Option<u64>,
}

impl TreeEntry {
    /// Returns true if this entry represents a directory/subtree.
    pub fn is_dir(&self) -> bool {
        self.kind.is_tree()
    }

    /// Returns the standard Unix-style permission string (e.g. `drwxr-xr-x`).
    pub fn mode_str(&self) -> &'static str {
        self.kind.mode_str()
    }

    /// Returns a human-friendly size representation (e.g. `1.2K`, `450B`, or `-` for trees).
    pub fn formatted_size(&self) -> String {
        if self.kind.is_tree() {
            return "-".to_string();
        }
        match self.size {
            None => "-".to_string(),
            Some(bytes) => format_human_size(bytes),
        }
    }
}

/// Format bytes into human-readable units (B, K, M, G).
pub fn format_human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes < KIB {
        format!("{bytes}B")
    } else if bytes < MIB {
        let whole = bytes / KIB;
        let frac = (bytes % KIB) * 10 / KIB;
        format!("{whole}.{frac}K")
    } else if bytes < GIB {
        let whole = bytes / MIB;
        let frac = (bytes % MIB) * 10 / MIB;
        format!("{whole}.{frac}M")
    } else {
        let whole = bytes / GIB;
        let frac = (bytes % GIB) * 10 / GIB;
        format!("{whole}.{frac}G")
    }
}

/// Directory listing at a specific path and commit.
#[derive(Debug, Clone)]
pub struct TreeListing {
    /// Commit ID this tree is inspected at.
    pub commit_oid: ObjectId,
    /// Current directory path relative to repository root (`""` for root).
    pub path: String,
    /// Parent directory path (`None` if at repository root).
    pub parent_path: Option<String>,
    /// Sorted list of directory entries.
    pub entries: Vec<TreeEntry>,
}

/// Raw or decoded content of a blob object.
#[derive(Debug, Clone)]
pub struct BlobContent {
    /// Blob Object ID.
    pub oid: ObjectId,
    /// Path of the file.
    pub path: String,
    /// Total byte size.
    pub size: usize,
    /// Whether the blob appears to be binary.
    pub is_binary: bool,
    /// Zero-allocation contiguous line buffer of text (empty if binary).
    pub lines: tigrs_core::LineBuffer,
}

/// Checks if a byte slice appears to be binary data (contains NUL within the first 8000 bytes).
pub fn is_binary_data(data: &[u8]) -> bool {
    let check_len = data.len().min(8000);
    memchr::memchr(0, &data[..check_len]).is_some()
}

/// Splits raw data into sanitized text lines (or empty if binary) using a contiguous `LineBuffer`.
pub fn decode_blob_lines(data: &[u8]) -> (bool, tigrs_core::LineBuffer) {
    tigrs_core::LineBuffer::from_raw_bytes(data)
}

/// Reads a directory tree at the specified path for a given commit.
pub fn read_tree_at_path(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    subpath: &str,
) -> Result<TreeListing> {
    read_tree_at_path_cancellable(
        repo,
        commit_oid,
        subpath,
        &tigrs_core::CancellationToken::none(),
    )
}

/// Cancellable variant of [`read_tree_at_path`].
pub fn read_tree_at_path_cancellable(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    subpath: &str,
    cancel: &tigrs_core::CancellationToken,
) -> Result<TreeListing> {
    cancel.check_cancelled()?;
    let commit = repo
        .find_object(commit_oid)
        .map_err(|e| TigError::Git(format!("Failed to find object {commit_oid}: {e}")))?
        .peel_to_kind(gix::object::Kind::Commit)
        .map_err(|e| {
            TigError::Git(format!(
                "Object {commit_oid} does not peel to a commit: {e}"
            ))
        })?
        .try_into_commit()
        .map_err(|_| TigError::Git(format!("Object {commit_oid} is not a commit")))?;

    let root_tree = commit
        .tree()
        .map_err(|e| TigError::Git(format!("Failed to read root tree: {e}")))?;

    let clean_path = crate::path_security::verify_relative_path(subpath)?;
    let target_tree = if clean_path.is_empty() {
        root_tree
    } else {
        let entry = root_tree
            .lookup_entry_by_path(&clean_path)
            .map_err(|e| TigError::Git(format!("Failed looking up path '{clean_path}': {e}")))?
            .ok_or_else(|| TigError::Git(format!("Path '{clean_path}' not found in commit")))?;

        if !entry.mode().is_tree() {
            return Err(TigError::Git(format!(
                "Path '{clean_path}' is not a directory"
            )));
        }

        entry
            .object()
            .map_err(|e| TigError::Git(format!("Failed to open tree object: {e}")))?
            .into_tree()
    };

    let mut entries = Vec::new();

    for (idx, entry_result) in target_tree.iter().enumerate() {
        if idx % 256 == 0 {
            cancel.check_cancelled()?;
        }
        let entry =
            entry_result.map_err(|e| TigError::Git(format!("Failed iterating tree: {e}")))?;

        let raw_mode = entry.mode().as_str();
        let mode_u32 = u32::from_str_radix(raw_mode, 8).unwrap_or(0);
        let kind = if entry.mode().is_tree() {
            TreeEntryKind::Tree
        } else if entry.mode().is_executable() {
            TreeEntryKind::Executable
        } else if entry.mode().is_blob() {
            TreeEntryKind::Blob
        } else if entry.mode().is_link() {
            TreeEntryKind::Link
        } else if entry.mode().is_commit() {
            TreeEntryKind::Commit
        } else {
            TreeEntryKind::Other(mode_u32)
        };

        let raw_filename = entry.filename().to_str_lossy();
        let filename = strip_control_chars(&raw_filename);
        let full_path = if clean_path.is_empty() {
            filename.to_string()
        } else {
            format!("{clean_path}/{filename}")
        };

        let oid = entry.id().detach();
        let size = if kind.is_tree() || kind == TreeEntryKind::Commit {
            None
        } else {
            repo.find_header(oid).ok().and_then(|h| {
                if h.kind() == gix::object::Kind::Blob {
                    Some(h.size())
                } else {
                    None
                }
            })
        };

        entries.push(TreeEntry {
            name: filename.into_owned(),
            path: full_path,
            kind,
            mode: mode_u32,
            oid,
            size,
        });
    }

    // Sort entries: directories first, then alphabetical by name
    entries.sort_by(|a, b| match (a.kind.is_tree(), b.kind.is_tree()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let parent_path = if clean_path.is_empty() {
        None
    } else {
        match Path::new(&clean_path).parent() {
            Some(p) if p.as_os_str().is_empty() => Some(String::new()),
            Some(p) => Some(p.to_string_lossy().to_string()),
            None => Some(String::new()),
        }
    };

    Ok(TreeListing {
        commit_oid,
        path: clean_path.clone(),
        parent_path,
        entries,
    })
}

/// Maximum blob size (in bytes) loaded into memory for viewing (50 MB).
pub const MAX_VIEW_BLOB_BYTES: usize = 50 * 1024 * 1024;

/// Reads a blob object by its OID.
pub fn read_blob(repo: &gix::Repository, oid: ObjectId, path: &str) -> Result<BlobContent> {
    let object = repo
        .find_object(oid)
        .map_err(|e| TigError::Git(format!("Failed to read object {oid} at '{path}': {e}")))?;

    if object.kind == gix::object::Kind::Commit {
        return Ok(BlobContent {
            oid,
            path: path.to_string(),
            size: 0,
            is_binary: false,
            lines: tigrs_core::LineBuffer::from(vec![format!("Subproject commit {oid}")]),
        });
    }
    if object.kind != gix::object::Kind::Blob {
        return Err(TigError::Git(format!(
            "Object {oid} at '{path}' is a {}, not a blob",
            object.kind
        )));
    }

    let size = object.data.len();
    if size > MAX_VIEW_BLOB_BYTES {
        return Ok(BlobContent {
            oid,
            path: path.to_string(),
            size,
            is_binary: true,
            lines: tigrs_core::LineBuffer::from(vec![format!(
                "[File exceeds 50 MB display limit: {} ({size} bytes)]",
                format_human_size(size as u64)
            )]),
        });
    }

    let data = object.data.clone();
    let (is_binary, lines) = decode_blob_lines(&data);

    Ok(BlobContent {
        oid,
        path: path.to_string(),
        size,
        is_binary,
        lines,
    })
}

/// Maximum blob size (16 MiB) decoded for diff context splicing (`]` / `[` / `+`).
const MAX_DIFF_CONTEXT_BLOB_BYTES: usize = 16 * 1024 * 1024;

/// Reads a blob object by its OID and splits into raw lines matching diff line splitting.
pub fn read_blob_raw_lines(repo: &gix::Repository, oid: ObjectId) -> Result<Arc<[Arc<str>]>> {
    let Ok(object) = repo.find_object(oid) else {
        return Ok(Arc::from([]));
    };

    if object.kind != gix::object::Kind::Blob
        || object.data.len() > MAX_DIFF_CONTEXT_BLOB_BYTES
        || is_binary_data(&object.data)
    {
        return Ok(Arc::from([]));
    }

    let raw_slices = crate::diff::split_lines(&object.data);
    let lines: Vec<Arc<str>> = raw_slices
        .into_iter()
        .map(|s| {
            let lossy = String::from_utf8_lossy(s);
            Arc::from(tigrs_core::ansi::strip_control_chars(&lossy).as_ref())
        })
        .collect();
    Ok(Arc::from(lines))
}

/// Reads a blob object at a specific path and commit.
pub fn read_blob_at_commit_path(
    repo: &gix::Repository,
    commit_oid: ObjectId,
    path: &str,
) -> Result<BlobContent> {
    let commit = repo
        .find_object(commit_oid)
        .map_err(|e| TigError::Git(format!("Failed to find object {commit_oid}: {e}")))?
        .peel_to_kind(gix::object::Kind::Commit)
        .map_err(|e| {
            TigError::Git(format!(
                "Object {commit_oid} does not peel to a commit: {e}"
            ))
        })?
        .try_into_commit()
        .map_err(|_| TigError::Git(format!("Object {commit_oid} is not a commit")))?;

    let tree = commit
        .tree()
        .map_err(|e| TigError::Git(format!("Failed to read tree: {e}")))?;

    let clean_path = crate::path_security::verify_relative_path(path)?;
    let entry = tree
        .lookup_entry_by_path(&clean_path)
        .map_err(|e| TigError::Git(format!("Failed looking up path '{clean_path}': {e}")))?
        .ok_or_else(|| TigError::Git(format!("Path '{clean_path}' not found in commit")))?;

    let oid = entry.id().detach();
    if entry.mode().is_commit() {
        return Ok(BlobContent {
            oid,
            path: clean_path,
            size: 0,
            is_binary: false,
            lines: tigrs_core::LineBuffer::from(vec![format!("Subproject commit {oid}")]),
        });
    }
    read_blob(repo, oid, &clean_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn create_test_repo() -> (tempfile::TempDir, gix::Repository, ObjectId) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git");
            assert!(status.success());
        };

        run(&["init"]);
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@example.com"]);

        fs::write(path.join("README.md"), "# Hello\nWorld\n").expect("write");
        fs::create_dir_all(path.join("src/nested")).expect("mkdir");
        fs::write(path.join("src/main.rs"), "fn main() {}\n").expect("write");
        fs::write(path.join("src/nested/mod.rs"), "// nested\n").expect("write");
        fs::write(path.join("binary.bin"), [0u8, 1, 2, 3, 0, 4]).expect("write");

        run(&["add", "."]);
        run(&["commit", "-m", "initial commit"]);

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .expect("rev-parse");
        let head_str = String::from_utf8(output.stdout).unwrap().trim().to_string();
        let head_oid = ObjectId::from_hex(head_str.as_bytes()).unwrap();

        let repo = gix::open(path).expect("open repo");
        (dir, repo, head_oid)
    }

    #[test]
    fn test_format_human_size() {
        assert_eq!(format_human_size(500), "500B");
        assert_eq!(format_human_size(1024), "1.0K");
        assert_eq!(format_human_size(2048), "2.0K");
        assert_eq!(format_human_size(1024 * 1024), "1.0M");
        assert_eq!(format_human_size(1024 * 1024 * 1024), "1.0G");
    }

    #[test]
    fn test_is_binary_data() {
        assert!(!is_binary_data(b"hello world\nthis is text"));
        assert!(is_binary_data(b"hello\0world"));
    }

    #[test]
    fn test_read_tree_root_and_subdirectories() {
        let (_dir, repo, commit_oid) = create_test_repo();

        // Root tree
        let root = read_tree_at_path(&repo, commit_oid, "").expect("read root tree");
        assert_eq!(root.path, "");
        assert_eq!(root.parent_path, None);

        // Directories sorted before files: `src` should be first
        assert_eq!(root.entries[0].name, "src");
        assert!(root.entries[0].kind.is_tree());
        assert_eq!(root.entries[0].mode_str(), "drwxr-xr-x");
        assert_eq!(root.entries[0].formatted_size(), "-");

        let names: Vec<&str> = root.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"README.md"));
        assert!(names.contains(&"binary.bin"));

        // Subtree `src`
        let src_tree = read_tree_at_path(&repo, commit_oid, "src").expect("read src tree");
        assert_eq!(src_tree.path, "src");
        assert_eq!(src_tree.parent_path, Some(String::new()));
        assert_eq!(src_tree.entries[0].name, "nested");
        assert!(src_tree.entries[0].kind.is_tree());
        assert_eq!(src_tree.entries[1].name, "main.rs");
        assert_eq!(src_tree.entries[1].path, "src/main.rs");

        // Subtree `src/nested`
        let nested_tree =
            read_tree_at_path(&repo, commit_oid, "src/nested").expect("read nested tree");
        assert_eq!(nested_tree.path, "src/nested");
        assert_eq!(nested_tree.parent_path, Some("src".to_string()));
        assert_eq!(nested_tree.entries[0].name, "mod.rs");
        assert_eq!(nested_tree.entries[0].path, "src/nested/mod.rs");
    }

    #[test]
    fn test_read_blob_content() {
        let (_dir, repo, commit_oid) = create_test_repo();

        let readme =
            read_blob_at_commit_path(&repo, commit_oid, "README.md").expect("read README.md");
        assert!(!readme.is_binary);
        assert_eq!(readme.lines, vec!["# Hello", "World"]);
        assert_eq!(readme.size, 14);

        let binary =
            read_blob_at_commit_path(&repo, commit_oid, "binary.bin").expect("read binary.bin");
        assert!(binary.is_binary);
        assert!(binary.lines.is_empty());
        assert_eq!(binary.size, 6);
    }

    #[test]
    fn test_format_human_size_units() {
        assert_eq!(format_human_size(0), "0B");
        assert_eq!(format_human_size(500), "500B");
        assert_eq!(format_human_size(1023), "1023B");
        assert_eq!(format_human_size(1024), "1.0K");
        assert_eq!(format_human_size(1536), "1.5K");
        assert_eq!(format_human_size(1024 * 1024), "1.0M");
        assert_eq!(format_human_size(1024 * 1024 * 1024), "1.0G");
        assert_eq!(
            format_human_size(5 * 1024 * 1024 * 1024 + (1024 * 1024 * 1024 / 2)),
            "5.5G"
        );
    }

    #[test]
    fn test_tree_entry_kind_and_mode_str() {
        assert!(TreeEntryKind::Tree.is_tree());
        assert_eq!(TreeEntryKind::Tree.mode_str(), "drwxr-xr-x");

        assert!(!TreeEntryKind::Blob.is_tree());
        assert_eq!(TreeEntryKind::Blob.mode_str(), "-rw-r--r--");

        assert!(!TreeEntryKind::Executable.is_tree());
        assert_eq!(TreeEntryKind::Executable.mode_str(), "-rwxr-xr-x");

        assert!(!TreeEntryKind::Link.is_tree());
        assert_eq!(TreeEntryKind::Link.mode_str(), "lrwxrwxrwx");

        assert!(!TreeEntryKind::Commit.is_tree());
        assert_eq!(TreeEntryKind::Commit.mode_str(), "m---------");

        assert!(!TreeEntryKind::Other(0).is_tree());
        assert_eq!(TreeEntryKind::Other(0).mode_str(), "----------");
    }

    #[test]
    fn test_tree_entry_formatted_size() {
        let dummy_oid = ObjectId::null(gix::hash::Kind::Sha1);
        let tree_entry = TreeEntry {
            name: "dir".to_string(),
            path: "dir".to_string(),
            kind: TreeEntryKind::Tree,
            mode: 0o040_000,
            oid: dummy_oid,
            size: Some(4096),
        };
        assert!(tree_entry.is_dir());
        assert_eq!(tree_entry.formatted_size(), "-");

        let blob_no_size = TreeEntry {
            name: "file.txt".to_string(),
            path: "file.txt".to_string(),
            kind: TreeEntryKind::Blob,
            mode: 0o100_644,
            oid: dummy_oid,
            size: None,
        };
        assert!(!blob_no_size.is_dir());
        assert_eq!(blob_no_size.formatted_size(), "-");

        let blob_with_size = TreeEntry {
            name: "file.txt".to_string(),
            path: "file.txt".to_string(),
            kind: TreeEntryKind::Blob,
            mode: 0o100_644,
            oid: dummy_oid,
            size: Some(2048),
        };
        assert_eq!(blob_with_size.formatted_size(), "2.0K");
    }

    #[test]
    fn test_decode_blob_lines() {
        let (is_bin, lines) = decode_blob_lines(b"binary\0file");
        assert!(is_bin);
        assert!(lines.is_empty());

        let (is_bin, lines) = decode_blob_lines(b"line 1\nline 2\r\nline 3\n");
        assert!(!is_bin);
        assert_eq!(lines, vec!["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn test_read_tree_and_blob_not_found() {
        let (_dir, repo, commit_oid) = create_test_repo();

        let non_existent_tree = read_tree_at_path(&repo, commit_oid, "does/not/exist");
        assert!(non_existent_tree.is_err());

        let non_existent_blob = read_blob_at_commit_path(&repo, commit_oid, "does/not/exist.txt");
        assert!(non_existent_blob.is_err());

        // Calling read_tree_at_path on a file path should fail with "is not a directory"
        let file_as_tree = read_tree_at_path(&repo, commit_oid, "README.md");
        assert!(file_as_tree.is_err());
        assert!(
            file_as_tree
                .unwrap_err()
                .to_string()
                .contains("is not a directory")
        );
    }

    #[test]
    fn test_tree_with_executable_and_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let run = |args: &[&str]| {
            let status = Command::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .current_dir(path)
                .status()
                .expect("git");
            assert!(status.success());
        };

        run(&["init"]);
        run(&["config", "user.name", "Tester"]);
        run(&["config", "user.email", "tester@example.com"]);

        fs::write(path.join("file.txt"), "regular\n").unwrap();
        fs::write(path.join("exec.sh"), "#!/bin/sh\necho hi\n").unwrap();
        run(&["update-index", "--add", "--chmod=+x", "exec.sh"]);

        std::os::unix::fs::symlink("file.txt", path.join("link.txt")).unwrap();
        run(&["add", "file.txt", "link.txt"]);
        run(&["commit", "-m", "add files"]);

        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .unwrap();
        let head_oid =
            ObjectId::from_hex(String::from_utf8(output.stdout).unwrap().trim().as_bytes())
                .unwrap();
        let repo = gix::open(path).unwrap();

        let tree = read_tree_at_path(&repo, head_oid, "").unwrap();
        let exec_entry = tree.entries.iter().find(|e| e.name == "exec.sh").unwrap();
        assert_eq!(exec_entry.kind, TreeEntryKind::Executable);
        assert_eq!(exec_entry.mode_str(), "-rwxr-xr-x");

        let link_entry = tree.entries.iter().find(|e| e.name == "link.txt").unwrap();
        assert_eq!(link_entry.kind, TreeEntryKind::Link);
        assert_eq!(link_entry.mode_str(), "lrwxrwxrwx");
    }
}
