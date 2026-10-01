//! Exercise Readline through the real PTY: screen replay alone would pass a text-only test
//! while Up still recalled the user's global history after the child was replaced.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cide_core::shell_history::{self, HistoryFiles};
use cide_ipc::PaneId;
use cide_pty::{PtySession, SpawnSpec};

struct Fixture(PathBuf);

impl Fixture {
    fn new(prompt_command: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cide bash history '{}", PaneId::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".bash_history"), "echo GLOBAL_HISTORY_SENTINEL\n").unwrap();
        fs::write(
            root.join(".bash_profile"),
            format!(
                "printf 'profile\\n' >> \"$HOME/startups\"\n\
                 . \"$HOME/.bashrc\"\n\
                 HISTFILE=\"$HOME/.bash_history\"\n\
                 builtin history -r\n\
                 {prompt_command}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join(".bashrc"),
            "printf 'rc\\n' >> \"$HOME/startups\"\n\
             PS1='__CIDE_PROMPT__ '\n\
             HISTSIZE=200\nHISTFILESIZE=200\nHISTCONTROL=ignorespace\n\
             HISTIGNORE='echo ignored'\n\
             set -o emacs\n\
             bind 'set enable-bracketed-paste off'\n",
        )
        .unwrap();
        // These must not run when .bash_profile exists.
        fs::write(root.join(".bash_login"), "echo BAD_LOGIN_PROFILE\n").unwrap();
        fs::write(root.join(".profile"), "echo BAD_PROFILE\n").unwrap();
        Self(root)
    }

    fn history(&self, pane: PaneId) -> HistoryFiles {
        shell_history::prepare_in(&self.0.join("state"), pane).unwrap()
    }

    fn spec(&self, history: &HistoryFiles) -> SpawnSpec {
        // Point at an older Bash to exercise the same PTY tests without changing login shell.
        let bash = std::env::var("CIDE_TEST_BASH").unwrap_or_else(|_| "/bin/bash".into());
        let mut spec = SpawnSpec::new(bash, &self.0)
            .env("HOME", self.0.to_string_lossy())
            .env("TERM", "xterm-256color");
        for arg in shell_history::BASH_ARGS {
            spec = spec.arg(arg);
        }
        for (key, value) in history.env(
            Some("original env value"),
            Some("printf 'inherited=%s\\n' \"$?\" >> \"$HOME/status\""),
        ) {
            spec = spec.env(key, value);
        }
        spec
    }

    fn spawn(&self, history: &HistoryFiles) -> Shell {
        let session = PtySession::spawn(self.spec(history)).unwrap();
        let (tx, output) = mpsc::channel();
        let (_, initial) = session.attach_with_snapshot(
            Arc::new(move |bytes: &[u8]| tx.send(bytes.to_vec()).is_ok()),
            false,
        );
        let mut shell = Shell {
            session,
            output,
            initial,
        };
        let initial = shell.until_prompt();
        assert!(!initial.contains("BAD_PROFILE"), "{initial}");
        assert!(!initial.contains("BAD_LOGIN_PROFILE"), "{initial}");
        shell
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Shell {
    session: Arc<PtySession>,
    output: mpsc::Receiver<Vec<u8>>,
    initial: Vec<u8>,
}

impl Shell {
    fn until_prompt(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bytes = std::mem::take(&mut self.initial);
        loop {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains("__CIDE_PROMPT__ ") {
                return text.into_owned();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.output.recv_timeout(remaining) {
                Ok(chunk) => bytes.extend(chunk),
                Err(error) => panic!(
                    "waiting for Bash prompt: {error}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            }
        }
    }

    fn command(&mut self, command: &str) -> String {
        self.session.write(format!("{command}\n").into_bytes());
        self.until_prompt()
    }

    fn recall(&mut self) -> String {
        self.session.write(b"\x1b[A\n".to_vec());
        self.until_prompt()
    }

    fn terminate(&self) {
        self.session.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.session.exit_status().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(self.session.exit_status().is_some(), "Bash did not exit");
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.session.kill();
    }
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn bash_panels_recall_their_own_commands_after_forced_restart() {
    let fixture = Fixture::new("PROMPT_COMMAND='history -a; history -n # existing hook'");
    let pane_a = PaneId::new();
    let pane_b = PaneId::new();
    let history_a = fixture.history(pane_a);
    let history_b = fixture.history(pane_b);
    let mut a = fixture.spawn(&history_a);
    let mut b = fixture.spawn(&history_b);
    // A new pane is empty even when a profile explicitly imported the global file.
    assert_eq!(read(&history_a.history), "");
    assert_eq!(read(&history_b.history), "");
    a.command("echo PANEL_A_OLDER");
    b.command("echo PANEL_B_OLDER");
    assert!(
        a.command("echo PANEL_A_COMMAND")
            .contains("PANEL_A_COMMAND")
    );
    assert!(
        b.command("echo PANEL_B_COMMAND")
            .contains("PANEL_B_COMMAND")
    );
    // This assertion is before any exit or signal, so shutdown flushing cannot mask a bug.
    assert_eq!(
        read(&history_a.history),
        "echo PANEL_A_OLDER\necho PANEL_A_COMMAND\n"
    );
    assert_eq!(
        read(&history_b.history),
        "echo PANEL_B_OLDER\necho PANEL_B_COMMAND\n"
    );
    a.terminate();
    b.terminate();
    let mut a = fixture.spawn(&fixture.history(pane_a));
    let mut b = fixture.spawn(&fixture.history(pane_b));
    let recalled_a = a.recall();
    let recalled_b = b.recall();
    assert!(recalled_a.contains("PANEL_A_COMMAND"), "{recalled_a}");
    assert!(!recalled_a.contains("PANEL_B_COMMAND"), "{recalled_a}");
    assert!(recalled_b.contains("PANEL_B_COMMAND"), "{recalled_b}");
    assert!(!recalled_b.contains("PANEL_A_COMMAND"), "{recalled_b}");
    assert!(
        !recalled_a.contains("GLOBAL_HISTORY_SENTINEL"),
        "{recalled_a}"
    );
    assert!(
        !recalled_b.contains("GLOBAL_HISTORY_SENTINEL"),
        "{recalled_b}"
    );
    a.terminate();
    b.terminate();
    assert_eq!(
        read(fixture.0.join(".bash_history")),
        "echo GLOBAL_HISTORY_SENTINEL\n"
    );
    assert_eq!(read(fixture.0.join("startups")), "profile\nrc\n".repeat(4));
}

#[test]
fn bash_history_preserves_prompt_status_filters_and_login_environment() {
    let fixture = Fixture::new(
        "PROMPT_COMMAND='printf \"status=%s\\n\" \"$?\" >> \"$HOME/status\" # trailing comment'",
    );
    let history = fixture.history(PaneId::new());
    let mut shell = fixture.spawn(&history);
    shell.command("false");
    assert!(read(fixture.0.join("status")).ends_with("status=1\n"));
    shell.command(" echo private");
    shell.command("echo ignored");
    assert_eq!(read(&history.history), "false\n");
    let mode = shell.command(
        " printf 'login=%s posix=%s env=%s\\n' \"$(shopt -q login_shell && echo yes)\" \"$(set -o | sed -n '/^posix/p')\" \"$ENV\"",
    );
    assert!(mode.contains("login=yes posix=posix"), "{mode}");
    assert!(mode.contains("off env=original env value"), "{mode}");
    shell.command(" printf '%s\\n' child >> \"$HOME/child-startup\"");
    let child = shell.command(" /bin/bash --noprofile --norc -c 'test -z \"${_CIDE_BASH_HISTORY_FILE+x}\" && test -z \"${PROMPT_COMMAND+x}\" && test -z \"${HISTFILE+x}\" && echo CLEAN_CHILD'");
    assert!(child.contains("\r\nCLEAN_CHILD\r\n"), "{child}");
    shell.terminate();
    assert_eq!(read(&history.history), "false\n");
}

#[test]
fn bash_history_preserves_prompt_command_arrays() {
    // Bash 3.2 supports indexed arrays but executes only element zero as PROMPT_COMMAND.
    // On older versions, the second element must stay unexecuted, including the first prompt.
    let fixture = Fixture::new(
        "if [ \"${BASH_VERSINFO[0]}\" -gt 5 ] || { [ \"${BASH_VERSINFO[0]}\" -eq 5 ] && [ \"${BASH_VERSINFO[1]}\" -ge 1 ]; }; then\n\
           PROMPT_COMMAND=('printf \"first=%s\\n\" \"$?\" >> \"$HOME/status\"' 'printf \"second\\n\" >> \"$HOME/status\"')\n\
           export PROMPT_COMMAND\n\
         else\n\
           PROMPT_COMMAND=('printf \"first=%s\\nsecond\\n\" \"$?\" >> \"$HOME/status\"' 'printf \"UNSUPPORTED_ELEMENT\\n\" >> \"$HOME/status\"')\n\
         fi",
    );
    let history = fixture.history(PaneId::new());
    let mut shell = fixture.spawn(&history);
    shell.command("false");
    assert!(!read(fixture.0.join("status")).contains("UNSUPPORTED_ELEMENT"));
    assert!(read(fixture.0.join("status")).ends_with("first=1\nsecond\n"));
    assert_eq!(read(&history.history), "false\n");
    shell.session.write(b"exit\n".to_vec());
    let deadline = Instant::now() + Duration::from_secs(5);
    while shell.session.exit_status().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(shell.session.exit_status().is_some());
    assert_eq!(read(&history.history), "false\nexit\n");
}

#[test]
fn bash_history_preserves_an_inherited_prompt_hook_from_the_first_prompt() {
    let fixture = Fixture::new("");
    let history = fixture.history(PaneId::new());
    let mut shell = fixture.spawn(&history);
    assert_eq!(read(fixture.0.join("status")), "inherited=0\n");
    shell.command("false");
    assert_eq!(read(fixture.0.join("status")), "inherited=0\ninherited=1\n");
    assert_eq!(read(&history.history), "false\n");
    shell.terminate();
}

#[test]
fn a_readonly_global_histfile_is_reported_without_overwriting_it() {
    let fixture = Fixture::new("readonly HISTFILE");
    let history = fixture.history(PaneId::new());
    let shell = PtySession::spawn(fixture.spec(&history)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while shell.exit_status().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        shell.exit_status().is_some(),
        "initializer should refuse readonly HISTFILE"
    );
    let output = String::from_utf8_lossy(&shell.screen_state()).into_owned();
    assert!(
        output.contains("cide: cannot select this panel history file"),
        "{output}"
    );
    assert_eq!(
        read(fixture.0.join(".bash_history")),
        "echo GLOBAL_HISTORY_SENTINEL\n"
    );
    assert_eq!(read(&history.history), "");
}
