//! Scratch files: a per-project drawer for buffers that are not part of the project.
//!
//! A scratch is a real file with a real path that the editor opens, saves and closes exactly
//! as it does a project file. What makes it a scratch is *where* it lives: outside every
//! project root, in state cide owns, so that a throwaway SQL query or a JSON payload never
//! turns up in `git status` and never has to be remembered about before a commit.
//!
//! # Where they live, and why not in the project
//!
//! `$XDG_STATE_HOME/cide/scratches/<key>/`, i.e. beside `cide-git`'s
//! `$XDG_STATE_HOME/cide/repos/<key>/` and derived the same way. Three reasons, in increasing
//! order of weight:
//!
//!   * **State, not config.** [`crate::persist::state_dir`] states the rule: written by the
//!     app rather than edited by the user, and a config directory invites a file into a
//!     dotfile repository. A scratch is written by cide on the user's behalf.
//!   * **Not inside the project**, because the whole point is a buffer that is not part of it.
//!     A scratch in the working tree is either an untracked file the user has to explain to
//!     themselves at every `git status`, or a line in `.gitignore` that cide put there.
//!   * And a third that only shows up here: `cide-fs`'s walk is **gitignore-aware**, so a
//!     scratch inside the project would be *hidden from cide's own file tree* by the user's
//!     own ignore file — the feature would work everywhere except in the panel that shows it.
//!
//! Not `$XDG_DATA_HOME`: nothing in this application uses it except the freedesktop trash.
//!
//! # The key
//!
//! `blake3(canonical(primary root))`, first 32 hex characters — [`crate::persist`]'s neighbour
//! `cide_git::repo::repo_key` makes the argument and this reuses it verbatim: hashing rather
//! than escaping the path keeps the directory name a fixed length and free of separators, and
//! canonicalising first means a symlinked checkout and its target share one drawer rather than
//! silently keeping two.
//!
//! The key is the **project's** primary root (`Project::roots[0]`), not a repository — a
//! multi-root project has one set of scratches, which is what "per project" means to the person
//! using it, and `roots[0]` is already the identity `RecentProject` is keyed by.
//!
//! ## What that costs, named rather than discovered
//!
//! A project that is *moved or renamed* gets a new key, and its scratches appear to vanish.
//! That is exactly what already happens to changelists and to the shelf, and nothing mitigates
//! it for those either. The mitigation here is one file and no new failure mode: [`ensure_dir`]
//! writes a sibling `<key>.json` recording the path the key came from, so a human who goes
//! looking can tell which hashed directory is theirs. A reverse index mapping key → path was
//! the alternative and loses — it is a second source of truth for a fact the directory name
//! already encodes, and re-adoption needs a user interface nobody asked for.
//!
//! **There is no garbage collection**, same as `repos/`. Closing a project does not delete its
//! scratches: `close_project` takes the project out of `workspace.json` but `recent.json` keeps
//! it, so re-opening the same folder recomputes the same key and the drawer is still there.
//!
//! # The origin record is a *sibling*, not a file inside the drawer
//!
//! `scratches/<key>.json`, beside `scratches/<key>/`. Inside would have been tidier on disk and
//! wrong on screen: the drawer's contents are listed verbatim as file-tree rows, so a metadata
//! file in there would be a row the user can see, cannot explain, and — if they deleted it —
//! would find recreated the next time cide wrote one.

use std::path::{Path, PathBuf};

use crate::{CoreError, Result};

/// How many `scratch_N.<ext>` candidates are tried before giving up.
///
/// The same cap and the same reasoning as `cide_fs::copy::MAX_CANDIDATES`, restated because
/// that constant is private to another crate: a thousand is far past any real gesture and still
/// finite, and without a cap a drawer that somehow held every candidate would spin for ever
/// inside a blocking worker — which presents as a hung command with no error at all.
const MAX_SCRATCHES: u32 = 1000;

/// The longest extension a scratch type may name.
///
/// Twelve is past `markdown` and `typescript` with room to spare. The limit is not about disk
/// layout; it is about the string arriving from the webview, where a value that is not one of
/// the offered types is a bug or somebody trying.
const MAX_EXT: usize = 12;

/// `$XDG_STATE_HOME/cide/scratches`.
pub fn scratches_root() -> PathBuf {
    crate::persist::state_dir().join("scratches")
}

/// A project's stable key: the blake3 of its canonical primary root, in hex, truncated.
///
/// 128 bits of a 256-bit digest. `cide_git::repo::repo_key` makes the same cut for the same
/// reason — this names a directory, not a signature, and a 64-character path component is
/// unreadable in a terminal for no gain.
pub fn key(primary_root: &Path) -> String {
    blake3::hash(canonical(primary_root).as_os_str().as_encoded_bytes()).to_hex()[..32].to_string()
}

/// Where this project's scratch files live. Does not create anything — see [`ensure_dir`].
pub fn dir_for(primary_root: &Path) -> PathBuf {
    scratches_root().join(key(primary_root))
}

/// The sibling record naming the project a drawer belongs to. See the module header.
pub fn origin_path(primary_root: &Path) -> PathBuf {
    scratches_root().join(format!("{}.json", key(primary_root)))
}

/// Make sure the drawer exists, and answer where it is.
///
/// Writes the origin record on the way, and **ignores a failure to write it**: it is a
/// breadcrumb for a human recovering an orphaned directory, and a read-only state directory
/// must not be the reason a user cannot make a scratch file. The directory itself is not
/// optional, so that error is returned.
pub fn ensure_dir(primary_root: &Path) -> Result<PathBuf> {
    let dir = dir_for(primary_root);
    std::fs::create_dir_all(&dir).map_err(|error| {
        CoreError::Io(format!(
            "could not create the scratch directory {}: {error}",
            dir.display()
        ))
    })?;
    write_origin(primary_root);
    Ok(dir)
}

/// Record which project this drawer belongs to, best-effort.
fn write_origin(primary_root: &Path) {
    let record = serde_json::json!({
        "root": canonical(primary_root).to_string_lossy(),
        "seen": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    let path = origin_path(primary_root);
    if let Err(error) = std::fs::write(&path, format!("{record}\n")) {
        tracing::debug!(path = %path.display(), %error, "could not record a scratch drawer's origin");
    }
}

/// The name to try on the `attempt`-th go: `scratch.rs`, `scratch_1.rs`, `scratch_2.rs`.
///
/// IDEA's spelling, and deliberately **not** `cide_fs::copy::candidate_name`'s ` copy` suffix
/// even though that is this codebase's house rule for a taken name. `main copy.rs` means "this
/// is a copy of that file"; a scratch is not a copy of anything, and `scratch copy 2.rs` reads
/// as a bug rather than as the second scratch.
///
/// The counter is **per extension**, which falls out of this for free: `scratch.rs` and
/// `scratch.json` are different names, so they coexist, and `scratch_1.rs` appears only once
/// `scratch.rs` is taken. That is also what IDEA does.
pub fn scratch_name(ext: &str, attempt: u32) -> String {
    if attempt == 0 {
        format!("scratch.{ext}")
    } else {
        format!("scratch_{attempt}.{ext}")
    }
}

/// Reject an extension that is not one a scratch file may carry.
///
/// The list of *offered* types lives in the frontend (`ui/src/editor/languages.ts`), because it
/// has to agree with the editor's own language table and shipping it to Rust would make it a
/// DTO that must stay in step with a TypeScript record. So this side validates the **shape**
/// rather than the membership: what arrives is a string from the webview, and the only thing
/// this layer can honestly say about it is that it is a plain lowercase extension and not a
/// path fragment.
///
/// Stricter than `cide_fs::ops::check_name` on purpose. That one is about a name a *user typed*
/// into a box, where the useful thing is to explain what is wrong; this is about a token the
/// application itself is supposed to have chosen from a fixed list, so anything surprising is a
/// refusal rather than a negotiation.
pub fn check_ext(ext: &str) -> Result<()> {
    let reason = if ext.is_empty() {
        "a scratch file needs an extension"
    } else if ext.len() > MAX_EXT {
        "that extension is too long to be a language's"
    } else if !ext
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        // Which rules out `.`, `/`, `..` and every separator in one clause, so there is no
        // second place to keep a list of dangerous characters in step with the first.
        "an extension is lowercase letters and digits"
    } else {
        return Ok(());
    };
    Err(CoreError::Io(format!("{ext:?}: {reason}")))
}

/// Create the next scratch file of this type in `dir`, and answer its path.
///
/// **The claim is the creation.** `create_new` fails with `EEXIST` in the kernel, so two windows
/// asking for a scratch in the same millisecond cannot land on one file — which is exactly the
/// property `cide_fs::copy::claim` exists for, and the reason this is not a `path.exists()` test
/// followed by a write.
///
/// The file is created **empty and immediately**, rather than opening an unsaved buffer. Three
/// things need it to: the claim above is the creation, the file tree's group has to have
/// something to list, and `TabKind::File` has no concept of a buffer with no file behind it.
/// IDEA does the same.
pub fn create(dir: &Path, ext: &str) -> Result<PathBuf> {
    check_ext(ext)?;
    for attempt in 0..MAX_SCRATCHES {
        let candidate = dir.join(scratch_name(ext, attempt));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CoreError::Io(format!(
                    "could not create {}: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(CoreError::Io(format!(
        "{} already holds {MAX_SCRATCHES} scratch files of that type",
        dir.display()
    )))
}

/// Everything in a drawer, in the order a file tree draws it.
///
/// A flat listing: scratches have no subdirectories, because a flat drawer is what IDEA offers
/// and because it makes the tree group one `read_dir` rather than a lazy walk. A directory that
/// somehow appears in there (a user's `mkdir`) is **skipped** rather than shown — the group
/// promises scratch files, and a folder in it would draw a twisty this listing cannot fill.
///
/// A drawer that does not exist yet lists as empty rather than as an error: no scratch has been
/// made, which is a state, not a failure.
pub fn list(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        // A name that is not UTF-8 is dropped for the reason `cide_fs::groups::read_dir` gives:
        // the path is the identifier the frontend hands back, and a lossy one no longer names
        // the file.
        .filter(|path| path.to_str().is_some() && path.file_name().is_some())
        .collect();
    paths.sort_by(|a, b| name_of(a).cmp(name_of(b)));
    paths
}

fn name_of(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// Resolve symlinks and `..`, falling back to the input when the path does not exist.
///
/// The same shape as `cide_git::repo::canonical`, and the fallback matters for the same reason:
/// a key must still be computable for a root that has been unmounted or deleted, or closing
/// such a project would fail rather than forgetting it.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-scratch-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_key_is_a_fixed_length_hex_string_with_no_separators_in_it() {
        let key = key(Path::new("/home/u/work/cide"));
        assert_eq!(key.len(), 32);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(key, super::key(Path::new("/home/u/work/other")));
    }

    /// A symlinked checkout and its target are one project, so they get one drawer. Without
    /// the canonicalisation a user with `~/work → /mnt/work` would keep two sets of scratches
    /// and see whichever the path they opened produced.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_root_and_its_target_share_one_drawer() {
        let dir = temp("symlink");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).expect("mkdir");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert_eq!(key(&real), key(&link));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The counter, and that it is per extension.
    #[test]
    fn names_run_scratch_then_scratch_1_and_each_type_counts_on_its_own() {
        let dir = temp("names");
        assert_eq!(scratch_name("rs", 0), "scratch.rs");
        assert_eq!(scratch_name("rs", 1), "scratch_1.rs");
        assert_eq!(scratch_name("json", 2), "scratch_2.json");

        assert_eq!(create(&dir, "rs").expect("first"), dir.join("scratch.rs"));
        assert_eq!(
            create(&dir, "rs").expect("second"),
            dir.join("scratch_1.rs"),
            "a taken name moves to the counter rather than to ` copy`"
        );
        assert_eq!(
            create(&dir, "json").expect("other type"),
            dir.join("scratch.json"),
            "the counter is per extension: scratch.rs must not push scratch.json to _1"
        );
        assert_eq!(create(&dir, "rs").expect("third"), dir.join("scratch_2.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The claim *is* the creation, so the file is on disk when the call returns.
    #[test]
    fn a_created_scratch_exists_and_is_empty() {
        let dir = temp("created");
        let path = create(&dir, "md").expect("create");
        assert!(path.is_file());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The extension is a token the application chose, so anything else is refused rather
    /// than negotiated. The path-fragment cases are the ones that matter.
    #[test]
    fn an_extension_that_is_not_a_plain_lowercase_token_is_refused() {
        for bad in [
            "",
            "../../etc/passwd",
            "rs/../../x",
            "R S",
            "RS",
            "rs.bak",
            "verylongextension",
            "rs\0",
        ] {
            assert!(check_ext(bad).is_err(), "{bad:?} must be refused");
        }
        for good in ["rs", "go", "md", "json", "txt", "py3", "h"] {
            assert!(check_ext(good).is_ok(), "{good:?} must be allowed");
        }

        // And `create` checks *before* it joins anything, which is the half a weaker assertion
        // misses. A path fragment refuses itself by accident — `dir/../../etc/passwd` has no
        // parent to create in, so `create_new` fails whether or not anything validated it — so
        // the case that actually proves the check runs is an extension that would **succeed**
        // unvalidated. `rs.bak` is exactly that: `scratch.rs.bak` is a perfectly creatable file.
        let dir = temp("badext");
        for bad in ["../../etc/passwd", "rs.bak", "RS", "verylongextension"] {
            assert!(
                create(&dir, bad).is_err(),
                "create({bad:?}) must be refused"
            );
            assert!(
                list(&dir).is_empty(),
                "and must leave nothing behind: create({bad:?}) wrote {:?}",
                list(&dir)
            );
        }
        assert!(!dir.join("..").join("passwd").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Listing: files only, sorted, and a missing drawer is empty rather than an error.
    #[test]
    fn a_drawer_lists_its_files_in_name_order_and_nothing_else() {
        let dir = temp("listing");
        assert!(
            list(&dir.join("never-made")).is_empty(),
            "no scratch has been made yet, which is a state and not a failure"
        );

        std::fs::write(dir.join("scratch_1.rs"), "").expect("write");
        std::fs::write(dir.join("scratch.rs"), "").expect("write");
        std::fs::write(dir.join("notes.md"), "").expect("write");
        std::fs::create_dir(dir.join("a-folder")).expect("mkdir");

        let names: Vec<String> = list(&dir).iter().map(|p| name_of(p).to_string()).collect();
        assert_eq!(names, ["notes.md", "scratch.rs", "scratch_1.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The origin record is a **sibling** of the drawer, so it is never a row in the tree.
    #[test]
    fn the_origin_record_sits_beside_the_drawer_and_not_in_it() {
        let root = temp("origin");
        let drawer = dir_for(&root);
        let record = origin_path(&root);
        assert_eq!(record.parent(), drawer.parent());
        assert!(!record.starts_with(&drawer));

        // And a real `ensure_dir` leaves the drawer empty, which is the property the file tree
        // depends on: a metadata file inside would be a row nobody can explain or delete.
        let made = ensure_dir(&root).expect("ensure");
        assert!(made.is_dir());
        assert!(list(&made).is_empty());
        assert!(record.is_file(), "the breadcrumb is written");
        let written = std::fs::read_to_string(&record).expect("read");
        assert!(
            written.contains(&canonical(&root).to_string_lossy().to_string()),
            "the record names the project it came from: {written}"
        );

        let _ = std::fs::remove_file(&record);
        let _ = std::fs::remove_dir_all(&drawer);
        let _ = std::fs::remove_dir_all(&root);
    }
}
