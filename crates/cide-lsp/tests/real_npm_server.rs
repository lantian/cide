//! An npm-installed server, end to end — the hint rung's live test.
//!
//! `#[ignore]`d by the workspace convention: it spawns a real `typescript-language-server`,
//! which must be installed (`npm install -g typescript-language-server typescript`) though not
//! necessarily on this shell's `PATH` — finding it in a Node version-manager directory when
//! `PATH` misses is the behaviour under test. Run it by hand:
//!
//! ```sh
//! cargo test -p cide-lsp --test real_npm_server -- --ignored --nocapture
//! ```
//!
//! # Why this is its own test binary
//!
//! It calls [`cide_lsp::discover::install`], and the registry is process-global — the same
//! reason `tests/registry.rs` gives at length. A unit test that installed a table of its own
//! would pull the builtins out from under every neighbour.
//!
//! # What it cannot arrange
//!
//! The desktop-launch case itself — a `PATH` with no `node` on it — cannot be set up
//! in-process (`set_var` is unsafe in edition 2024, and the parent's `PATH` is what `locate`
//! reads). The manual check is:
//!
//! ```sh
//! PATH=/usr/bin:/bin ./run.sh   # with node installed only under a version manager
//! ```
//!
//! then open a project with a `tsconfig.json` and watch the server reach Ready.

use std::time::{Duration, Instant};

use cide_lsp::{LspEvent, LspHandle};

#[test]
#[ignore = "spawns the real typescript-language-server"]
fn an_npm_installed_server_is_found_and_reaches_ready() {
    // The registry def the web extension contributes, verbatim where it matters: bare binary
    // name, `--stdio`, the builtin `typescript` id.
    let mut wanted = cide_ipc::lang::builtin_servers();
    wanted.push(cide_ipc::lang::LanguageServerDef {
        binary: "typescript-language-server".into(),
        args: vec!["--stdio".into()],
        language_ids: vec!["typescript".into()],
        project_markers: vec![
            "tsconfig.json".into(),
            "jsconfig.json".into(),
            "package.json".into(),
        ],
        project_kind: "TypeScript or JavaScript project".into(),
        install_hint: "npm install -g typescript-language-server typescript".into(),
        declares_watched_files: false,
        extra_path_hints: vec![],
        init_options: None,
    });
    let handles = cide_lsp::discover::install(wanted);
    let server = handles[2];

    let dir = std::env::temp_dir().join(format!("cide-lsp-npm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(
        dir.join("tsconfig.json"),
        "{\n  \"compilerOptions\": { \"strict\": true }\n}\n",
    )
    .expect("tsconfig");
    std::fs::write(dir.join("main.ts"), "export const answer: number = 42;\n").expect("main.ts");

    // The whole road: `locate`'s ladder (PATH, then the hint rung with the Node directories),
    // the candidate's `child_path_dirs` reaching the spawn, the handshake, Ready.
    let found = cide_lsp::discover::find(server, std::slice::from_ref(&dir));
    let cide_lsp::discover::Found::Ready(path) = found else {
        panic!("typescript-language-server was not found: {found:?} — is it installed?");
    };
    eprintln!("resolved to {}", path.display());

    let handle = LspHandle::start(server, vec![dir.clone()]).expect("start");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    let ready = loop {
        seen.extend(handle.drain());
        if seen
            .iter()
            .any(|e| matches!(e, LspEvent::Status(cide_ipc::SourceStatus::Ready)))
        {
            break true;
        }
        if let Some(reason) = seen.iter().find_map(|e| match e {
            LspEvent::Status(cide_ipc::SourceStatus::Unavailable { reason }) => {
                Some(reason.clone())
            }
            _ => None,
        }) {
            panic!("the supervisor gave up: {reason}");
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    drop(handle);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ready, "never reached Ready; saw {seen:?}");
}
