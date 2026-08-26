//! When the Update button lights, against a synthetic marketplace driven with the real `git`.
//!
//! The check compares the **declared version** and nothing else. It used to also compare the
//! installed extension's pinned commit against the clone's HEAD — and HEAD moves for the whole
//! repository, so the first time the marketplace gained a new extension, every other extension
//! installed from it offered a byte-identical "update". The middle assertion here is the one
//! that failed under that rule.
//!
//! A synthetic repository rather than the sibling checkout, because this test has to *commit* to
//! its marketplace mid-run, and the sibling is somebody's working tree. Real `git` all the same:
//! `market.rs` routes every source through the binary, and a stubbed clone would test this
//! crate's idea of one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway XDG home — same shape as `marketplace.rs`'s, duplicated because integration test
/// binaries share no code, and process-global `set_var` is why each of these files holds exactly
/// one test.
struct Xdg(PathBuf);

impl Xdg {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cide-ext-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("config");
        std::fs::create_dir_all(root.join("state")).expect("state");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
            std::env::set_var("XDG_STATE_HOME", root.join("state"));
        }
        Self(root)
    }
}

impl Drop for Xdg {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(repo: &Path, message: &str) {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message]);
}

fn manifest(version: &str) -> String {
    format!(
        // `r##`, because the grammar's `"#"` line comment contains a raw string's `"#` closer.
        r##"{{
  "schema": 1,
  "id": "demo",
  "name": "Demo",
  "version": "{version}",
  "description": "A declarative language, for the update test.",
  "contributes": {{
    "languages": [
      {{
        "id": "demo",
        "label": "Demo",
        "extensions": [{{ "ext": "demo" }}],
        "grammar": {{ "name": "Demo", "lineComment": "#" }}
      }}
    ]
  }}
}}
"##
    )
}

/// The `demo` row of the connected marketplace, from a snapshot.
fn demo_entry(snapshot: &cide_ipc::ext::ExtensionSnapshot) -> cide_ipc::ext::MarketplaceEntry {
    snapshot.marketplaces[0]
        .entries
        .iter()
        .find(|e| e.id.as_str() == "demo")
        .expect("demo entry")
        .clone()
}

#[test]
fn update_lights_on_a_version_bump_and_on_nothing_else() {
    let xdg = Xdg::new("update");

    // --- a marketplace of one extension ------------------------------------------------------
    let source = xdg.0.join("market-src");
    std::fs::create_dir_all(source.join("extensions/demo")).expect("dirs");
    std::fs::write(
        source.join("cide-marketplace.json"),
        r#"{ "schema": 1, "name": "update test", "extensions": [{ "id": "demo" }] }"#,
    )
    .expect("index");
    std::fs::write(
        source.join("extensions/demo/cide-extension.json"),
        manifest("0.1.0"),
    )
    .expect("manifest");
    git(&source, &["init"]);
    // Local identity and no signing, so the commits below work on a machine configured for
    // neither or for GPG prompts.
    git(&source, &["config", "user.email", "test@cide.invalid"]);
    git(&source, &["config", "user.name", "cide test"]);
    git(&source, &["config", "commit.gpgsign", "false"]);
    commit_all(&source, "demo 0.1.0");

    let proxy = cide_core::proxy::ProxyEnv::default();
    let store = cide_ext::ExtStore::new();
    let snapshot = store
        .connect(&proxy, &source.to_string_lossy())
        .expect("connect");
    let market_id = snapshot.marketplaces[0].id.clone();
    assert!(
        !demo_entry(&snapshot).update_available,
        "nothing is installed, so nothing can have an update"
    );

    // --- install ------------------------------------------------------------------------------
    let reference = cide_ipc::ext::ExtensionRef {
        marketplace: market_id.clone(),
        extension: cide_ipc::ids::ExtensionId("demo".into()),
    };
    let snapshot = store.install(&reference, &[]).expect("install");
    assert_eq!(demo_entry(&snapshot).installed.as_deref(), Some("0.1.0"));
    assert!(!demo_entry(&snapshot).update_available);

    // --- an unrelated commit is not an update -------------------------------------------------
    std::fs::write(source.join("README.md"), "another extension landed\n").expect("readme");
    commit_all(&source, "unrelated");
    let snapshot = store.refresh(&proxy, &market_id);
    assert!(
        matches!(
            snapshot.marketplaces[0].state,
            cide_ipc::ext::MarketplaceState::Ready { .. }
        ),
        "the refresh pulled: {:?}",
        snapshot.marketplaces[0].state
    );
    assert!(
        !demo_entry(&snapshot).update_available,
        "demo's own files did not change, so there is nothing to offer — this is the assertion \
         the HEAD comparison failed"
    );

    // --- a version bump is ---------------------------------------------------------------------
    std::fs::write(
        source.join("extensions/demo/cide-extension.json"),
        manifest("0.2.0"),
    )
    .expect("manifest");
    commit_all(&source, "demo 0.2.0");
    let snapshot = store.refresh(&proxy, &market_id);
    let entry = demo_entry(&snapshot);
    assert_eq!(entry.version, "0.2.0");
    assert_eq!(entry.installed.as_deref(), Some("0.1.0"));
    assert!(
        entry.update_available,
        "a bumped version is what Update means"
    );

    // --- and taking the update clears it -------------------------------------------------------
    let snapshot = store.install(&reference, &[]).expect("update");
    let entry = demo_entry(&snapshot);
    assert_eq!(entry.installed.as_deref(), Some("0.2.0"));
    assert!(!entry.update_available);
}
