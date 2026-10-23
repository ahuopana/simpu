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

    pub async fn home(&self) -> Result<String, SftpError> {
        Ok(self.inner.canonicalize(".").await?)
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
