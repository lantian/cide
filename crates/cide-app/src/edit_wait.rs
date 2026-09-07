//! `$CIDE_EDIT_SOCK` — the socket `cide --wait <file>` blocks on. (M20)
//!
//! # What this is for
//!
//! Claude Code's Ctrl+G — *edit this plan / this prompt in an external editor* — is not part of
//! the IDE integration and never was. `cide_core::child_env::editor_env` holds the whole of that
//! diagnosis; the short version is that the CLI resolves `$EDITOR` (falling back to the first of
//! `code`, `vi`, `nano` on `PATH`, which is why the hint read *VS Code*), `spawnSync`s it with a
//! file, **blocks its own turn until that process exits**, and then reads the file back off disk.
//!
//! The read-back is the whole shape of the problem. `cide-ide-mcp` already has an `openFile`, and
//! routing Ctrl+G through it is the obvious-looking fix and the wrong one: the CLI does not need
//! a file *shown*, it needs **a process whose exit means the human is finished**, and a call that
//! returns as soon as a tab appears contains no such moment. So cide grows a second mode of its
//! own binary — one that opens a tab in the running application and does not exit until the tab
//! is gone — and this socket is how the two halves of that one binary find each other.
//!
//! # Why this is a third socket
//!
//! There are now three, and the two that already existed are both wrong for this:
//!
//! * [`crate::hooks`] is **write-and-forget and strictly serialised**. A hook writes one frame
//!   and disconnects; every frame in the process funnels through one applier thread so a `Stop`
//!   cannot be overtaken by a `PostToolUse`. A verb that blocks for as long as a human is editing
//!   would occupy that thread and take every session's busy/idle chrome down with it.
//! * [`crate::agent_rpc`] is **MCP, and its header line is an authorisation**. Everything about
//!   it — the `hello`, the `run`/`session` identity, [`crate::agent_rpc`]'s scope table — exists
//!   to decide which project's tasks a model may write. `cide --wait` is not a model and asks for
//!   no tools; putting a non-MCP verb on that socket would mean the one place in the app that
//!   answers *what may this caller reach* also answers something else.
//!
//! What is shared is the lifecycle, and it is copied from those two deliberately: bind under
//! `$XDG_RUNTIME_DIR` at a path carrying our pid, `0600`, remove any file already at our own path
//! before binding, unlink on `Drop`, one accept thread, one thread per connection.
//!
//! # The protocol
//!
//! Newline-delimited JSON, one object each way, for the reason `crate::hooks` states: a writer
//! that dies mid-write costs one truncated line the reader discards, where a length prefix
//! desynchronises a stream for ever.
//!
//! ```json
//! → {"v":1,"open":"/tmp/claude-…/plan.md","session":"<uuid|null>","cwd":"<path|null>","pid":1234}
//! ← {"v":1,"done":true}
//! ← {"v":1,"error":"a sentence naming cide"}
//! ```
//!
//! **The path must already be absolute.** Only the client knows the cwd it was launched in — the
//! app's own is somewhere else entirely — so resolving it is the client's job and a relative path
//! arriving here is a bug in the client, refused rather than guessed at.
//!
//! # What "finished" means, and why it is not a tab id
//!
//! The wait ends when **no tab anywhere in the workspace is showing that path**. Three cheaper
//! definitions were considered and each is wrong:
//!
//! * *Watch the `TabId` we opened.* A tab detached into its own window and re-docked comes back
//!   with a **fresh id** — `cide_core::workspace` says so where it prunes the old one — so a user
//!   who tears the editor out into a window would release the CLI's turn while still typing.
//! * *Wait for a save.* A human saves several times and closes once. The CLI reads the file after
//!   we exit, so exiting on the first `Ctrl+S` would hand it a half-finished edit.
//! * *Resolve from the close paths, as [`crate::ide`]'s diff broker does.* That is edge-triggered
//!   and needs every close path enumerated — `tab_close`, closing a project, closing a window,
//!   the quit ladder — and a path added later silently stops resolving. The cost of missing one
//!   is not a stale tab: it is a `claude` turn that never ends. The check below is level-
//!   triggered, so it cannot be defeated by a close path that does not exist yet.
//!
//! Level-triggered means polling, and the poll is honest about its cost: one uncontended mutex
//! and a walk of a handful of tab lists every [`TICK`], for as long as one human has one file
//! open. It rides on the socket's own read timeout, so the same syscall that paces the loop is
//! also what notices the client dying — see [`wait_until_closed`].
//!
//! # Two things this deliberately does not do
//!
//! **It does not refuse a path outside every project root.** `cmd::file`'s `terminal_open_path`
//! does refuse those, and rightly — a path scraped out of terminal output is attacker-adjacent.
//! This one is the opposite case: the file is a scratch file the CLI just wrote under a temp
//! directory *by design*, and refusing it would refuse the entire feature.
//!
//! **It does not close the tab when the client goes away.** A `claude` killed mid-edit leaves the
//! user looking at their own unsaved buffer, which is the only answer that cannot lose work.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use cide_ipc::{ProjectId, SessionId, TabKind, WindowRole, Workspace};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::workspace_state::WorkspaceState;

/// The protocol version both ends carry. Bump it when a field is added or removed.
const PROTOCOL_VERSION: u32 = 1;

/// How long the request line may take to arrive before the connection is written off.
///
/// The client writes it immediately after connecting and then never writes again, so anything
/// slower than this is a client that has already failed. Short enough that a stray `nc` cannot
/// hold a thread, long enough to survive a loaded machine.
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the wait loop asks whether the file is still open — and, for free, how quickly it
/// notices the client has died. See [`wait_until_closed`].
const TICK: Duration = Duration::from_millis(250);

/// How long a reply may block before we give up on it.
///
/// One line into a socket whose reader is blocked on exactly that line, so this should never
/// elapse. It is here because the alternative is an unbounded write, and cide can `SIGSTOP` a
/// paused agent's whole process group — `crate::agent_rpc` and `cide_hook::mcp` both carry a
/// timeout for that reason, and this end is inside the same hazard.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

// --- the wire ----------------------------------------------------------------------------------

/// What `cide --wait` asks for.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Request {
    v: u32,
    /// The file to open. Absolute; see the module header.
    open: String,
    /// `$CIDE_SESSION`, when the client had one. **Read from the client's own environment and
    /// never composed** — the rule `cide_hook::forward`'s `spawned_as` states and both other
    /// sockets depend on. Here it only steers which project the tab lands in, but a caller that
    /// could name a session it does not own could open a file in a project it cannot see.
    #[serde(default)]
    session: Option<String>,
    /// The client's working directory, which for a shell pane is the only clue to the project.
    #[serde(default)]
    cwd: Option<String>,
    /// The client's pid. For the log only.
    #[serde(default)]
    pid: Option<u32>,
}

/// What it is told. Exactly one of these goes back, and then the connection closes.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct Reply {
    v: u32,
    /// `true` when the file was opened and has since been closed. Never `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Reply {
    fn done() -> Self {
        Self {
            v: PROTOCOL_VERSION,
            done: true,
            error: None,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            done: false,
            error: Some(message.into()),
        }
    }
}

// --- the server --------------------------------------------------------------------------------

/// The socket, and nothing else.
///
/// No registry of waiters, for [`crate::agent_rpc`]'s reason: nothing in the app addresses a live
/// wait — the tab on screen *is* the state — so a table of them would be a lock with no reader.
pub struct EditWaitServer {
    path: PathBuf,
}

impl EditWaitServer {
    /// Bind the socket and start accepting.
    ///
    /// Failure costs Ctrl+G, and nothing else: `editor_env` sets neither variable when there is
    /// no socket to name, so a child simply gets the environment it had before this existed and
    /// the CLI falls back to its own guess. Degrades rather than refusing to launch.
    pub fn start(app: AppHandle) -> std::io::Result<Self> {
        let path = socket_path();

        // A socket left by a previous run of this same pid would make `bind` fail with
        // `AddrInUse`. Removing it is safe: the path carries our pid, so nothing else owns it.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        // `XDG_RUNTIME_DIR` is usually 0700 and `temp_dir()` is not, so the socket is narrowed
        // rather than trusted to inherit — the same choice `hooks.rs` and `agent_rpc.rs` make.
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        thread::Builder::new()
            .name("cide-edit-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let app = app.clone();
                    // One thread per connection, and **it lives as long as the human is
                    // editing**. That is the whole reason the accept loop may never do the work
                    // itself: a single Ctrl+G would otherwise stop every other pane's editor
                    // from connecting for minutes.
                    let _ = thread::Builder::new()
                        .name("cide-edit-conn".into())
                        .spawn(move || serve(stream, &app));
                }
            })?;

        tracing::info!(path = %path.display(), "edit wait socket listening");
        Ok(Self { path })
    }

    /// The value for a child's `CIDE_EDIT_SOCK`.
    pub fn socket(&self) -> &Path {
        &self.path
    }
}

impl Drop for EditWaitServer {
    fn drop(&mut self) {
        // A socket file outliving its process is a path a child would connect to and then block
        // on with nothing on the other end — and blocking is the entire contract here.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("cide-edit-{}.sock", std::process::id()))
}

// --- one connection ----------------------------------------------------------------------------

/// Read one request, open the tab, and stay until it is gone.
fn serve(stream: UnixStream, app: &AppHandle) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_read_timeout(Some(HEADER_TIMEOUT));

    let mut line = String::new();
    // `&UnixStream` reads, so the borrow ends with the reader and the raw stream is free for the
    // wait loop below. Nothing is buffered past the line: the client writes one and then waits.
    if BufReader::new(&stream).read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }

    let request: Request = match serde_json::from_str(line.trim()) {
        Ok(request) => request,
        Err(error) => {
            reply(
                &stream,
                &Reply::error(format!("cide: unusable request: {error}")),
            );
            return;
        }
    };
    if request.v != PROTOCOL_VERSION {
        reply(
            &stream,
            &Reply::error(format!(
                "cide: this build speaks edit-wait v{PROTOCOL_VERSION}, the caller speaks v{}",
                request.v
            )),
        );
        return;
    }

    let path = PathBuf::from(&request.open);
    if !path.is_absolute() {
        reply(
            &stream,
            &Reply::error("cide: the file to edit must be given as an absolute path"),
        );
        return;
    }

    let Some(state) = app.try_state::<WorkspaceState>() else {
        // Teardown. Saying so is the only honest answer, and it is one the caller can act on.
        reply(
            &stream,
            &Reply::error("cide: the workspace is shutting down"),
        );
        return;
    };

    let Some(project) = state.with(|ws| project_for(ws, &request)) else {
        reply(
            &stream,
            &Reply::error("cide: no project is open to show this file in"),
        );
        return;
    };

    if let Err(error) = crate::cmd::file::open_file_tab(&state, project, path.clone()) {
        reply(&stream, &Reply::error(format!("cide: {error}")));
        return;
    }
    // A second mutation rather than one, because `open_file_tab` owns the deduplication rule that
    // keeps two tabs off one path and reimplementing it here to save a broadcast is exactly the
    // trade that rule exists to refuse. Ctrl+G happens when a person presses a key; two `rev`
    // bumps is the right price for the tab being *visible* when they do — the file may belong to
    // a project in the window behind this one.
    let _ = state.update(|ws| {
        cide_core::workspace::activate_project(ws, project);
        Ok(())
    });

    tracing::debug!(
        path = %path.display(),
        %project,
        pid = request.pid,
        "an external editor is waiting on a file tab"
    );

    if wait_until_closed(&stream, &state, &path) {
        reply(&stream, &Reply::done());
    }
}

/// Block until the file is closed everywhere, or until the client stops caring.
///
/// Returns `true` when the file is gone from the workspace — the one case with an answer to send.
/// `false` means the client disconnected or the socket failed, and there is nobody left to reply
/// to.
///
/// # The read *is* the tick
///
/// The loop needs two things: to re-ask the workspace periodically, and to notice a client that
/// has died so its thread does not outlive it. A blocking read with a [`TICK`] timeout is both.
/// The client writes exactly one line and then waits, so every outcome is unambiguous — `Ok(0)`
/// is EOF and only EOF, a timeout is the client alive and still waiting, and a byte is a client
/// speaking a protocol this build does not have, which is not a reason to abandon a human's open
/// buffer.
///
/// The condition is checked *before* the first read, so a file that is already closed by the time
/// we get here answers immediately rather than after a tick.
fn wait_until_closed(stream: &UnixStream, state: &WorkspaceState, path: &Path) -> bool {
    let _ = stream.set_read_timeout(Some(TICK));
    let mut scratch = [0u8; 1];
    loop {
        if !state.with(|ws| file_is_open(ws, path)) {
            return true;
        }
        match (&mut &*stream).read(&mut scratch) {
            Ok(0) => return false,
            Ok(_) => {}
            Err(error) if would_block(&error) => {}
            Err(_) => return false,
        }
    }
}

/// Did this read time out rather than fail?
///
/// A socket read timeout surfaces as `EAGAIN` on Linux and `ETIMEDOUT` on some other unices, and
/// `std` maps them to two different kinds. Treating only one as a tick would turn the loop into a
/// busy spin on the platform that reports the other.
fn would_block(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Send one line, and never care whether it landed.
///
/// Nothing useful follows a failed reply: the caller is a process that has already gone or a
/// socket that has already broken, and this connection is over either way.
fn reply(mut stream: &UnixStream, reply: &Reply) {
    let Ok(mut line) = serde_json::to_string(reply) else {
        return;
    };
    line.push('\n');
    let _ = stream.write_all(line.as_bytes());
    let _ = stream.flush();
}

// --- the decisions, over a workspace -------------------------------------------------------

/// Is this file showing in any tab, in any project?
///
/// Every project rather than the one it was opened in, because a tab moves: detaching it into its
/// own window gives it a fresh id but leaves it in the same project, and the level-triggered
/// check the module header argues for is only as good as its breadth. Compared by path equality
/// against what the client resolved, which is the same value [`serve`] opened the tab with — see
/// [`cli`] for why the client canonicalises before sending.
fn file_is_open(ws: &Workspace, path: &Path) -> bool {
    ws.projects.values().any(|project| {
        project
            .tabs
            .iter()
            .any(|tab| matches!(&tab.kind, TabKind::File { path: p, .. } if p == path))
    })
}

/// Which project should show this file.
///
/// Four questions in a fixed order, each answering a different caller:
///
/// 1. **The session.** A Claude pane is spawned with `CIDE_SESSION`, so a plan opened from one
///    lands in the project that pane belongs to — even when the file itself is a temp file that
///    is under no project root at all, which is the case Ctrl+G actually produces.
/// 2. **The client's cwd.** A *shell* pane gets no `CIDE_SESSION` (see `cmd::session`), so a
///    `git commit` in one has nothing but where it is standing. That is enough.
/// 3. **The file's own path.** For a caller launched from somewhere unhelpful but editing a file
///    that plainly belongs to an open project.
/// 4. **Whatever the user is looking at.** Last, because it is a guess; still better than
///    refusing, since a tab in the wrong header is visible and recoverable where an error in a
///    CLI's warning line is neither.
fn project_for(ws: &Workspace, request: &Request) -> Option<ProjectId> {
    request
        .session
        .as_deref()
        .filter(|value| !value.is_empty())
        // Parsed, never compared as a string: an id we did not mint must not match anything by
        // being similarly shaped. `agent_rpc::scope_of` makes the identical point.
        .and_then(|value| value.parse::<SessionId>().ok())
        .and_then(|session| project_of_session(ws, session))
        .or_else(|| {
            request
                .cwd
                .as_deref()
                .and_then(|cwd| project_containing(ws, Path::new(cwd)))
        })
        .or_else(|| project_containing(ws, Path::new(&request.open)))
        .or_else(|| active_project(ws))
}

/// The project holding a pane bound to this session.
///
/// Every pane, not `Project::primary_session`: a conversation in the second Claude tab of a
/// project is as entitled to Ctrl+G as its console is, and `primary_session` names only the
/// console.
fn project_of_session(ws: &Workspace, session: SessionId) -> Option<ProjectId> {
    ws.projects
        .iter()
        .find(|(_, project)| {
            project.tabs.iter().any(|tab| {
                tab.tree
                    .panes
                    .values()
                    .any(|pane| pane.session == Some(session))
            })
        })
        .map(|(id, _)| *id)
}

/// The open project whose root contains this path, longest root first.
///
/// Longest wins because roots nest: a project opened on a repository and another opened on one of
/// its subdirectories both contain the file, and the narrower one is the one the user meant.
fn project_containing(ws: &Workspace, path: &Path) -> Option<ProjectId> {
    ws.projects
        .iter()
        .flat_map(|(id, project)| project.roots.iter().map(move |root| (*id, &root.path)))
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len())
        .map(|(id, _)| id)
}

/// The project the user is currently looking at, in whichever shell window has one.
fn active_project(ws: &Workspace) -> Option<ProjectId> {
    ws.windows
        .values()
        .find_map(|role| match role {
            WindowRole::Shell { active, .. } => *active,
            _ => None,
        })
        .or_else(|| ws.projects.keys().copied().next())
}

// --- the client --------------------------------------------------------------------------------

/// Exit code for a wait that ended with the file closed. The CLI reads the file back on this and
/// on nothing else.
const EXIT_DONE: i32 = 0;
/// Exit code for a wait that could not happen. The CLI reports it and leaves the buffer alone.
const EXIT_FAILED: i32 = 1;
/// Exit code for `--wait` with no file after it.
const EXIT_USAGE: i32 = 2;

/// `cide --wait <file>`, if that is what this process was asked to be.
///
/// `None` means these arguments are not a wait and the caller should go on to start the
/// application. `Some(code)` means the whole job is done and the process should exit with it —
/// **before anything touches GTK**, which is why `main` calls this first: this mode must not
/// create a window, and on a machine with no display it must still work.
///
/// # Why the exit code is the answer
///
/// The CLI's contract is `spawnSync`: a non-zero status, a signal or a spawn error means *the
/// editor failed*, and it discards the buffer rather than reading the file back. So every failure
/// here exits non-zero and says why on stderr — which the CLI shows the user, naming cide. The
/// alternative, exiting 0 when nothing happened, would have the CLI read back an unchanged file
/// and report success for an edit that never took place.
pub fn cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = cide_core::child_env::WAIT_FLAG;
    if !args.iter().any(|arg| arg == flag || arg == "-w") {
        return None;
    }
    let Some(target) = args
        .iter()
        .find(|arg| *arg != flag && *arg != "-w" && !arg.starts_with('-'))
    else {
        eprintln!("cide: usage: cide {flag} <file>");
        return Some(EXIT_USAGE);
    };

    Some(match wait_for(target) {
        Ok(()) => EXIT_DONE,
        Err(message) => {
            eprintln!("{message}");
            EXIT_FAILED
        }
    })
}

/// Ask the running cide to open `target`, and block until it is closed.
///
/// The `Err` is the sentence the user sees. Every one of them names cide, so a line read out of a
/// CLI's log points at the process responsible rather than at "the editor".
fn wait_for(target: &str) -> Result<(), String> {
    let Some(socket) = std::env::var_os("CIDE_EDIT_SOCK") else {
        // Reachable only from a shell that is not cide's child — `editor_env` sets `EDITOR` and
        // this variable together or not at all — so the useful thing to say is *which* cide.
        return Err("cide: not running inside a cide session ($CIDE_EDIT_SOCK is unset)".into());
    };

    // Resolved **here** and not in the app: the cwd this was launched in is the `claude`'s, and
    // the app's own is somewhere else entirely. `canonicalize` on top of that so the path we ask
    // to be opened is the same string a tab already showing this file through a symlink would
    // carry — the comparison that ends the wait is path equality, and two spellings of one file
    // would make it wait for a tab nobody has open.
    let absolute = std::path::absolute(target)
        .map_err(|error| format!("cide: cannot resolve {target}: {error}"))?;
    let path = std::fs::canonicalize(&absolute).unwrap_or(absolute);

    let stream = UnixStream::connect(&socket).map_err(|error| {
        format!(
            "cide: cannot reach the cide that started this session ({}): {error}",
            Path::new(&socket).display()
        )
    })?;
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));

    let request = Request {
        v: PROTOCOL_VERSION,
        open: path.to_string_lossy().into_owned(),
        // From our own environment, never from an argument. See [`Request::session`].
        session: std::env::var("CIDE_SESSION").ok(),
        cwd: std::env::current_dir()
            .ok()
            .map(|cwd| cwd.to_string_lossy().into_owned()),
        pid: Some(std::process::id()),
    };
    let mut line = serde_json::to_string(&request).map_err(|error| format!("cide: {error}"))?;
    line.push('\n');
    (&stream)
        .write_all(line.as_bytes())
        .map_err(|error| format!("cide: cannot send the file to cide: {error}"))?;

    // **No read timeout at all**, and that is the contract rather than an oversight: the answer
    // comes when a human closes a tab, and there is no length of time after which "still editing"
    // becomes an error. The connection dying is what ends this early, and it is what happens if
    // cide quits.
    let mut answer = String::new();
    BufReader::new(&stream)
        .read_line(&mut answer)
        .map_err(|error| format!("cide: lost contact with cide: {error}"))?;
    if answer.trim().is_empty() {
        return Err("cide: cide exited before the file was closed".into());
    }

    let reply: Reply = serde_json::from_str(answer.trim())
        .map_err(|error| format!("cide: unusable answer from cide: {error}"))?;
    match reply.error {
        Some(message) => Err(message),
        None if reply.done => Ok(()),
        None => Err("cide: cide answered without opening the file".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{Pane, PaneId, PaneKind, PaneRole, ProjectRoot};

    fn request(open: &str, session: Option<&str>, cwd: Option<&str>) -> Request {
        Request {
            v: PROTOCOL_VERSION,
            open: open.to_string(),
            session: session.map(str::to_string),
            cwd: cwd.map(str::to_string),
            pid: None,
        }
    }

    /// A workspace with two projects, the second nested inside the first's root.
    fn workspace() -> (Workspace, ProjectId, ProjectId) {
        let mut ws = cide_core::workspace::demo_workspace();
        let ids: Vec<ProjectId> = ws.projects.keys().copied().take(2).collect();
        let (outer, inner) = (ids[0], ids[1]);
        ws.projects[&outer].roots = vec![ProjectRoot {
            path: PathBuf::from("/home/u/work"),
            label: "work".into(),
        }];
        ws.projects[&inner].roots = vec![ProjectRoot {
            path: PathBuf::from("/home/u/work/nested"),
            label: "nested".into(),
        }];
        (ws, outer, inner)
    }

    /// Give a project a pane bound to `session`, and answer with the session.
    fn bind_session(ws: &mut Workspace, project: ProjectId) -> SessionId {
        let session = SessionId::new();
        let tab = &mut ws.projects[&project].tabs[0];
        let pane = tab.tree.panes.values_mut().next().expect("a pane");
        pane.session = Some(session);
        session
    }

    /// Put a file tab on a project through the domain's own `open_tab`.
    ///
    /// Not a hand-built `Tab`: the shape a file tab actually has — one auxiliary editor pane —
    /// is `open_file_tab`'s, and a fixture that invented its own would be asserting against a
    /// tab this app never produces.
    fn open_file(ws: &mut Workspace, project: ProjectId, path: &str) -> cide_ipc::TabId {
        cide_core::workspace::open_tab(
            ws,
            project,
            TabKind::File {
                path: PathBuf::from(path),
                dirty: false,
            },
            Pane {
                id: PaneId::new(),
                kind: PaneKind::Editor,
                role: PaneRole::Auxiliary,
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                title: "f".into(),
            },
        )
        .expect("opens")
    }

    /// The case in the report: a plan under `/tmp`, which is inside no project at all.
    ///
    /// Every other rule would have had to guess, and the session is the one fact that is not a
    /// guess — it is the id cide minted for that pane's own child.
    #[test]
    fn a_temp_file_lands_in_the_project_whose_session_asked_for_it() {
        let (mut ws, _outer, inner) = workspace();
        let session = bind_session(&mut ws, inner);
        let plan = "/tmp/claude-x/plan.md";
        assert_eq!(
            project_for(&ws, &request(plan, Some(&session.to_string()), None)),
            Some(inner),
            "the pane that pressed Ctrl+G decides, not the path"
        );
        // And the contrast that makes the rule load-bearing: with no session the same file has
        // nothing to place it by and falls all the way through to a guess.
        assert_eq!(
            project_for(&ws, &request(plan, None, None)),
            active_project(&ws)
        );
    }

    /// A shell pane never gets `CIDE_SESSION`, so `git commit` has only where it is standing.
    #[test]
    fn a_shell_with_no_session_is_placed_by_its_working_directory() {
        let (ws, _outer, inner) = workspace();
        assert_eq!(
            project_for(
                &ws,
                &request(
                    "/tmp/x/COMMIT_EDITMSG",
                    None,
                    Some("/home/u/work/nested/src")
                )
            ),
            Some(inner)
        );
    }

    /// Roots nest, and the narrower one is the one the user meant.
    #[test]
    fn the_innermost_project_wins_a_nested_root() {
        let (ws, outer, inner) = workspace();
        assert_eq!(
            project_containing(&ws, Path::new("/home/u/work/nested/src/main.rs")),
            Some(inner)
        );
        assert_eq!(
            project_containing(&ws, Path::new("/home/u/work/README.md")),
            Some(outer)
        );
    }

    /// A session id this process never minted must not be matched by resembling one.
    #[test]
    fn an_unparseable_session_falls_through_rather_than_matching() {
        let (ws, _outer, inner) = workspace();
        // Names the nested root, so a fall-through has somewhere unambiguous to land.
        let placed = project_for(
            &ws,
            &request("/home/u/work/nested/a.rs", Some("not-a-uuid"), None),
        );
        assert_eq!(placed, Some(inner));
    }

    /// With nothing else to go on, the file still opens — in whatever the user is looking at.
    #[test]
    fn an_unplaceable_file_lands_in_the_active_project_rather_than_being_refused() {
        let (ws, _outer, _inner) = workspace();
        let placed = project_for(
            &ws,
            &request("/tmp/nowhere/x.md", None, Some("/tmp/nowhere")),
        );
        assert!(placed.is_some(), "refusing here would refuse the feature");
        assert_eq!(placed, active_project(&ws));
    }

    /// The wait ends on the file being gone from *every* project, not from the one it opened in.
    ///
    /// This is what survives a tab being detached into its own window and re-docked, which gives
    /// it a fresh `TabId` — the reason the wait is not watching an id.
    #[test]
    fn the_wait_condition_is_the_path_anywhere_and_not_the_tab_it_opened() {
        let (mut ws, outer, inner) = workspace();
        let path = Path::new("/tmp/claude-x/plan.md");
        assert!(!file_is_open(&ws, path));

        let first = open_file(&mut ws, inner, "/tmp/claude-x/plan.md");
        assert!(file_is_open(&ws, path));

        // The same file, now in the other project and under a **different tab id** — which is
        // what a detach into its own window and a re-dock actually produces. Closed through the
        // domain's own `close_tab`, so the removal is the one the app performs.
        let second = open_file(&mut ws, outer, "/tmp/claude-x/plan.md");
        cide_core::workspace::close_tab(&mut ws, inner, first, true).expect("closes");
        assert_ne!(first, second);
        assert!(
            file_is_open(&ws, path),
            "a tab that moved is still a file the human has open"
        );

        cide_core::workspace::close_tab(&mut ws, outer, second, true).expect("closes");
        assert!(!file_is_open(&ws, path));
    }

    /// A near-miss path does not end somebody else's wait.
    #[test]
    fn a_different_file_does_not_satisfy_the_wait() {
        let (mut ws, _outer, inner) = workspace();
        let _ = open_file(&mut ws, inner, "/tmp/claude-x/plan.md.bak");
        assert!(!file_is_open(&ws, Path::new("/tmp/claude-x/plan.md")));
    }

    /// Both ends agree on the flag, and it is spelled in exactly one place.
    #[test]
    fn the_flag_the_client_parses_is_the_one_the_environment_advertises() {
        assert_eq!(cide_core::child_env::WAIT_FLAG, "--wait");
    }
}
