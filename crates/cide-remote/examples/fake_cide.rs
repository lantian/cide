//! A cide that serves nothing but exists, so another implementation can talk to it. (M72)
//!
//! The companion application's crypto is checked against generated vectors, and its connection
//! logic is checked against a test double. Neither of those is the same as *the two programs
//! speaking to each other*, and the gap between them is where a protocol lives: a field name that
//! serialises differently, a frame the client never sends because it is the server that starts,
//! an assumption about who says the first word.
//!
//! So this is a real [`RemoteServer`], with a real sealed transport, over a real socket, serving
//! a host made of constants. It needs no workspace, no PTY and no `claude`.
//!
//! ```sh
//! cargo run -p cide-remote --example fake_cide -- <dir> <port>
//! ```
//!
//! It writes its own key and device list into `<dir>` using nothing but the ordinary API — which
//! is deliberate. An example that needed a constructor the application does not have would be an
//! example of something else.
//!
//! It prints one line of JSON on stdout when it is listening, so a test can wait for it without
//! sleeping:
//!
//! ```json
//! {"port":17643,"device":"d-example","key":"<hex>","serverPublic":"<hex>",
//!  "publicBase64":"<b64url>","code":"XXXXXXXX"}
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use cide_ipc::ProjectId;
use cide_ipc::remote::{
    AwaitingEntry, InstanceInfo, PermissionPrompt, RemoteProject, RemoteSession,
};
use cide_ipc::screen::{ScreenCapture, ScreenInfo, ScreenLine, ScrollbackCapture, StyleRun};
use cide_remote::seal::StaticKey;
use cide_remote::{DeviceStore, RemoteHost, RemoteServer};

/// The one device this example serves, with a key that is a constant because it is an example.
const DEVICE_ID: &str = "d-example";
const DEVICE_KEY: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const INSTANCE_KEY: [u8; 32] = [2u8; 32];

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: fake_cide <dir> [port]"));
    let port: u16 = args.next().map_or(0, |p| p.parse().expect("a port"));
    std::fs::create_dir_all(&dir).expect("creates the directory");

    // The instance key, written as `StaticKey::load_or_mint` reads one: 32 raw bytes, and that
    // is the whole format.
    let key_path = dir.join("remote-key");
    std::fs::write(&key_path, INSTANCE_KEY).expect("writes the key");
    let statik = Arc::new(StaticKey::load_or_mint(&key_path).expect("reads the key"));

    // And the device list, as the real one is written. No new API: an example that needed a
    // constructor the application does not have would be an example of something else.
    let devices_path = dir.join("remote-devices.json");
    std::fs::write(
        &devices_path,
        format!(
            r#"{{"schema":1,"devices":[{{"id":"{DEVICE_ID}","name":"example","platform":"test","sealKey":"{DEVICE_KEY}","createdUnixMs":0,"lastSeenUnixMs":0,"lastAddr":""}}],"port":null}}"#
        ),
    )
    .expect("writes the device list");
    let devices = Arc::new(DeviceStore::load(devices_path).expect("reads the device list"));

    // A pairing window, open from the start, so the other end can exercise the road a device
    // actually takes: scan, redeem, come away with credentials. Without it the only thing
    // testable from outside is the half that assumes pairing already happened.
    let code = devices.begin_pairing().expect("opens a pairing window");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let host: Arc<dyn RemoteHost> = Arc::new(Constants::default());

    let server = runtime
        .block_on(RemoteServer::bind(
            ([127, 0, 0, 1], port).into(),
            host,
            devices,
            Arc::clone(&statik),
        ))
        .expect("binds");

    // One line, flushed, so a test can wait on it rather than sleeping.
    println!(
        r#"{{"port":{},"device":"{DEVICE_ID}","key":"{DEVICE_KEY}","serverPublic":"{}","publicBase64":"{}","code":"{code}"}}"#,
        server.port(),
        statik
            .public_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        statik.public_base64(),
    );
    use std::io::Write;
    std::io::stdout().flush().ok();

    // Until it is killed. The caller owns its lifetime.
    //
    // While waiting, the pairing attempt is printed whenever it changes — which stands in for
    // cide's Settings panel, and is the only way to check by hand that the six digits a phone
    // shows are the six digits the *server* derived. A test can assert the two ends agree
    // (`contract/seal-vectors.json` pins them), but nobody can look at a panel this example
    // does not have.
    runtime.block_on(async {
        let mut last: Option<String> = None;
        loop {
            let now = server.pairing_attempt();
            let line = now.as_ref().map(|a| format!("{} {}", a.sas, a.addr));
            if line != last {
                match &line {
                    Some(text) => println!(r#"{{"pairing":"{text}"}}"#),
                    None => println!(r#"{{"pairing":null}}"#),
                }
                use std::io::Write;
                std::io::stdout().flush().ok();
                last = line;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
}

/// cide, if cide were four constants.
///
/// Almost. `acknowledge` genuinely removes a session from the awaiting set, because a fixture
/// that accepts a write and does nothing cannot test that write — it reports success and leaves
/// the device showing a marker for ever, which is indistinguishable from the device failing to
/// send it. That is not hypothetical: it is how "opening a console clears the mark" came to be
/// reported as verified when nothing had been verified at all.
#[derive(Default)]
struct Constants {
    acknowledged: std::sync::Mutex<std::collections::HashSet<cide_ipc::SessionId>>,
    /// What has been typed at the prompt, echoed back into [`RemoteHost::screen`]. (M76)
    ///
    /// `write` used to answer `Ok(())` and drop the bytes, which is the trap this file already
    /// names about `acknowledge`: a fixture that accepts a write and does nothing cannot test
    /// that write — it reports success and the device shows nothing, which is indistinguishable
    /// from the device never having sent anything. Typing on a phone is the one road where that
    /// matters most, because the *only* evidence a keystroke arrived is the terminal echoing it.
    typed: std::sync::Mutex<String>,
}

/// How many bytes a UTF-8 sequence starting with this one takes.
fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn project() -> ProjectId {
    // Stable, so a test can name it.
    "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("a uuid")
}

impl Constants {
    /// The one task, and the conversation on it. See [`RemoteHost::board`].
    fn detail(&self) -> cide_ipc::TaskDetail {
        let id = cide_ipc::TaskId("t-7".to_owned());
        let comment =
            |n: u128, author: cide_ipc::TaskAuthor, at: u64, text: &str| cide_ipc::TaskComment {
                id: cide_ipc::CommentId(format!("00000000-0000-4000-8000-{n:012}")),
                author,
                text: text.to_owned(),
                at_unix_ms: at,
                edited_at_unix_ms: None,
                deleted: false,
                attachments: Vec::new(),
            };
        cide_ipc::TaskDetail {
            row: cide_ipc::TaskRow {
                id: id.clone(),
                title: "The phone draws a comment the way the panel does".to_owned(),
                status: cide_ipc::TaskStatus::Doing,
                agent: Some("reviewer".to_owned().into()),
                session: None,
                change: None,
                links: Vec::new(),
                created_by: cide_ipc::TaskAuthor::User,
                created_unix_ms: 1_789_900_000_000,
                updated_unix_ms: 1_789_903_600_000,
                comment_count: 3,
                attachment_count: 0,
            },
            body: "Comments are **markdown** on the desktop and were raw text here.\n\n- the author was a tagged enum, printed as JSON\n- no time was drawn at all\n- `text` reached the screen as its own source"
                .to_owned(),
            comments: vec![
                comment(
                    1,
                    cide_ipc::TaskAuthor::User,
                    1_789_900_060_000,
                    "Have a look at the card on a phone — the author line is wrong.",
                ),
                comment(
                    2,
                    cide_ipc::TaskAuthor::Orchestrator,
                    1_789_901_400_000,
                    "Handing this to `reviewer`. Three separate defects, one screen.",
                ),
                comment(
                    3,
                    cide_ipc::TaskAuthor::Agent {
                        agent: "reviewer".to_owned().into(),
                        label: "Reviewer".to_owned(),
                    },
                    1_789_903_600_000,
                    "## What I found\n\n1. The header renders `JSON.stringify(author)`.\n2. Nothing carries `at_unix_ms` to the screen.\n3. Markdown is drawn as source.\n\nThe parser is already shared with the panel — see `editor/markdown/blocks.ts`.",
                ),
            ],
            attachments: Vec::new(),
            history: vec![cide_ipc::TaskStatusChange {
                from: cide_ipc::TaskStatus::Todo,
                to: cide_ipc::TaskStatus::Doing,
                by: cide_ipc::TaskAuthor::Orchestrator,
                at_unix_ms: 1_789_901_400_000,
            }],
        }
    }
}

impl RemoteHost for Constants {
    fn instance(&self) -> InstanceInfo {
        InstanceInfo {
            id: "i-example".to_owned(),
            name: "[EXAMPLE] cide".to_owned(),
            profile: Some("example".to_owned()),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn projects(&self) -> (u64, Vec<RemoteProject>) {
        (
            1,
            vec![RemoteProject {
                id: project(),
                name: "cide".to_owned(),
                display_path: "~/work/cide".to_owned(),
                dot: "var(--accent)".to_owned(),
            }],
        )
    }

    /// Two runs, deliberately in different states. (M75)
    ///
    /// A paused one and a running one, because the device's Agents screen draws a different
    /// control for each and a fixture with one state can only ever exercise half of it.
    fn runs(&self, project: ProjectId) -> Vec<cide_ipc::AgentRun> {
        if project != self::project() {
            return Vec::new();
        }
        let base = cide_ipc::AgentRun {
            run: "00000000-0000-4000-8000-00000000000a"
                .parse()
                .expect("a uuid"),
            agent: "reviewer".to_owned().into(),
            agent_label: "Reviewer".to_owned(),
            harness: cide_ipc::Harness::Claude,
            project,
            session: None,
            state: cide_ipc::RunState::Running,
            task: None,
            started_unix_ms: 1_726_900_000_000,
            worked_ms: 0,
            // Running, so an interval is open — `AgentRun::working_since_unix_ms`'s invariant,
            // and the paused sibling below closes it by overriding the state *and* this.
            working_since_unix_ms: Some(1_726_900_000_000),
            notify: cide_ipc::RunNotify::default(),
            stale_turn: false,
            note: None,
            openable: true,
            model: None,
            pool_position: None,
            worktree: false,
        };
        vec![
            base.clone(),
            cide_ipc::AgentRun {
                run: "00000000-0000-4000-8000-00000000000b"
                    .parse()
                    .expect("a uuid"),
                agent: "builder".to_owned().into(),
                agent_label: "Builder".to_owned(),
                state: cide_ipc::RunState::Paused {
                    since_unix_ms: 1_726_900_500_000,
                },
                worked_ms: 500_000,
                working_since_unix_ms: None,
                ..base
            },
        ]
    }

    fn roster(&self, project: ProjectId) -> cide_ipc::remote::RemoteRoster {
        if project != self::project() {
            return cide_ipc::remote::RemoteRoster {
                agents: Vec::new(),
                dispatching: false,
            };
        }
        // Deliberately *paused*, because that is the state the first cut of this could not
        // show: the phone drew a paused project as idle, and a fixture whose queue is open
        // cannot fail that way.
        cide_ipc::remote::RemoteRoster {
            dispatching: false,
            agents: vec![
                cide_ipc::remote::RemoteAgent {
                    id: "reviewer".to_owned().into(),
                    label: "Reviewer".to_owned(),
                    scope: cide_ipc::agents::AgentScope::Project,
                    harness: cide_ipc::Harness::Claude,
                    description: "Reads a diff and argues with it.".to_owned(),
                    model: Some("claude-opus-5".to_owned()),
                    // A role that declares its hue, beside one that does not, so both rungs of
                    // the colour rule are on a device's screen rather than only the derived one.
                    color: Some("cyan".to_owned()),
                    unavailable: None,
                    max_concurrent: 2,
                    worktree: true,
                    running: 1,
                    queued: 0,
                },
                cide_ipc::remote::RemoteAgent {
                    id: "builder".to_owned().into(),
                    label: "Builder".to_owned(),
                    scope: cide_ipc::agents::AgentScope::Project,
                    harness: cide_ipc::Harness::Opencode,
                    description: "Builds the thing.".to_owned(),
                    model: None,
                    color: None,
                    unavailable: None,
                    max_concurrent: 1,
                    worktree: true,
                    running: 1,
                    queued: 0,
                },
            ],
        }
    }

    /// One task, with a conversation on it. (M76)
    ///
    /// An empty board is the third fixture in this file to have hidden a whole screen: a device
    /// that never renders a comment cannot show that it renders the *author* as
    /// `JSON.stringify(...)` of a tagged enum, that it prints no time, and that it draws markdown
    /// as its own source. All three shipped, and all three were found in a photograph of a phone.
    ///
    /// So the comments below are one of each author arm — a person, the orchestrator, and a role
    /// that declares a colour — carrying the markdown an agent actually writes: a heading, a
    /// list, `code`, and **weight**.
    fn board(&self, project: ProjectId) -> Vec<cide_ipc::TaskRow> {
        if project != self::project() {
            return Vec::new();
        }
        vec![self.detail().row]
    }

    fn task(&self, project: ProjectId, task: cide_ipc::TaskId) -> Option<cide_ipc::TaskDetail> {
        let detail = self.detail();
        if project != self::project() || task != detail.row.id {
            return None;
        }
        Some(detail)
    }

    /// Three consoles, of the two kinds, in three states. (M76)
    ///
    /// One of each kind on purpose. The device draws a Claude console and a shell differently — a
    /// colour, a mark and a word, because "typing into this talks to an agent" and "typing into
    /// this runs a command" are the two things on that screen least alike — and a fixture with
    /// one kind in it cannot tell the two renderings apart. It also cannot tell *4 open* from
    /// *4 open · 2 working*, which is why one of these is busy and the other is not.
    ///
    /// The third is in [`SessionState::Spawning`], which is the state a fixture full of decided
    /// ones cannot show: cide learns a state from a hook, a shell fires none, and its jobs
    /// watcher says nothing until a foreground job has held the terminal for two minutes — so
    /// *nothing has been heard from this session* is a shell's resting state and not a moment.
    /// A device that renders it as `starting` says so about a console somebody opened an hour
    /// ago, which is what it did.
    fn sessions(&self, only: Option<ProjectId>) -> Vec<RemoteSession> {
        if only.is_some_and(|wanted| wanted != project()) {
            return Vec::new();
        }
        vec![
            RemoteSession {
                session: "00000000-0000-4000-8000-000000000002"
                    .parse()
                    .expect("a uuid"),
                project: project(),
                pane: "00000000-0000-4000-8000-000000000003"
                    .parse()
                    .expect("a uuid"),
                tab: None,
                tab_title: Some("Claude".to_owned()),
                title: "claude".to_owned(),
                kind: cide_ipc::PaneKind::Claude,
                role: cide_ipc::PaneRole::Primary,
                state: cide_ipc::SessionState::Idle,
                awaiting: false,
                run: None,
                agent: None,
                task: None,
            },
            RemoteSession {
                session: "00000000-0000-4000-8000-000000000004"
                    .parse()
                    .expect("a uuid"),
                project: project(),
                pane: "00000000-0000-4000-8000-000000000005"
                    .parse()
                    .expect("a uuid"),
                tab: None,
                tab_title: Some("Claude".to_owned()),
                title: "bash".to_owned(),
                kind: cide_ipc::PaneKind::Shell,
                role: cide_ipc::PaneRole::Auxiliary,
                state: cide_ipc::SessionState::Busy,
                awaiting: false,
                run: None,
                agent: None,
                task: None,
            },
            RemoteSession {
                session: "00000000-0000-4000-8000-000000000006"
                    .parse()
                    .expect("a uuid"),
                project: project(),
                pane: "00000000-0000-4000-8000-000000000007"
                    .parse()
                    .expect("a uuid"),
                tab: None,
                tab_title: Some("shell".to_owned()),
                title: "bash".to_owned(),
                kind: cide_ipc::PaneKind::Shell,
                role: cide_ipc::PaneRole::Auxiliary,
                state: cide_ipc::SessionState::Spawning,
                awaiting: false,
                run: None,
                agent: None,
                task: None,
            },
        ]
    }

    fn awaiting(&self) -> Vec<AwaitingEntry> {
        // The one session this fixture has, *waiting*. An empty set here would let every
        // unread marker — the badge on a machine, the badge on Consoles, the dot on a chip and
        // the dot on the console itself — regress without a single screenshot changing, which
        // is the same trap a fixture whose queue was always open set for the paused state.
        let session: cide_ipc::SessionId = "00000000-0000-4000-8000-000000000002"
            .parse()
            .expect("a uuid");
        if self
            .acknowledged
            .lock()
            .expect("not poisoned")
            .contains(&session)
        {
            return Vec::new();
        }
        vec![AwaitingEntry {
            session,
            since_unix_ms: 1_726_900_000_000,
        }]
    }

    /// A Claude pane, at the width a Claude pane actually is.
    ///
    /// Twenty columns of `hello from cide` is the same class of fixture as the `depth: 0`
    /// scrollback below: every rule the phone has about *narrow screen, wide terminal* — wrapping,
    /// the character budget, what a glyph the monospace face does not have does to a line's width
    /// — is unreachable from a row that fits. The first phone screenshot of a real Claude pane
    /// showed box-drawing borders overflowing their display lines and being ellipsised, on a build
    /// whose every test was green against this.
    ///
    /// So the rows here are the three the phone has to survive: a **box-drawn** frame (the input
    /// prompt, which is the one thing on a Claude screen somebody needs to read to the end), a
    /// line of **Cyrillic** prose, and a **braille** spinner — each a different block, and none of
    /// them in the same font file as the Latin text on a stock Android.
    fn screen(&self, _session: cide_ipc::SessionId) -> Option<ScreenCapture> {
        const COLS: u16 = 120;
        /// The frames Claude Code's own spinner uses, so the moving row moves the way one does.
        const SPINNER: [char; 8] = [
            '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}',
            '\u{2827}',
        ];
        let spin = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as usize / 250);
        let line = |row: u16, text: String| ScreenLine {
            row,
            wrapped: false,
            runs: vec![StyleRun {
                text,
                fg: None,
                bg: None,
                flags: 0,
            }],
        };
        let rule = |ends: (char, char)| {
            let mut s = String::new();
            s.push(ends.0);
            for _ in 0..(COLS as usize - 2) {
                s.push('\u{2500}');
            }
            s.push(ends.1);
            s
        };
        let padded = |text: &str| {
            let mut s = String::from("\u{2502} ");
            s.push_str(text);
            // Saturating, because what is typed at the prompt goes through here and a line longer
            // than the box is an underflow panic rather than a wrapped line.
            for _ in 0..(COLS as usize).saturating_sub(3 + text.chars().count()) {
                s.push(' ');
            }
            s.push('\u{2502}');
            s
        };
        Some(ScreenCapture {
            info: ScreenInfo {
                cols: COLS,
                rows: 8,
                alt: false,
                app_cursor: false,
                bracketed_paste: false,
                // Tracking on, SGR — what `claude` asks for, so a device's wheel is not
                // refused by the one fixture that can answer it.
                mouse: cide_ipc::screen::MouseReporting::Sgr,
            },
            cursor: None,
            lines: vec![
                line(0, "hello from cide".to_owned()),
                // **Moving**, and that is the point of it. A screen that never changes cannot
                // show what a repainting one does to somebody who has scrolled up to read — and
                // a Claude console repaints several times a second for as long as it is
                // thinking. A fixture of still pictures said scrolling worked.
                line(
                    1,
                    format!(
                        "{} Crunched for 4m 18s \u{b7} {} tokens",
                        SPINNER[spin % SPINNER.len()],
                        626_058 + spin
                    ),
                ),
                line(
                    2,
                    "  \u{421}\u{43f}\u{435}\u{43a}\u{430} \u{43e}\u{43a}\u{430}\u{437}\u{430}\u{43b}\u{430}\u{441}\u{44c} \u{43f}\u{43e}\u{43b}\u{43d}\u{43e}\u{439}: \u{a7}3.9.3 \u{437}\u{430}\u{434}\u{430}\u{451}\u{442} \u{432}\u{441}\u{435} \u{447}\u{438}\u{441}\u{43b}\u{430}, \u{a7}3.9.5 \u{2014} \u{43a}\u{43e}\u{43d}\u{442}\u{440}\u{430}\u{43a}\u{442}."
                        .to_owned(),
                ),
                line(4, rule(('\u{256d}', '\u{256e}'))),
                line(
                    5,
                    padded(&format!(
                        "\u{276f} {}",
                        self.typed.lock().expect("not poisoned")
                    )),
                ),
                line(6, rule(('\u{2570}', '\u{256f}'))),
                line(7, "  auto mode on (shift+tab to cycle)".to_owned()),
            ],
        })
    }

    /// A real window onto a synthetic five-hundred-line history.
    ///
    /// It used to answer `depth: 0` with no lines, which is the shape of fixture this file has
    /// already been burned by once: a device asking *how deep is it* was told *there is nothing
    /// there*, so every scrollback road ended before it started and the test that drove it
    /// passed. The arithmetic mirrors `cide_pty::screen::capture_scrollback_of` — count from the
    /// oldest line, clamp the window to the depth — because a fixture that windows differently
    /// from the thing it stands in for tests the fixture.
    fn scrollback(
        &self,
        _session: cide_ipc::SessionId,
        from_top: u32,
        rows: u16,
    ) -> Option<ScrollbackCapture> {
        const DEPTH: u32 = 500;
        const MAX_PAGE_ROWS: u16 = 200;
        let first = from_top.min(DEPTH);
        let last = (first + u32::from(rows.min(MAX_PAGE_ROWS))).min(DEPTH);
        let lines = (first..last)
            .map(|index| ScreenLine {
                row: (index - first) as u16,
                wrapped: false,
                runs: vec![StyleRun {
                    text: format!("history line {index}"),
                    fg: None,
                    bg: None,
                    flags: 0,
                }],
            })
            .collect();
        Some(ScrollbackCapture {
            from_top: first,
            depth: DEPTH,
            lines,
        })
    }

    fn prompt(&self, _session: cide_ipc::SessionId) -> Option<PermissionPrompt> {
        None
    }

    fn answer_prompt(
        &self,
        _session: cide_ipc::SessionId,
        _option: u8,
        _expect_screen: &str,
    ) -> Result<(), String> {
        Err("this example answers nothing".to_owned())
    }

    /// Echo, the way a terminal in canonical mode would. See [`Constants::typed`].
    ///
    /// Enough of one to prove a keystroke arrived and no more: printable characters append,
    /// `\r` clears the line as a submit would, and both backspaces pop. Everything else — the
    /// escape sequences an arrow key or a Ctrl chord encodes to — is dropped rather than drawn,
    /// because a fixture that rendered `^[[A` as three characters would be teaching the eye to
    /// accept a bug.
    fn write(
        &self,
        _session: cide_ipc::SessionId,
        bytes: Vec<u8>,
        _writer: &str,
        _epoch: &str,
        _seq: u64,
    ) -> Result<(), String> {
        let mut typed = self.typed.lock().expect("not poisoned");
        let mut rest = bytes.as_slice();
        while let Some((first, tail)) = rest.split_first() {
            match first {
                0x1b => {
                    // An escape sequence: skip to the end of it rather than printing it.
                    let end = tail
                        .iter()
                        .position(|b| b.is_ascii_alphabetic() || *b == b'~')
                        .map_or(tail.len(), |at| at + 1);
                    rest = &tail[end..];
                }
                b'\r' | b'\n' => {
                    typed.clear();
                    rest = tail;
                }
                0x7f | 0x08 => {
                    typed.pop();
                    rest = tail;
                }
                b if *b < 0x20 => rest = tail,
                _ => {
                    let width = utf8_width(*first);
                    let (glyph, tail) = rest.split_at(width.min(rest.len()));
                    typed.push_str(&String::from_utf8_lossy(glyph));
                    rest = tail;
                }
            }
        }
        Ok(())
    }

    fn acknowledge(&self, session: cide_ipc::SessionId) -> Result<(), String> {
        self.acknowledged
            .lock()
            .expect("not poisoned")
            .insert(session);
        Ok(())
    }

    /// A desk with no pane to scroll: accepted, as the real one accepts it. (M91)
    fn scroll_view(&self, _session: cide_ipc::SessionId, _pages: i8) -> Result<(), String> {
        Ok(())
    }

    fn run_stop(
        &self,
        _project: ProjectId,
        _run: cide_ipc::RunId,
        _reason: Option<String>,
        _force: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn run_pause(&self, _project: ProjectId, _run: Option<cide_ipc::RunId>) -> Result<(), String> {
        Ok(())
    }

    fn run_resume(&self, _project: ProjectId, _run: Option<cide_ipc::RunId>) -> Result<(), String> {
        Ok(())
    }

    fn dispatch(&self, _request: cide_ipc::DispatchRequest) -> Result<cide_ipc::RunId, String> {
        Err("this example dispatches nothing".to_owned())
    }

    fn task_new(&self, _task: cide_ipc::TaskNew) -> Result<(), String> {
        Ok(())
    }

    fn task_edit(
        &self,
        _project: ProjectId,
        _task: cide_ipc::TaskId,
        _edit: cide_ipc::TaskEdit,
    ) -> Result<(), String> {
        Ok(())
    }
}
