//! `log.jsonl`: the record of what happened, one line per fact.
//!
//! # Why a log and not a database
//!
//! SQLite would answer every question here and it is a C build, a new line in the root
//! `Cargo.toml` and a second storage idiom in a workspace with none so far. What the store needs
//! from it is small: append a batch, replay at open, and rewrite whole during retention. That is
//! a file. A row is ~200 bytes of JSON, which replays at a million rows in a second or two on a
//! worker thread, and being JSON means a user who has lost something can read it with `grep`.
//!
//! # What makes an append safe
//!
//! A line is committed when its newline is durable. Everything else follows from that:
//!
//! * A crash mid-append leaves a line with no newline. [`replay`] stops before it and the store
//!   truncates there, because the next append would otherwise glue a new row onto the fragment
//!   and lose both.
//! * A failed append is `set_len`'d back to the last good length, for the same reason.
//! * A complete line that does not parse is skipped and counted, not fatal. It was written by
//!   somebody, and one bad line must not cost the rows after it.
//!
//! Compaction replaces the whole file, so [`Writer`] is reopened afterwards: a handle opened
//! before the rename still points at the old inode, and everything appended through it would be
//! durably written to a file with no name.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use cide_ipc::local_history::{VersionKind, VersionSource};
use serde::{Deserialize, Serialize};

use crate::Stamp;
use crate::pack::{BlobRef, FILE_MODE};

/// Bumped only for a change an older reader would misread. A store from a newer cide is opened
/// read-only rather than rewritten by code that does not understand it.
pub(crate) const FORMAT: u32 = 1;

pub(crate) const LOG: &str = "log.jsonl";
const TEMP: &str = "log.jsonl.tmp";

/// The line kinds are told apart by how they start, which `serde_json` guarantees because it
/// writes struct fields in declaration order. An untagged enum would ask serde to try each
/// shape, and this runs once per row at every project open.
const ROW: &[u8] = b"{\"id\":";
const STAMP: &[u8] = b"{\"stamp\":";
const ROOT: &[u8] = b"{\"root\":";
const HEADER: &[u8] = b"{\"format\":";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Header {
    pub format: u32,
    /// Stored because compaction can remove the newest rows, and a reused id would make an old
    /// reference to a removed version resolve to an unrelated new one.
    pub next_id: u64,
    /// The newest row retention has dropped. In the header because the log is what forgets: this
    /// is the one fact about those rows that has to outlive them. See `Store::plan_at`.
    #[serde(default)]
    pub trimmed_through: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RootLine {
    pub root: u16,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BlobLine {
    #[serde(rename = "h")]
    pub hash: String,
    #[serde(rename = "p")]
    pub pack: u32,
    #[serde(rename = "o")]
    pub offset: u64,
    #[serde(rename = "c")]
    pub stored: u64,
}

impl BlobLine {
    pub(crate) fn of(blob: &BlobRef) -> Self {
        Self {
            hash: blake3::Hash::from_bytes(blob.hash).to_hex().to_string(),
            pack: blob.pack,
            offset: blob.offset,
            stored: blob.stored,
        }
    }

    pub(crate) fn blob(&self) -> Option<BlobRef> {
        Some(BlobRef {
            hash: *blake3::Hash::from_hex(&self.hash).ok()?.as_bytes(),
            pack: self.pack,
            offset: self.offset,
            stored: self.stored,
        })
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RowLine {
    pub id: u64,
    pub ts: u64,
    #[serde(rename = "g")]
    pub group: u64,
    #[serde(rename = "r")]
    pub root: u16,
    #[serde(rename = "p")]
    pub rel: String,
    #[serde(rename = "k")]
    pub kind: VersionKind,
    #[serde(rename = "s")]
    pub source: VersionSource,
    #[serde(rename = "b", default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<BlobLine>,
    #[serde(rename = "n", default)]
    pub size: u64,
    #[serde(rename = "m", default)]
    pub mode: u32,
    #[serde(rename = "v", default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<String>,
    #[serde(rename = "l", default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The file changed and was over the size limit, so the row exists and the bytes do not.
    #[serde(rename = "o", default, skip_serializing_if = "is_false")]
    pub omitted: bool,
    #[serde(rename = "st", default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
}

/// A file's stamp moved and its bytes did not: `touch`, or a `git stash` and `pop`.
///
/// Not a row, because nothing happened that a user would want to see or restore. It exists so
/// that the next reconcile can trust the stamp again instead of hashing that file on every pass
/// for as long as it stays touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StampLine {
    pub stamp: Stamp,
    #[serde(rename = "r")]
    pub root: u16,
    #[serde(rename = "p")]
    pub rel: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Header(Header),
    Root(RootLine),
    Row(Box<RowLine>),
    Stamp(StampLine),
}

fn parse(line: &[u8]) -> Option<Line> {
    if line.starts_with(ROW) {
        serde_json::from_slice(line)
            .ok()
            .map(|row| Line::Row(Box::new(row)))
    } else if line.starts_with(STAMP) {
        serde_json::from_slice(line).ok().map(Line::Stamp)
    } else if line.starts_with(ROOT) {
        serde_json::from_slice(line).ok().map(Line::Root)
    } else if line.starts_with(HEADER) {
        serde_json::from_slice(line).ok().map(Line::Header)
    } else {
        None
    }
}

/// Append one line's JSON to a buffer.
pub(crate) fn push(out: &mut Vec<u8>, line: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *out, line).map_err(io::Error::other)?;
    out.push(b'\n');
    Ok(())
}

/// What reading the log found, beyond the lines themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Replayed {
    /// The end of the last line with a newline. Everything past it is a torn append.
    pub good_len: u64,
    /// Whether the file is longer than that.
    pub torn: bool,
    /// Complete lines that did not parse.
    pub skipped: u32,
}

/// Read every committed line. A missing file is an empty log.
pub(crate) fn replay(path: &Path, mut sink: impl FnMut(Line)) -> io::Result<Replayed> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Replayed::default()),
        Err(e) => return Err(e),
    };
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut line = Vec::new();
    let mut seen = Replayed::default();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(seen);
        }
        if line.last() != Some(&b'\n') {
            seen.torn = true;
            return Ok(seen);
        }
        seen.good_len += read as u64;
        match parse(&line[..line.len() - 1]) {
            Some(parsed) => sink(parsed),
            None => {
                seen.skipped += 1;
                tracing::warn!(
                    offset = seen.good_len - read as u64,
                    "skipped an unreadable Local History log line"
                );
            }
        }
    }
}

/// The append end of the log.
#[derive(Debug)]
pub(crate) struct Writer {
    file: File,
    path: PathBuf,
    len: u64,
}

impl Writer {
    /// Open for appending, cutting a torn tail off first.
    pub(crate) fn open(path: &Path, good_len: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(FILE_MODE)
            .open(path)?;
        file.set_len(good_len)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            len: good_len,
        })
    }

    /// Durably append complete lines, or leave the log exactly as it was.
    pub(crate) fn append(&mut self, lines: &[u8]) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let written = self
            .file
            .write_all(lines)
            .and_then(|()| self.file.sync_data());
        if let Err(e) = written {
            let _ = self.file.set_len(self.len);
            return Err(e);
        }
        self.len += lines.len() as u64;
        Ok(())
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    /// Replace the log with what `write` produces, and move this writer onto the new file.
    ///
    /// Streamed to a temp file rather than built in memory and handed to `write_atomic`: at the
    /// row counts this has to handle, that would be a second copy of a 400 MB log on the heap at
    /// the moment the store is trying to shrink.
    pub(crate) fn replace(
        &mut self,
        write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
    ) -> io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let temp = dir.join(TEMP);
        let result = (|| {
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(FILE_MODE)
                .open(&temp)?;
            let mut out = BufWriter::with_capacity(1 << 20, file);
            write(&mut out)?;
            let file = out.into_inner().map_err(|e| e.into_error())?;
            file.sync_all()?;
            fs::rename(&temp, &self.path)?;
            File::open(dir)?.sync_all()
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        self.len = file.seek(SeekFrom::End(0))?;
        self.file = file;
        Ok(())
    }
}

/// Clear a temp file a crash left behind. The rename is the commit, so it is never the real log.
pub(crate) fn remove_temp(dir: &Path) {
    let _ = fs::remove_file(dir.join(TEMP));
}
