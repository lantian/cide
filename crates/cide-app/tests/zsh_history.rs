//! The zsh counterpart of `bash_history.rs`: on macOS the panel shell is zsh, and before this
//! its panels all shared `~/.zsh_history`. Real PTY, so ZLE's Up-arrow recall is what is tested.
#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use cide_core::shell_history::{self, ZshFiles};
use cide_ipc::PaneId;
use cide_pty::{PtySession, SpawnSpec};

fn zsh() -> Option<String> {
    ["/bin/zsh", "/usr/bin/zsh", "/usr/local/bin/zsh"]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
        .map(str::to_owned)
}

struct Fixture(PathBuf);

impl Fixture {
    fn new(zshrc_extra: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cide zsh history '{}", PaneId::new()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".zsh_history"), "echo GLOBAL_HISTORY_SENTINEL\n").unwrap();
        // What Apple's /etc/zshrc and a typical user rc do: pick the shared file, share it.
        fs::write(
            root.join(".zshrc"),
            format!(
                "PROMPT='__CIDE_PROMPT__ '\nHISTFILE=\"$HOME/.zsh_history\"\n\
                 HISTSIZE=200\nSAVEHIST=200\nsetopt SHARE_HISTORY\nbindkey -e\n{zshrc_extra}\n"
            ),
        )
        .unwrap();
        Self(root)
    }

    fn spawn(&self, program: &str, files: &ZshFiles) -> Shell {
        let mut spec = SpawnSpec::new(program, &self.0)
            .env("HOME", self.0.to_string_lossy())
            .env("TERM", "xterm-256color")
            .env_remove("ZDOTDIR");
        for arg in shell_history::ZSH_ARGS {
            spec = spec.arg(arg);
        }
        for (key, value) in files.env(None) {
            spec = spec.env(key, value);
        }
        let session = PtySession::spawn(spec).unwrap();
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
        shell.until_prompt();
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
                    "waiting for zsh prompt: {error}: {}",
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
        assert!(self.session.exit_status().is_some(), "zsh did not exit");
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.session.kill();
    }
}

#[test]
fn zsh_panels_recall_their_own_commands_after_forced_restart() {
    let Some(zsh) = zsh() else { return };
    let fixture = Fixture::new("");
    let dir = fixture.0.join("state");
    let (pane_a, pane_b) = (PaneId::new(), PaneId::new());
    let files_a = shell_history::prepare_zsh_in(&dir, pane_a).unwrap();
    let files_b = shell_history::prepare_zsh_in(&dir, pane_b).unwrap();
    let mut a = fixture.spawn(&zsh, &files_a);
    let mut b = fixture.spawn(&zsh, &files_b);
    a.command("echo PANEL_A_COMMAND");
    b.command("echo PANEL_B_COMMAND");
    // Before any exit, so shutdown flushing cannot mask a missing incremental append.
    let read = |p: &PathBuf| fs::read_to_string(p).unwrap();
    assert!(read(&files_a.history).contains("echo PANEL_A_COMMAND"));
    assert!(!read(&files_a.history).contains("PANEL_B"));
    assert!(!read(&files_b.history).contains("PANEL_A"));
    // The shared file is untouched.
    assert_eq!(
        read(&fixture.0.join(".zsh_history")),
        "echo GLOBAL_HISTORY_SENTINEL\n"
    );
    // Recall: only this panel's last command, never the other panel's or the global one.
    assert!(a.recall().contains("PANEL_A_COMMAND"));
    a.terminate();
    b.terminate();

    let mut a = fixture.spawn(&zsh, &files_a);
    let recalled = a.recall();
    assert!(
        recalled.matches("PANEL_A_COMMAND").count() >= 1,
        "{recalled}"
    );
    assert!(!recalled.contains("PANEL_B_COMMAND"), "{recalled}");
    assert!(!recalled.contains("GLOBAL_HISTORY_SENTINEL"), "{recalled}");
    // A fresh pane starts empty even though the user's rc selected the global file.
    let files_c = shell_history::prepare_zsh_in(&dir, PaneId::new()).unwrap();
    let mut c = fixture.spawn(&zsh, &files_c);
    let recalled = c.recall();
    assert!(!recalled.contains("GLOBAL_HISTORY_SENTINEL"), "{recalled}");
    assert!(!recalled.contains("PANEL_"), "{recalled}");
}
