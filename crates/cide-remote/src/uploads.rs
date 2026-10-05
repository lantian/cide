//! Connection-owned, bounded console uploads. A destination is supplied by the host, never the
//! phone. Files are streamed rather than accumulated in memory, and unfinished files disappear
//! when the connection dies. Completed files remain available to the harness and its transcript.
use cide_ipc::{SessionId, remote::ATTACHMENT_CHUNK};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(crate) const MAX_BYTES: u64 = 32 * 1024 * 1024;
struct Upload {
    session: SessionId,
    file: File,
    dir: PathBuf,
    path: PathBuf,
    size: u64,
    offset: u64,
}
#[derive(Default)]
pub(crate) struct Uploads {
    entries: HashMap<String, Upload>,
}

/// A filename is one component on both host platforms; control characters could become terminal
/// input when the resulting path is pasted, so they must be removed as well as path separators.
fn safe_name(name: &str) -> String {
    let name: String = name
        .chars()
        .take(120)
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '\'' | '"' | '`' | '$') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_start_matches('.');
    if name.is_empty() {
        "attachment".into()
    } else {
        name.into()
    }
}

fn directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("the upload directory is not a regular directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}
impl Uploads {
    pub fn begin(
        &mut self,
        session: SessionId,
        root: &Path,
        name: &str,
        size: u64,
    ) -> Result<String, String> {
        if size > MAX_BYTES {
            return Err("each console attachment must be at most 32 MB".into());
        }
        if self.entries.len() >= 2 {
            return Err("finish or cancel an upload before starting another".into());
        }
        directory(root.parent().ok_or("the upload directory has no parent")?)?;
        directory(root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = root.join(&id);
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(safe_name(name));
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) => {
                let _ = fs::remove_dir(&dir);
                return Err(e.to_string());
            }
        };
        self.entries.insert(
            id.clone(),
            Upload {
                session,
                file,
                dir,
                path,
                size,
                offset: 0,
            },
        );
        Ok(id)
    }
    pub fn session(&self, id: &str) -> Result<SessionId, String> {
        self.entries
            .get(id)
            .map(|u| u.session)
            .ok_or_else(|| "that upload is no longer active; retry it".into())
    }
    pub fn chunk(&mut self, id: &str, offset: u64, bytes: &[u8]) -> Result<u64, String> {
        let u = self
            .entries
            .get_mut(id)
            .ok_or("that upload is no longer active; retry it")?;
        if bytes.is_empty() || bytes.len() > ATTACHMENT_CHUNK as usize {
            return Err("invalid upload chunk size".into());
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or("invalid upload offset")?;
        if end > u.size {
            return Err("the upload exceeds its declared size".into());
        }
        if offset < u.offset && end <= u.offset {
            // A repeated acknowledged chunk is harmless only when its bytes are identical.
            u.file
                .seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut previous = vec![0; bytes.len()];
            u.file
                .read_exact(&mut previous)
                .map_err(|e| e.to_string())?;
            return if previous == bytes {
                Ok(u.offset)
            } else {
                Err("a repeated upload chunk changed".into())
            };
        }
        if offset != u.offset {
            return Err("upload chunks arrived out of order".into());
        }
        u.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        u.file.write_all(bytes).map_err(|e| e.to_string())?;
        u.offset = end;
        Ok(end)
    }
    pub fn finish(&mut self, id: &str) -> Result<String, String> {
        let u = self
            .entries
            .get_mut(id)
            .ok_or("that upload is no longer active; retry it")?;
        if u.offset != u.size {
            return Err("the upload is incomplete".into());
        }
        u.file.sync_all().map_err(|e| e.to_string())?;
        let path = u
            .path
            .to_str()
            .ok_or("the upload path cannot be pasted as text")?
            .to_owned();
        if path.chars().any(char::is_control) {
            return Err("the upload path contains terminal control characters".into());
        }
        self.entries.remove(id);
        Ok(path)
    }
    pub fn cancel(&mut self, id: &str) -> Result<(), String> {
        if let Some(u) = self.entries.remove(id) {
            drop(u.file);
            fs::remove_dir_all(u.dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl Drop for Uploads {
    fn drop(&mut self) {
        for (_, u) in self.entries.drain() {
            drop(u.file);
            let _ = fs::remove_dir_all(u.dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("cide-upload-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn root(&self) -> PathBuf {
            self.0.join(".cide/mobile-uploads")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn uploads_stream_check_offsets_and_keep_finished_files() {
        let temp = Temp::new();
        let mut uploads = Uploads::default();
        let root = temp.root();
        let id = uploads
            .begin(SessionId::new(), &root, "../../a\nfile.txt", 5)
            .unwrap();
        assert!(uploads.chunk(&id, 2, b"xx").is_err());
        assert_eq!(uploads.chunk(&id, 0, b"hel").unwrap(), 3);
        assert_eq!(uploads.chunk(&id, 0, b"hel").unwrap(), 3);
        assert!(uploads.chunk(&id, 0, b"bad").is_err());
        assert!(uploads.finish(&id).is_err());
        uploads.chunk(&id, 3, b"lo").unwrap();
        let path = PathBuf::from(uploads.finish(&id).unwrap());
        drop(uploads);
        assert_eq!(fs::read(&path).unwrap(), b"hello");
        assert!(path.starts_with(root));
        assert!(!path.to_str().unwrap().contains('\n'));
    }
    #[test]
    fn bounds_cancellation_and_disconnect_remove_partial_files() {
        let temp = Temp::new();
        let root = temp.root();
        let mut uploads = Uploads::default();
        let s = SessionId::new();
        assert!(uploads.begin(s, &root, "big", MAX_BYTES + 1).is_err());
        let a = uploads.begin(s, &root, "a", 1).unwrap();
        let b = uploads.begin(s, &root, "b", 1).unwrap();
        assert!(uploads.begin(s, &root, "c", 1).is_err());
        assert!(uploads.chunk(&a, 0, b"ab").is_err());
        assert!(
            uploads
                .chunk(&a, 0, &vec![0; ATTACHMENT_CHUNK as usize + 1])
                .is_err()
        );
        uploads.cancel(&a).unwrap();
        uploads.cancel(&a).unwrap();
        assert!(!root.join(a).exists());
        drop(uploads);
        assert!(!root.join(b).exists());
    }
    #[test]
    fn an_empty_file_can_finish_and_names_cannot_inject_terminal_input() {
        let temp = Temp::new();
        let mut uploads = Uploads::default();
        let id = uploads
            .begin(SessionId::new(), &temp.root(), "..", 0)
            .unwrap();
        assert!(uploads.finish(&id).unwrap().ends_with("/attachment"));
        assert_eq!(safe_name("x\u{1b}[31m/\"`$a"), "x_[31m____a");
    }
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_upload_directory() {
        let temp = Temp::new();
        let other = Temp::new();
        fs::create_dir(temp.0.join(".cide")).unwrap();
        std::os::unix::fs::symlink(&other.0, temp.root()).unwrap();
        assert!(
            Uploads::default()
                .begin(SessionId::new(), &temp.root(), "a", 0)
                .is_err()
        );
    }
}
