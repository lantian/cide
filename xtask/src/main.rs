//! Repository automation. Run as `cargo xtask <task>`.
//!
//! The tasks that matter are the CI gates:
//!
//! * `codegen [--check]`  — regenerate `ui/src/ipc/generated.ts` from the `cide-ipc` DTOs.
//!   With `--check` it fails when the checked-in output is stale, so a Rust field rename
//!   turns into a red build rather than a runtime `undefined` in the webview.
//! * `contract-check`     — reflect over the registered commands and events and diff them
//!   against `contract/{commands,events}.json`, so adding a command is a deliberate
//!   three-file change that a reviewer can see.
//! * `bench-ipc`          — measure the PTY transport: build `cide-app`, run it under
//!   `CIDE_BENCH=1`, print the report and exit non-zero on NO-GO. This is the M0 GO/NO-GO
//!   gate. It needs a display, and it needs a frontend for the binary to load — a Vite dev
//!   server on :1420 for the debug profile, a built `ui/dist` for `--release`. See
//!   `BENCH.md`.
//! * `verify-cli`         — check that the installed `claude` still completes the IDE
//!   handshake, and that `SUPPORTED_CLI` records the version that did. **Not a CI gate** and
//!   must never be added to one: it needs the CLI installed and a working login. See
//!   [`verify_cli`].
//! * `package`            — preflight the packaging and print the plan; `--write`
//!   regenerates the Flatpak files and `--check` gates them. It builds nothing unless asked
//!   with `--run`. `--src` is the cheap one: `git archive` of HEAD into a release-page source
//!   tarball, no compiler involved. See `package.rs` and `docs/adr/0007`.
//!
//! Both gates are line-oriented lints over source text rather than macro reflection or a
//! linked-in `cide-app`. That is a deliberate trade: `generate_handler!` expands inside a
//! crate that links a webview, and building it to ask it what it registered would make the
//! cheapest check in CI the most expensive. A text lint is readable by anyone reviewing
//! this file, and its failure mode is a false positive that a human clears in one command
//! (`cargo xtask contract-check --write`) rather than a wrong answer nobody notices.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};

mod package;

const USAGE: &str = "\
cargo xtask <task>

Tasks:
  codegen [--check]        regenerate ui/src/ipc/generated.ts from cide-ipc
  contract-check [--write] diff the live command/event surface against contract/*.json
  verify-cli [--no-build]  check that the installed `claude` still speaks the IDE protocol
                             and that SUPPORTED_CLI records it. Spawns the real CLI under a
                             pty; no model turn, so it costs nothing. Not a CI gate.
  bench-ipc [--release]    measure IPC throughput (M0 GO/NO-GO gate). Builds cide-app,
            [--no-build]     runs it under CIDE_BENCH=1, prints the report and fails on
                             NO-GO. Needs a display. The debug profile also needs the Vite
                             dev server on :1420 (`pnpm --dir ui dev`); --release builds
                             ui/dist and embeds it instead. See BENCH.md.
  package [targets] [...]  preflight the packaging and print the plan
                             targets: --appimage --deb --flatpak --tarball (Linux)
                                      --app --dmg                          (macOS)
                                      --src                                (any host)
                             default: everything this host is responsible for. Nothing
                             here cross-compiles, so naming a bundle the host cannot
                             produce fails the preflight rather than printing a plan.
                             --src is the exception — any host can run `git archive` —
                             but it is in the Linux default set only, so a release
                             matrix uploads one source tarball rather than two.
                             --write  regenerate packaging/flatpak/*
                             --check  fail if those files are stale (CI gate)
                             --run    actually build; otherwise nothing is built
  help                     show this message
";

/// Where `ts-rs` drops one `.ts` file per exported type when `cide-ipc`'s tests run.
const BINDINGS_DIR: &str = "crates/cide-ipc/bindings";

/// The single file the frontend imports. Nothing else in `ui/` may import from
/// `bindings/`, which is why that directory stays out of the Vite root.
const GENERATED_TS: &str = "ui/src/ipc/generated.ts";

/// The header `ts-rs` puts at the top of every file it writes. Kept verbatim during
/// concatenation, which makes it a dependable block delimiter — that is what lets
/// `--check` name the types that drifted instead of printing a line offset.
const TS_RS_BANNER: &str = "// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.";

/// The builtin language and language-server tables, emitted beside the `ts-rs` bindings by
/// `cide-ipc`'s own `export_builtin_languages` test. A `.json` rather than a `.ts`, so
/// [`refresh_bindings`] — which clears `*.ts` before every run — leaves it alone.
const BUILTINS_JSON: &str = "crates/cide-ipc/bindings/builtins.json";

/// Where those tables land as TypeScript.
///
/// A second generated file rather than more of `generated.ts`, because it is not a type: it is
/// *data*, `ui/src/editor/languages.ts` imports it as a value, and `ui/scripts/check-editor.mjs`
/// compiles it standalone to pin the fold specs against the grammars. Concatenating a value into
/// a file of `export type` declarations would make that check import the whole wire contract to
/// read eleven records.
const BUILTIN_LANGUAGES_TS: &str = "ui/src/editor/builtinLanguages.ts";

const HANDLER_SRC: &str = "crates/cide-app/src/lib.rs";
const EMIT_SRC: &str = "crates/cide-app/src/emit.rs";
const COMMANDS_JSON: &str = "contract/commands.json";
const EVENTS_JSON: &str = "contract/events.json";

const BANNER: &str = "\
// GENERATED FILE — DO NOT EDIT.
//
// Every type below comes from a `#[derive(TS)]` type in `crates/cide-ipc`, concatenated in
// type-name order. Regenerate with `cargo xtask codegen`.
//
// CI runs `cargo xtask codegen --check`, so a Rust field rename that never reached this
// file fails the build instead of surfacing as an `undefined` in the webview at runtime.
";

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    let rest: Vec<String> = std::env::args().skip(2).collect();

    let result = match task.as_str() {
        "codegen" => flags(&rest, &["--check"]).and_then(|f| codegen(f.contains("--check"))),
        "contract-check" => {
            flags(&rest, &["--write"]).and_then(|f| contract_check(f.contains("--write")))
        }
        "verify-cli" => {
            flags(&rest, &["--no-build"]).and_then(|f| verify_cli(!f.contains("--no-build")))
        }
        "bench-ipc" => flags(&rest, &["--release", "--no-build"])
            .and_then(|f| bench_ipc(f.contains("--release"), !f.contains("--no-build"))),
        "package" => flags(
            &rest,
            &[
                "--appimage",
                "--deb",
                "--flatpak",
                "--tarball",
                "--app",
                "--dmg",
                "--src",
                "--write",
                "--check",
                "--run",
            ],
        )
        .and_then(|f| {
            // Naming no target means everything *this host* is responsible for, which is what
            // someone typing `package` to see the state of things wants. Which those are is
            // decided in `package.rs`, from `rustc -vV`, because the host is that module's
            // business and it is also what the preflight has to report against. Naming one means
            // only that one — including one this machine cannot produce, which is a refusal with
            // a reason rather than a silently narrowed plan.
            //
            // `--src` is in that list like any other: it is not in the macOS default set (one
            // source tarball per release, cut on Linux), but asking for it anywhere is a plan and
            // not a refusal, because `git archive` runs the same everywhere.
            let named = [
                "--appimage",
                "--deb",
                "--flatpak",
                "--tarball",
                "--app",
                "--dmg",
                "--src",
            ]
            .iter()
            .any(|t| f.contains(*t));
            let targets = named.then(|| package::Targets {
                appimage: f.contains("--appimage"),
                deb: f.contains("--deb"),
                flatpak: f.contains("--flatpak"),
                app: f.contains("--app"),
                dmg: f.contains("--dmg"),
                tarball: f.contains("--tarball"),
                src: f.contains("--src"),
                // Named on the command line, by construction: this whole literal only runs when
                // `named` is true. The distinction it carries is about the DIRTY-TREE verdict —
                // a release artifact must match HEAD, while a default run that happens to
                // include the tarball must not refuse to build the AppImage somebody asked for.
                src_named: f.contains("--src"),
            });
            let root = workspace_root()?;
            package::package(
                &root,
                package::Options {
                    targets,
                    write: f.contains("--write"),
                    check: f.contains("--check"),
                    run: f.contains("--run"),
                },
            )
        }),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("unknown task: {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Accept only the flags a task knows about.
///
/// A typo in a CI script is otherwise silent: `--wrtie` would leave `contract-check`
/// reporting drift it was asked to accept.
fn flags<'a>(args: &[String], allowed: &[&'a str]) -> Result<BTreeSet<&'a str>> {
    let mut seen = BTreeSet::new();
    for arg in args {
        let Some(flag) = allowed.iter().find(|a| **a == arg.as_str()) else {
            bail!("unexpected argument `{arg}`\n\n{USAGE}");
        };
        seen.insert(*flag);
    }
    Ok(seen)
}

fn workspace_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest.parent() else {
        bail!(
            "xtask manifest has no parent directory: {}",
            manifest.display()
        );
    };
    Ok(root.to_path_buf())
}

// --- codegen ---------------------------------------------------------------------------

/// One exported type: its name, and its `.ts` text with sibling imports removed.
struct Block {
    name: String,
    body: String,
}

fn codegen(check: bool) -> Result<()> {
    let root = workspace_root()?;
    let bindings = root.join(BINDINGS_DIR);
    let target = root.join(GENERATED_TS);

    refresh_bindings(&root, &bindings)?;
    let blocks = read_blocks(&bindings)?;
    if blocks.is_empty() {
        bail!(
            "no bindings in {} — did the `export_bindings_*` tests run?",
            bindings.display()
        );
    }
    let expected = render(&blocks);

    let builtins_target = root.join(BUILTIN_LANGUAGES_TS);
    let builtins_expected = render_builtins(&root)?;

    if check {
        let current = match fs::read_to_string(&target) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!("{GENERATED_TS} is missing — run `cargo xtask codegen`");
            }
            Err(e) => return Err(e).context(format!("reading {}", target.display())),
        };
        if current != expected {
            bail!(
                "{GENERATED_TS} is stale — run `cargo xtask codegen`\n{}",
                describe_drift(&current, &expected)
            );
        }
        let current = match fs::read_to_string(&builtins_target) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!("{BUILTIN_LANGUAGES_TS} is missing — run `cargo xtask codegen`");
            }
            Err(e) => return Err(e).context(format!("reading {}", builtins_target.display())),
        };
        if current != builtins_expected {
            bail!("{BUILTIN_LANGUAGES_TS} is stale — run `cargo xtask codegen`");
        }
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).context(format!("creating {}", parent.display()))?;
    }
    fs::write(&target, &expected).context(format!("writing {}", target.display()))?;
    println!("codegen: wrote {GENERATED_TS} ({} types)", blocks.len());

    if let Some(parent) = builtins_target.parent() {
        fs::create_dir_all(parent).context(format!("creating {}", parent.display()))?;
    }
    fs::write(&builtins_target, &builtins_expected)
        .context(format!("writing {}", builtins_target.display()))?;
    println!("codegen: wrote {BUILTIN_LANGUAGES_TS}");
    Ok(())
}

/// Render `cide_ipc::lang::builtins()` as a TypeScript module.
///
/// The JSON is emitted verbatim as the module's two values, pretty-printed by `serde_json` so the
/// file diffs line by line when a keyword is added to a table. Types come from `generated.ts` and
/// are `import type`, so nothing here is imported at runtime by the check script that compiles
/// this module alone.
fn render_builtins(root: &Path) -> Result<String> {
    let path = root.join(BUILTINS_JSON);
    let text = fs::read_to_string(&path).context(format!(
        "reading {} — did the `export_builtin_languages` test run?",
        path.display()
    ))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context(format!("parsing {}", path.display()))?;
    let languages = serde_json::to_string_pretty(&value["languages"])
        .context("rendering the builtin languages")?;
    let servers =
        serde_json::to_string_pretty(&value["servers"]).context("rendering the builtin servers")?;
    Ok(format!(
        "{BANNER}
// Written by `cargo xtask codegen` from `cide_ipc::lang::builtins()`.
//
// A builtin's *tokenizer* is not here and cannot be: it contains a function, so it stays in
// `ui/src/editor/languages/<id>.ts` and is reached by the dynamic import in `languages.ts`.
// Everything else about a language — which extensions it claims, what the status bar calls it,
// how it folds, whether the scratch picker offers it — is a table, and a table that has to agree
// with the ones an extension contributes has to have exactly one home. This is it.
import type {{ LanguageDef, LanguageServerDef }} from '../ipc/generated'

export const BUILTIN_LANGUAGES: readonly LanguageDef[] = {languages}

export const BUILTIN_SERVERS: readonly LanguageServerDef[] = {servers}
"
    ))
}

/// Re-run the `ts-rs` exporters, having first cleared the output directory.
///
/// The clear is the point: `ts-rs` only ever writes, so a DTO deleted from `cide-ipc`
/// would otherwise leave its `.ts` file behind and the generated bundle would keep
/// exporting a type that no longer exists on the wire.
///
/// The cleared files are held in memory and put back if the test run fails, so a `cide-ipc`
/// that does not compile costs a red exit and nothing else. Without that, `codegen --check`
/// — a task whose whole contract is that it only reads — would strip the bindings directory
/// on its way out and leave the next `--check` complaining about a file it destroyed.
fn refresh_bindings(root: &Path, bindings: &Path) -> Result<()> {
    let mut cleared: Vec<(PathBuf, String)> = Vec::new();
    if bindings.is_dir() {
        for entry in fs::read_dir(bindings).context(format!("reading {}", bindings.display()))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "ts") {
                let text =
                    fs::read_to_string(&path).context(format!("reading {}", path.display()))?;
                fs::remove_file(&path).context(format!("removing {}", path.display()))?;
                cleared.push((path, text));
            }
        }
    }

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(&cargo)
        .args(["test", "-p", "cide-ipc"])
        .current_dir(root)
        // Test-harness chatter goes to stdout and says nothing useful here; cargo's
        // progress — including "Blocking waiting for file lock" — goes to stderr and is
        // exactly what a developer needs to see while this waits.
        .stdout(Stdio::null())
        .status()
        .context("running `cargo test -p cide-ipc`")?;
    if !status.success() {
        for (path, text) in &cleared {
            fs::write(path, text).context(format!("restoring {}", path.display()))?;
        }
        bail!("`cargo test -p cide-ipc` failed; the bindings were not regenerated");
    }
    Ok(())
}

/// Read every `.ts` file in `bindings`, sorted by type name so the output is byte-stable
/// regardless of directory iteration order.
fn read_blocks(bindings: &Path) -> Result<Vec<Block>> {
    let paths: Vec<PathBuf> = fs::read_dir(bindings)
        .context(format!("reading {}", bindings.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "ts"))
        .collect();

    let names: BTreeSet<String> = paths
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();

    let mut blocks = Vec::with_capacity(paths.len());
    for path in &paths {
        let text = fs::read_to_string(path).context(format!("reading {}", path.display()))?;
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        blocks.push(Block {
            name,
            body: strip_sibling_imports(&text, &names),
        });
    }
    blocks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(blocks)
}

/// Drop the `import type { X } from "./X";` lines, which are meaningless once every type
/// shares one file.
///
/// Only imports naming a type we actually generated are removed. Anything else — a future
/// `ts-rs` emitting an import from a real npm package, say — is kept verbatim, so the
/// failure is a TypeScript error somebody reads rather than a silently dangling reference.
fn strip_sibling_imports(text: &str, generated: &BTreeSet<String>) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !sibling_import_of(line).is_some_and(|m| generated.contains(m)))
        .collect();
    kept.join("\n").trim_end().to_string()
}

/// The module name of a relative `import type ... from "./Name";` line, if that is what
/// this line is.
fn sibling_import_of(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with("import ") {
        return None;
    }
    let rest = line.split_once("from \"./")?.1;
    Some(rest.split_once('"')?.0)
}

/// Join the blocks under the banner: one blank line between blocks, exactly one trailing
/// newline, no other whitespace variation. A no-op run has to be byte-identical or the
/// `--check` gate cries wolf.
fn render(blocks: &[Block]) -> String {
    let mut out = String::from(BANNER);
    for block in blocks {
        out.push('\n');
        out.push_str(&block.body);
        out.push('\n');
    }
    out
}

/// Split a rendered file back into per-type bodies, keyed by exported type name.
fn parse_blocks(text: &str) -> BTreeMap<String, &str> {
    text.split(TS_RS_BANNER)
        .skip(1)
        .filter_map(|chunk| Some((exported_name(chunk)?.to_string(), chunk.trim())))
        .collect()
}

/// The identifier in the block's `export type <Name> = ...` line.
fn exported_name(chunk: &str) -> Option<&str> {
    let decl = chunk
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("export type "))?;
    let end = decl
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(decl.len());
    Some(&decl[..end])
}

/// A per-type summary of how the file on disk differs from what codegen would write.
///
/// Line offsets would be useless here: one added field near the top of a 40-type bundle
/// shifts every line after it. Naming the types is what tells a reviewer whether the diff
/// is the change they made.
fn describe_drift(current: &str, expected: &str) -> String {
    let (current, expected) = (parse_blocks(current), parse_blocks(expected));
    let mut out = String::new();
    for name in expected.keys().filter(|n| !current.contains_key(*n)) {
        out.push_str(&format!("  new      {name}\n"));
    }
    for name in current.keys().filter(|n| !expected.contains_key(*n)) {
        out.push_str(&format!("  removed  {name}\n"));
    }
    for (name, body) in &expected {
        if current.get(name).is_some_and(|c| c != body) {
            out.push_str(&format!("  changed  {name}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("  no type differs; the banner or the spacing between blocks does\n");
    }
    out
}

// --- contract-check --------------------------------------------------------------------

fn contract_check(write: bool) -> Result<()> {
    let root = workspace_root()?;
    let commands = scan_commands(&root.join(HANDLER_SRC))?;
    let events = scan_events(&root.join(EMIT_SRC))?;

    if write {
        write_contract(&root.join(COMMANDS_JSON), &commands)?;
        write_contract(&root.join(EVENTS_JSON), &events)?;
        println!(
            "contract-check: wrote {} commands, {} events",
            commands.len(),
            events.len()
        );
        return Ok(());
    }

    let mut report = String::new();
    report.push_str(&compare(
        COMMANDS_JSON,
        &read_contract(&root.join(COMMANDS_JSON))?,
        &commands,
    ));
    report.push_str(&compare(
        EVENTS_JSON,
        &read_contract(&root.join(EVENTS_JSON))?,
        &events,
    ));

    if !report.is_empty() {
        bail!(
            "the command surface has drifted from the checked-in contract\n{report}\n\
             Accept it with `cargo xtask contract-check --write`, and make sure the \
             frontend client in ui/src/ipc/client.ts moved with it."
        );
    }
    println!(
        "contract-check: {} commands, {} events — in sync",
        commands.len(),
        events.len()
    );
    Ok(())
}

/// Additions and removals, reported separately: an addition is usually intentional and an
/// removal usually breaks a caller, and a reviewer wants to see which one happened.
fn compare(what: &str, contract: &BTreeSet<String>, live: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for name in live.difference(contract) {
        out.push_str(&format!("  {what}: + {name}\n"));
    }
    for name in contract.difference(live) {
        out.push_str(&format!("  {what}: - {name}\n"));
    }
    out
}

/// Read a contract file, treating absence as an empty set so that seeding a new contract
/// is one `--write` away rather than an error about a file the task is about to create.
fn read_contract(path: &Path) -> Result<BTreeSet<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let names: Vec<String> = serde_json::from_str(&text)
        .context(format!("{} is not a JSON array of strings", path.display()))?;
    Ok(names.into_iter().collect())
}

fn write_contract(path: &Path, names: &BTreeSet<String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context(format!("creating {}", parent.display()))?;
    }
    let names: Vec<&String> = names.iter().collect();
    let mut json = serde_json::to_string_pretty(&names)?;
    json.push('\n');
    fs::write(path, json).context(format!("writing {}", path.display()))
}

/// Pull the command names out of the single `tauri::generate_handler![...]` list.
///
/// Tauri derives a command's name from its function name, so the last path segment of each
/// entry is the string the frontend invokes: `cmd::session::session_attach` is
/// `session_attach`.
fn scan_commands(path: &Path) -> Result<BTreeSet<String>> {
    let text = fs::read_to_string(path).context(format!("reading {}", path.display()))?;
    let code = strip_line_comments(&text);
    let Some(after) = code.split_once("generate_handler!") else {
        bail!(
            "no `generate_handler!` in {} — the command surface cannot be read",
            path.display()
        );
    };
    let list = bracketed(after.1).with_context(|| {
        format!(
            "`generate_handler!` in {} is not followed by a closed `[...]` list",
            path.display()
        )
    })?;

    let mut names = BTreeSet::new();
    for entry in list.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let name = entry.rsplit("::").next().unwrap_or(entry);
        // The whole entry is checked, not only the segment after the last `::`. A block
        // comment around an entry (`/* cmd::app::app_quit */`) still ends in a plausible
        // name, so validating the tail alone would register a handler that is commented
        // out — the exact mistake this gate exists to catch.
        let path_shaped = entry
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':');
        let name_shaped = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !path_shaped || !name_shaped {
            bail!(
                "cannot read `{entry}` in the `generate_handler!` list of {} as a command \
                 name; this lint parses source text, so keep the list one plain path per \
                 entry",
                path.display()
            );
        }
        names.insert(name.to_string());
    }
    Ok(names)
}

/// Blank out `//` comments, preserving line structure.
///
/// This runs over the whole file before anything else looks at it, and the order is
/// load-bearing in two directions. Stripping after the search would let a doc comment that
/// mentions `generate_handler!` win the search and silently reduce the command surface to
/// whatever brackets follow it. Stripping after splitting the list on commas would let a
/// comma inside a comment cut one entry in two, leaving a tail that reads as a command name
/// nobody registered.
///
/// String literals are not respected, so a `//` inside one is cut as well. Nothing between
/// `generate_handler!` and its closing bracket is a string literal, and this function's only
/// caller never reads past that point.
fn strip_line_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The contents of the first `[...]` in `text`, respecting nesting.
fn bracketed(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let mut depth = 0usize;
    for (i, c) in text[start..].char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start + 1..start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Collect the `cide://...` event names emitted by the app.
///
/// `emit.rs` arrives in a later milestone. Its absence is an empty event surface, not a
/// failure: the gate has to be green on a tree that has not grown events yet.
fn scan_events(path: &Path) -> Result<BTreeSet<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };

    // A whole-line comment is not an emit site, and event names get discussed in doc
    // comments more than most strings do. Trailing comments are deliberately left in
    // place: `//` also occurs inside the literal being searched for, so cutting each line
    // at its first `//` would truncate every event name to `cide:`.
    let code: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut names = BTreeSet::new();
    let mut rest = code.as_str();
    while let Some(open) = rest.find("\"cide://") {
        let literal = &rest[open + 1..];
        let Some(end) = literal.find('"') else {
            bail!(
                "unterminated `cide://` string literal in {}",
                path.display()
            );
        };
        names.insert(literal[..end].to_string());
        rest = &literal[end + 1..];
    }
    Ok(names)
}

// --- bench-ipc -------------------------------------------------------------------------

/// How long the app gets to open a window, run the measurement and exit.
///
/// Generous on purpose: the gate now moves several GiB through the pull path, and a debug
/// build on a loaded machine is slow. It exists only so a window that never appears — a
/// compositor refusing the surface, a webview that failed to create — fails the task instead
/// of hanging a CI job until the job's own timeout kills it with no output.
const BENCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

/// How often the deadline is checked while the app runs.
const BENCH_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// The port `tauri.conf.json` names as `devUrl`.
///
/// A *debug* `cide` does not embed a frontend: `frontendDist` is only compiled in for a
/// release build, and the debug binary opens `http://localhost:1420` instead. With nothing
/// listening there the window comes up on WebKitGTK's "could not connect" page, which
/// never reaches `runBench`, never calls `diag_bench_report` and never exits — so the gate
/// would sit out the whole [`BENCH_TIMEOUT`] and then report a timeout. Fifteen minutes to
/// say "you forgot the dev server" is not a gate. `run.sh` starts Vite for this same reason.
const DEV_SERVER_PORT: u16 = 1420;

/// How long to wait for the loopback connect that answers "is the dev server up".
///
/// Loopback either accepts immediately or refuses immediately; the timeout is only so a
/// pathological firewall rule that black-holes the SYN cannot hang the gate before it has
/// even started.
const DEV_SERVER_PROBE: std::time::Duration = std::time::Duration::from_millis(250);

/// Is something accepting connections on `port` on the loopback interface?
///
/// A bare connect rather than an HTTP request: the question is only whether the debug
/// binary will find *a* server where it is about to look, and Vite accepts on the same
/// socket it serves on. Asking for a document instead would mean deciding what a valid
/// answer looks like, which is the webview's job and not this one's.
fn port_listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, DEV_SERVER_PROBE).is_ok()
}

/// The GO/NO-GO answer parsed out of a report.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    go: bool,
    line: String,
}

/// Read the verdict line out of a benchmark report.
///
/// Anchored with `starts_with` rather than `contains`, which is the whole reason this is a
/// named function with tests: `"NO-GO — …"` contains `"GO"`, so the obvious inline check is
/// a gate that passes exactly when it should fail. With the anchor the two arms are
/// mutually exclusive and their order does not matter — the anchor is doing the work, not
/// the ordering, and `a_no_go_report_is_not_mistaken_for_a_pass` is what keeps it that way.
fn bench_verdict(report: &str) -> Option<Verdict> {
    report.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with("NO-GO") {
            Some(Verdict {
                go: false,
                line: line.to_string(),
            })
        } else if line.starts_with("GO") {
            Some(Verdict {
                go: true,
                line: line.to_string(),
            })
        } else {
            None
        }
    })
}

/// The marker `real_cli.rs` prints so this task can read the verdict off an ordinary test run.
const VERIFY_MARKER: &str = "VERIFY-CLI:";

/// Check that the installed `claude` still speaks the IDE protocol, and that
/// `SUPPORTED_CLI` records the version that proved it.
///
/// # Why this is a task and not just a test
///
/// The assertion belongs in a `#[test]`: the handshake needs a WebSocket client, a `script(1)`
/// pty, the diff broker and the process-group reaping that `tests/real_cli.rs` already
/// contains, and re-implementing that here would be a second copy of the most fragile code in
/// the repository.
///
/// But *telling a developer* "run `cargo test -p cide-ide-mcp -- --ignored` and then edit
/// `protocol.rs:45`" is not a gate. This repository has already made that argument once, about
/// this exact shape: `bench_ipc` used to `bail!` with the command to type, and the note on it
/// says why that was changed — "that is not a gate — it fails identically on a fast machine
/// and a slow one". Same here. So the front door does the preflight, drives the test, and
/// prints the one-line edit the outcome asks for.
///
/// # It must never be added to `ci.yml`
///
/// `.github/workflows/ci.yml` says `--ignored` "is deliberately NOT passed, and must not be
/// added", and this needs the same three things a CI runner does not have: the CLI installed,
/// a logged-in account, and network access. Unlike its neighbour in that file it needs no
/// money — the test types no prompt and calls no model — but the first three are enough.
///
/// # What it does *not* do
///
/// It does not edit `protocol.rs`. The whole point of `SUPPORTED_CLI` is that a version is in
/// it because somebody looked at the evidence; a task that appended the version automatically
/// would restore exactly the property this work removed — a record nothing produced.
fn verify_cli(build: bool) -> anyhow::Result<()> {
    let root = workspace_root()?;

    // Preflight first, so an absent CLI costs a second rather than a full test build, and is
    // reported as itself rather than as a mysterious skip buried in test output.
    let Some(claude) = on_path("claude") else {
        bail!(
            "no `claude` on PATH. This task checks the installed CLI against the protocol \
             this build transcribes; without one there is nothing to check."
        );
    };
    if on_path("script").is_none() {
        bail!(
            "no `script(1)` on PATH. The CLI only opens an IDE connection from its \
             interactive UI — a `-p` run opens no socket at all — so the check needs a pty, \
             and `script` is how it gets one."
        );
    }
    eprintln!("xtask: using {}", claude.display());
    if let Some(version) = probe_version(&claude) {
        // Printed before the run, so a hang has a version attached to it in the scrollback.
        eprintln!("xtask: `claude --version` says {version}");
    }

    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(&root).args([
        "test",
        "-p",
        "cide-ide-mcp",
        "--test",
        "real_cli",
        "the_installed_cli_is_one_this_build_has_checked",
        "--",
        "--ignored",
        "--nocapture",
        // One at a time: the test spawns a real `claude` that writes a lockfile into the
        // user's `~/.claude/ide`, and two of those racing for the same directory is the one
        // way this can fail for a reason that is about the harness.
        "--test-threads=1",
    ]);
    if !build {
        // `--no-run` would defeat the point; this only skips the *rebuild* by relying on
        // whatever is already compiled, which is what a developer iterating wants.
        cargo.env("CARGO_INCREMENTAL", "1");
    }

    // stderr to a file rather than a pipe, and stdout inherited: the test's own narration —
    // which version connected, what the transcript said — is the part a human reads live,
    // while the marker line has to be recoverable afterwards. A pipe would have to be drained
    // concurrently or the child blocks once the buffer fills.
    let log = root.join("target").join("verify-cli.out");
    fs::create_dir_all(log.parent().unwrap_or(&root))?;
    let sink = fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
    let tee = sink.try_clone()?;

    let status = cargo
        .stdout(Stdio::from(sink))
        .stderr(Stdio::from(tee))
        .status()
        .context("running the real-CLI check")?;

    let output = fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
    print!("{output}");

    let verdict = verify_verdict(&output);
    match verdict.as_deref() {
        Some(line) => eprintln!("xtask: {line}"),
        // The marker is printed before any assertion fires, so its absence means the test did
        // not reach the point of having an opinion — a compile failure, a panic in setup, a
        // harness that never ran it. That is a failed check, not a passed one. Silence is the
        // answer `bench_ipc` used to give by accident, and it is the same mistake here.
        None => bail!(
            "the run produced no `{VERIFY_MARKER}` line — see {}. The check never reached a \
             verdict, which is not the same as passing it.",
            log.display()
        ),
    }

    if !status.success() {
        bail!(
            "the installed CLI did not pass. The failure message above names the exact edit; \
             full output in {}.",
            log.display()
        );
    }
    eprintln!("xtask: full output written to {}", log.display());
    Ok(())
}

/// The last `VERIFY-CLI:` line of a run, which is the verdict.
///
/// Last rather than first, so a rerun in the same log answers about the rerun.
fn verify_verdict(output: &str) -> Option<String> {
    output
        .lines()
        .rfind(|line| line.trim_start().starts_with(VERIFY_MARKER))
        .map(|line| line.trim().to_string())
}

/// The first `program` on `PATH`.
///
/// The same three lines as the four copies in the real-CLI tests. Not shared with them: this
/// crate is not in that dependency graph and adding it to one so a preflight can find a binary
/// would be the wrong direction for four lines.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// `claude --version`, for the preflight line only. The verdict comes off the wire.
fn probe_version(claude: &Path) -> Option<String> {
    let output = Command::new(claude).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Run the M0 GO/NO-GO gate: build the app, run it under `CIDE_BENCH=1`, judge the report.
///
/// This used to `bail!` with the command to type. That is not a gate — it fails identically
/// on a fast machine and a slow one — so it now does what it was telling you to do. The
/// measurement still has to happen inside a real webview on a real compositor; what changed
/// is that the driving, the timeout and the verdict are here instead of in a human's memory.
///
/// **It needs a display.** There is no headless mode: the number being measured is the cost
/// of a payload crossing into a live WebKitGTK webview, and a mocked one would measure
/// nothing anyone cares about. The display check up front is so that absence is reported as
/// itself rather than as a webview crash.
///
/// **And it needs a frontend**, which is not the same thing in both profiles and is the one
/// way this task can look like it works and quietly not:
///
/// * debug — the binary loads `devUrl`, so a Vite dev server has to be listening on
///   [`DEV_SERVER_PORT`]. Without one the window opens on a connection-error page, `runBench`
///   never runs, and the only symptom is the full [`BENCH_TIMEOUT`] elapsing.
/// * `--release` — the binary embeds `ui/dist`, which `cargo build --release -p cide-app`
///   does *not* produce: `beforeBuildCommand` belongs to `cargo tauri build`. So the frontend
///   is built here, first, explicitly.
///
/// Both are checked before anything is compiled, so the refusal costs a second rather than a
/// full build.
fn bench_ipc(release: bool, build: bool) -> anyhow::Result<()> {
    let root = workspace_root()?;

    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        bail!(
            "no WAYLAND_DISPLAY and no DISPLAY — this gate measures a real webview and cannot \
             run headless. See BENCH.md."
        );
    }

    if !release && !port_listening(DEV_SERVER_PORT) {
        bail!(
            "nothing is listening on 127.0.0.1:{DEV_SERVER_PORT}, and a debug `cide` loads its \
             UI from there rather than from ui/dist. It would open a connection-error page, \
             never reach the benchmark, and time out after {}s.\n\n\
             Start one:   pnpm --dir ui dev\n\
             Or measure the build worth quoting, which embeds its own frontend:\n\
             \x20            cargo xtask bench-ipc --release",
            BENCH_TIMEOUT.as_secs()
        );
    }

    if build {
        // The frontend first, and only for `--release`. A release `cide` embeds `ui/dist` at
        // compile time and `cargo build` does not run `tauri.conf.json`'s
        // `beforeBuildCommand` — only `cargo tauri build` does — so skipping this measures
        // whatever `ui/dist` was last left holding. That is not a harmless staleness here:
        // what this gate measures is decided by frontend constants (`PULL_ITERATIONS`,
        // `SIZES`), so an old `ui/dist` produces a confident report of the wrong code that
        // is indistinguishable from a report of the right one.
        if release {
            let status = Command::new("pnpm")
                .current_dir(&root)
                .args(["--dir", "ui", "build"])
                .status()
                .context("running pnpm --dir ui build (needed: --release embeds ui/dist)")?;
            if !status.success() {
                bail!("pnpm --dir ui build failed");
            }
        }

        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.current_dir(&root).args(["build", "-p", "cide-app"]);
        if release {
            cargo.arg("--release");
        }
        let status = cargo.status().context("running cargo build -p cide-app")?;
        if !status.success() {
            bail!("cargo build -p cide-app failed");
        }
    }

    let profile = if release { "release" } else { "debug" };
    let binary = root.join("target").join(profile).join("cide");
    if !binary.exists() {
        bail!(
            "{} does not exist — drop --no-build, or build it first",
            binary.display()
        );
    }

    // stdout to a file rather than a pipe. A pipe would have to be drained while the child
    // runs or the child blocks once the buffer fills, and draining concurrently with a
    // deadline needs a reader thread; a file has neither problem and leaves the artefact
    // behind for anyone who wants to paste it into BENCH.md.
    let log = root.join("target").join("bench-ipc.out");
    let sink = fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;

    let mut child = Command::new(&binary)
        .current_dir(&root)
        .env("CIDE_BENCH", "1")
        .stdout(Stdio::from(sink))
        // Inherited, so the app's tracing log stays visible while the gate runs — on a
        // machine where the window never appears, that log is the only explanation there is.
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;

    let deadline = std::time::Instant::now() + BENCH_TIMEOUT;
    let status = loop {
        match child.try_wait().context("waiting for the bench run")? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "the bench run did not finish within {}s — killed. Partial output: {}",
                    BENCH_TIMEOUT.as_secs(),
                    log.display()
                );
            }
            None => std::thread::sleep(BENCH_POLL),
        }
    };

    let report = fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
    print!("{report}");

    if !status.success() {
        bail!("the app exited with {status} — see {}", log.display());
    }

    // No verdict means the window opened but the measurement never reported, which is a
    // failed gate and not a passed one. Silence is the answer this used to give by accident.
    let Some(verdict) = bench_verdict(&report) else {
        bail!(
            "the run produced no GO/NO-GO line — see {}. The webview may have failed to \
             reach `runBench`.",
            log.display()
        );
    };
    if !verdict.go {
        bail!("{}", verdict.line);
    }

    eprintln!("xtask: {}", verdict.line);
    eprintln!("xtask: full report written to {}", log.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verdict parser, which is the only part of `verify-cli` that has a rule in it.
    ///
    /// Everything else in that task is preflight and process handling; this is where a
    /// mistake would be silent, because the failure mode is answering `None` for a run that
    /// did produce a verdict — which `verify_cli` correctly reports as a failure, so the
    /// silent half is the *other* direction: reading a stale verdict from an earlier run in
    /// the same log and calling the check passed.
    #[test]
    fn the_last_verdict_line_wins_and_absence_is_not_a_pass() {
        assert_eq!(verify_verdict(""), None);
        assert_eq!(
            verify_verdict("running 1 test\nsome narration\ntest result: ok\n"),
            None,
            "a run that never reached a verdict has not passed one"
        );

        // A rerun appended to the same log: the second answer is the one about this run.
        let log = "VERIFY-CLI: handshake=ok version=2.1.227 range=2.1.224–2.1.227 verdict=verified\n\
                   ... a later run ...\n\
                   VERIFY-CLI: handshake=ok version=2.1.231 range=2.1.224–2.1.227 verdict=newer\n";
        assert_eq!(
            verify_verdict(log).as_deref(),
            Some("VERIFY-CLI: handshake=ok version=2.1.231 range=2.1.224–2.1.227 verdict=newer")
        );

        // `--nocapture` output arrives indented under the test harness's own framing.
        assert_eq!(
            verify_verdict("    VERIFY-CLI: skipped=no-claude-on-path\n").as_deref(),
            Some("VERIFY-CLI: skipped=no-claude-on-path")
        );

        // The broken-handshake cell still produces a line, because the test prints it before
        // it panics — a failing run is exactly the one with something to do about it.
        assert_eq!(
            verify_verdict("VERIFY-CLI: handshake=broken version=unknown\n").as_deref(),
            Some("VERIFY-CLI: handshake=broken version=unknown")
        );
    }

    /// The marker is one string in two crates, and a rename in either is silent: the test
    /// stops being parsed and `verify_cli` reports "no verdict" for a run that produced one.
    #[test]
    fn the_marker_is_the_one_the_test_prints() {
        let source = fs::read_to_string(
            workspace_root()
                .expect("a workspace root")
                .join("crates/cide-ide-mcp/tests/real_cli.rs"),
        )
        .expect("the real-CLI test is readable");
        assert!(
            source.contains(&format!(
                "const VERDICT_MARKER: &str = \"{VERIFY_MARKER}\";"
            )),
            "`{VERIFY_MARKER}` is not what tests/real_cli.rs prints, so verify-cli would \
             report every run as verdictless"
        );
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A report shaped exactly like `formatReport` in `ui/src/bench/ipcBench.ts` produces.
    fn report(verdict: &str) -> String {
        format!(
            "custom protocol : YES (fast path)\n\
             webkit          : 605.1.15\n\
             \n\
             | transport            | payload  | iters | MiB/s   | mean ms | p99 ms |\n\
             | pull (Response raw)  | 8 KiB    | 10000 |    78.1 |   0.100 |  1.000 |\n\
             \n\
             {verdict}\n"
        )
    }

    /// The dev-server probe has to answer "yes" to a real listener, or the release-less gate
    /// refuses a run that would have worked.
    #[test]
    fn a_listening_socket_is_seen() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("bound").port();
        assert!(port_listening(port));
    }

    /// And "no" to a closed one, which is the direction that saves the fifteen minutes.
    ///
    /// The port is obtained by binding and dropping rather than hardcoded: a fixed number
    /// would make this test depend on nothing else on the machine having chosen it, and the
    /// one it would fail on is a developer who happens to be running Vite.
    #[test]
    fn a_closed_port_is_not_a_dev_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("bound").port();
        drop(listener);
        assert!(!port_listening(port));
    }

    #[test]
    fn a_go_report_is_read_as_a_pass() {
        let text = report("GO — raw pull peaks at 195.3 MiB/s, above the 30 MiB/s floor.");
        let verdict = bench_verdict(&text).expect("the report has a verdict line");
        assert!(verdict.go);
        assert!(verdict.line.starts_with("GO —"));
    }

    /// The one that matters: `NO-GO` contains `GO`, so a naive `contains` check reports the
    /// failing run as a passing one — a gate that is green exactly when it should be red.
    #[test]
    fn a_no_go_report_is_not_mistaken_for_a_pass() {
        let text = report("NO-GO — raw pull peaks at 12.4 MiB/s, below the 30 MiB/s floor.");
        let verdict = bench_verdict(&text).expect("the report has a verdict line");
        assert!(!verdict.go);
        assert!(verdict.line.contains("12.4"));
    }

    /// A run whose webview never reached the measurement prints a table-less log. That is a
    /// failed gate, so the absence has to be distinguishable from a pass.
    #[test]
    fn a_report_with_no_verdict_line_yields_none() {
        assert_eq!(bench_verdict("custom protocol : YES\nwebkit : 605\n"), None);
        assert_eq!(bench_verdict(""), None);
    }

    /// `println!` in `diag_bench_report` wraps the report in blank lines, and a shell may
    /// add its own indentation when the log is pasted around. Trimming is what makes the
    /// parse survive that.
    #[test]
    fn a_verdict_line_is_found_through_surrounding_whitespace() {
        let verdict = bench_verdict("\n\n   GO — fine.\n\n").expect("found");
        assert!(verdict.go);
        assert_eq!(verdict.line, "GO — fine.");
    }

    #[test]
    fn sibling_imports_are_dropped_and_other_lines_survive() {
        let names = set(&["SessionId"]);
        let text = "// banner\nimport type { SessionId } from \"./SessionId\";\n\nexport type A = SessionId;\n";
        assert_eq!(
            strip_sibling_imports(text, &names),
            "// banner\n\nexport type A = SessionId;"
        );
    }

    #[test]
    fn an_import_of_a_type_we_did_not_generate_is_kept() {
        let text = "import type { Foo } from \"./Foo\";\nexport type A = Foo;\n";
        assert_eq!(
            strip_sibling_imports(text, &BTreeSet::new()),
            text.trim_end()
        );
    }

    #[test]
    fn a_package_import_is_not_mistaken_for_a_sibling() {
        assert_eq!(sibling_import_of("import type { X } from \"zod\";"), None);
        assert_eq!(
            sibling_import_of("import type { X } from \"./X\";"),
            Some("X")
        );
    }

    #[test]
    fn rendering_is_stable_across_runs() {
        let blocks = vec![
            Block {
                name: "A".into(),
                body: format!("{TS_RS_BANNER}\n\nexport type A = string;"),
            },
            Block {
                name: "B".into(),
                body: format!("{TS_RS_BANNER}\n\nexport type B = number;"),
            },
        ];
        let once = render(&blocks);
        assert_eq!(once, render(&blocks));
        assert!(once.ends_with("export type B = number;\n"));
        assert!(!once.ends_with("\n\n"));
    }

    #[test]
    fn drift_is_reported_per_type() {
        let old = render(&[Block {
            name: "A".into(),
            body: format!("{TS_RS_BANNER}\n\nexport type A = string;"),
        }]);
        let new = render(&[
            Block {
                name: "A".into(),
                body: format!("{TS_RS_BANNER}\n\nexport type A = number;"),
            },
            Block {
                name: "B".into(),
                body: format!("{TS_RS_BANNER}\n\nexport type B = number;"),
            },
        ]);
        let report = describe_drift(&old, &new);
        assert!(report.contains("new      B"), "{report}");
        assert!(report.contains("changed  A"), "{report}");
        assert!(!report.contains("removed"), "{report}");
    }

    #[test]
    fn identical_types_with_a_different_banner_still_report_something() {
        let block = Block {
            name: "A".into(),
            body: format!("{TS_RS_BANNER}\n\nexport type A = string;"),
        };
        let rendered = render(std::slice::from_ref(&block));
        let report = describe_drift(
            &rendered.replace("GENERATED FILE", "generated file"),
            &rendered,
        );
        assert!(report.contains("banner"), "{report}");
    }

    #[test]
    fn handler_entries_reduce_to_their_last_path_segment() {
        let src =
            "tauri::generate_handler![\n  cmd::app::app_ready,\n  cmd::session::session_attach,\n]";
        let path = scratch("handler.rs", src);
        assert_eq!(
            scan_commands(&path).unwrap(),
            set(&["app_ready", "session_attach"])
        );
    }

    #[test]
    fn a_commented_out_handler_is_not_part_of_the_surface() {
        let src = "generate_handler![\n  cmd::app::app_ready,\n  // cmd::app::app_quit,\n]";
        let path = scratch("commented.rs", src);
        assert_eq!(scan_commands(&path).unwrap(), set(&["app_ready"]));
    }

    #[test]
    fn a_comma_inside_a_comment_does_not_invent_a_command() {
        let src =
            "generate_handler![\n  cmd::app::app_ready,\n  // TODO: add app_quit, app_reload\n]";
        let path = scratch("comma-in-comment.rs", src);
        assert_eq!(scan_commands(&path).unwrap(), set(&["app_ready"]));
    }

    #[test]
    fn a_comment_naming_the_macro_does_not_win_the_search() {
        let src = "//! The surface is the `generate_handler![app_ready]` list below.\n\
                   tauri::generate_handler![\n  cmd::app::app_ready,\n  cmd::session::session_kill,\n]";
        let path = scratch("macro-in-doc.rs", src);
        assert_eq!(
            scan_commands(&path).unwrap(),
            set(&["app_ready", "session_kill"])
        );
    }

    #[test]
    fn a_block_commented_entry_is_rejected_rather_than_registered() {
        let src = "generate_handler![\n  cmd::app::app_ready,\n  /* cmd::app::app_quit */\n]";
        let path = scratch("block-comment.rs", src);
        assert!(scan_commands(&path).is_err());
    }

    #[test]
    fn a_handler_list_that_cannot_be_parsed_fails_loudly() {
        let src = "generate_handler![ cmd::app::app_ready(), ]";
        let path = scratch("weird.rs", src);
        assert!(scan_commands(&path).is_err());
    }

    #[test]
    fn a_missing_generate_handler_is_an_error_not_an_empty_surface() {
        let path = scratch("empty.rs", "fn main() {}");
        assert!(scan_commands(&path).is_err());
    }

    #[test]
    fn events_are_collected_from_string_literals() {
        let src = "emit(\"cide://workspace/changed\");\nemit(\"cide://session/state\");\nlet _ = \"not an event\";";
        let path = scratch("emit.rs", src);
        assert_eq!(
            scan_events(&path).unwrap(),
            set(&["cide://workspace/changed", "cide://session/state"])
        );
    }

    #[test]
    fn an_event_named_in_a_comment_is_not_part_of_the_surface() {
        let src = "/// Pairs with `\"cide://session/state\"`.\nemit(\"cide://session/exit\");";
        let path = scratch("documented-emit.rs", src);
        assert_eq!(scan_events(&path).unwrap(), set(&["cide://session/exit"]));
    }

    #[test]
    fn a_missing_emit_module_is_an_empty_event_surface() {
        let path = scratch_dir().join("no-such-emit.rs");
        assert!(!path.exists());
        assert_eq!(scan_events(&path).unwrap(), BTreeSet::new());
    }

    #[test]
    fn comparison_separates_additions_from_removals() {
        let report = compare("x.json", &set(&["a", "b"]), &set(&["b", "c"]));
        assert!(report.contains("+ c"), "{report}");
        assert!(report.contains("- a"), "{report}");
        assert!(compare("x.json", &set(&["a"]), &set(&["a"])).is_empty());
    }

    #[test]
    fn unknown_flags_are_rejected() {
        assert!(flags(&["--check".into()], &["--check"]).is_ok());
        assert!(flags(&["--wrtie".into()], &["--write"]).is_err());
    }

    #[test]
    fn nested_brackets_do_not_end_the_handler_list() {
        assert_eq!(bracketed(" [a, b[0], c]"), Some("a, b[0], c"));
        assert_eq!(bracketed("[unclosed"), None);
    }

    /// A scratch directory outside the repository, so no test can scribble on a real
    /// source file. Scoped to the process: a fixed name under a shared `/tmp` belongs to
    /// whoever ran the tests first, and every later user gets a permission denial instead
    /// of a test result.
    /// A directory of this process's own.
    ///
    /// Keyed by pid **and a counter**, because the pid alone is not unique enough under the
    /// runner this repository's CI actually uses. `cargo nextest` gives every test its own
    /// process, so a pid-keyed path is already private there — but plain `cargo test`, which the
    /// macOS CI job runs, puts every test in this module in ONE process on parallel threads,
    /// sharing a single directory. That was an intermittent failure that appeared roughly one run
    /// in six and named a different test each time, which is the shape that gets written off as
    /// "the flaky one" rather than diagnosed.
    ///
    /// `Relaxed` is enough: the only requirement is that two calls differ, and there is nothing
    /// else for this counter to be ordered against.
    fn scratch_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "cide-xtask-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn scratch(name: &str, contents: &str) -> PathBuf {
        let dir = scratch_dir();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}
