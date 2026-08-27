# ADR 0012 — cide reads OpenSpec by running its CLI, and links it to a task with one field

**Status:** accepted (M28)
**Date:** 2026-08-26

## Context

M18 built the task tracker and M27 closed the orchestration loop around it: assigning a task
starts the role, an `@mention` dispatches, a started run moves its task `Todo → Doing`. What none
of that machinery has is any idea *what the work is*. `cide-ipc/src/tasks.rs`'s header rejects
subtasks, dependencies, priorities and ordering by argument, and a task body is prose nobody
validates. So cide now runs unattended agents, competently, against requirements that exist only
in a chat transcript — which is precisely the disease
[OpenSpec](https://github.com/Fission-AI/OpenSpec) was written for: *"AI coding assistants are
powerful but unpredictable when requirements live only in chat history."*

OpenSpec is a small, tool-agnostic convention (MIT, npm `@fission-ai/openspec`): canonical
requirements in `openspec/specs/<capability>/spec.md`, and each piece of proposed work as
`openspec/changes/<name>/` — a proposal, a design, a `tasks.md` checklist, and *delta* files
stating `## ADDED | MODIFIED | REMOVED | RENAMED Requirements`. Archiving a change merges its
deltas into `specs/`. Thirty other tools read the same folder.

## Decision 1 — cide never parses OpenSpec's markdown

A new crate, `cide-spec`, shells out to the CLI's `--json` surface and maps the result into
`cide_ipc::spec`. Nothing in cide reads a `.md` file to learn what a change *is*.

A Rust parser was the first design and reading the shipped package killed it. The grammar carries
code-fence masking, case-folded requirement-name matching for typo detection, `RENAMED`
`FROM:`/`TO:` sections, dropped-scenario detection on `MODIFIED` — and, decisively, a
**schema-driven artifact set**: `openspec/config.yaml` selects a workflow schema whose artifacts
are declared with `generates` globs, so even `tasks.md` is not a filename cide may assume, and
the checklist may be several files matched by a glob. A second parser would agree with the first
until the next `npm i -g`, and every way it could then disagree is silent: a checkbox not counted,
a requirement not found, a board that looks right and is not.

The cost is a hard dependency on a binary cide does not ship, and it is paid deliberately — see
Decision 4.

### The exception, and why it is not a parser

`cide_spec::block` reads delta files. The CLI has **no content-mutation command** (`new change`
scaffolds, `archive` merges, and nothing adds or edits a requirement), so a panel that lets
somebody fix a requirement has to write the markdown. What that module does is deliberately
smaller than parsing: it answers *where in these bytes is the block named X under
`## <OP> Requirements`* as a **byte range**, and the writer splices a replacement into it and
copies every other byte. Front matter, comments, blank-line style, trailing whitespace and every
other requirement survive byte for byte, which is the round-trip test in `crates/cide-spec/tests/`.
It mirrors the CLI's own scanner, and where it differs it is stricter: a duplicate requirement
name is refused rather than resolved.

## Decision 2 — the two systems join at exactly one field

`Task.change: Option<ChangeName>`. Nothing else is mirrored.

OpenSpec owns content — why, how, the checklist, the requirement deltas. The tracker owns
orchestration — status, assignee, the dispatch trigger, the review conversation. Each file keeps a
single writer, which is the discipline `.cide/` already runs on, and the alternative was worse in
both directions: exploding a checklist into cide tasks would create two writers for one bit of
state (an agent ticking a box in its worktree while the store merges `tasks.json`), and copying
assignment into `openspec/` would put orchestration into a file thirty other tools read.

`tasks.rs`'s header rejects six proposed fields on one test — *"a field an agent must be taught to
fill and a column the panel must draw, and neither changes what anybody does next."* This one
passes it three times: it changes what a dispatched run reads (`spec_preamble` points it at the
change), what the panel draws (progress, deltas, validity) and what accepting the task does
(integrate, archive, `Done`). A task without it takes none of those paths and renders exactly as
it did before M28 — asserted, not asserted-to: `check:agents-render`'s existing digests are
unchanged.

## Decision 3 — cide supplies buttons, context and state; upstream supplies the prose

`openspec init` installs its workflow into the very session the pinned Claude tab runs — as
Claude Code **skills** under `.claude/skills/openspec-<name>/` on a current CLI, and as **slash
commands** under `.claude/commands/opsx/` on an older one. Six commands either way, plus several
more in the CLI's templates, `onboard` among them, described upstream as a guided walk through a
whole change with real work in the codebase.

So cide runs `init --tools claude` and **never `--tools none`**, and anywhere cide would otherwise
write a paragraph of workflow instruction it names an upstream command instead.

**Which commands exist, and how they are invoked, is read from the project — never listed in
cide.** Both halves of that sentence were learned the same way.

The first was one name. cide offered `/opsx:onboard` for exactly one iteration before a user
pressed it and Claude answered `Unknown command`, with nothing in cide able to say why: `onboard`
is in the templates and is not installed by the profile `init` uses. The test that should have
caught it asserted the directory existed rather than its contents — a test reporting green on
precisely the thing it was written to cover.

The second was the whole prefix, and it broke every project rather than one button. OpenSpec moved
the same six commands to skills, so `/opsx:propose` became `/openspec-propose`; cide had the old
spelling in nine strings and refused every gesture on every correctly set up project, recommending
`openspec update` — which on such a project answers *"all tools up to date"* and writes nothing.
The rewritten contents test could have seen it and did not, because it asserted against the
surface rather than against the question the panel asks.

`cide_spec::claude` is the answer to both. A *surface* is enumerated exactly like a name —
`SURFACES` is a two-row table, newest first, so an older project keeps working and a third
spelling costs one row — `spec_run_command` resolves through it immediately before typing, the
board carries the resolved line so the composer previews what it will send, `check:openspec`
fails on any invocation spelled anywhere in the panel, and the real-CLI test now asks *what does
this project type to run propose* through the same function the panel uses. `onboard` stays
pinned as **absent**, so a release that adds it fails loudly rather than going unnoticed.
`SPEC_PREAMBLE`
is six sentences of cide's own rules — the checklist is the record, tick as you go, set `review`
when done, never archive — followed by one command to run, `openspec instructions apply --change
<c> --json`, whose answer is stated to **outrank the preamble itself**. The rules are cide's
because `openspec` knows nothing about `.cide/tasks.json`; the work list is upstream's because
`apply.tracks` is schema-configurable and any summary here goes stale on the next release.

## Decision 4 — the binary is found by a ladder that lives in `cide-spec`, not in `toolchain`

`openspec` is installed by `npm -g`, so it lives in a Node directory that a *shell* rc file puts
on `PATH` and a desktop launcher never does. `cide_core::toolchain::extra_dirs` is `~/.cargo/bin`
and `~/go/bin` on Linux, and its header forbids widening that list: the directories cide searches
to find a binary and the directories it gives that binary's process are one list, and widening
`search_paths` alone would turn `claude_cli::resolve`'s refusal — which names a remedy — into an
opaque `ENOENT`.

So the extra rungs are contained in `cide_spec::discover`: `CIDE_OPENSPEC_PATH` (alone, never a
fall-through), then `toolchain::which`, then the Node directories, newest nvm version first. The
one-list rule is then honoured from the other side by `child_env::run_filter_with`, which appends
the directory the binary was found in to the child's `PATH`. That is not tidiness: `openspec` is a
`#!/usr/bin/env node` script, so without it `execve` **succeeds** and the shebang dies with
`env: node: No such file or directory` — bit for bit the compounding failure `toolchain`'s header
records for a `gopls` that cannot exec `go`.

There is deliberately **no `npx` rung**. `npx -y` downloads a package on first use with nothing on
screen saying so, which is what `cide-deps` passes `--frozen` to prevent. A refusal naming the
install command is the honest answer, and one the user can act on.

## Decision 5 — three facts about the CLI that the code turns on

Each of these is a silent bug if assumed the other way, and each is asserted in
`crates/cide-spec/tests/real_cli.rs` against the installed binary.

**The exit code is not the failure signal.** Every `--json` command exits 0 and reports failure as
a `status` array on stdout. `cli::parse` therefore reads `status` *before* building the typed
value; a caller that did not would read `openspec show nope --json` as a real board describing
nothing.

**Root resolution walks ancestors.** `openspec` climbs parent directories and reports what it
found as `root: {path, source}`. Since a live agent's progress is read from that agent's worktree,
a worktree with no `openspec/` of its own would otherwise answer with *the parent repository's*
board — a plausible screen about somebody else's files. Every read pins the resolved root against
the directory it asked about.

**A bare invocation prompts.** `openspec init` asks which of forty AI tools to configure and plays
an animation. `OPEN_SPEC_INTERACTIVE=0` and a null stdin close that off twice over, because a
prompt on a command worker is not a failure — it is a hang until the deadline.

## Decision 6 — writing a requirement is compare-and-swap, validate, and roll back

The structured editor is the only place cide authors OpenSpec content, and it is the riskiest
surface in the milestone. The write path locates the file by asking the CLI (`status --json`
reports absolute `existingOutputPaths`; nothing joins a filename), splices one byte range, writes
atomically at `0o644` — `openspec/` is committed and reviewed like `.cide/tasks.json` — and then
re-validates.

The after-check is a **subset** rule and not "green": requiring validity would make an
already-broken file uneditable, which is the state somebody opens the editor to fix.
`TaskBoard::Unreadable`'s argument in miniature — a tool that repairs a file it cannot parse by
overwriting it has destroyed the user's data to fix its own display.

The compare-and-swap is against a `FileStamp` taken at the read, because an agent working in a
worktree edits these same files. Losing that race must be a refusal the panel can draw, not a
clobber.

## Decision 7 — one hole in the worktree storm filter, exactly one component wide

`dotcide::classify` routes everything under `.cide/worktrees/` to `Route::Nothing`, because an
agent's `cargo build` puts thousands of paths a second through it. M28 cuts one hole:
`.cide/worktrees/<agent>/openspec/**` routes to `Route::Spec`.

Without it, a task card's progress bar sits at zero for the whole of a run and jumps only at
integration — which reads as an agent that did nothing for an hour. With it any wider, the storm
comes back. The match therefore peeks exactly one component past the agent's name, and
`classify` now takes the project root and strips it first — `openspec` is an ordinary lowercase
word that can legitimately appear at `src/openspec/`, unlike `.cide`. That anchoring tightened the
`.cide` rule for free: a vendored dependency shipping its own `.cide/` used to fire.

## Decision 8 — the panel's requirements are read from the file, not from `show --json`

`openspec show <change> --json` returns each requirement's `text` with its `### Requirement:`
header already folded away, and each scenario's `rawText` as the body alone. So **the requirement's
name and every scenario's title are not on the wire at all** — and the name is what `archive`
matches on and what an edit addresses a block by.

Building the panel's requirements from that JSON therefore produced nameless cards, an
`issuesFor` that matched nothing, and a write path that could not have addressed what it drew.
`cide_spec::requirements_in` reads them from the delta file with `block::parse` instead — the
*same* scanner the writer uses, which is the property that makes this safe rather than merely
equivalent: the name on a card is by construction the name an edit to that card will address. The
CLI still decides everything else: which specs a change touches, each delta's operation and
description, and whether the whole thing validates.

The same read also fixed a double count. The CLI sends `requirement` **and** `requirements` — the
same requirement in two shapes — for a delta carrying one, so summing them drew every
single-requirement change twice.

Both were invisible from a fixture, because the fixtures were written from the documentation
rather than from the tool. `crates/cide-spec/tests/real_cli.rs` now pins both against the real
binary.

## Decision 9 — the editor edits the file's bytes, and composes only what it must

A requirement card's editor opens on `SpecRequirement::block` — the block exactly as it is in the
file — and its fields are derived from that. Saving composes a fresh block from the fields, which
is *not* byte-identical to what was read: blank-line runs and trailing spaces inside prose are
normalised. That is why the editor tracks `dirty` field-by-field rather than by comparing
compositions, and why an untouched draft is never written back — a save that reformatted a file
because somebody opened and closed an editor is a diff nobody asked for.

A scenario's body is a **textarea**, not a pair of WHEN/THEN inputs. Structured clause fields
would need a parser and a serialiser that round-trip every scenario anybody ever hand-wrote, and
the first one that did not would silently rewrite a committed file a reviewer had already
approved. `+ Scenario` seeds the canonical shape and the clause chips insert a line at the caret,
so the vocabulary is taught without being owned: a user who types their own bullets, or three
clauses, or a table, gets exactly the file they wrote.

## Consequences

- A project without `openspec/` sees one rail button and nothing else. A task without a change
  renders as it did before M28. The feature is optional in both directions.
- `cide-spec` is linked by `cide-headless`, so the no-tauri proof covers it, and
  `cide-headless spec <root>` is the only way to see whether a given launch can find the binary.
- The `--json` shapes are pinned by checked-in fixtures for the ordinary tests and by
  `#[ignore]`d tests against the real CLI for the ones that keep the fixtures honest. An upstream
  release that renames a field is caught by `cargo test -p cide-spec -- --ignored` and nowhere
  else.
- `cide://spec-changed` carries no revision. It does not need one: `spec_state::SpecBoards`' rules
  exactly one flusher per burst, so only one read is ever in flight per project and two boards
  cannot land out of order. That is a property of the coalescer — a second path that read a board
  and emitted it directly would break it.
