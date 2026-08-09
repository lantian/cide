//! Discovery: the file in `~/.claude/ide/` that tells a `claude` child we are here.
//!
//! The CLI has no configuration for finding an IDE. It lists `~/.claude/ide/*.lock`, parses
//! each one, and connects to the port named by the filename. So this module is the whole of
//! how cide announces itself — and removing the file is the whole of how it stops claiming to
//! be running. A lockfile outliving its process is worse than no lockfile at all: the CLI
//! will try the port, fail, and report a broken IDE rather than no IDE.
//!
//! # The token is a credential
//!
//! `authToken` is the only thing between any local process and an RPC channel that can open
//! diffs over arbitrary paths and be told where the user is looking. The directory is `0700`
//! and the file `0600` so that only the user's own processes can read it, and the token comes
//! from the operating system's CSPRNG. See [`random_token`] for why a UUID is not a
//! substitute.
//!
//! # The directory is shared
//!
//! Every editor with Claude Code integration writes here. VS Code, JetBrains and cide all put
//! their lockfiles in one directory with no namespacing, which makes [`sweep_stale`] the most
//! dangerous function in this crate: an over-eager sweep disconnects the user's other editor.
//!
//! # Why a shared directory is nonetheless safe to use, in both directions
//!
//! Two facts make it so. Neither is deducible from the lockfile format, and both were read
//! out of the 2.1.226 binary, so they are written down here rather than rediscovered.
//!
//! **Other editors cannot confuse a `claude` we spawned — but only because we set
//! `CLAUDE_CODE_SSE_PORT`.** That variable is the precondition for all of this, not a
//! convenience, and stating it without the condition would be false. Writing `n` for its
//! value, the CLI does three separate things with it, and every one is gated on it:
//!
//! ```text
//! else if(c.port===n) u=!0;                    // isValid, skipping cwd containment
//! if(!(n!==null&&c.port===n)){                 // else require pid alive AND our ancestor
//!   if(!c.pid||!m0d(c.pid))continue;
//!   if(process.ppid!==c.pid){ if(!(await a()).has(c.pid))continue }
//! }
//! if(!e&&n){ let c=t.filter((u)=>u.isValid&&u.port===n); if(c.length===1) return c }
//! ```
//!
//! With `n` set — which every pane we spawn has — a port match alone makes our lockfile valid,
//! skips the liveness and PID-ancestry checks, and filters the candidate list *before* the
//! exactly-one count, so a user's VS Code or JetBrains lockfile alongside ours is excluded
//! rather than competing with it. This is what lets the real-CLI integration test run against
//! the true `$HOME` — which it must, since the child resolves the lockfile under its own
//! `HOME` and its subscription credentials live there too.
//!
//! Without `n`, none of that happens: the `if(!e&&n)` branch never runs, the function returns
//! the whole unfiltered list, and disambiguating among every valid lockfile in a shared
//! directory is exactly as fragile as it sounds. That is the situation for a `claude` a user
//! started by hand in a terminal, and it would silently become ours again if a spawn path
//! ever stopped setting the variable.
//!
//! **As of this writing nothing in production sets it.** `CLAUDE_CODE_SSE_PORT` appears only
//! in this crate's tests; `cide-claude` is still a stub and the app does not yet spawn panes
//! against a server. So the paragraph above describes a guarantee the wiring must *establish*,
//! not one it currently has. Whoever lands that wiring owns it: set the variable on every
//! Claude child from `IdeServer::port()`, or everything here degrades to the fragile case
//! while this comment still claims otherwise.
//!
//! **We cannot strand the user's other editors, and a crash cannot strand us.** Our sweep
//! refuses anything whose `ideName` is not [`IDE_NAME`], so it is structurally incapable of
//! deleting someone else's file. In the other direction, the CLI prunes on its own terms —
//! unparseable, or `kill(pid, 0)` failing — *regardless* of `ideName`, so a lockfile we leave
//! behind by being `SIGKILL`ed is cleaned up by the next CLI that reads the directory rather
//! than lying in wait. The two rules are asymmetric on purpose and only safe together.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{IdeError, Result};

/// The `ideName` we publish, and the only value [`sweep_stale`] will delete.
pub const IDE_NAME: &str = "cide";

/// The `transport` we publish. **The only value that selects WebSocket is the bare `"ws"`.**
///
/// This is not a naming preference, and it is worth stating exactly because the obvious
/// guess is wrong. The CLI's lockfile reader in 2.1.226 does, verbatim:
///
/// ```text
/// i = u.transport === "ws"
/// ... return { ..., useWebSocket: i, ... }
/// ```
///
/// `useWebSocket` *is* that equality test. Anything else — including the plausible-looking
/// `"ws-ide"` — makes it false and sends the CLI down the SSE branch, where it issues
/// `GET /sse` with `Accept: text/event-stream` and never attempts an upgrade. The server in
/// this crate speaks only WebSocket, so publishing `"ws-ide"` does not degrade the
/// connection, it silently prevents one: no handshake, no error, just an IDE that never
/// connects.
///
/// `"ws-ide"` and `"sse-ide"` are real strings in the binary, which is what makes this trap
/// convincing. They are values of the `type` field of an *MCP server config* — members of
/// the enum `["stdio","sse","sse-ide","http","ws","sdk"]` — that the CLI constructs for
/// itself after deriving a URL scheme. They are never read from a lockfile.
pub const TRANSPORT: &str = "ws";

/// 256 bits. Long enough that guessing is not a strategy, short enough to sit in a header.
const TOKEN_BYTES: usize = 32;

/// A published lockfile, which is removed when this handle is dropped.
pub struct Lockfile {
    path: PathBuf,
    token: String,
}

impl std::fmt::Debug for Lockfile {
    /// Hand-written rather than derived because `token` is a bearer credential. A derived
    /// `Debug` puts it in full into any `tracing` field or `{:?}` that ever formats the
    /// handle, and a token in the application log defeats the 0600 file mode entirely — the
    /// log is the one copy of it that outlives the process and is not mode-protected.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lockfile")
            .field("path", &self.path)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The JSON body, exactly as the CLI reads it.
///
/// Field names are the protocol's. `pid` is lowercase where everything around it is
/// camelCase, so `rename_all` covers the rest and leaves that one alone by coincidence rather
/// than by exception.
// No `Debug`: this borrows the auth token, and a derived `Debug` is the easiest accidental
// route from a credential to a log file.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Body<'a> {
    pid: u32,
    workspace_folders: Vec<String>,
    ide_name: &'a str,
    transport: &'a str,
    auth_token: &'a str,
    running_in_windows: bool,
}

/// The two fields [`sweep_stale`] needs. Everything else in another editor's lockfile is none
/// of our business, and refusing to parse an unfamiliar field would make us delete files we
/// merely failed to understand.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    ide_name: Option<String>,
}

impl Lockfile {
    /// Write `~/.claude/ide/<port>.lock` and return the handle that owns it.
    pub fn publish(port: u16, workspace_folders: Vec<PathBuf>) -> Result<Self> {
        Self::publish_in(&ide_dir()?, port, workspace_folders)
    }

    /// Publish into an explicit directory instead of `~/.claude/ide`.
    ///
    /// Public for the sake of tests, and deliberately so. The alternative — a test that sets
    /// `HOME` for the whole process — is `unsafe` in edition 2024, has to be serialised
    /// against every other test in the binary, and cannot be serialised at all against a
    /// *different* test binary running concurrently. Worse, a test that mutates `HOME` and
    /// then fails or panics between setting and restoring it leaves the real
    /// `~/.claude/ide` as the target for whatever runs next, which is how a test suite
    /// deletes a developer's live VS Code lockfile. An explicit directory parameter removes
    /// the whole category.
    pub fn publish_in(dir: &Path, port: u16, workspace_folders: Vec<PathBuf>) -> Result<Self> {
        // `mode` applies only to directories this call actually creates, which is the
        // behaviour we want: an existing `~/.claude/ide` keeps whatever permissions the
        // user's other editor gave it. Widening or narrowing another tool's directory is not
        // ours to do, and the file mode below is what protects our token either way.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| lock_err(dir, e))?;

        let token = random_token()?;
        let body = Body {
            pid: std::process::id(),
            // JSON has no encoding for a non-UTF-8 path. Refusing to publish would deny IDE
            // integration to a project whose path we can still open perfectly well, so the
            // lossy form goes on the wire; the CLI only ever compares these as strings.
            workspace_folders: workspace_folders
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            ide_name: IDE_NAME,
            transport: TRANSPORT,
            auth_token: &token,
            running_in_windows: cfg!(target_os = "windows"),
        };
        let json = serde_json::to_vec(&body).map_err(|e| IdeError::Lockfile(e.to_string()))?;

        let path = dir.join(format!("{port}.lock"));
        write_atomically(&path, &json).map_err(|e| lock_err(&path, e))?;

        Ok(Self { path, token })
    }

    /// The token a connecting CLI must present in `X-Claude-Code-Ide-Authorization`.
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the file. Safe to call more than once, and safe when something else already
    /// deleted it — both happen on the shutdown path, where an explicit `remove` is followed
    /// by the drop of the same handle.
    pub fn remove(&self) {
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != io::ErrorKind::NotFound
        {
            // Not fatal, but worth saying: what is left behind will send the next CLI to a
            // port with nothing on it.
            tracing::warn!(path = %self.path.display(), error = %e, "could not remove the IDE lockfile");
        }
    }
}

impl Drop for Lockfile {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Remove lockfiles written by **this application** whose process is gone.
///
/// # This is the most dangerous function in the crate
///
/// `~/.claude/ide` is shared with every other editor the user runs, with no namespacing of
/// any kind. Deleting VS Code's lockfile disconnects a session that has nothing to do with
/// us, and the user has no way to tell what happened. Three rules keep that from happening,
/// and none of them may be relaxed for tidiness:
///
/// * a file that does not parse is **left alone** — not understanding a file is not evidence
///   that it is dead;
/// * a file whose `ideName` is not [`IDE_NAME`] is **left alone**, however dead its pid;
/// * a pid is treated as alive unless the kernel says specifically that no such process
///   exists.
///
/// The failure this biases towards is leaving a stale file behind, because a pid can be
/// recycled by an unrelated process between our writing the file and reading it back. A
/// missed sweep costs one failed connection attempt; a wrong sweep costs someone else's
/// session.
///
/// Returns the number of files removed.
pub fn sweep_stale() -> usize {
    match ide_dir() {
        Ok(dir) => sweep_stale_in(&dir),
        // No HOME means no directory to sweep, which is not a failure worth reporting.
        Err(_) => 0,
    }
}

/// The pid encoded in one of our temp filenames, if this is one.
///
/// [`write_atomically`] names its temp `.<port>.lock.<pid>.tmp`, which is what makes an
/// abandoned one identifiable at all: the contents may be a partial write with no parseable
/// `ideName`, so the name is the only evidence available. Three things must match — the
/// leading dot, the `.lock` stem and the `.tmp` suffix — so an unrelated dotfile is not a
/// candidate however it happens to end.
fn temp_file_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (stem, pid) = rest.rsplit_once('.')?;
    if !stem.ends_with(".lock") {
        return None;
    }
    pid.parse().ok()
}

fn sweep_stale_in(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();

        // Abandoned temp files first, because they are not `.lock` and would otherwise be
        // skipped by the extension check below and accumulate for the life of the account.
        // One is left behind whenever a process dies between `open(2)` and `rename(2)`.
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(pid) = temp_file_pid(name)
        {
            // Safe by construction, and safe even in the impossible case that some other
            // tool adopted this exact naming: a temp file whose owning process is gone is
            // abandoned by definition, and nothing will ever rename it into place.
            if !pid_is_alive(pid) && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
            continue;
        }

        if path.extension() != Some(OsStr::new("lock")) {
            continue;
        }
        // An unreadable file may be another user's, or may be mid-rename. Either way it is
        // not established as ours and dead.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(claim) = serde_json::from_str::<Claim>(&text) else {
            continue;
        };
        if claim.ide_name.as_deref() != Some(IDE_NAME) {
            continue;
        }
        let Some(pid) = claim.pid else {
            continue;
        };
        if pid_is_alive(pid) {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `~/.claude/ide`, the one location the CLI looks in.
fn ide_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| {
        IdeError::Lockfile("HOME is not set, so ~/.claude/ide cannot be found".into())
    })?;
    Ok(PathBuf::from(home).join(".claude").join("ide"))
}

/// A bearer token from the operating system's CSPRNG.
///
/// Not a UUID. A UUID is an *identifier*, and its type contract is uniqueness rather than
/// unpredictability: v1 and v7 embed a timestamp and are partly guessable by construction,
/// and nothing stops a later change from swapping v4 for v7 "so they sort" and quietly
/// turning this credential into a countdown. Nor is it derived from the port or the pid, both
/// of which any local process can read out of `/proc`. Taking bytes straight from the kernel
/// makes the security property a property of the code rather than of a version number.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| IdeError::Lockfile(format!("no system randomness available: {e}")))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Both indices are a nibble, so they are in range by construction.
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 0x0f)] as char);
    }
    out
}

/// Write to a temp file in the same directory, then rename over the target.
///
/// The CLI scans the directory on its own schedule and rejects a lockfile it cannot parse,
/// with no retry. Writing in place would give it a window in which the file exists and is
/// half a JSON object, and the cost of losing that race is an IDE that appears to be present
/// and broken.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or(OsStr::new("lock"));
    // Named after the pid so two cide processes publishing at once cannot collide, and
    // hidden so a scanning CLI ignores it even before the extension check.
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));

    // `create_new` rather than `create`: a leftover temp file from a crashed process must not
    // be inherited, because its mode is not ours to trust. `mode` is passed to `open(2)`
    // rather than applied afterwards so the token never exists at a readable mode, not even
    // for an instant. A umask can only clear further bits, never add them, so this is a
    // ceiling rather than a hope.
    let opened = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp);
    let mut file = match opened {
        Ok(f) => f,
        // A stale temp file is the one case worth a second attempt: clear it and retry once,
        // so a previous crash does not permanently prevent publishing.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(&tmp)?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?
        }
        Err(e) => return Err(e),
    };

    let written = file
        .write_all(bytes)
        // rename makes the *name* appear atomically, not the contents: without the flush a
        // crash between the two can publish a file of zeroes at the final path.
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Whether a process with this pid exists, erring towards "yes".
fn pid_is_alive(pid: u32) -> bool {
    // `kill(0, ...)` addresses the caller's entire process group, so a zero pid in a lockfile
    // must never reach `kill`. It is nonsense in a lockfile anyway; call it alive and leave
    // the file for a human.
    if pid == 0 {
        return true;
    }
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };

    // Signal 0 performs the permission and existence checks without delivering anything.
    // SAFETY: `kill` with signal 0 has no effect beyond setting errno.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // EPERM means the process exists and belongs to someone else; only ESRCH means it is
    // gone. Reading any other error as death would delete a live editor's lockfile.
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn lock_err(path: &Path, e: io::Error) -> IdeError {
    IdeError::Lockfile(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    use parking_lot::{Mutex, MutexGuard};

    /// `HOME` is process-wide, so the tests that move it cannot run concurrently. parking_lot
    /// rather than std because a failing assertion inside the guard must not poison every
    /// subsequent test.
    static HOME_LOCK: Mutex<()> = Mutex::new(());
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A temporary `HOME`, restored on drop.
    struct TempHome {
        dir: PathBuf,
        previous: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }

    impl TempHome {
        fn new() -> Self {
            let guard = HOME_LOCK.lock();
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("cide-lockfile-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp home");
            let previous = std::env::var_os("HOME");
            // SAFETY: HOME_LOCK serialises every test in this binary that reads *or* writes
            // the environment. `Command::spawn` counts as a read — it copies the whole
            // environ block to build the child's — so `a_dead_pid` must only ever be called
            // with this lock held, or the copy races a `setenv` that reallocates the block.
            unsafe { std::env::set_var("HOME", &dir) };
            Self {
                dir,
                previous,
                _guard: guard,
            }
        }

        fn ide_dir(&self) -> PathBuf {
            self.dir.join(".claude").join("ide")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            // SAFETY: as above; the guard is still held for the rest of this drop.
            unsafe {
                match &self.previous {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    /// A pid that certainly names no process: spawn a child, wait for it, and reuse its pid
    /// once it has been reaped. Inventing a large number instead risks hitting a real
    /// process and turning "is it swept" into a coin toss.
    ///
    /// Only call this with `HOME_LOCK` held: spawning copies the environment, which must not
    /// run concurrently with the `set_var` in [`TempHome::new`].
    fn a_dead_pid() -> u32 {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        child.wait().expect("reap");
        pid
    }

    fn write_claim(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).expect("dir");
        let path = dir.join(name);
        fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn a_published_lockfile_carries_exactly_the_keys_the_cli_reads() {
        let home = TempHome::new();
        let lock = Lockfile::publish(41234, vec![PathBuf::from("/w/one")]).expect("publish");

        assert_eq!(lock.path(), home.ide_dir().join("41234.lock"));

        let text = fs::read_to_string(lock.path()).expect("read");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let object = value.as_object().expect("object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        // Exact, not a superset: an unrecognised key has been enough to make the CLI reject
        // a lockfile, and a missing one certainly is.
        assert_eq!(
            keys,
            [
                "authToken",
                "ideName",
                "pid",
                "runningInWindows",
                "transport",
                "workspaceFolders"
            ]
        );
        assert_eq!(object["ideName"], "cide");
        // Deliberately the literal and not [`TRANSPORT`]. Asserting a constant against
        // itself is vacuous, and this field is the one place where a wrong-but-plausible
        // value costs nothing at compile time, passes every unit test, and then simply
        // never connects: the CLI computes `useWebSocket` as `transport === "ws"`, so
        // "ws-ide" silently routes it to SSE. An earlier version of this file pinned
        // "ws-ide" here, which is how the mistake would have shipped green.
        assert_eq!(object["transport"], "ws");
        assert_eq!(object["runningInWindows"], false);
        assert_eq!(object["pid"], std::process::id());
        assert_eq!(object["workspaceFolders"], serde_json::json!(["/w/one"]));
        assert_eq!(object["authToken"], lock.token());
    }

    #[test]
    fn the_token_is_random_and_never_world_readable() {
        let home = TempHome::new();
        let first = Lockfile::publish(41235, vec![]).expect("publish");
        let second = Lockfile::publish(41236, vec![]).expect("publish");

        assert_eq!(first.token().len(), TOKEN_BYTES * 2);
        assert!(first.token().chars().all(|c| c.is_ascii_hexdigit()));
        // Two tokens from one process within the same instant: anything derived from the
        // port or the pid would be predictable from data in /proc, and this is the cheapest
        // check that neither is what we published.
        assert_ne!(first.token(), second.token());

        assert_eq!(
            mode_of(first.path()),
            0o600,
            "the token must not be readable by other users"
        );
        assert_eq!(mode_of(&home.ide_dir()), 0o700);
    }

    #[test]
    fn dropping_the_handle_removes_the_file() {
        let home = TempHome::new();
        let path = {
            let lock = Lockfile::publish(41237, vec![]).expect("publish");
            lock.path().to_path_buf()
        };
        assert!(
            !path.exists(),
            "a lockfile outliving its process misdirects the next CLI"
        );
        assert!(home.ide_dir().exists(), "the shared directory stays");
    }

    #[test]
    fn removing_twice_and_then_dropping_is_fine() {
        let _home = TempHome::new();
        let lock = Lockfile::publish(41238, vec![]).expect("publish");
        let path = lock.path().to_path_buf();

        lock.remove();
        lock.remove();
        assert!(!path.exists());
        drop(lock);
    }

    #[test]
    fn a_dead_cide_lockfile_is_swept_and_a_live_one_is_kept() {
        let home = TempHome::new();
        let live = Lockfile::publish(41239, vec![]).expect("publish");
        let dead = write_claim(
            &home.ide_dir(),
            "41240.lock",
            &format!(
                r#"{{"pid":{},"workspaceFolders":[],"ideName":"cide","transport":"ws","authToken":"x","runningInWindows":false}}"#,
                a_dead_pid()
            ),
        );

        assert_eq!(sweep_stale(), 1);
        assert!(!dead.exists());
        assert!(
            live.path().exists(),
            "our own running server must survive a sweep"
        );
    }

    #[test]
    fn another_editors_lockfile_is_never_swept() {
        let home = TempHome::new();
        let theirs = write_claim(
            &home.ide_dir(),
            "41241.lock",
            &format!(
                r#"{{"pid":{},"workspaceFolders":[],"ideName":"Visual Studio Code","transport":"ws","authToken":"x","runningInWindows":false}}"#,
                a_dead_pid()
            ),
        );

        // Dead by our reckoning and still not ours to delete: the pid may have been recycled,
        // and being wrong here disconnects a session in a different application.
        assert_eq!(sweep_stale(), 0);
        assert!(theirs.exists());
    }

    #[test]
    fn an_unparseable_lockfile_is_left_alone() {
        let home = TempHome::new();
        let truncated = write_claim(&home.ide_dir(), "41242.lock", r#"{"pid":1,"ideNa"#);
        let empty = write_claim(&home.ide_dir(), "41243.lock", "");
        // Right shape, wrong type for the one field that decides deletion.
        let odd = write_claim(
            &home.ide_dir(),
            "41244.lock",
            r#"{"pid":"1234","ideName":"cide"}"#,
        );

        assert_eq!(sweep_stale(), 0);
        assert!(truncated.exists() && empty.exists() && odd.exists());
    }

    #[test]
    fn a_lockfile_with_no_pid_is_left_alone() {
        let home = TempHome::new();
        let path = write_claim(&home.ide_dir(), "41245.lock", r#"{"ideName":"cide"}"#);
        assert_eq!(sweep_stale(), 0);
        assert!(path.exists());
    }

    #[test]
    fn two_ports_coexist() {
        let home = TempHome::new();
        let a = Lockfile::publish(41246, vec![PathBuf::from("/w/a")]).expect("publish");
        let b = Lockfile::publish(41247, vec![PathBuf::from("/w/b")]).expect("publish");

        assert_ne!(a.path(), b.path());
        assert!(a.path().exists() && b.path().exists());
        assert_eq!(fs::read_dir(home.ide_dir()).expect("read_dir").count(), 2);

        drop(a);
        assert!(!home.ide_dir().join("41246.lock").exists());
        assert!(
            b.path().exists(),
            "one project closing must not unpublish another"
        );
    }

    #[test]
    fn a_stale_temp_file_does_not_block_publishing() {
        let home = TempHome::new();
        let dir = home.ide_dir();
        fs::create_dir_all(&dir).expect("dir");
        let tmp = dir.join(format!(".41248.lock.{}.tmp", std::process::id()));
        fs::write(&tmp, "leftover").expect("write");

        let lock = Lockfile::publish(41248, vec![]).expect("publish");
        assert!(lock.path().exists());
        assert!(!tmp.exists(), "the temp file must not survive the rename");
    }

    #[test]
    fn a_temp_file_from_a_dead_process_is_swept() {
        // Left behind by a crash between `open(2)` and `rename(2)`. Nothing else removes it:
        // it is not a `.lock`, so the extension check skips it, and it holds a real auth
        // token, so one accumulates per hard kill for the life of the account.
        let home = TempHome::new();
        let dir = home.ide_dir();
        fs::create_dir_all(&dir).expect("dir");

        let dead = a_dead_pid();
        let abandoned = dir.join(format!(".41250.lock.{dead}.tmp"));
        fs::write(&abandoned, "partial").expect("write");

        assert_eq!(sweep_stale(), 1);
        assert!(
            !abandoned.exists(),
            "an abandoned temp file survived the sweep"
        );
    }

    #[test]
    fn a_temp_file_from_a_live_process_is_left_alone() {
        // Another cide, publishing right now. Removing its temp mid-write would make its
        // `rename` fail and cost it IDE integration entirely.
        let home = TempHome::new();
        let dir = home.ide_dir();
        fs::create_dir_all(&dir).expect("dir");

        let live = dir.join(format!(".41251.lock.{}.tmp", std::process::id()));
        fs::write(&live, "in flight").expect("write");

        assert_eq!(sweep_stale(), 0);
        assert!(
            live.exists(),
            "a live process's temp file was deleted from under it"
        );
    }

    #[test]
    fn only_our_own_temp_shape_is_a_candidate() {
        // The sweep runs in a directory shared with every other editor, so the name has to be
        // specific. All three of the leading dot, the `.lock` stem and the `.tmp` suffix are
        // required; anything else is somebody else's file.
        assert_eq!(temp_file_pid(".41234.lock.9182.tmp"), Some(9182));

        for other in [
            "41234.lock.9182.tmp",  // not hidden
            ".41234.lock.9182",     // no .tmp
            ".41234.json.9182.tmp", // not a lockfile temp
            ".vimrc.swp.tmp",       // no pid at all
            ".41234.lock.notapid.tmp",
        ] {
            assert_eq!(temp_file_pid(other), None, "{other} was treated as ours");
        }
    }

    #[test]
    fn sweeping_ignores_files_that_are_not_lockfiles() {
        let home = TempHome::new();
        let other = write_claim(
            &home.ide_dir(),
            "notes.json",
            r#"{"pid":1,"ideName":"cide"}"#,
        );
        assert_eq!(sweep_stale(), 0);
        assert!(other.exists());
    }

    #[test]
    fn our_own_pid_is_alive_and_a_reaped_child_is_not() {
        // Held only for `a_dead_pid`, which spawns: the environment copy that `posix_spawn`
        // makes must not overlap another test's `set_var`.
        let _guard = HOME_LOCK.lock();
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(a_dead_pid()));
        // pid 1 always exists and is not ours to signal; EPERM must read as alive.
        assert!(pid_is_alive(1));
        assert!(pid_is_alive(0), "a zero pid must never reach kill(2)");
    }
}
