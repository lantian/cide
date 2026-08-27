//! What `openspec init --tools claude` leaves in the project, and how to invoke it. (M28)
//!
//! # Why this is read from the directory and never from a table in cide
//!
//! Because the table was wrong twice, and the second time it was wrong for everybody.
//!
//! The first version of the panel typed `/opsx:onboard`, a command that lives in the CLI's
//! *templates* and that its default profile does not install: Claude answered `Unknown command`
//! and nothing in cide could explain why. That is why the check exists at all — which commands a
//! project has is decided by a profile cide does not choose.
//!
//! The second time was the *shape* of the invocation rather than one name in it. OpenSpec shipped
//! its workflow as Claude Code **slash commands** under `.claude/commands/opsx/`, so every command
//! was `/opsx:<name>`; somewhere before 1.7 it moved the same workflow to **skills** —
//! `.claude/skills/openspec-<name>/SKILL.md`, invoked as `/openspec-<name>`. A correctly set up
//! project on a current CLI therefore had none of the names cide looked for, and every button in
//! the panel refused with a sentence recommending `openspec update` — which on such a project
//! answers *"all tools up to date"* and writes nothing. A dead end, in cide's own words, produced
//! by cide.
//!
//! So a *surface* is a thing this module enumerates, exactly like a name, and [`SURFACES`] is
//! ordered newest-first. Supporting the old one is not nostalgia: those files are still on disk
//! in a project set up by an older CLI, and Claude Code still runs them.
//!
//! # Why not just read the skill's own front matter
//!
//! A `SKILL.md` carries `name: openspec-propose`, which is the invocation, so reading it would
//! remove the prefix rule from this file. It would also make listing a project's commands a
//! parse of six YAML documents on every board read, put cide back in the business of tracking a
//! format it does not own, and — the part that decides it — give no answer at all for the older
//! surface, whose files carry no name. The directory layout *is* the contract here, and it is
//! one that fails loudly: a third surface is a new row in [`SURFACES`] and nothing else.

use std::path::Path;

use cide_ipc::SpecCommand;

/// One way OpenSpec has installed its workflow into a Claude Code project.
///
/// `dir` is relative to the project root, `entry` is the file that must exist for the command to
/// be real, and `line` is what gets typed. All three are `{name}`-templated on cide's own handle
/// for the command.
struct Surface {
    dir: &'static str,
    entry: &'static str,
    line: &'static str,
    /// cide's handle → what *this* surface calls the same command, for the ones that differ.
    ///
    /// # The prefix was not the only thing that changed
    ///
    /// Moving from `.claude/commands/opsx/` to `.claude/skills/` renamed four of the six
    /// commands as well: `apply` became `apply-change`, `archive` became `archive-change`,
    /// `sync` became `sync-specs`, `update` became `update-change`. A surface table that
    /// templated one handle into both rows therefore answered `None` on the older surface for
    /// exactly the commands cide dispatches with — silently, since a missing command is a
    /// legitimate answer here and the refusal reads as *this project does not have that*.
    ///
    /// cide's handle is the **current** name, so this list is empty on the current surface and
    /// grows only when upstream renames something again.
    renames: &'static [(&'static str, &'static str)],
}

impl Surface {
    /// What this surface calls `name`.
    fn spelling(&self, name: &str) -> String {
        self.renames
            .iter()
            .find_map(|(handle, local)| (*handle == name).then_some((*local).to_string()))
            .unwrap_or_else(|| name.to_string())
    }

    /// The inverse: cide's handle for something this surface spells `local`.
    fn handle(&self, local: &str) -> String {
        self.renames
            .iter()
            .find_map(|(handle, spelt)| (*spelt == local).then_some((*handle).to_string()))
            .unwrap_or_else(|| local.to_string())
    }
}

/// Newest first, and the order is load-bearing: a project migrated by `openspec update` can hold
/// **both**, and the one to type is the one the current CLI wrote.
const SURFACES: &[Surface] = &[
    // Claude Code skills. What `openspec init --tools claude` writes today (verified against
    // 1.7.0, 1.9.0 and 1.10.0), and the CLI prints `/openspec-propose` itself as the first thing
    // to run after a successful init.
    Surface {
        dir: ".claude/skills",
        entry: "openspec-{name}/SKILL.md",
        line: "/openspec-{name}",
        renames: &[],
    },
    // Slash commands. What M28 was written against, and what a project set up by an older CLI
    // still has on disk — those files did not stop working, they stopped being written.
    Surface {
        dir: ".claude/commands/opsx",
        entry: "{name}.md",
        line: "/opsx:{name}",
        renames: &[
            ("apply-change", "apply"),
            ("archive-change", "archive"),
            ("sync-specs", "sync"),
            ("update-change", "update"),
        ],
    },
];

/// Is this a name cide may look up and interpolate?
///
/// The value reaches [`line`] from the frontend and is about to be joined onto a path *and*
/// interpolated into a line typed at a shell-adjacent TUI. Kebab only, and checked here rather
/// than at the call site so both uses are covered by one rule.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What this project types to run `name`, or `None` if it has no such command.
///
/// The authority. `spec_run_command` resolves through this immediately before typing, so a
/// project that was set up — or migrated — while cide was running gets the right line without a
/// board refresh, and a stale list in a panel can never put a name on a terminal.
pub fn line(root: &Path, name: &str) -> Option<String> {
    if !valid_name(name) {
        return None;
    }
    SURFACES.iter().find_map(|surface| {
        let local = surface.spelling(name);
        entry_path(root, surface, &local)
            .is_file()
            .then(|| surface.line.replace("{name}", &local))
    })
}

/// When this project's `name` command was last written, or `None` if it has none.
///
/// # What it is for, and why a *file time* is the honest signal
///
/// Claude Code reads a project's skills and slash commands **once, at startup**. So a
/// conversation that was already running when `openspec init` wrote them does not have them, and
/// typing `/openspec-propose` into it answers `Unknown command` — which is exactly what a user
/// sees after enabling OpenSpec from the panel of a project whose console pane came up with the
/// window. Nothing in the transcript says why, and nothing in cide did either.
///
/// cide knows when it started each child, so the comparison it can actually make is *is this
/// command's file newer than that conversation*. `spec_run_command` makes it, and refuses with
/// the restart rather than sending a line that cannot work. Deliberately the file's own mtime
/// and not "did cide run init this session": a project set up by hand in a terminal, or by an
/// agent, is the same failure and this sees it.
pub fn installed_at(root: &Path, name: &str) -> Option<std::time::SystemTime> {
    if !valid_name(name) {
        return None;
    }
    SURFACES.iter().find_map(|surface| {
        entry_path(root, surface, &surface.spelling(name))
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
    })
}

/// The file that must exist for `local` — already this surface's own spelling — to be real.
fn entry_path(root: &Path, surface: &Surface, local: &str) -> std::path::PathBuf {
    root.join(surface.dir)
        .join(surface.entry.replace("{name}", local))
}

/// Every OpenSpec command this project has, sorted by name, newest surface winning a tie.
///
/// Two callers, and they want it for opposite reasons: the board carries it so the panel can
/// preview exactly what it is about to send, and the refusal path prints it because *"that
/// command is not here"* is only useful next to the ones that are.
pub fn installed(root: &Path) -> Vec<SpecCommand> {
    let mut found: Vec<SpecCommand> = Vec::new();
    for surface in SURFACES {
        let Ok(entries) = std::fs::read_dir(root.join(surface.dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(raw) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(name) = name_of(surface, &raw) else {
                continue;
            };
            // `name` is cide's handle; the file on disk and the line to type are this surface's
            // own spelling of it, which is not the same string on the older one.
            let local = surface.spelling(&name);
            // Stat the entry rather than trusting the directory listing: a skill directory with
            // no `SKILL.md` is a name Claude Code does not offer, and offering it here would be
            // the `/opsx:onboard` failure again with a different cause.
            if !entry_path(root, surface, &local).is_file() {
                continue;
            }
            // Both surfaces present is the migrated project, and `SURFACES` is newest-first, so
            // the first spelling seen is the one to keep.
            if found.iter().any(|command| command.name == name) {
                continue;
            }
            found.push(SpecCommand {
                line: surface.line.replace("{name}", &local),
                name,
            });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// cide's handle for a directory entry under one surface, if it is one of OpenSpec's at all.
///
/// The skills directory is **shared** — a project's own `release` or `update-portal-docs` skill
/// sits beside OpenSpec's — so the prefix is what separates them, and stripping it is what keeps
/// `name` the same handle on both surfaces.
fn name_of(surface: &Surface, raw: &str) -> Option<String> {
    // A template may name a file *inside* a directory (`openspec-{name}/SKILL.md`), and what
    // `read_dir` hands back is only that first segment — so the match is against the segment and
    // never the whole template. The commands arm's segment is the whole of it.
    let segment = surface.entry.split('/').next()?;
    let (prefix, suffix) = segment.split_once("{name}")?;
    let local = raw.strip_prefix(prefix)?.strip_suffix(suffix)?;
    // Back to cide's handle before anything else sees it: `installed` is what the panel lists and
    // what a refusal enumerates, and a list mixing `apply` with `archive-change` would be two
    // vocabularies in one sentence.
    valid_name(local).then(|| surface.handle(local))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-spec-claude-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directories");
        std::fs::write(path, "---\nname: x\n---\n").expect("the file");
    }

    #[test]
    fn a_current_project_is_invoked_as_a_skill() {
        // The shape `openspec init --tools claude` writes today, and the failure this module
        // exists for: cide looked for `.claude/commands/opsx/propose.md`, found nothing, and told
        // a correctly set up project to run `openspec update`.
        let root = scratch("skills");
        for name in ["propose", "explore", "apply-change", "archive-change"] {
            touch(&root, &format!(".claude/skills/openspec-{name}/SKILL.md"));
        }
        // …beside skills of the project's own, which must not be mistaken for OpenSpec's.
        touch(&root, ".claude/skills/release/SKILL.md");
        touch(&root, ".claude/skills/update-portal-docs/SKILL.md");

        assert_eq!(line(&root, "propose").as_deref(), Some("/openspec-propose"));
        assert_eq!(line(&root, "explore").as_deref(), Some("/openspec-explore"));
        assert_eq!(
            line(&root, "onboard"),
            None,
            "a name upstream does not ship"
        );

        let names: Vec<String> = installed(&root).into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            ["apply-change", "archive-change", "explore", "propose"],
            "sorted, and the project's own skills are not OpenSpec's"
        );
    }

    #[test]
    fn the_older_surface_spells_four_of_the_six_commands_differently() {
        // The prefix was not the only thing that changed. `apply` became `apply-change`,
        // `archive` became `archive-change`, `sync` became `sync-specs`, `update` became
        // `update-change` — so a table that templated cide's handle into both rows answered
        // `None` on the older surface for exactly the commands a dispatch types, and a `None`
        // here reads as *this project does not have that command*: a silent refusal, on a
        // correctly set up project, with a sentence recommending something that would not help.
        let root = scratch("legacy-names");
        for name in ["propose", "explore", "apply", "archive", "sync", "update"] {
            touch(&root, &format!(".claude/commands/opsx/{name}.md"));
        }
        assert_eq!(
            line(&root, "apply-change").as_deref(),
            Some("/opsx:apply"),
            "cide's handle is the current name; the line is what this project answers to"
        );
        assert_eq!(
            line(&root, "archive-change").as_deref(),
            Some("/opsx:archive")
        );
        assert_eq!(line(&root, "sync-specs").as_deref(), Some("/opsx:sync"));
        assert_eq!(
            line(&root, "update-change").as_deref(),
            Some("/opsx:update")
        );

        // And the listing speaks one vocabulary: cide's handles, whatever the files are called.
        let names: Vec<String> = installed(&root).into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "apply-change",
                "archive-change",
                "explore",
                "propose",
                "sync-specs",
                "update-change"
            ],
        );
        assert!(
            installed(&root)
                .iter()
                .all(|command| command.line.starts_with("/opsx:")),
            "…while every line is the one this project actually answers to"
        );
    }

    #[test]
    fn a_command_written_after_a_conversation_started_is_visible_as_such() {
        // The whole of `installed_at`'s job: Claude Code reads its skills once, at startup, so a
        // conversation older than the file cannot know the command. Compared as times rather
        // than "did cide run init" because a project set up by hand in a terminal is the same
        // failure — see the function's own doc.
        let root = scratch("mtime");
        assert_eq!(
            installed_at(&root, "propose"),
            None,
            "nothing installed yet"
        );
        touch(&root, ".claude/skills/openspec-propose/SKILL.md");
        let at = installed_at(&root, "propose").expect("the file is there now");
        assert!(
            at.elapsed()
                .map(|since| since.as_secs() < 60)
                .unwrap_or(true),
            "and it is the file's own mtime, not a constant"
        );
        assert_eq!(
            installed_at(&root, "explore"),
            None,
            "a command this project does not have has no time either"
        );
    }

    #[test]
    fn a_project_set_up_by_an_older_cli_still_works() {
        // Those files are still on disk and Claude Code still runs them. Dropping the arm would
        // break every project that has not re-run `init`.
        let root = scratch("commands");
        for name in ["propose", "explore"] {
            touch(&root, &format!(".claude/commands/opsx/{name}.md"));
        }
        assert_eq!(line(&root, "propose").as_deref(), Some("/opsx:propose"));
        assert_eq!(
            installed(&root)
                .iter()
                .map(|c| c.line.clone())
                .collect::<Vec<_>>(),
            ["/opsx:explore", "/opsx:propose"]
        );
    }

    #[test]
    fn a_migrated_project_types_the_current_spelling() {
        // `openspec update` writes the new surface and does not remove the old, so both are on
        // disk. Typing the stale one would be the original bug in the other direction.
        let root = scratch("both");
        touch(&root, ".claude/commands/opsx/propose.md");
        touch(&root, ".claude/skills/openspec-propose/SKILL.md");
        assert_eq!(line(&root, "propose").as_deref(), Some("/openspec-propose"));
        let installed = installed(&root);
        assert_eq!(installed.len(), 1, "one command, not one per surface");
        assert_eq!(installed[0].line, "/openspec-propose");
    }

    #[test]
    fn a_skill_directory_without_its_file_is_not_a_command() {
        // Claude Code does not offer it, so neither may cide: a name in the panel that answers
        // `Unknown command` is the failure this whole module was written after.
        let root = scratch("empty-skill");
        std::fs::create_dir_all(root.join(".claude/skills/openspec-propose")).expect("the dir");
        assert_eq!(line(&root, "propose"), None);
        assert!(installed(&root).is_empty());
    }

    #[test]
    fn a_project_with_nothing_installed_is_empty_and_not_an_error() {
        let root = scratch("bare");
        assert_eq!(line(&root, "propose"), None);
        assert!(installed(&root).is_empty());
    }

    #[test]
    fn a_name_that_is_not_kebab_is_refused_before_it_touches_a_path() {
        // It is joined onto a path *and* typed at a TUI. Neither use survives a surprise.
        let root = scratch("names");
        touch(&root, ".claude/skills/openspec-propose/SKILL.md");
        for rogue in ["../../etc/passwd", "propose x", "Propose", "", "propose\n"] {
            assert!(!valid_name(rogue), "{rogue:?}");
            assert_eq!(line(&root, rogue), None, "{rogue:?}");
        }
        assert!(valid_name("apply-change"));
    }
}
