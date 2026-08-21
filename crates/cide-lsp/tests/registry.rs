//! The server table's append-only invariant.
//!
//! # Why this is its own test binary
//!
//! `cide_lsp::discover`'s registry is **process-global**, and this test mutates it. `cargo test`
//! runs a crate's unit tests in parallel threads of one process, so a unit test that installed a
//! table of its own would pull the builtins out from under every other test in `discover` and
//! `server` — which is exactly what happened when it was written there, and it failed five
//! neighbours at once.
//!
//! An integration test is its own binary and therefore its own process, which is the cheapest way
//! to own a global. `cide-ext`'s marketplace test makes the same move for `$XDG_STATE_HOME`.
//!
//! # What is being defended
//!
//! `Server` is a `Copy` index precisely so it can live in a `BTreeMap` key and be compared in
//! ninety places. A handle minted before an extension was disabled therefore has to keep resolving
//! to the row it was minted for. If indices shifted, a live rust-analyzer session would start
//! reporting its diagnostics under a different server's name — and nothing in the build would
//! notice, because both indices are valid.

use cide_lsp::discover::{Server, by_binary, install, servers};

fn def(binary: &str) -> cide_ipc::lang::LanguageServerDef {
    cide_ipc::lang::LanguageServerDef {
        binary: binary.into(),
        args: vec![],
        language_ids: vec![binary.into()],
        project_markers: vec![],
        project_kind: "any".into(),
        install_hint: "-".into(),
        declares_watched_files: false,
        extra_path_hints: vec![],
    }
}

/// One test, and it owns this process's registry from start to finish.
///
/// Two would not work and the reason is worth writing down rather than rediscovering: cargo runs a
/// binary's tests on several threads, `install` merges by binary name, and a table that starts
/// empty gives index 0 to whatever is installed first. So a second test asserting that index 0 is
/// rust-analyzer passes or fails depending on which thread got there first. A global has one
/// owner.
#[test]
fn the_table_is_append_only_and_falls_back_to_the_builtins() {
    // Before anything is installed: the builtins, at the indices the constants name.
    assert_eq!(
        Server::RUST_ANALYZER.binary(),
        "rust-analyzer",
        "an uninstalled registry reports the builtins — `install` is called by `cide-app` once the \
         extension store has been read, and everything that runs before that would otherwise \
         conclude cide drives no language servers at all"
    );
    assert_eq!(Server::GOPLS.binary(), "gopls");
    assert_eq!(servers().len(), 2);

    // The real startup call, then two contributed servers on top of it.
    let mut wanted = cide_ipc::lang::builtin_servers();
    wanted.push(def("sqls"));
    wanted.push(def("yaml-language-server"));
    let handles = install(wanted);
    assert_eq!(handles.len(), 4);
    assert_eq!(
        handles[0],
        Server::RUST_ANALYZER,
        "the builtins keep the indices their constants name, which ninety call sites rely on"
    );
    assert_eq!(handles[1], Server::GOPLS);
    let sqls = handles[2];
    assert_eq!(sqls.binary(), "sqls");

    // `sqls`' extension is disabled.
    install(vec![
        cide_ipc::lang::builtin_servers()[0].clone(),
        cide_ipc::lang::builtin_servers()[1].clone(),
        def("yaml-language-server"),
    ]);
    assert_eq!(
        sqls.binary(),
        "sqls",
        "the handle still names the server it was minted for — an index that shifted would make a \
         live rust-analyzer session report its diagnostics under a different server's name, and \
         nothing in the build would notice because both indices are valid"
    );
    assert_eq!(
        servers().len(),
        3,
        "and the inactive row is not offered for starting"
    );
    assert!(servers().iter().all(|s| s.binary() != "sqls"));
    assert_eq!(
        by_binary("sqls"),
        None,
        "nor looked up, so nothing routes a document to it"
    );
    assert_eq!(
        Server::RUST_ANALYZER.binary(),
        "rust-analyzer",
        "and the builtins are untouched by any of it"
    );

    // Re-enabled: the same row, not a second one.
    let mut wanted = cide_ipc::lang::builtin_servers();
    wanted.push(def("sqls"));
    let again = install(wanted);
    assert_eq!(again[2], sqls, "`sqls` is the same handle it always was");

    // An updated extension's new arguments reach the row rather than appending another.
    let mut updated = def("sqls");
    updated.args = vec!["--config".into()];
    let mut wanted = cide_ipc::lang::builtin_servers();
    wanted.push(updated);
    install(wanted);
    assert_eq!(sqls.args(), vec!["--config".to_string()]);
    assert_eq!(sqls.binary(), "sqls");
}
