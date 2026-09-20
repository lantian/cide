//! The whole road, against the real sibling marketplace.
//!
//! Connect a local path, clone it, read its index, install every extension with the capabilities
//! its manifest asks for, and check what the registry resolved to. Every step goes through the
//! same [`ExtStore`](cide_ext::ExtStore) the app uses — no shortcut past the consent check, the
//! path jail or the merge.
//!
//! # Why this is an integration test and not a unit one
//!
//! It forks `git`, and that is the point of the test rather than an accident of it: `market.rs`
//! routes *every* source through the binary, a local path included, and a unit test that stubbed
//! the fork would be testing this crate's idea of a clone.
//!
//! It **skips itself** when the marketplace is not beside the checkout, which is the ordinary
//! state on a CI runner, and says so on stderr. A test that silently passed when its subject was
//! absent would be worse than one that says it was skipped.

use std::path::{Path, PathBuf};

/// The sibling marketplace, if this checkout has one beside it.
fn marketplace() -> Option<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidate = here.parent()?.parent()?.parent()?.join("cide-marketplace");
    candidate
        .join("cide-marketplace.json")
        .is_file()
        .then_some(candidate)
}

/// A throwaway XDG home, so the test never reads or writes the developer's own extensions.
///
/// `set_var` rather than a parameter, because `cide_core::persist` reads the environment directly
/// and threading a root through it would be a seam that exists only for this test. That makes it
/// process-global, which is why there is exactly one test in this file.
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

#[test]
fn the_sibling_marketplace_installs_and_supersedes_two_builtins() {
    let Some(source) = marketplace() else {
        eprintln!("skipped: no ../cide-marketplace beside this checkout");
        return;
    };
    let _xdg = Xdg::new("install");
    let proxy = cide_core::proxy::ProxyEnv::default();
    let store = cide_ext::ExtStore::new();

    // --- connect ---------------------------------------------------------------------------
    let snapshot = store
        .connect(&proxy, &source.to_string_lossy())
        .expect("connect");
    assert_eq!(snapshot.marketplaces.len(), 1);
    // Cloned rather than borrowed: the installs below move `snapshot` forward, and a reference
    // into the first one would pin it.
    let market = snapshot.marketplaces[0].clone();
    assert!(
        !market.authenticated,
        "a path on this machine reaches no network and uses no credential helper — the panel \
         shows that rather than making the user infer it from the URL"
    );
    assert!(
        matches!(market.state, cide_ipc::ext::MarketplaceState::Ready { .. }),
        "the clone did not land: {:?}",
        market.state
    );
    let ids: Vec<&str> = market.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "sql", "yaml", "proto", "graphql", "web", "showcase", "godot"
        ],
        "every extension the index lists was read"
    );
    assert!(
        market.problems.is_empty() && snapshot.problems.is_empty(),
        "the shipped manifests parse clean: {:?} {:?}",
        market.problems,
        snapshot.problems
    );
    for entry in &market.entries {
        assert!(
            entry.unavailable.is_none(),
            "{} is greyed: {:?}",
            entry.id,
            entry.unavailable
        );
    }
    for id in ["sql", "yaml", "proto", "graphql", "web"] {
        let entry = market
            .entries
            .iter()
            .find(|e| e.id.as_str() == id)
            .expect("entry");
        assert!(
            entry
                .capabilities
                .contains(&cide_ipc::ext::Capability::ProcessSpawn),
            "{id} contributes a language server, so it must ask for process:spawn"
        );
    }
    // The godot extension attaches to a server it does not start, so it asks for `lsp:connect`
    // and **not** `process:spawn` — the install sheet would otherwise say "run programs on your
    // machine" about an extension that runs none. (M59)
    let godot = market
        .entries
        .iter()
        .find(|e| e.id.as_str() == "godot")
        .expect("godot");
    assert_eq!(
        godot.capabilities,
        vec![cide_ipc::ext::Capability::LspConnect],
        "a connect server needs exactly the capability its transport needs"
    );
    // The showcase asks for everything, and that is the point of it: its Requests panel exists to
    // show what each capability answers.
    let showcase = market
        .entries
        .iter()
        .find(|e| e.id.as_str() == "showcase")
        .expect("showcase");
    assert_eq!(
        showcase.capabilities.len(),
        cide_ipc::ext::Capability::ALL.len(),
        "the showcase asks for every capability"
    );

    // --- the consent check -------------------------------------------------------------------
    let sql_ref = cide_ipc::ext::ExtensionRef {
        marketplace: market.id.clone(),
        extension: cide_ipc::ids::ExtensionId("sql".into()),
    };
    assert!(
        store
            .install(&sql_ref, &[cide_ipc::ext::Capability::EditorRead])
            .is_err(),
        "installing with a capability list that does not match the manifest must be refused — \
         without that, a marketplace could add process:spawn between the sheet being drawn and \
         the button being pressed"
    );

    // --- install all --------------------------------------------------------------------------
    let mut snapshot = snapshot;
    for id in [
        "sql", "yaml", "proto", "graphql", "web", "showcase", "godot",
    ] {
        let reference = cide_ipc::ext::ExtensionRef {
            marketplace: market.id.clone(),
            extension: cide_ipc::ids::ExtensionId(id.into()),
        };
        let granted: Vec<cide_ipc::ext::Capability> = snapshot.marketplaces[0]
            .entries
            .iter()
            .find(|e| e.id.as_str() == id)
            .expect("entry")
            .capabilities
            .clone();
        snapshot = store.install(&reference, &granted).expect("install");
    }
    assert_eq!(snapshot.extensions.len(), 7);
    for installed in &snapshot.extensions {
        assert!(installed.enabled);
        assert!(
            installed.unavailable.is_none(),
            "{} is greyed after install: {:?}",
            installed.id,
            installed.unavailable
        );
        if installed.id.extension.0 == "web" || installed.id.extension.0 == "godot" {
            // The marketplace's declarative extensions: no worker, contributions only. A
            // positive assertion rather than a skip, because "main is None and it still
            // contributes" is exactly the shape the manifest format promises is first-class.
            assert_eq!(
                installed.main, None,
                "{} ships no worker — languages and servers are data",
                installed.id
            );
        } else {
            assert_eq!(
                installed.main.as_deref(),
                Some("main.js"),
                "{} ships a worker, and the manifest's own entry is what reaches the frontend \
                 rather than a hard-coded name",
                installed.id
            );
            assert!(
                installed.path.join("main.js").is_file(),
                "the worker was copied, not linked — a symlink into the clone would let a \
                 refresh change code under a running app"
            );
        }
        assert!(
            !installed.path.join(".git").exists(),
            "and `.git` was not copied with it"
        );
        assert!(!installed.commit.is_empty(), "the install pinned a commit");
    }

    // --- what it all resolved to ---------------------------------------------------------------
    let resolved = &snapshot.resolved;
    for id in ["sql", "yaml"] {
        let binding = resolved
            .languages
            .iter()
            .find(|b| b.def.id == id)
            .unwrap_or_else(|| panic!("{id} is in the language registry"));
        assert!(
            matches!(
                binding.source,
                cide_ipc::ext::ContributionSource::Extension { .. }
            ),
            "{id} is contributed, not builtin — an extension that installs and changes nothing \
             on screen is indistinguishable from a broken extension host"
        );
        assert_eq!(
            binding.supersedes,
            Some(cide_ipc::ext::ContributionSource::Builtin),
            "and it says which source it displaced, so \"why is my .{id} coloured like that\" is \
             answerable without reading two manifests"
        );
    }
    // The complement of the supersede loop: a language cide never shipped displaces nothing, and
    // saying so is what keeps the panel's `was builtin` badge meaning something. The seven web
    // languages belong here *by design*: `web` deliberately re-declares neither `typescript`
    // nor `json` — the builtin grammars are hook-based and better than the rule form — and a
    // `supersedes` appearing on any of these would mean it started displacing a builtin.
    for id in [
        "proto",
        "graphql",
        "html",
        "css",
        "scss",
        "less",
        "vue",
        "svelte",
        "xml",
        "gdscript",
        "gdshader",
        "godot-resource",
    ] {
        let binding = resolved
            .languages
            .iter()
            .find(|b| b.def.id == id)
            .unwrap_or_else(|| panic!("{id} is in the language registry"));
        assert!(
            matches!(
                binding.source,
                cide_ipc::ext::ContributionSource::Extension { .. }
            ),
            "{id} is contributed"
        );
        assert_eq!(
            binding.supersedes, None,
            "{id} has no builtin to displace, so it must not claim one"
        );
    }
    assert!(
        resolved
            .languages
            .iter()
            .filter(|b| b.source == cide_ipc::ext::ContributionSource::Builtin)
            .count()
            >= 9,
        "installing six language extensions displaced only the two languages with builtins"
    );

    let servers: Vec<&str> = resolved
        .servers
        .iter()
        .map(|s| s.def.binary.as_str())
        .collect();
    assert!(servers.contains(&"rust-analyzer") && servers.contains(&"gopls"));
    assert!(
        servers.contains(&"sqls")
            && servers.contains(&"yaml-language-server")
            && servers.contains(&"protols")
            && servers.contains(&"buf")
            && servers.contains(&"graphql-lsp"),
        "every contributed server reached the registry — proto's two share one languageId, \
         which is first-installed-wins, not a conflict: {servers:?}"
    );
    // `web`'s five, and the two that matter most bind *builtin* language ids: that a server
    // may enrich a language its extension does not contribute is how JS/TS and JSON get an
    // LSP at all without displacing the builtin grammars.
    for binary in [
        "typescript-language-server",
        "vscode-html-language-server",
        "vscode-css-language-server",
        "vscode-json-language-server",
        "svelteserver",
    ] {
        assert!(servers.contains(&binary), "{binary} missing: {servers:?}");
    }
    let ts_server = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "typescript-language-server")
        .expect("typescript-language-server");
    assert_eq!(
        ts_server.def.language_ids,
        vec!["typescript".to_string()],
        "bound to the builtin id — the whole point of the web extension's server half"
    );
    // The marketplace's first `connect` server (M59): reached, never run. The binding carries
    // the endpoint and nothing a spawn would need, and the whole road — manifest validation
    // (loopback, a marker, no `args`), install, resolve — let it through clean.
    let godot_server = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "godot")
        .expect("godot reached the registry");
    let endpoint = godot_server
        .def
        .connect
        .as_ref()
        .expect("a connect server carries its endpoint");
    assert_eq!((endpoint.host.as_str(), endpoint.port), ("127.0.0.1", 6005));
    assert!(
        godot_server.def.args.is_empty(),
        "nothing is spawned, so nothing takes arguments"
    );
    assert_eq!(
        godot_server.def.project_markers,
        vec!["project.godot".to_string()]
    );
    let css_server = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "vscode-css-language-server")
        .expect("vscode-css-language-server");
    assert_eq!(
        css_server.def.language_ids,
        vec!["css".to_string(), "scss".to_string(), "less".to_string()],
        "one binary, three dialects — the multi-languageId shape `language_id_for` labels \
         documents for"
    );
    let yaml_server = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "yaml-language-server")
        .expect("yaml-language-server");
    assert_eq!(
        yaml_server.def.args,
        vec!["--stdio".to_string()],
        "the field that made this milestone necessary: `cide-lsp` built `Command::new(binary)` \
         with no arguments, and both builtins happen not to need any"
    );
    // Args are the load-bearing feature for both of these: `buf` without `lsp serve` is a CLI
    // that reads stdin and exits, and `graphql-lsp` without `--method stream` binds a socket.
    let buf = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "buf")
        .expect("buf");
    assert_eq!(buf.def.args, vec!["lsp".to_string(), "serve".to_string()]);
    let graphql_lsp = resolved
        .servers
        .iter()
        .find(|s| s.def.binary == "graphql-lsp")
        .expect("graphql-lsp");
    assert_eq!(
        graphql_lsp.def.args,
        vec![
            "server".to_string(),
            "--method".to_string(),
            "stream".to_string()
        ]
    );

    assert_eq!(
        resolved.panels.len(),
        6,
        "SQL's and GraphQL's bottom tabs, YAML's and proto's sidebar panels, and the showcase's \
         one of each"
    );
    let sidebar = resolved
        .panels
        .iter()
        .find(|p| p.def.location == cide_ipc::ext::PanelLocation::Sidebar)
        .expect("a sidebar panel");
    assert!(
        sidebar.view.starts_with("ext:"),
        "a contributed panel is addressed as an `ext:` view: {}",
        sidebar.view
    );
    assert!(sidebar.def.icon.is_some(), "and a rail button has an icon");

    let commands: Vec<&str> = resolved.commands.iter().map(|c| c.id.as_str()).collect();
    assert!(
        commands.iter().all(|id| id.starts_with("ext.")),
        "every contributed command id carries the prefix `keys/dispatch.ts` routes on: \
         {commands:?}"
    );
    assert!(resolved.conflicts.is_empty(), "{:?}", resolved.conflicts);

    // --- disabling puts the builtin back --------------------------------------------------
    let snapshot = store.set_enabled(&sql_ref, false).expect("disable");
    let binding = snapshot
        .resolved
        .languages
        .iter()
        .find(|b| b.def.id == "sql")
        .expect("sql");
    assert_eq!(
        binding.source,
        cide_ipc::ext::ContributionSource::Builtin,
        "disabling an extension returns the language to the builtin rather than leaving a gap"
    );
    assert!(
        snapshot
            .resolved
            .servers
            .iter()
            .all(|s| s.def.binary != "sqls"),
        "and takes its server with it"
    );
    assert_eq!(
        snapshot.resolved.panels.len(),
        5,
        "and its panel — a disabled extension keeps its row and nothing else"
    );
    assert!(
        store.asset_root(&sql_ref).is_none(),
        "a disabled extension serves no files, or \"disabled\" would mean \"its panels are \
         hidden\" rather than \"it is not running\""
    );

    // --- hiding a rail icon is not disabling anything (M31) -----------------------------------
    //
    // > *"we should be able to disable icon on left panel of installed extesions via settings"*
    //
    // The whole value of the flag is that it is *not* `enabled`. A user who does not want YAML
    // taking a rail slot still wants YAML highlighted, so this must leave the contribution
    // registry untouched — the panel is still resolved, the language is still the extension's,
    // the assets are still served — and change one boolean the rail reads. Folding it into
    // `set_enabled` would have been the obvious shortcut and it would have silently stopped a
    // worker; this asserts it did not.
    let yaml_ref = cide_ipc::ext::ExtensionRef {
        marketplace: market.id.clone(),
        extension: cide_ipc::ids::ExtensionId("yaml".into()),
    };
    let before = store.snapshot().resolved.panels.len();
    let snapshot = store
        .set_rail_icon(&yaml_ref, false)
        .expect("hide the rail icon");
    let yaml = snapshot
        .extensions
        .iter()
        .find(|e| e.id.extension.0 == "yaml")
        .expect("yaml is still installed");
    assert!(!yaml.rail_icon, "the flag is what moved");
    assert!(
        yaml.enabled,
        "and the extension is still enabled — hiding a button must not stop a worker"
    );
    assert_eq!(
        snapshot.resolved.panels.len(),
        before,
        "the panel is still contributed: the rail is what stops drawing it, and the panel stays \
         reachable from the overflow menu and the palette"
    );
    assert!(
        store.asset_root(&yaml_ref).is_some(),
        "and it still serves its files, unlike a disabled one"
    );
    let snapshot = store.set_rail_icon(&yaml_ref, true).expect("show it again");
    assert!(
        snapshot
            .extensions
            .iter()
            .find(|e| e.id.extension.0 == "yaml")
            .expect("yaml")
            .rail_icon,
        "and the flag goes back, so this is a toggle and not a one-way door"
    );

    // --- uninstalling removes the files ------------------------------------------------------
    let snapshot = store.uninstall(&sql_ref).expect("uninstall");
    assert_eq!(snapshot.extensions.len(), 6);
    assert!(
        !cide_ext::install_path(&market.id, &cide_ipc::ids::ExtensionId("sql".into())).exists(),
        "and the directory this crate created is the directory it removed"
    );

    // --- disconnecting takes the rest ---------------------------------------------------------
    let snapshot = store.disconnect(&market.id).expect("disconnect");
    assert!(snapshot.marketplaces.is_empty());
    assert!(
        snapshot.extensions.is_empty(),
        "an installed extension whose marketplace is gone cannot be updated or reinstalled, so \
         forgetting one uninstalls what came from it"
    );
    assert_eq!(
        snapshot
            .resolved
            .languages
            .iter()
            .filter(|b| b.source != cide_ipc::ext::ContributionSource::Builtin)
            .count(),
        0,
        "and the registry is back to the builtins"
    );
}
