//! Browsing and reading a container's filesystem. (M44 — ADR 0013)
//!
//! # The Engine API has no directory listing, and that decides this whole module
//!
//! `GET /containers/{id}/archive?path=X` is the only filesystem endpoint, and it answers with a
//! **tar of the whole subtree** — so asking it for `/` tars the container. `HEAD` on the same
//! path answers a base64 `X-Docker-Container-Path-Stat` header describing *one* entry and nothing
//! about its children. There is no third endpoint.
//!
//! So the two halves take different roads, and each is the right one for its half:
//!
//! * **Listing runs `ls` inside the container**, through M42's exec machinery. This is what every
//!   other Docker UI does, and it fails the same way they do — on an image with no shell. That
//!   failure is reported as a sentence naming the cause ([`cide_ipc::docker::ContainerListing`]),
//!   never as an empty directory, because an empty tree is a *lie* about a filesystem that is
//!   full and there is nothing on screen to correct it.
//! * **Reading one file is `GET /archive?path=<file>`** — a tar with a single entry, exact,
//!   cheap, and needing no shell at all. So a distroless image cannot be browsed and its files
//!   can still be read once their paths are known.
//!
//! # Read-only, deliberately
//!
//! `PUT /archive` exists and cide does not call it. An edit written back into a container is lost
//! the moment that container is recreated — which for anything under compose is the ordinary way
//! it is restarted — and a silently-lost edit is a worse feature than no feature. The buffer a
//! file opens in says read-only for the same reason.

use std::path::PathBuf;

use cide_ipc::docker::{ContainerEntry, ContainerListing};

use crate::{Docker, DockerError};

/// The largest file cide will read out of a container.
///
/// `document::read`'s own cap, matched deliberately: a file that opens from a container and a
/// file that opens from disk should refuse at the same size, or the limit becomes a property of
/// where the file happened to be.
const MAX_FILE: u64 = 32 * 1024 * 1024;

/// The listing command.
///
/// # Why `-p` is the load-bearing flag and the mode string is not read
///
/// `-p` appends `/` to a directory's name, and it is the *only* field GNU coreutils and BusyBox
/// spell identically. The mode string looks like the obvious source — `d` in column one — and is
/// not: BusyBox pads differently, ACLs add a `+`, SELinux adds a `.`, and a symlink to a
/// directory is `l` in the mode and still a directory to descend into. So the trailing slash
/// decides, and the mode string is never parsed.
///
/// `-A` and not `-a`: dotfiles yes, `.` and `..` no. A tree that listed `..` would let somebody
/// walk out of the directory they opened by clicking a row that looks like any other.
///
/// `--` before the path, because a path is user input and a container may hold a directory whose
/// name begins with a hyphen.
fn list_argv(path: &str) -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        // Through `sh` rather than exec'ing `ls` directly so a missing `ls` is a message on the
        // stream rather than an exec failure with the daemon's own wording — and so the `2>&1`
        // is applied, which is what makes "not found" visible at all.
        format!("ls -lAp -- {} 2>&1", shell_quote(path)),
    ]
}

/// Single-quote a path for `sh`.
///
/// The whole of the rule: wrap in `'`, and replace each embedded `'` with `'\''`. Nothing else
/// is special inside single quotes in any POSIX shell — not `$`, not backticks, not backslash —
/// which is why this is four lines rather than an escaping table.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Read one `ls -lAp` line.
///
/// # Lenient on purpose
///
/// The long format is not a standard. GNU prints nine fields, BusyBox prints nine differently
/// spaced, `-l` on some systems inserts an SELinux context, and a file name may contain spaces.
/// A strict parser would drop real files on real images.
///
/// So: the **name** is everything after the eighth whitespace-separated field, which is where
/// every implementation puts it; the **size** is the fifth field *only if it parses as a number*,
/// and `None` otherwise; and the **kind** comes from the trailing `/`. A line that yields no name
/// is skipped rather than guessed at.
fn parse_line(line: &str) -> Option<ContainerEntry> {
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    // `total 48`, which every implementation prints first.
    if line.starts_with("total ") {
        return None;
    }

    let mut fields = line.split_whitespace();
    let mode = fields.next()?;
    // Link count, then owner and group — the two are shown as one `owner:group` string.
    let _ = fields.next()?;
    let owner = fields.next()?;
    let group = fields.next()?;
    let size = fields.next().and_then(|f| f.parse::<i64>().ok());
    // Month, day, time-or-year.
    let month = fields.next()?;
    let day = fields.next()?;
    let when = fields.next()?;

    // Everything after the eighth field, with its original spacing — a name may contain spaces,
    // and `split_whitespace` has already thrown them away.
    let rest = line
        .split_whitespace()
        .take(8)
        .fold(line, |acc, field| match acc.find(field) {
            Some(at) => &acc[at + field.len()..],
            None => acc,
        });
    let name = rest.trim_start();
    if name.is_empty() {
        return None;
    }

    // `a -> b`. Only for a symlink, and only when `ls` printed the arrow — some implementations
    // omit it without `-l`, and a name containing ` -> ` is otherwise a real file name.
    let (name, link) = if mode.starts_with('l') {
        match name.split_once(" -> ") {
            Some((from, to)) => (from, Some(to.to_string())),
            None => (name, None),
        }
    } else {
        (name, None)
    };

    let directory = name.ends_with('/');
    Some(ContainerEntry {
        name: name.trim_end_matches('/').to_string(),
        directory,
        // A directory's `ls` size is the size of its *inode*, not of its contents, and showing
        // "4096" beside a folder is a number that means nothing to anybody.
        size: if directory { None } else { size },
        link,
        mode: Some(mode.to_string()),
        owner: Some(format!("{owner}:{group}")),
        // Rejoined with single spaces rather than sliced out of the line: `ls` pads the day
        // column to two characters, so ` 1` and `11` would come back differently spaced.
        modified: Some(format!("{month} {day} {when}")),
    })
}

/// Read a whole `ls -lAp` output.
///
/// Sorted directories-first then by name, which is what every file tree does and what makes a
/// listing stable: `ls` output order is the filesystem's and differs between reads.
pub fn parse_listing(output: &str) -> Vec<ContainerEntry> {
    let mut entries: Vec<ContainerEntry> = output.lines().filter_map(parse_line).collect();
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Whether what came back is a shell's complaint rather than a listing.
///
/// # Why this is checked at all, when the exec's exit code exists
///
/// Because it does not reach here. `exec_capture` collects the *stream*, and an exec's status is
/// a second round trip to `GET /exec/{id}/json` that would double the cost of every directory
/// expansion. The shell's own words are on the stream already — and they are also the only thing
/// that can distinguish "no such directory" from "no shell at all", which the exit code cannot.
fn complaint_in(output: &str) -> Option<String> {
    let first = output.lines().map(str::trim).find(|l| !l.is_empty())?;
    let lowered = first.to_lowercase();
    // `docker`'s own wording when there is no shell, and `sh`'s when there is no `ls`.
    let no_shell = lowered.contains("no such file or directory")
        && (lowered.contains("/bin/sh") || lowered.contains("exec"));
    let no_ls = lowered.contains("ls:") || lowered.contains("not found");
    (no_shell || no_ls).then(|| first.to_string())
}

impl Docker {
    /// List one directory inside a container.
    pub fn list_dir(&self, container: &str, path: &str) -> ContainerListing {
        let printed = match self.exec_capture(container, &list_argv(path)) {
            Ok(printed) => printed,
            Err(error) => {
                // The daemon refused the exec outright, which on a `scratch` image is what "no
                // shell" looks like from here.
                return ContainerListing::Unusable {
                    reason: format!(
                        "cide could not list `{path}` in this container: {error} A distroless or \
                         `scratch` image has no shell, so its files cannot be browsed — they can \
                         still be opened by path."
                    ),
                };
            }
        };

        if let Some(complaint) = complaint_in(&printed) {
            return ContainerListing::Unusable {
                reason: format!("`ls {path}` in this container said: {complaint}"),
            };
        }

        let entries = parse_listing(&printed);
        // An *empty* directory is a real thing and must not be reported as unusable — but a
        // listing that parsed nothing out of non-empty output is cide failing to read a format,
        // which is worth saying rather than drawing as an empty folder.
        if entries.is_empty() && !printed.trim().is_empty() && !printed.trim().starts_with("total")
        {
            return ContainerListing::Unusable {
                reason: format!(
                    "cide could not read this container's `ls` output for `{path}`. It said: {}",
                    printed.lines().next().unwrap_or_default().trim()
                ),
            };
        }
        ContainerListing::Ready { entries }
    }

    /// Read one file out of a container as **bytes**, for saving rather than showing. (M47)
    ///
    /// The same `/archive` road as [`Self::read_file`] and deliberately not the same refusals: a
    /// download is allowed to be binary, because a binary is exactly the thing somebody wants a
    /// copy of. It is still capped, and a directory still refuses — a directory downloads as its
    /// tar through [`Self::read_archive`], which is what `docker cp` gives you.
    pub fn read_bytes(&self, container: &str, path: &str) -> Result<Vec<u8>, DockerError> {
        let (entry, bytes) = self.one_entry(container, path)?;
        let _ = entry;
        Ok(bytes)
    }

    /// The tar of a path, exactly as `docker cp` produces it. (M47)
    ///
    /// For a directory there is nothing else to hand over: the API has no recursive read that is
    /// not a tar, and unpacking it here would mean deciding where — which is the user's answer,
    /// through a save dialog, not this crate's.
    pub fn read_archive(&self, container: &str, path: &str) -> Result<Vec<u8>, DockerError> {
        self.archive_bytes(container, path)
    }

    /// Read one file out of a container.
    ///
    /// `GET /archive` and **not** an exec: it needs no shell, so this works on a distroless image
    /// whose directories cannot be listed, and it is exact rather than going through a terminal's
    /// line discipline.
    pub fn read_file(&self, container: &str, path: &str) -> Result<String, DockerError> {
        let (_, bytes) = self.one_entry(container, path)?;
        // Binary is refused rather than shown as replacement characters — `document::read`'s own
        // rule, and a container's `/bin` is full of files somebody will click.
        if bytes.contains(&0) {
            return Err(DockerError::Refused(format!("`{path}` is a binary file.")));
        }
        String::from_utf8(bytes)
            .map_err(|_| DockerError::Refused(format!("`{path}` is not UTF-8 text.")))
    }

    /// The raw archive the daemon sends for a path.
    fn archive_bytes(&self, container: &str, path: &str) -> Result<Vec<u8>, DockerError> {
        use futures_util::StreamExt as _;

        let (client, runtime) = self.parts();
        runtime.block_on(async {
            let mut stream = client.download_from_container(
                container,
                Some(
                    bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
                        .path(path)
                        .build(),
                ),
            );
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;
                buf.extend_from_slice(&chunk);
                // The cap is applied to the *archive* as it arrives, not to the entry after it is
                // extracted. A caller that read the whole stream first would have already spent
                // the memory the cap exists to protect.
                if buf.len() as u64 > MAX_FILE + 512 * 1024 {
                    return Err(DockerError::Refused(format!(
                        "`{path}` is larger than the {} MiB cide will read out of a container.",
                        MAX_FILE / (1024 * 1024)
                    )));
                }
            }
            Ok(buf)
        })
    }

    /// The one file an `/archive` read is about, and its bytes.
    ///
    /// The *first regular entry*, and only it. Asking for a file gives a one-entry tar; asking for
    /// a directory gives its whole subtree, and reading the first file out of that would silently
    /// hand back something the user did not click on. So a directory is refused by name, and so is
    /// a symlink — `GET /archive` returns a link entry with **no content** rather than following
    /// it, and `/bin/sh` on Alpine is a symlink to busybox, so this is the common case.
    fn one_entry(&self, container: &str, path: &str) -> Result<(String, Vec<u8>), DockerError> {
        let bytes = self.archive_bytes(container, path)?;
        let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
        let entries = archive.entries().map_err(|error| DockerError::Unreadable {
            what: format!("the archive for `{path}`"),
            detail: error.to_string(),
        })?;

        for entry in entries {
            let mut entry = entry.map_err(|error| DockerError::Unreadable {
                what: format!("the archive for `{path}`"),
                detail: error.to_string(),
            })?;
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                return Err(DockerError::Refused(format!(
                    "`{path}` is a directory, not a file."
                )));
            }
            if kind.is_symlink() || kind.is_hard_link() {
                let target = entry
                    .link_name()
                    .ok()
                    .flatten()
                    .map(|p| p.to_string_lossy().into_owned());
                return Err(DockerError::Refused(match target {
                    Some(to) => format!(
                        "`{path}` is a symlink to `{to}`, and the daemon does not follow one. Open \
                         that path instead."
                    ),
                    None => format!("`{path}` is a symlink, and the daemon does not follow one."),
                }));
            }
            if !kind.is_file() {
                continue;
            }
            let size = entry.header().size().unwrap_or(0);
            if size > MAX_FILE {
                return Err(DockerError::Refused(format!(
                    "`{path}` is {} MiB, over the {} MiB cide will read out of a container.",
                    size / (1024 * 1024),
                    MAX_FILE / (1024 * 1024)
                )));
            }
            let name = entry
                .path()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut bytes = Vec::with_capacity(size as usize);
            std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|error| {
                DockerError::Unreadable {
                    what: format!("`{path}`"),
                    detail: error.to_string(),
                }
            })?;
            return Ok((name, bytes));
        }

        Err(DockerError::Refused(format!(
            "`{path}` held no file cide could read."
        )))
    }
}

/// Join a directory and a name, the way a container's filesystem spells it.
///
/// Always `/`, never `std::path::Path`: the container is Linux whatever cide is running on, and
/// building a path with `PathBuf` on Windows would produce backslashes the container has never
/// heard of. Exported because the frontend's tree needs the same rule and Rust is where it is
/// tested.
#[must_use]
pub fn join(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Unused today; kept beside [`join`] so the path rules live together.
#[allow(dead_code)]
fn as_path(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GNU coreutils, as Debian and Ubuntu images print it.
    const GNU: &str = "total 84\n\
        drwxr-xr-x   1 root root  4096 Sep  9 11:02 bin/\n\
        -rw-r--r--   1 root root   220 Apr 23  2023 .profile\n\
        lrwxrwxrwx   1 root root     7 Sep  9 11:02 lib -> usr/lib\n\
        -rwxr-xr-x   1 root root 12345 Sep  9 11:02 a file with spaces.sh\n";

    /// BusyBox, as Alpine prints it — different padding, different date column.
    const BUSYBOX: &str = "total 12\n\
        drwxr-xr-x    2 root     root          4096 Sep  9 11:02 etc/\n\
        -rw-r--r--    1 root     root           102 Sep  9 11:02 hosts\n";

    #[test]
    fn both_ls_dialects_parse() {
        // The whole reason the parser is lenient: the long format is not a standard, and a
        // strict parser drops real files on real images.
        let gnu = parse_listing(GNU);
        assert_eq!(gnu.len(), 4, "{gnu:#?}");
        let busybox = parse_listing(BUSYBOX);
        assert_eq!(busybox.len(), 2, "{busybox:#?}");
        assert_eq!(busybox[0].name, "etc");
        assert!(busybox[0].directory);
        assert_eq!(busybox[1].size, Some(102));
    }

    #[test]
    fn the_info_columns_survive_both_dialects_and_a_padded_day() {
        // The three columns the Info view draws, and the reason they are strings: BusyBox pads
        // owner and group to eight characters and GNU does not, so the *values* must come out
        // identical from two very differently spaced lines.
        let gnu = parse_listing(GNU);
        let busybox = parse_listing(BUSYBOX);
        let bin = gnu.iter().find(|e| e.name == "bin").expect("bin");
        let etc = busybox.iter().find(|e| e.name == "etc").expect("etc");
        assert_eq!(bin.owner.as_deref(), Some("root:root"));
        assert_eq!(etc.owner.as_deref(), Some("root:root"));
        assert_eq!(bin.mode.as_deref(), Some("drwxr-xr-x"));

        // `ls` pads the day to two characters, so `Sep  9` is two spaces. Rejoining the three
        // fields is what makes this one space in both dialects rather than a ragged column.
        assert_eq!(bin.modified.as_deref(), Some("Sep 9 11:02"));
        assert_eq!(etc.modified.as_deref(), Some("Sep 9 11:02"));

        // And the year form, which `ls` prints instead of the time past six months — the whole
        // reason this is not parsed into an instant.
        let old = parse_listing("-rw-r--r-- 1 root root 12 Jan  3  2024 old.txt\n");
        assert_eq!(old[0].modified.as_deref(), Some("Jan 3 2024"));
    }

    #[test]
    fn a_directory_is_decided_by_the_trailing_slash_and_never_by_the_mode() {
        // The mode string looks like the obvious source and is not: BusyBox pads differently,
        // an ACL adds `+`, SELinux adds `.`, and a symlink *to* a directory is `l` in the mode
        // and still a directory to descend into.
        let entries = parse_listing(GNU);
        let bin = entries.iter().find(|e| e.name == "bin").expect("bin");
        assert!(bin.directory);
        assert_eq!(
            bin.size, None,
            "a directory's inode size means nothing to anybody"
        );

        let profile = entries
            .iter()
            .find(|e| e.name == ".profile")
            .expect("dotfile");
        assert!(!profile.directory, "and -A keeps dotfiles");
    }

    #[test]
    fn a_symlink_keeps_its_name_and_reports_its_target() {
        let entries = parse_listing(GNU);
        let lib = entries
            .iter()
            .find(|e| e.name == "lib")
            .expect("the symlink");
        assert_eq!(lib.link.as_deref(), Some("usr/lib"));

        // And a *file* whose name contains ` -> ` is not split: the arrow is only meaningful on
        // a line whose mode says symlink.
        let odd = "-rw-r--r-- 1 root root 5 Sep 9 11:02 a -> b\n";
        let parsed = parse_listing(odd);
        assert_eq!(parsed[0].name, "a -> b");
        assert_eq!(parsed[0].link, None);
    }

    #[test]
    fn a_name_containing_spaces_survives() {
        // `split_whitespace` throws the spacing away, so the name is taken from the original
        // line. A parser that rejoined the fields would rename every such file.
        let entries = parse_listing(GNU);
        assert!(
            entries.iter().any(|e| e.name == "a file with spaces.sh"),
            "{entries:#?}"
        );
    }

    #[test]
    fn directories_sort_first_and_then_by_name_case_insensitively() {
        // `ls` order is the filesystem's and differs between reads; an unsorted tree would
        // shuffle under the user's cursor.
        let listing = "-rw-r--r-- 1 r r 1 Sep 9 11:02 zebra\n\
                       drwxr-xr-x 1 r r 1 Sep 9 11:02 Var/\n\
                       -rw-r--r-- 1 r r 1 Sep 9 11:02 Apple\n\
                       drwxr-xr-x 1 r r 1 Sep 9 11:02 bin/\n";
        let names: Vec<&str> = parse_listing(listing)
            .iter()
            .map(|e| e.name.clone())
            .map(|n| Box::leak(n.into_boxed_str()) as &str)
            .collect();
        assert_eq!(names, vec!["bin", "Var", "Apple", "zebra"]);
    }

    #[test]
    fn total_and_blank_lines_are_not_entries() {
        assert!(parse_listing("total 0\n").is_empty());
        assert!(parse_listing("\n\n").is_empty());
        assert!(parse_listing("").is_empty());
    }

    #[test]
    fn a_path_is_quoted_for_the_shell_and_a_quote_in_it_cannot_escape() {
        // A path is user input and reaches `sh -c`. The whole rule: nothing is special inside
        // single quotes in any POSIX shell — not `$`, not a backtick, not a backslash.
        assert_eq!(shell_quote("/etc"), "'/etc'");
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("/$(id)"), "'/$(id)'");
        assert_eq!(shell_quote("/it's"), r#"'/it'\''s'"#);
        // The argv itself must keep `--`, or a directory whose name starts with `-` is read as
        // a flag.
        let argv = list_argv("-rf");
        assert!(argv[2].contains("-- '-rf'"), "{argv:?}");
    }

    #[test]
    fn a_shell_that_is_not_there_is_a_complaint_and_not_an_empty_directory() {
        // The failure this module exists to report properly: an empty tree is a *lie* about a
        // filesystem that is full, and there is nothing on screen to correct it.
        assert!(complaint_in("/bin/sh: no such file or directory").is_some());
        assert!(complaint_in("ls: /nope: No such file or directory").is_some());
        assert!(complaint_in("sh: ls: not found").is_some());
        // And an ordinary listing is not a complaint.
        assert!(complaint_in(GNU).is_none());
        assert!(complaint_in(BUSYBOX).is_none());
        assert!(complaint_in("").is_none());
    }

    #[test]
    fn paths_are_joined_the_way_a_container_spells_them() {
        // Always `/`, never `PathBuf`: the container is Linux whatever cide is running on.
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(join("", "etc"), "/etc");
        assert_eq!(join("/etc", "hosts"), "/etc/hosts");
        assert_eq!(join("/etc/", "hosts"), "/etc/hosts");
    }
}
