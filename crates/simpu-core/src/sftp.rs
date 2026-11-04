//! SFTP browsing and resumable, cancellable file transfer with progress.

use crate::ssh::SshSession;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum SftpError {
    #[error("sftp: {0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("ssh: {0}")]
    Ssh(#[from] russh::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("transfer cancelled")]
    Cancelled,
    #[error("refusing unsafe file name from the server: {0:?}")]
    UnsafeName(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// A path component that is safe to create locally: not empty, not `.`/`..`, and free of
/// separators and characters Windows reserves. A malicious server must not be able to make
/// a download write outside the chosen folder.
pub fn safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
        && !name.ends_with([' ', '.'])
}

/// Files and directories below a root, as `/`-separated relative paths (parents before children).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub dirs: Vec<String>,
    pub files: Vec<(String, u64)>,
}

impl Tree {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|(_, n)| n).sum()
    }
}

const MAX_DEPTH: usize = 64;

/// Walk a local directory (symbolic links are skipped).
pub async fn walk_local(root: &Path) -> Result<Tree, SftpError> {
    let mut tree = Tree::default();
    let mut stack = vec![(root.to_path_buf(), String::new(), 0usize)];
    while let Some((dir, rel, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let mut rd = tokio::fs::read_dir(&dir).await?;
        let mut kids = Vec::new();
        while let Some(e) = rd.next_entry().await? {
            kids.push(e);
        }
        kids.sort_by_key(|e| e.file_name());
        for e in kids {
            let ft = e.file_type().await?;
            if ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_dir() {
                tree.dirs.push(child.clone());
                stack.push((e.path(), child, depth + 1));
            } else if ft.is_file() {
                tree.files.push((child, e.metadata().await?.len()));
            }
        }
    }
    tree.dirs.sort();
    tree.files.sort();
    Ok(tree)
}

pub struct Sftp {
    inner: SftpSession,
}

impl SshSession {
    pub async fn open_sftp(&self) -> Result<Sftp, SftpError> {
        let channel = self.open_channel().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(Sftp {
            inner: SftpSession::new(channel.into_stream()).await?,
        })
    }
}

impl Sftp {
    /// Directory listing, directories first, then by name.
    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, SftpError> {
        let mut out: Vec<Entry> = self
            .inner
            .read_dir(path)
            .await?
            .map(|e| {
                let m = e.metadata();
                Entry {
                    name: e.file_name(),
                    is_dir: e.file_type().is_dir(),
                    size: m.size.unwrap_or(0),
                    mtime: m.mtime,
                }
            })
            .filter(|e| e.name != "." && e.name != "..")
            .collect();
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    pub async fn is_dir(&self, path: &str) -> Result<bool, SftpError> {
        Ok(self.inner.metadata(path).await?.is_dir())
    }

    /// Walk a remote directory. Every name is checked with [`safe_component`].
    pub async fn walk_remote(&self, root: &str) -> Result<Tree, SftpError> {
        let mut tree = Tree::default();
        let mut stack = vec![(
            root.trim_end_matches('/').to_string(),
            String::new(),
            0usize,
        )];
        while let Some((dir, rel, depth)) = stack.pop() {
            if depth > MAX_DEPTH {
                continue;
            }
            for e in self.list(if dir.is_empty() { "/" } else { &dir }).await? {
                if !safe_component(&e.name) {
                    return Err(SftpError::UnsafeName(e.name));
                }
                let child = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if e.is_dir {
                    tree.dirs.push(child.clone());
                    stack.push((format!("{dir}/{}", e.name), child, depth + 1));
                } else {
                    tree.files.push((child, e.size));
                }
            }
        }
        tree.dirs.sort();
        tree.files.sort();
        Ok(tree)
    }

    async fn ensure_remote_dir(&self, path: &str) -> Result<(), SftpError> {
        if self.is_dir(path).await.unwrap_or(false) {
            return Ok(());
        }
        self.mkdir(path).await
    }

    /// Upload a local directory as `remote_root` (created if needed). Progress is aggregated
    /// over all files. Existing complete files are skipped by the per-file resume logic.
    pub async fn upload_tree(
        &self,
        local_root: &Path,
        remote_root: &str,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<u64, SftpError> {
        let tree = walk_local(local_root).await?;
        let total = tree.total();
        self.ensure_remote_dir(remote_root).await?;
        for d in &tree.dirs {
            self.ensure_remote_dir(&format!("{}/{d}", remote_root.trim_end_matches('/')))
                .await?;
        }
        on_progress(Progress { done: 0, total });
        let mut base = 0;
        for (rel, size) in &tree.files {
            let local = rel
                .split('/')
                .fold(local_root.to_path_buf(), |p, c| p.join(c));
            let remote = format!("{}/{rel}", remote_root.trim_end_matches('/'));
            self.upload(&local, &remote, true, cancel, |p| {
                on_progress(Progress {
                    done: base + p.done.min(*size),
                    total,
                })
            })
            .await?;
            base += size;
        }
        on_progress(Progress { done: total, total });
        Ok(total)
    }

    /// Download a remote directory into `local_root` (created if needed).
    pub async fn download_tree(
        &self,
        remote_root: &str,
        local_root: &Path,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<u64, SftpError> {
        let tree = self.walk_remote(remote_root).await?;
        let total = tree.total();
        tokio::fs::create_dir_all(local_root).await?;
        for d in &tree.dirs {
            let dir = d
                .split('/')
                .fold(local_root.to_path_buf(), |p, c| p.join(c));
            tokio::fs::create_dir_all(dir).await?;
        }
        on_progress(Progress { done: 0, total });
        let mut base = 0;
        for (rel, size) in &tree.files {
            let local = rel
                .split('/')
                .fold(local_root.to_path_buf(), |p, c| p.join(c));
            let remote = format!("{}/{rel}", remote_root.trim_end_matches('/'));
            self.download(&remote, &local, true, cancel, |p| {
                on_progress(Progress {
                    done: base + p.done.min(*size),
                    total,
                })
            })
            .await?;
            base += size;
        }
        on_progress(Progress { done: total, total });
        Ok(total)
    }

    pub async fn home(&self) -> Result<String, SftpError> {
        Ok(self.inner.canonicalize(".").await?)
    }

    /// Absolute form of `path` as the server sees it (`.` is the home directory).
    pub async fn canonical(&self, path: &str) -> Result<String, SftpError> {
        Ok(self.inner.canonicalize(path).await?)
    }

    pub async fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.inner.remove_dir(path).await?)
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.inner.create_dir(path).await?)
    }

    pub async fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        Ok(self.inner.remove_file(path).await?)
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        Ok(self.inner.rename(from, to).await?)
    }

    /// Download `remote` to `local`. With `resume`, continues after the bytes
    /// already present locally.
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        resume: bool,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<u64, SftpError> {
        let total = self.inner.metadata(remote).await?.size.unwrap_or(0);
        let mut src = self.inner.open(remote).await?;
        let mut done = 0;
        let mut dst = if resume && local.exists() {
            done = tokio::fs::metadata(local).await?.len().min(total);
            src.seek(std::io::SeekFrom::Start(done)).await?;
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(local)
                .await?
        } else {
            tokio::fs::File::create(local).await?
        };
        let mut buf = vec![0u8; CHUNK];
        on_progress(Progress { done, total });
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(SftpError::Cancelled);
            }
            let n = src.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n]).await?;
            done += n as u64;
            on_progress(Progress { done, total });
        }
        dst.flush().await?;
        Ok(done)
    }

    /// Upload `local` to `remote`. With `resume`, continues after the bytes
    /// already present on the server.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        resume: bool,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<u64, SftpError> {
        let total = tokio::fs::metadata(local).await?.len();
        let mut src = tokio::fs::File::open(local).await?;
        let mut done = 0;
        let mut flags = OpenFlags::CREATE | OpenFlags::WRITE;
        if resume {
            done = match self.inner.metadata(remote).await {
                Ok(m) => m.size.unwrap_or(0).min(total),
                Err(_) => 0,
            };
        }
        if done == 0 {
            flags |= OpenFlags::TRUNCATE;
        }
        let mut dst = self.inner.open_with_flags(remote, flags).await?;
        if done > 0 {
            src.seek(std::io::SeekFrom::Start(done)).await?;
            dst.seek(std::io::SeekFrom::Start(done)).await?;
        }
        let mut buf = vec![0u8; CHUNK];
        on_progress(Progress { done, total });
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(SftpError::Cancelled);
            }
            let n = src.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n]).await?;
            done += n as u64;
            on_progress(Progress { done, total });
        }
        dst.shutdown().await?;
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_components() {
        for ok in ["a.txt", "Report 2026", "ä.txt", ".hidden", "a-b_c"] {
            assert!(safe_component(ok), "{ok}");
        }
        for bad in [
            "", ".", "..", "a/b", "a\\b", "C:evil", "x\0y", "a:b", "con?", "trail.", "trail ",
            "<x>",
        ] {
            assert!(!safe_component(bad), "{bad:?}");
        }
    }
}
