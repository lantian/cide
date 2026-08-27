//! The round-trip guarantee for `openspec/config.yaml`, over files shaped like real ones. (M28)
//!
//! `cide_spec::config` replaces exactly the lines that spell one key and copies every other byte.
//! This asserts it, and it asserts the half that matters more: **writing a value back unchanged
//! changes no byte at all**.
//!
//! Every failure this suite exists to catch is invisible until somebody reads a pull request. The
//! `openspec init` file is roughly eight hundred bytes of commented examples that are the only
//! documentation a user ever sees for `context`, `rules` and `operations`; a reserialising writer
//! deletes all of it and reports success. Beside that, the quieter ones: a lost byte-order mark,
//! LF where a Windows file had CRLF, a four-space file growing a two-space island, a blank line
//! accumulating on every save, a `context` block reflowed so its paragraphs run together.
//!
//! The corpus lives in `tests/config-corpus/` rather than in string literals so that `init.yaml`
//! can be the **real** `openspec init` output byte for byte, and so that `crlf-bom.yaml` can hold
//! bytes a Rust literal would make it too easy to write wrongly.

use std::path::{Path, PathBuf};

use cide_spec::config::{self, Config, Edit};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/config-corpus")
}

/// Every corpus file, by name, read losslessly.
fn corpus() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(corpus_dir())
        .expect("the corpus directory is checked in")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "yaml"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let bytes = std::fs::read(entry.path()).expect("readable");
            // `from_utf8` and not `from_utf8_lossy`: the BOM file is valid UTF-8 and the BOM is a
            // character the scanner has to survive, not one the test may quietly strip.
            (name, String::from_utf8(bytes).expect("the corpus is UTF-8"))
        })
        .collect();
    files.sort();
    assert!(files.len() >= 5, "the corpus lost files");
    files
}

fn read_corpus(name: &str) -> String {
    corpus()
        .into_iter()
        .find(|(had, _)| had == name)
        .unwrap_or_else(|| panic!("{name} is checked in"))
        .1
}

/// Every edit that writes a config back exactly as it reads.
fn identity_edits(config: &Config) -> Vec<Edit> {
    let mut edits = Vec::new();
    if let Some(schema) = &config.schema {
        edits.push(Edit::Schema(schema.clone()));
    }
    edits.push(Edit::Context(config.context.clone()));
    for entry in &config.rules {
        edits.push(Edit::Rules {
            artifact: entry.artifact.clone(),
            rules: entry.rules.clone(),
        });
    }
    for entry in &config.operations {
        edits.push(Edit::OperationGuidance {
            operation: entry.operation.clone(),
            guidance: entry.guidance.clone(),
        });
    }
    edits
}

/// A scratch project directory, removed on drop unless the test failed.
///
/// `real_cli.rs`'s `Scratch`, including the part that matters when something breaks: a panicking
/// test **leaves the directory behind** and prints where, because the interesting evidence for a
/// file-rewriting bug is the file.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cide-spec-config-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("openspec")).expect("a scratch project");
        Self(dir)
    }

    fn with(tag: &str, config: &str) -> Self {
        let scratch = Self::new(tag);
        std::fs::write(config::config_path(&scratch.0), config).expect("written");
        scratch
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn text(&self) -> String {
        std::fs::read_to_string(config::config_path(&self.0)).expect("readable")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "[cide-spec config tests] leaving {} for inspection",
                self.0.display()
            );
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// -------------------------------------------------------------------------------------------
// 1. The guarantee
// -------------------------------------------------------------------------------------------

#[test]
fn writing_every_value_of_every_corpus_file_back_unchanged_leaves_it_byte_identical() {
    // The whole reason this module is line-oriented instead of a YAML loader. A settings panel
    // Saves a form, which means it writes every key whether or not the user touched it; if that
    // is not a no-op, opening the panel and pressing Save rewrites a committed file.
    for (name, text) in corpus() {
        let config = config::read_text(&text);
        let edits = identity_edits(&config);
        let after = config::edit_text(&text, &edits).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(after, text, "{name} changed when nothing was changed");

        // And again, to catch anything that is only stable on the second pass.
        let twice = config::edit_text(&after, &edits).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(twice, text, "{name} is not idempotent");

        // The values themselves survive a round trip too — a writer that produced a stable file
        // by writing nothing at all would pass the assertion above and nothing else.
        assert_eq!(
            config::read_text(&after),
            config,
            "{name}: the values changed"
        );
    }
}

#[test]
fn the_real_init_file_reads_as_a_bare_config_and_states_only_a_schema() {
    // Every key but `schema` in the file `openspec init` writes is *commented out*. A reader that
    // saw the examples would report a project as configured when it is not, and — worse — an
    // editor that saw them would edit a comment.
    let text = read_corpus("init.yaml");
    let config = config::read_text(&text);
    assert_eq!(config.schema.as_deref(), Some("spec-driven"));
    assert!(
        config.is_bare(),
        "the commented examples were read as values: {config:?}"
    );
    assert!(
        text.contains("#   context: |"),
        "the fixture lost its examples"
    );
}

// -------------------------------------------------------------------------------------------
// 2. The comments
// -------------------------------------------------------------------------------------------

#[test]
fn every_comment_survives_an_edit_to_every_key() {
    // The failure this suite exists for. Eight hundred bytes of instructions, deleted silently,
    // by a Save the user thought set one field.
    for (name, text) in corpus() {
        let comments: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| line.trim_start().starts_with('#'))
            .collect();
        if comments.is_empty() {
            continue;
        }
        let edits = [
            Edit::Schema("some-other-schema".into()),
            Edit::Context(Some("A context nobody had before.".into())),
            Edit::Rules {
                artifact: "proposal".into(),
                rules: vec!["A rule nobody had before".into()],
            },
            Edit::Rules {
                artifact: "brand-new-artifact".into(),
                rules: vec!["And another".into()],
            },
            Edit::OperationGuidance {
                operation: "archive".into(),
                guidance: vec!["Say what was archived".into()],
            },
        ];
        for edit in &edits {
            let after = match config::edit_text(&text, std::slice::from_ref(edit)) {
                Ok(after) => after,
                // `malformed.yaml` states `rules` twice-shaped nonsense but no duplicate top-level
                // key, so nothing here should refuse. If one does, that is the news.
                Err(error) => panic!("{name}: {edit:?} was refused: {error}"),
            };
            for comment in &comments {
                assert!(
                    after
                        .lines()
                        .map(str::trim_end)
                        .any(|line| line == *comment),
                    "{name}: {edit:?} lost the comment {comment:?}"
                );
            }
        }

        // …and all five at once, which is what a panel's Save actually looks like.
        let after = config::edit_text(&text, &edits).unwrap_or_else(|e| panic!("{name}: {e}"));
        for comment in &comments {
            assert!(
                after
                    .lines()
                    .map(str::trim_end)
                    .any(|line| line == *comment),
                "{name}: a whole-form save lost the comment {comment:?}"
            );
        }
    }
}

// -------------------------------------------------------------------------------------------
// 3. Adding versus replacing
// -------------------------------------------------------------------------------------------

#[test]
fn a_key_that_is_absent_is_appended_and_one_that_is_present_is_replaced_where_it_stands() {
    let text = read_corpus("init.yaml");

    // Absent: appended at the end, one blank line clear of what was there, and every existing
    // byte of the file still in front of it.
    let added = config::edit_text(&text, &[Edit::Context(Some("Ships on Fridays.".into()))])
        .expect("edited");
    assert!(added.starts_with(&text), "the existing bytes moved");
    assert_eq!(&added[text.len()..], "\ncontext: Ships on Fridays.\n");

    // Present: replaced in place, and nothing around it moves.
    let replaced = config::edit_text(&added, &[Edit::Context(Some("Ships on Tuesdays.".into()))])
        .expect("edited");
    assert_eq!(
        replaced,
        added.replace("Ships on Fridays.", "Ships on Tuesdays.")
    );
    assert_eq!(replaced.lines().count(), added.lines().count());

    // The same for a key that already has a value: `schema` is line one and stays line one.
    let schema =
        config::edit_text(&text, &[Edit::Schema("custom-workflow".into())]).expect("edited");
    assert!(schema.starts_with("schema: custom-workflow\n"));
    assert_eq!(
        schema["schema: custom-workflow\n".len()..],
        text["schema: spec-driven\n".len()..],
        "the rest of the file changed"
    );
}

#[test]
fn a_new_entry_joins_the_mapping_it_belongs_to_rather_than_starting_a_second_one() {
    // Two `rules:` keys is a file a YAML loader reads as one of them, and which one is not
    // something a user can see. The editor has to grow the mapping that is there.
    let text = read_corpus("full.yaml");
    let after = config::edit_text(
        &text,
        &[Edit::Rules {
            artifact: "design".into(),
            rules: vec!["Draw the state machine".into()],
        }],
    )
    .expect("edited");
    assert_eq!(after.matches("\nrules:").count(), 1, "{after}");
    let config = config::read_text(&after);
    assert_eq!(
        config
            .rules
            .iter()
            .map(|r| r.artifact.as_str())
            .collect::<Vec<_>>(),
        ["proposal", "tasks", "design"],
        "document order was not preserved"
    );
    // It landed inside `rules:` and before the comment that documents `operations:`.
    let design = after.find("  design:").expect("added");
    assert!(design < after.find("# Per-operation").expect("the comment survived"));
    assert!(
        design
            > after
                .find("Break tasks into chunks")
                .expect("the last entry")
    );
}

#[test]
fn a_files_own_indentation_and_list_style_are_reused_rather_than_normalised() {
    // `hand-written.yaml` indents four spaces and writes its list items flush with their key,
    // which is legal YAML and what somebody who has written YAML before tends to type. A panel
    // that re-indented it would produce a diff on lines the user never edited.
    let text = read_corpus("hand-written.yaml");
    let after = config::edit_text(
        &text,
        &[
            Edit::Rules {
                artifact: "proposal".into(),
                rules: vec!["Mention the on-call rota".into(), "And the rollback".into()],
            },
            Edit::Rules {
                artifact: "tasks".into(),
                rules: vec!["Two hours each".into()],
            },
        ],
    )
    .expect("edited");
    assert!(
        after.contains("\n    proposal:\n    - Mention the on-call rota\n"),
        "{after}"
    );
    assert!(
        after.contains("\n    tasks:\n    - Two hours each\n"),
        "{after}"
    );
    assert!(
        !after.contains("      - "),
        "a two-space island appeared:\n{after}"
    );
    assert_eq!(
        config::read_text(&after).rules_for("tasks"),
        ["Two hours each".to_string()]
    );
}

// -------------------------------------------------------------------------------------------
// 4. Block scalars
// -------------------------------------------------------------------------------------------

#[test]
fn a_block_scalar_context_round_trips_with_its_indentation_and_its_blank_lines() {
    let text = read_corpus("full.yaml");
    let config = config::read_text(&text);
    let context = config.context.clone().expect("stated");

    // The paragraph break is content, and so is the indentation *inside* the block.
    assert!(
        context.contains("\n\nWe use conventional commits."),
        "{context:?}"
    );
    assert!(
        context.contains("\n  - and a nested note"),
        "the deeper indentation inside the block was flattened: {context:?}"
    );
    assert!(
        !context.starts_with(' '),
        "the block's own indentation leaked into the value: {context:?}"
    );

    // Written back unchanged it is the same file…
    assert_eq!(
        config::edit_text(&text, &[Edit::Context(Some(context.clone()))]).expect("edited"),
        text
    );

    // …and written back *changed* it is still a block scalar with the same shape, whose value
    // reads back as exactly what was set.
    let longer = format!("{context}\nWe deploy behind a flag.");
    let after = config::edit_text(&text, &[Edit::Context(Some(longer.clone()))]).expect("edited");
    assert!(
        after.contains("context: |\n"),
        "the block header changed:\n{after}"
    );
    assert_eq!(
        config::read_text(&after).context.as_deref(),
        Some(longer.as_str())
    );
    assert!(
        !after.lines().any(|line| line != line.trim_end()),
        "a blank line inside the block acquired trailing whitespace:\n{after:?}"
    );
    // Everything after the block is untouched.
    assert!(after.contains("# Per-artifact rules (optional)\nrules:\n  proposal:"));
}

#[test]
fn a_context_written_as_a_block_stays_one_even_when_it_shrinks_to_a_single_line() {
    // Somebody who wrote `context: |` meant prose. Folding it back to `context: Some words` the
    // first time it is shortened is a reformat of a file cide was asked to edit.
    let text = "context: |\n  One.\n  Two.\n";
    let after = config::edit_text(text, &[Edit::Context(Some("Just one.".into()))]).expect("ok");
    assert_eq!(after, "context: |\n  Just one.\n");
    assert_eq!(
        config::read_text(&after).context.as_deref(),
        Some("Just one.")
    );
}

// -------------------------------------------------------------------------------------------
// 5. CRLF and a BOM
// -------------------------------------------------------------------------------------------

#[test]
fn crlf_and_a_byte_order_mark_survive_every_edit() {
    // A cide that silently converted a CRLF file to LF would produce a diff touching every line
    // of a file somebody else maintains — and losing the BOM would make the first key invisible
    // to the scanner, which reports as "this file states no schema" about a file that visibly
    // does.
    let text = read_corpus("crlf-bom.yaml");
    assert!(
        text.starts_with('\u{feff}'),
        "the fixture lost its BOM on disk"
    );
    assert!(text.contains("\r\n"), "the fixture lost its CRLF on disk");

    let config = config::read_text(&text);
    assert_eq!(config.schema.as_deref(), Some("spec-driven"));
    assert_eq!(
        config.context.as_deref(),
        Some("Written on Windows.\n\nSecond paragraph."),
        "the CR leaked into the value"
    );
    assert_eq!(config.rules_for("tasks"), ["Keep them short".to_string()]);

    for edit in [
        Edit::Schema("other".into()),
        Edit::Context(Some("Rewritten.".into())),
        Edit::Context(None),
        Edit::Rules {
            artifact: "tasks".into(),
            rules: vec!["Different".into()],
        },
        Edit::OperationGuidance {
            operation: "apply".into(),
            guidance: vec!["Be brief".into()],
        },
    ] {
        let after = config::edit_text(&text, std::slice::from_ref(&edit)).expect("edited");
        assert!(after.starts_with('\u{feff}'), "{edit:?} dropped the BOM");
        assert!(
            !after.replace("\r\n", "").contains('\n'),
            "{edit:?} left a bare LF: {after:?}"
        );
        assert!(
            after.contains("# Project context (optional)\r\n"),
            "{edit:?} lost a comment"
        );
    }
}

// -------------------------------------------------------------------------------------------
// 6. Tolerance
// -------------------------------------------------------------------------------------------

#[test]
fn a_file_this_reader_does_not_understand_is_read_empty_and_is_never_rewritten_into_something_else()
{
    // Reading nothing is the safe direction: a key that was not understood is a key that is never
    // rewritten, so a misread cannot become a lost value. The alternative — a reader that took a
    // confident guess at a flow mapping — is a settings panel that shows two rules and then saves
    // over the anchors and directives around them.
    let text = read_corpus("malformed.yaml");
    let config = config::read_text(&text);
    assert!(config.rules_for("proposal").is_empty(), "{config:?}");
    assert!(config.guidance_for("apply").is_empty(), "{config:?}");
    assert_eq!(config.context, None);
    assert_eq!(config.schema, None);
    assert_eq!(config.schema_or_default(), "spec-driven");

    // Every clearing edit is a no-op, because nothing it names was understood as present.
    let untouched = config::edit_text(
        &text,
        &[
            Edit::Context(None),
            Edit::Rules {
                artifact: "proposal".into(),
                rules: Vec::new(),
            },
        ],
    )
    .expect("edited");
    assert_eq!(untouched, text);

    // …and an edit that does write appends rather than reformatting anything above it.
    let added = config::edit_text(&text, &[Edit::Schema("spec-driven".into())]).expect("edited");
    assert!(added.starts_with(&text), "the file above the edit changed");
}

#[test]
fn a_duplicate_top_level_key_is_refused_rather_than_edited_arbitrarily() {
    // A YAML reader takes the last, so editing the first would change nothing the tool can see and
    // editing the last would leave an earlier key contradicting it. Neither is something to do
    // silently to a committed file.
    let text = "schema: one\ncontext: a\nschema: two\n";
    assert_eq!(config::read_text(text).schema.as_deref(), Some("two"));
    let error = config::edit_text(text, &[Edit::Schema("three".into())]).expect_err("refused");
    assert!(error.to_string().contains("stated 2 times"), "{error}");
    // An edit to a *different* key is unaffected — the refusal is about the key being edited.
    assert!(config::edit_text(text, &[Edit::Context(Some("b".into()))]).is_ok());
}

#[test]
fn a_missing_config_file_reads_as_nothing_and_a_file_with_no_trailing_newline_keeps_none() {
    let scratch = Scratch::new("missing");
    assert_eq!(
        config::read(scratch.root()).expect("read"),
        Config::default()
    );

    // No trailing newline is a real file and a one-character diff if it is invented.
    let text = "schema: spec-driven";
    assert_eq!(
        config::edit_text(text, &[Edit::Schema("other".into())]).expect("edited"),
        "schema: other"
    );
}

// -------------------------------------------------------------------------------------------
// 7. Clearing
// -------------------------------------------------------------------------------------------

#[test]
fn clearing_a_value_removes_its_lines_and_leaves_the_comments_around_it() {
    let text = read_corpus("full.yaml");
    let after = config::edit_text(&text, &[Edit::Context(None)]).expect("edited");

    assert_eq!(config::read_text(&after).context, None);
    assert!(
        !after.contains("Tech stack: TypeScript"),
        "the block survived:\n{after}"
    );
    assert!(!after.contains("\ncontext:"), "the key survived:\n{after}");
    // The comments that documented it are still there — they belong to the file, not to the key.
    assert!(after.contains("# Project context (optional)"));
    assert!(after.contains("# This is shown to AI when creating artifacts."));
    // …and so is everything else.
    assert_eq!(
        config::read_text(&after).rules,
        config::read_text(&text).rules
    );
    assert_eq!(
        config::read_text(&after).operations,
        config::read_text(&text).operations
    );
}

#[test]
fn setting_a_key_and_clearing_it_again_returns_the_file_to_what_it_was() {
    // Otherwise a panel that sets a context and then clears it leaves the file one blank line
    // longer every time round — a diff on a committed file produced by doing and undoing nothing.
    for (name, text) in corpus() {
        if config::read_text(&text).context.is_some() {
            continue;
        }
        let with = config::edit_text(&text, &[Edit::Context(Some("Temporary.".into()))])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let without = config::edit_text(&with, &[Edit::Context(None)])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(without, text, "{name} did not come back to itself");
    }
}

#[test]
fn clearing_the_last_entry_of_a_mapping_takes_the_mapping_with_it() {
    // `rules:` alone is YAML `null`, not an empty mapping. Leaving one behind would be cide
    // inventing a key the user never wrote — and one that reads back as a value.
    let text = read_corpus("full.yaml");
    let after = config::edit_text(
        &text,
        &[
            Edit::Rules {
                artifact: "proposal".into(),
                rules: Vec::new(),
            },
            Edit::Rules {
                artifact: "tasks".into(),
                rules: Vec::new(),
            },
        ],
    )
    .expect("edited");
    assert!(
        !after.contains("\nrules:"),
        "the empty key stayed:\n{after}"
    );
    assert!(after.contains("# Per-artifact rules (optional)"), "{after}");
    // Emptying one of two leaves the other and the key.
    let one = config::edit_text(
        &text,
        &[Edit::Rules {
            artifact: "proposal".into(),
            rules: Vec::new(),
        }],
    )
    .expect("edited");
    assert!(one.contains("\nrules:\n  tasks:\n"), "{one}");
    assert!(!one.contains("proposal"), "{one}");
}

// -------------------------------------------------------------------------------------------
// The file on disk
// -------------------------------------------------------------------------------------------

#[test]
fn a_save_that_changes_nothing_does_not_touch_the_file_at_all() {
    // Not a micro-optimisation: `openspec/` is committed, and a panel whose Save always writes
    // shows up as a modified file in every `git status` the user runs after opening it.
    let text = read_corpus("init.yaml");
    let scratch = Scratch::with("noop", &text);
    let config = config::read(scratch.root()).expect("read");
    assert_eq!(config.schema.as_deref(), Some("spec-driven"));

    let wrote = config::apply(scratch.root(), &identity_edits(&config)).expect("applied");
    assert!(!wrote, "an unchanged save wrote the file");
    assert_eq!(scratch.text(), text);

    // And a real change does write, and writes only what changed.
    let wrote = config::set_schema(scratch.root(), "custom").expect("applied");
    assert!(wrote);
    assert_eq!(scratch.text(), text.replacen("spec-driven", "custom", 1));
    assert_eq!(
        config::read(scratch.root())
            .expect("read")
            .schema
            .as_deref(),
        Some("custom")
    );
}

#[test]
fn the_file_is_created_when_it_is_absent_and_holds_only_what_was_asked_for() {
    // A scaffold with commented examples would be cide impersonating `openspec init`, and the
    // examples would go stale against a CLI this crate deliberately does not track.
    let scratch = Scratch::new("create");
    let wrote = config::set_rules(
        scratch.root(),
        "proposal",
        &["Keep proposals under 500 words".to_string()],
    )
    .expect("applied");
    assert!(wrote);
    assert_eq!(
        scratch.text(),
        "rules:\n  proposal:\n    - Keep proposals under 500 words\n"
    );

    // The mode is the one a committed file wants; `openspec/` is read in pull requests.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(config::config_path(scratch.root()))
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "the file is not world-readable");
    }
}

#[test]
fn the_four_setters_read_back_as_the_values_they_were_given() {
    // The plain end-to-end pass, over a file that starts as the real `openspec init` output.
    let scratch = Scratch::with("setters", &read_corpus("init.yaml"));
    let root = scratch.root();

    config::set_context(
        root,
        Some("Tech stack: Rust, React.\n\nWe ship on Fridays."),
    )
    .expect("context");
    config::set_rules(
        root,
        "proposal",
        &[
            "Under 500 words".to_string(),
            "No goals: state them".to_string(),
        ],
    )
    .expect("rules");
    config::set_operation_guidance(root, "archive", &["Summarise the outcome".to_string()])
        .expect("guidance");

    let config = config::read(root).expect("read");
    assert_eq!(
        config.context.as_deref(),
        Some("Tech stack: Rust, React.\n\nWe ship on Fridays.")
    );
    assert_eq!(
        config.rules_for("proposal"),
        [
            "Under 500 words".to_string(),
            "No goals: state them".to_string()
        ],
        "a value holding a colon came back different"
    );
    assert_eq!(
        config.guidance_for("archive"),
        ["Summarise the outcome".to_string()]
    );
    assert!(!config.is_bare());

    // Every one of the file's original comments is still there.
    let text = scratch.text();
    for comment in read_corpus("init.yaml")
        .lines()
        .filter(|line| line.trim_start().starts_with('#'))
    {
        assert!(text.contains(comment), "the setters lost {comment:?}");
    }
}

#[test]
fn a_value_that_would_not_mean_itself_survives_being_written_down() {
    // The class of bug that only shows up in somebody else's rule text: a leading dash, a colon,
    // a word a YAML loader resolves to a boolean, a trailing space. Each one is a value that
    // reads back as something other than what was typed, and none of them fails loudly.
    let awkward = [
        "- starts with a dash",
        "has: a colon in it",
        "no",
        "true",
        "1.5",
        "#not a comment",
        "trailing space ",
        "quotes \"inside\" it",
        "@reserved",
        "",
    ];
    let scratch = Scratch::new("awkward");
    let items: Vec<String> = awkward.iter().map(|s| s.to_string()).collect();
    config::set_rules(scratch.root(), "proposal", &items).expect("rules");
    assert_eq!(
        config::read(scratch.root())
            .expect("read")
            .rules_for("proposal"),
        items
    );

    // …and an ordinary rule is still written as ordinary readable YAML.
    config::set_rules(
        scratch.root(),
        "tasks",
        &["Break tasks into chunks of max 2 hours".to_string()],
    )
    .expect("rules");
    assert!(
        scratch
            .text()
            .contains("    - Break tasks into chunks of max 2 hours\n"),
        "{}",
        scratch.text()
    );
}
