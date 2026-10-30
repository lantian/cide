//! The bytes of every version, in append-only pack files.
//!
//! # Why packs and not one file per blob
//!
//! One file per blob is the obvious shape and it was the first design. It loses on exactly the
//! thing this feature is expected to do: a first scan of a large project. A durable new file is
//! a create, a write, an fsync and a directory fsync, which is at least two journal commits on
//! ext4, so a 100,000-file baseline is 200,000 commits, on the same filesystem the user's saves
//! are waiting on. It also costs 100,000 inodes and a block's rounding per file, none of which a
//! storage cap expressed in stored bytes can see.
//!
//! A pack turns all of that into one append and one fsync per batch. The price is that dropping
//! a blob frees nothing until the pack is rewritten, which is what `Store::compact` is for.
//!
//! # A pack has no index of its own
//!
//! `log.jsonl` is the index: a row names a blob by pack, offset and length. A pack is therefore
//! only ever read at a known offset, and bytes no row points at are harmless. That is what makes
//! the crash story short: blobs reach the disk first, so a crash between the two fsyncs leaves
//! unreferenced bytes at the end of a pack and nothing that is wrong.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

use crate::{Error, Hash, Result};

/// A pack past this size is sealed and the next blob starts another.
///
/// Compaction reclaims space a pack at a time, so this is the granularity of that: small enough
/// that rewriting one is not an event, and large enough that a 1 GiB store is a handful of files.
pub(crate) const ROLL_AT: u64 = 64 * 1024 * 1024;

/// The first bytes of every pack, so a stray file in the directory is not mistaken for one.
const MAGIC: &[u8; 8] = b"CLHPACK1";

/// The first byte of every stored blob says how the rest is encoded.
const RAW: u8 = 0;
const DEFLATE: u8 = 1;

pub(crate) const FILE_MODE: u32 = 0o600;

/// Where a blob lives. Copied freely: it is an address, and only compaction changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRef {
    pub hash: Hash,
    pub pack: u32,
    pub offset: u64,
    /// Bytes in the pack, including the encoding byte.
    pub stored: u64,
}

/// A blob that can be read without holding the store.
///
/// The `Arc<File>` is why a read can outlive compaction: unlinking a pack leaves an open
/// descriptor working, so a diff being computed while retention runs finishes on the bytes it
/// started with rather than failing on a file that just went away.
#[derive(Debug, Clone)]
pub struct BlobHandle {
    pub(crate) file: Arc<File>,
    pub(crate) blob: BlobRef,
}

impl BlobHandle {
    /// The version's bytes, or [`Error::Corrupt`] when they are not the bytes that were stored.
    ///
    /// Checked on every read rather than at some scrub, because the one moment that matters is
    /// the one where these bytes are about to overwrite a file.
    pub fn read(&self) -> Result<Vec<u8>> {
        let stored = read_stored(&self.file, &self.blob)?;
        let bytes = decode(&stored)?;
        if blake3::hash(&bytes).as_bytes() != &self.blob.hash {
            return Err(Error::Corrupt("its hash does not match".into()));
        }
        Ok(bytes)
    }
}

fn read_stored(file: &File, blob: &BlobRef) -> Result<Vec<u8>> {
    let mut stored = vec![0; blob.stored as usize];
    file.read_exact_at(&mut stored, blob.offset)
        .map_err(|e| Error::Corrupt(format!("it could not be read: {e}")))?;
    Ok(stored)
}

/// Compress, unless that saves too little to be worth inflating on every read.
///
/// One-eighth, because a PNG or an already-compressed asset deflates by a percent or two and
/// that is not worth the CPU the next diff spends undoing it.
pub(crate) fn encode(bytes: &[u8]) -> Vec<u8> {
    let compressed = (|| {
        // The tag goes into the vector before the encoder owns it: the encoder appends, so the
        // tag stays the one byte of the blob that is not compressed.
        let mut out = Vec::with_capacity(bytes.len() / 2 + 16);
        out.push(DEFLATE);
        let mut encoder = DeflateEncoder::new(out, Compression::default());
        encoder.write_all(bytes)?;
        encoder.finish()
    })();
    match compressed {
        Ok(deflated) if deflated.len() + bytes.len() / 8 < bytes.len() => deflated,
        _ => {
            let mut raw = Vec::with_capacity(bytes.len() + 1);
            raw.push(RAW);
            raw.extend_from_slice(bytes);
            raw
        }
    }
}

fn decode(stored: &[u8]) -> Result<Vec<u8>> {
    let Some((&tag, payload)) = stored.split_first() else {
        return Err(Error::Corrupt("it is empty".into()));
    };
    match tag {
        RAW => Ok(payload.to_vec()),
        DEFLATE => {
            let mut bytes = Vec::with_capacity(payload.len().saturating_mul(3));
            DeflateDecoder::new(payload)
                .read_to_end(&mut bytes)
                .map_err(|e| Error::Corrupt(format!("it could not be inflated: {e}")))?;
            Ok(bytes)
        }
        other => Err(Error::Corrupt(format!("unknown encoding {other}"))),
    }
}

#[derive(Debug)]
struct PackFile {
    file: Arc<File>,
    len: u64,
}

/// The pack directory. Every write goes to the newest pack.
#[derive(Debug)]
pub(crate) struct Packs {
    dir: PathBuf,
    files: BTreeMap<u32, PackFile>,
    /// Packs with bytes that have not been through `sync_data` yet.
    dirty: BTreeSet<u32>,
    /// A pack was created, so the directory needs its own sync before a row can name it.
    created: bool,
    writable: bool,
}

fn file_name(id: u32) -> String {
    format!("{id:05}.pack")
}

fn id_of(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".pack")?.parse().ok()
}

impl Packs {
    pub(crate) fn open(dir: &Path, writable: bool) -> io::Result<Self> {
        let mut files = BTreeMap::new();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry.path();
            let Some(id) = id_of(&path) else { continue };
            let file = OpenOptions::new().read(true).append(writable).open(&path)?;
            let len = file.metadata()?.len();
            files.insert(
                id,
                PackFile {
                    file: Arc::new(file),
                    len,
                },
            );
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            files,
            dirty: BTreeSet::new(),
            created: false,
            writable,
        })
    }

    /// The pack new blobs go to, rolling or creating one as needed.
    fn tail_for_write(&mut self) -> io::Result<u32> {
        if let Some((&id, pack)) = self.files.last_key_value()
            && pack.len < ROLL_AT
        {
            return Ok(id);
        }
        let id = self.files.keys().next_back().map_or(1, |id| id + 1);
        let path = self.dir.join(file_name(id));
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&path)?;
        file.write_all(MAGIC)?;
        self.files.insert(
            id,
            PackFile {
                file: Arc::new(file),
                len: MAGIC.len() as u64,
            },
        );
        self.created = true;
        Ok(id)
    }

    /// Append stored bytes, undoing a partial write. Durable only after [`Self::sync`].
    pub(crate) fn append(&mut self, stored: &[u8]) -> io::Result<(u32, u64)> {
        let id = self.tail_for_write()?;
        let pack = self.files.get_mut(&id).expect("the tail was just resolved");
        let offset = pack.len;
        if let Err(e) = (&*pack.file).write_all(stored) {
            // ENOSPC mid-blob. Unreferenced bytes are harmless, but `offset` is computed from
            // `len`, so leaving them there would put the next blob somewhere its row does not say.
            let _ = pack.file.set_len(offset);
            return Err(e);
        }
        pack.len += stored.len() as u64;
        self.dirty.insert(id);
        Ok((id, offset))
    }

    pub(crate) fn sync(&mut self) -> io::Result<()> {
        for id in &self.dirty {
            if let Some(pack) = self.files.get(id) {
                pack.file.sync_data()?;
            }
        }
        self.dirty.clear();
        if self.created {
            File::open(&self.dir)?.sync_all()?;
            self.created = false;
        }
        Ok(())
    }

    pub(crate) fn handle(&self, blob: BlobRef) -> Option<BlobHandle> {
        let pack = self.files.get(&blob.pack)?;
        Some(BlobHandle {
            file: Arc::clone(&pack.file),
            blob,
        })
    }

    /// A blob's stored bytes, unverified. For compaction, which moves them without opening them.
    pub(crate) fn stored(&self, blob: &BlobRef) -> Result<Vec<u8>> {
        let pack = self
            .files
            .get(&blob.pack)
            .ok_or_else(|| Error::Corrupt(format!("pack {} is missing", blob.pack)))?;
        read_stored(&pack.file, blob)
    }

    /// The pack being written. Never a compaction target: rewriting it would be appending to
    /// the file being rewritten.
    pub(crate) fn tail(&self) -> Option<u32> {
        self.files.keys().next_back().copied()
    }

    pub(crate) fn sizes(&self) -> impl Iterator<Item = (u32, u64)> + '_ {
        self.files.iter().map(|(&id, pack)| (id, pack.len))
    }

    pub(crate) fn disk_bytes(&self) -> u64 {
        self.files.values().map(|pack| pack.len).sum()
    }

    /// Delete a pack no row refers to. An open [`BlobHandle`] keeps working.
    pub(crate) fn remove(&mut self, id: u32) -> io::Result<()> {
        if !self.writable {
            return Ok(());
        }
        self.files.remove(&id);
        self.dirty.remove(&id);
        fs::remove_file(self.dir.join(file_name(id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_compresses_and_round_trips() {
        let text = "fn main() {}\n".repeat(200);
        let stored = encode(text.as_bytes());
        assert_eq!(stored[0], DEFLATE);
        assert!(stored.len() < text.len() / 4);
        assert_eq!(decode(&stored).unwrap(), text.as_bytes());
    }

    /// A stored blob must never be larger than one byte over its input, or an incompressible
    /// file costs more than copying it.
    #[test]
    fn incompressible_bytes_are_kept_raw() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let stored = encode(&noise);
        assert_eq!(stored[0], RAW);
        assert_eq!(stored.len(), noise.len() + 1);
        assert_eq!(decode(&stored).unwrap(), noise);
    }

    #[test]
    fn empty_content_round_trips() {
        assert_eq!(decode(&encode(b"")).unwrap(), b"");
    }

    #[test]
    fn a_blob_with_an_unknown_encoding_is_corrupt_and_not_a_panic() {
        assert!(matches!(decode(&[9, 1, 2]), Err(Error::Corrupt(_))));
        assert!(matches!(decode(&[]), Err(Error::Corrupt(_))));
    }
}
