//! Filesystem-backed SFTP server handler rooted at a directory.

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

enum Open {
    File(std::fs::File),
    Dir { entries: Vec<File>, sent: bool },
}

pub struct FsSftp {
    pub root: PathBuf,
    handles: HashMap<String, Open>,
    next: u32,
}

impl FsSftp {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            handles: HashMap::new(),
            next: 0,
        }
    }

    fn real(&self, p: &str) -> PathBuf {
        self.root.join(p.trim_start_matches('/'))
    }

    fn handle(&mut self, o: Open) -> String {
        self.next += 1;
        let h = format!("h{}", self.next);
        self.handles.insert(h.clone(), o);
        h
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en".into(),
    }
}

fn attrs(m: &std::fs::Metadata) -> FileAttributes {
    FileAttributes {
        size: Some(m.len()),
        permissions: Some(if m.is_dir() { 0o040755 } else { 0o100644 }),
        mtime: m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as u32),
        ..Default::default()
    }
}

fn io_err(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _v: u32, _e: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _a: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut o = std::fs::OpenOptions::new();
        o.read(flags.contains(OpenFlags::READ) || !flags.contains(OpenFlags::WRITE))
            .write(flags.contains(OpenFlags::WRITE))
            .create(flags.contains(OpenFlags::CREATE))
            .truncate(flags.contains(OpenFlags::TRUNCATE));
        let f = o.open(self.real(&filename)).map_err(io_err)?;
        Ok(Handle {
            id,
            handle: self.handle(Open::File(f)),
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).map_err(io_err)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        f.write_all(&data).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = std::fs::metadata(self.real(&path)).map_err(io_err)?;
        Ok(Attrs {
            id,
            attrs: attrs(&m),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Open::File(f)) = self.handles.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        Ok(Attrs {
            id,
            attrs: attrs(&f.metadata().map_err(io_err)?),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut entries = vec![];
        for e in std::fs::read_dir(self.real(&path)).map_err(io_err)? {
            let e = e.map_err(io_err)?;
            let m = e.metadata().map_err(io_err)?;
            entries.push(File::new(
                e.file_name().to_string_lossy().to_string(),
                attrs(&m),
            ));
        }
        Ok(Handle {
            id,
            handle: self.handle(Open::Dir {
                entries,
                sent: false,
            }),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir { entries, sent }) if !*sent => {
                *sent = true;
                Ok(Name {
                    id,
                    files: std::mem::take(entries),
                })
            }
            Some(Open::Dir { .. }) => Err(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _a: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.real(&path)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.real(&path)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, from: String, to: String) -> Result<Status, Self::Error> {
        std::fs::rename(self.real(&from), self.real(&to)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let p = if path == "." || path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        Ok(Name {
            id,
            files: vec![File::dummy(p)],
        })
    }
}
