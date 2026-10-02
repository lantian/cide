//! Exact, capped image bytes for Git comparisons. Only the app materializes and serves them.
use std::io::Read;
use std::path::Path;

use cide_core::image::{MAX_IMAGE_BYTES, is_image_path, sniff};
use cide_ipc::git::{DiffSide, FileDiff};
use git2::{Oid, Repository, Tree};

use crate::{Result, Wrap, diff};

#[derive(Debug)]
pub enum ImageBytes {
    Absent,
    Ready(Vec<u8>),
    Unavailable(String),
}

impl ImageBytes {
    fn checked(bytes: Vec<u8>) -> Self {
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            Self::Unavailable("This image exceeds the 32 MiB preview limit.".into())
        } else if sniff(&bytes[..bytes.len().min(8 * 1024)]).is_some() {
            Self::Ready(bytes)
        } else {
            Self::Unavailable(
                if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1") {
                    "This revision contains a Git LFS pointer; the image bytes are unavailable."
                        .into()
                } else {
                    "This file does not contain a supported image.".into()
                },
            )
        }
    }

    fn read(mode: u32, read: impl FnOnce() -> std::result::Result<Vec<u8>, String>) -> Self {
        if mode == 0 {
            return Self::Absent;
        }
        if mode & 0o170000 != 0o100000 {
            return Self::Unavailable("This side is not a regular image file.".into());
        }
        match read() {
            Ok(bytes) => Self::checked(bytes),
            Err(reason) => Self::Unavailable(reason),
        }
    }
}

#[derive(Debug)]
pub struct ImageDiffBytes {
    pub old_path: Option<String>,
    pub old: ImageBytes,
    pub new: ImageBytes,
}

pub(crate) fn candidate(file: &diff::RawFile) -> bool {
    is_image_path(&file.path) || file.old_path.as_deref().is_some_and(is_image_path)
}

fn blob(repo: &Repository, oid: Oid) -> std::result::Result<Vec<u8>, String> {
    let odb = repo.odb().map_err(|e| e.to_string())?;
    let (size, _) = odb.read_header(oid).map_err(|e| e.to_string())?;
    if size as u64 > MAX_IMAGE_BYTES {
        return Err("This image exceeds the 32 MiB preview limit.".into());
    }
    repo.find_blob(oid)
        .map(|b| b.content().to_vec())
        .map_err(|e| e.to_string())
}

pub(crate) fn tree_image(
    repo: &Repository,
    tree: Option<&Tree<'_>>,
    path: &str,
    mode: u32,
) -> ImageBytes {
    ImageBytes::read(mode, || {
        let tree = tree.ok_or("The previous image tree is unavailable.")?;
        let entry = tree.get_path(Path::new(path)).map_err(|e| e.to_string())?;
        blob(repo, entry.id())
    })
}

fn index_image(repo: &Repository, path: &str, mode: u32) -> ImageBytes {
    ImageBytes::read(mode, || {
        let oid = diff::index_blob(repo, path)
            .map_err(|e| e.to_string())?
            .ok_or("The image is unavailable in the index.")?;
        blob(repo, oid)
    })
}

pub(crate) fn workdir_image(repo: &Repository, path: &str, mode: u32) -> ImageBytes {
    ImageBytes::read(mode, || {
        let root = repo
            .workdir()
            .ok_or("This repository has no working tree.")?;
        let full = root.join(path);
        let meta = std::fs::symlink_metadata(&full).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("This side is not a regular image file.".into());
        }
        let canonical = full.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
            return Err("The image resolves outside the repository.".into());
        }
        if meta.len() > MAX_IMAGE_BYTES {
            return Err("This image exceeds the 32 MiB preview limit.".into());
        }
        let file = std::fs::File::open(canonical).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    })
}

/// Preserve the staging patch; use a whole-repository delta only to resolve image renames.
pub fn file_diff(
    repo: &Repository,
    path: &str,
    side: DiffSide,
) -> Result<(FileDiff, Option<ImageDiffBytes>)> {
    let request = diff::DiffRequest::new(side).renames(true);
    let raw = diff::file_diff(repo, path, request)?
        .ok_or_else(|| cide_ipc::git::GitError::NoSuchChange { path: path.into() })?;
    let mut view = diff::view(repo, &raw, side);
    if !candidate(&raw) {
        return Ok((view, None));
    }

    // SVG hunks still define the staging patch, but the preview needs neither full text.
    view.old_text = None;
    view.new_text = None;
    view.texts_omitted = false;
    let whole = if matches!(
        raw.status,
        git2::Delta::Added | git2::Delta::Deleted | git2::Delta::Untracked
    ) {
        let mut whole = diff::build(repo, request.renames(false), None)?;
        let mut find = git2::DiffFindOptions::new();
        find.renames(true).for_untracked(true);
        whole.find_similar(Some(&mut find)).wrap()?;
        Some(whole)
    } else {
        None
    };
    let renamed = whole.as_ref().and_then(|whole| {
        whole.deltas().find(|delta| {
            delta.new_file().path() == Some(Path::new(path))
                || delta.old_file().path() == Some(Path::new(path))
        })
    });
    let new_path = renamed
        .as_ref()
        .and_then(|d| {
            d.new_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| raw.path.clone());
    let old_path = renamed
        .as_ref()
        .and_then(|d| {
            d.old_file()
                .path()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .or_else(|| raw.old_path.clone())
        .unwrap_or_else(|| raw.path.clone());
    let old_mode = renamed
        .as_ref()
        .map_or(raw.old_mode, |d| u32::from(d.old_file().mode()));
    let new_mode = renamed
        .as_ref()
        .map_or(raw.new_mode, |d| u32::from(d.new_file().mode()));
    let head = diff::head_tree(repo)?;
    let (old, new) = match side {
        DiffSide::Staged => (
            tree_image(repo, head.as_ref(), &old_path, old_mode),
            index_image(repo, &new_path, new_mode),
        ),
        DiffSide::Unstaged => (
            index_image(repo, &old_path, old_mode),
            workdir_image(repo, &new_path, new_mode),
        ),
        DiffSide::Combined => (
            tree_image(repo, head.as_ref(), &old_path, old_mode),
            workdir_image(repo, &new_path, new_mode),
        ),
    };
    Ok((
        view,
        Some(ImageDiffBytes {
            old_path: (old_path != new_path).then_some(old_path),
            old,
            new,
        }),
    ))
}
