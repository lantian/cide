# cide


An IDE whose centre of gravity is a live Claude Code session rather than a text buffer.

The distinguishing feature is a **pinned, non-closable Claude tab per project**, hosting a
tiling grid of panes — the project's primary Claude session, additional sessions, plain
shells, and read-only diffs — alongside the ordinary IDE furniture that serves it: a file
tree, a tabbed editor, an IDEA-style git commit tool window, `Ctrl+P`, and `Shift+Ctrl+P`.

Rust + Tauri 2. Linux-first (developed on KDE/Wayland), with the code kept portable.

**Status: M0, M1, M3 and M4 complete; M5 code complete, its audit unrun.** The workspace
builds and runs, real `claude` processes render in xterm panes, the domain core owns the
workspace tree, the shell chrome matches the design mock to within a pixel in both themes,
and the pinned Claude tab tiles into a recursive split tree whose panes hold live sessions.
Panes detach into their own windows, quitting saves the workspace, and relaunching resumes
each project's primary conversation. Four automated gates guard the seams: the IPC transport
benchmark (`BENCH.md`), the chrome layout audit, the pane lifecycle audit, and the window
audit.

## Run it

```sh
pnpm --dir ui install
cargo build -p cide-app
./run.sh
```

**Use `./run.sh` rather than launching the binary directly.** Beyond the process hygiene
below, it starts the piece a debug build cannot do without.

A **debug** build does not load `ui/dist`. `tauri.conf.json` sets
`devUrl: http://localhost:1420`, and that URL is baked into the binary — so launching
`./target/debug/cide` with nothing on port 1420 gives a white window and
`Could not connect to localhost: Connection refused`. The app is running correctly; it just
has no document. `run.sh` starts Vite, waits for the port, and stops it again on exit.

If you would rather not have a dev server at all, `./run.sh --release` runs a release build,
which embeds the frontend:

```sh
pnpm --dir ui build
cargo build --release -p cide-app
./run.sh --release
```

`run.sh` also stops any instance already running before starting a new one, reaps `claude`
children orphaned by a previous crash, and refuses to start when the saved workspace would
open more than eight windows.

That last guard is not hypothetical. A workspace here accumulated 242 copies of one
directory, was left in per-project window mode, and the restore path faithfully opened a
window for every one of them — enough to make the machine unusable. Three things now stand in
the way: opening an already-open path activates it instead of duplicating it, the restore
caps how many windows a file may produce, and `run.sh` checks before anything is on screen.

```sh
./run.sh --release          # release build; embeds the UI, needs no dev server
./run.sh --fresh            # start from an empty workspace
./run.sh --bench            # IPC transport gate (M0)
./run.sh --audit-chrome     # chrome vs the design mock (M3)
./run.sh --audit-panes      # pane host registry under churn (M4)
./run.sh --audit-windows    # detach/re-dock and window modes (M5)
./run.sh --on-top           # keep the window above others, for screenshots
```

### Environment variables

| variable | effect |
| --- | --- |
| `CIDE_BENCH=1` | Run the IPC benchmark on first paint, print the report to stdout, exit. This is the M0 GO/NO-GO gate — see `BENCH.md`. |
| `CIDE_ON_TOP=1` | Build the window `always_on_top`. Needed for screenshot-based verification: KDE's focus-stealing prevention keeps a shell-launched window behind everything else. |
| `CIDE_AUDIT=1` | Measure the chrome against the design mock's stated dimensions in both themes at 1440x900, print the table to stdout. This is M3's acceptance check — see below. |
| `CIDE_AUDIT_WINDOWS=1` | Detach and re-dock a pane, flip the window mode, and assert no session was lost to any of it. This is M5's acceptance check. **Not yet run** — see the milestone table. |
| `CIDE_AUDIT_PANES=1` | Run 100 split/close/maximize/tab-switch cycles against the real domain, asserting `term.open()` happened exactly once per pane and no host was destroyed while mounted. This is M4's acceptance check. |
| `CIDE_NO_GRAPHICS_WORKAROUNDS=1` | Skip the Linux graphics ladder, for bisecting a rendering bug against stock behaviour. **The app does not start on a stock KDE Wayland desktop without it** — see ADR 0006. |

## Check it

```sh
cargo build --workspace
cargo test --workspace           # domain invariants + PTY behaviour under load
cargo clippy --workspace --all-targets
cd ui && pnpm typecheck
CIDE_BENCH=1 ./target/debug/cide # re-measure the IPC transport
CIDE_AUDIT=1 ./target/debug/cide # re-check the chrome against the design mock
CIDE_AUDIT_PANES=1 ./target/debug/cide # re-check the pane host registry under churn
CIDE_AUDIT_WINDOWS=1 ./target/debug/cide # re-check detach/re-dock and window modes
```

## Panes, tabs and the reopen stack (M15), and what is not done

| key | what it does |
| --- | --- |
| **Ctrl+Shift+T** | reopen the last closed tab, and the one before that |
| **Ctrl+W** | close a tab — and land on the tab you came *from*, not the one to the left |

**The focus ring is gone from editor panes, and only from editor panes.** It was added in M14 for
every pane kind and the user asked for half of it back: two accent pixels around the file you are
reading are noise. It stays on `claude`, `shell` and `diff` panes, where several terminals share a
tab and nothing else answers "which one am I typing into" — an editor answers that itself, because
CodeMirror hides its cursor when the surface is blurred. **The state behind it did not move.** The
removal is two CSS rules under `.frame[data-kind='editor']`; `PaneTitleBar.tsx` still applies
`frameFocused` from `focused` for every kind and still publishes `data-focused`, so
`keys/context.ts` goes on deriving `paneFocused`, `editorFocused` and `fileTabActive` from
`tree.focused` and the clauses that gate `file.save`, the find bar, the outline, Find Usages and
`pane.navigate.*` are untouched. `check:rows` pins both halves — that the exception exists, and
that the class and the attribute are still unconditional — because the rationale for *adding* the
ring is still a dozen lines above the exception and has to be, the ring is still drawn for three
kinds out of four. The canvas's 3px gutter stays too: it was justified by the ring in the comment,
but what it actually fixes is a pane's ordinary `--border` sitting flush against the app's.

**Closing a tab activates the most recently used survivor, and that moved the MRU into Rust.** The
old rule was the tab to the *left*, which is strip position, which is insertion order — so closing
a file opened an hour ago dropped you beside whatever happened to be opened just before it.
`Project::tab_mru` is now a workspace field, written by the one `set_active` that every writer of
`active_tab` goes through, and `close_tab` picks the first survivor in it. The webview's
`localStorage` copy under `cide.tabMru` is **deleted**; `ui/src/store/workspace.ts` reads the order
off the snapshot and only `reconcile`s tabs the order has never seen onto the end so the switcher
can walk to them.

The argument that had kept it in the webview is in `store/workspace.ts`'s own note, which admitted
it was weak. What settled it: **`close_tab` has to pick a successor and `close_tab` is in Rust**,
and two of its callers have no webview to ask — `cide_app::ide` closes a withdrawn diff from the
MCP server's thread, and the quit ladder closes projects wholesale. A successor computed in a
renderer would have had to exist twice, and the copy in Rust would have been the rule this replaces.
The two objections evaporated: the field moves only inside mutations that already bump `rev`, so it
costs no extra broadcast, and `#[serde(default)]` plus `repair_tab_mru` on load is not a schema
break — `CURRENT_SCHEMA` stays at 2.

**A workspace written by any earlier build still opens.** That is the expensive thing to get wrong:
`tab_mru` defaults to empty, `validate` refuses an empty order, and `WorkspaceState::load` answers a
failed validation by replacing the whole workspace with defaults — so a missing repair would greet
every existing user with no projects, with their file intact on disk and never read again.
`persist::load` repairs to `[active_tab]` and **invents nothing else**: strip order is not use
order, and a plausible-looking history would make the first close land somewhere the user has never
been. `close_tab`'s left-neighbour rule is kept as the fallback for exactly that state.

**Ctrl+Shift+T reopens closed tabs, and keeps going.** The stack is 16 deep (`persist::MAX_RECENT`'s
number), lives in `cide_app::closed_tabs` — Tauri state, *not* `Workspace`, so a close does not
broadcast the tree for a record nothing draws — and is popped **project-scoped**, so Ctrl+Shift+T in
a window showing project A can never resurrect a file from B. A record carries the project, the tab
kind, the strip index and the pane tree *with its original pane ids and session bindings*: the
frontend's `paneHosts` map is keyed by pane id and a tab close does not clear it, so a reopened
`ClaudeFull` tab re-adopts its own parked terminal and its live conversation rather than starting a
second one. Scroll and caret need no record at all — `positions.json` is keyed by path and
`EditorPane` flushes on unmount.

Three things it deliberately does not do. It **does not survive a restart**: the gesture means "I
closed that ten seconds ago by accident", and the first press of a session reopening something
deliberately closed last week is the opposite of that. It **never remembers a diff Claude is
blocked on** — closing that tab cancels the agent's request, so the `request_id` names nothing —
and it filters at *push* rather than at pop, because a record that can never be popped is a hole
the user counts through. And it carries **no `when` clause**: the depth is not in the snapshot by
design, so a flag for it would need a supplier the snapshot has no business carrying, and a flag
with no supplier is the `repoOpen` mistake that hid the whole Git group for a milestone. The row is
always offered and reports "nothing to reopen" instead.

`theme.toggle` gave up `ctrl+shift+t` and ships **unbound** — palette-only, like
`project.switcher.prev`, and one line of `keymap.json` takes it back. No gate requires a registered
command to carry a binding, and `theme_toggle_ships_unbound_and_is_still_a_command` exists to stop
one being added: "every palette row has a key" sounds like an invariant and is not one. **Ctrl+T
still pulls.**

**Not done.** None of it has been confirmed on screen — same reason as M14, KDE will not raise a
shell-launched window. The reopen decision is covered by `reopen_plan`'s test and the stack by
`closed_tabs`'s, but `tab_reopen_closed` itself is a Tauri command and the three lines that wire
`stack.pop` to `reinsert_tab` are checked by nothing. A **`ClaudeFull` tab reopened after its child
has exited** comes back as a Resume splash rather than as a live pane; that is the existing restore
behaviour and not new, but Ctrl+Shift+T is a new way to reach it. The stack is not offered in the
tab strip's right-click menu: a menu item that cannot be honestly disabled — the depth is not in the
webview — and that silently does nothing is the surface where silence is worst, so the two routes
are the chord and the palette.

## Scrolling a Claude session (M16), and what is not done

**The scrolling belongs to Claude Code, not to cide.** A Claude pane runs on the alternate
screen (`ESC[?1049h`, confirmed live from the spawned CLI's first 60 bytes), and the alternate
buffer has no scrollback by definition — so there is no cide-side scrollback to give. What
happens instead is that xterm.js forwards the wheel to the child as an SGR mouse report and the
CLI scrolls its own transcript. That path works: driving a resumed 2.3 MB session, 400 wheel-ups
travelled roughly 1200 transcript lines without ever reaching the top, and every single report
produced a repaint. `PageUp`/`PageDown` scroll it too, and are bound to no command in
`cide_core::keymap`, so the gate passes them straight through.

Two numbers multiply to give what one notch of the wheel is worth, and **cide owns only one of
them**. xterm.js sends at most **one report per DOM wheel event** — `sendEvent` computes a line
count and then discards it — and `CoreMouseService.consumeWheelEvent` scales any event with
`|deltaY| < 50` by `0.3`, believing it to be a trackpad. A high-resolution wheel under Wayland
delivers one notch as several small deltas, which is exactly that regime. The other number is
`CLAUDE_CODE_SCROLL_SPEED`, the transcript lines the CLI moves per report, and that is now
**Settings → Claude sessions → Scroll speed**, 1 to 20.

**Three environment switches were reachable from nothing, and that is case #20.** `disableMouse`,
`altScreenFullRepaint` and `disableAlternateScreen` were declared in `cide-ipc` with doc comments
naming the variable each one sets, persisted, bound to TypeScript, and drawn as switches under a
panel reading *"Applied at spawn — these reach a pane's child process when it starts"*. Nothing in
the workspace read any of them; `resumeAllOnLaunch` was the struct's only field with a consumer.
It mattered more than the usual instance, because *"Disable the alternate screen"* is precisely
the control a user chasing a scrollable transcript reaches for. They are wired now, through
`cide_core::child_env::claude_env` into `base_env`, and `check:claude-env` compares the set of
variables the Settings screen names against the set a spawn actually sets, in both directions.

**Not done.** Whether this machine's mouse is in the `< 50` regime is **unmeasured** — it needs a
physical wheel over a running window, and no lane may launch the app while sibling `claude`
children are alive. The measurement is `./run.sh --inspect`, then
`addEventListener('wheel', e => console.log(e.deltaMode, e.deltaY), {capture: true})` over a Claude
pane, one notch. Several events each under 50 means the trackpad clamp is eating the gesture, and
the fix is `scrollSensitivity` — a real xterm 6.0.0 option, and the *only* lever that does not
require cide to encode mouse reports itself, which ADR 0003 assigns to xterm. It must be applied
**only while the pane is in the mouse-reporting regime**: the normal-screen path is a separate
VS Code `ScrollableElement` with no trackpad clamp, so raising it globally would speed up a bash
pane's scrollback, which is not broken. `shift`+wheel is inert in a Claude pane and stays that
way — xterm drops it in `consumeWheelEvent` before any protocol runs, confirmed at zero bytes.

## The tab strip: drag to reorder, Close to the left, and Back into a closed file (M16)

| gesture | what it does |
| --- | --- |
| **drag a tab** | move it along the strip; an accent caret shows where it will land |
| **right-click → Close to the left** | close everything before this tab, asking about unsaved work once |
| **Back / Forward** (the mouse's thumb buttons) | reopen a closed file *as it was* — split, pane ids and all |

`navigate.back` and `navigate.forward` have **no key binding by default** — the thumb buttons are
it. `Ctrl+Alt+←`/`→` are not them and must not be written down as if they were: `ctrl+alt+right` is
`pane.split.right`, which on any tab but the console splits the pane and spawns a *new* `claude`
child, so a reader who trusted that line got a running process instead of a navigation. *Language
support (M12)* below carries the `keymap.json` entries if you want keys as well —
`ctrl+alt+shift+left`/`right` are free in every layer.

**Reordering was implemented and reachable from nothing, and that is two more for the count.**
`cide_core::workspace::reorder_tab` has been complete, invariant-preserving and covered by three
passing tests since M4, and no `#[tauri::command]` called it: the domain rule existed, the seam did
not, and the tests said nothing about whether a user could do the thing. That is case #18 of this
project's signature defect. **Case #19 is still open beside it**: `project_reorder` is complete end
to end — `workspace.rs:217` → `cmd/project.rs` → registered in `lib.rs` → exposed as
`project.reorder` in `ui/src/ipc/client.ts` — and `grep -rn reorder ui/src` finds that definition
line and **zero call sites**. The header's project tabs have no drag handling at all. It is written
here rather than fixed, because the fix is the same shape as this one and belongs with a gate.

`tab_reorder` takes **ids, not indices**, and a `before` boundary rather than a destination. The
drop position is computed in the webview at pointer-move time against a snapshot, and an agent's
`openDiff` or another window's close can move the strip before `pointerup`; an index would then
move whichever tab now sits there, silently and somewhere nobody aimed. Both ids resolve to indices
*inside* the workspace lock. The boundary→index conversion (`to = boundary > from ? boundary - 1 :
boundary`) is a free function with tests, because the wrong version makes every rightward drag a
no-op while every leftward drag works perfectly — the sort of half-working a manual test passes.

**Pointer events, not HTML5 drag and drop**, the third time this repository has settled that
question: Tauri's native drag-drop handler is on, `windows.rs` never calls
`disable_drag_drop_handler()`, and a gesture that works in `pnpm dev` and does nothing in the
shipped app is the worse failure. The rules live in `ui/src/chrome/tabDrag.ts`, which imports
nothing so `check:tab-drag` can compile and run it; `useTabDrag.ts` is the mechanism and no check
in this repo can execute it. Feedback is a **2px caret**, never a gap that opens between two tabs —
the strip is 30px tall and reflows *under the pointer*, which is the rule its stylesheet already
spends a paragraph on for the awaiting marker. The pinned console is refused three ways so the
refusal is never reached: the grab returns `null` for it, the caret clamps to boundary 1, and the
drop refuses a boundary of 0. Rust refuses it a fourth time, and that is the authority.

**The three bulk closes ask once.** *Close others*, *Close to the left* and *Close to the right*
used to fire one `closeTab` per tab; each did its own risk round trip, each parked its own refusal,
and the queue in `closeConfirmStore` showed them one after another — three modals each headed
*"Closing this tab"*, each naming a different file, for a gesture the user made once. They now go
through one `closeTabs`, which asks `quitRequested` once, narrows it to the tabs actually closing,
and raises **one** dialog under the new `tabs` scope that names every file at stake. `Close to the
left` filters by `closable` rather than testing `index === 0`, which is what makes it skip the
pinned console without knowing it exists — and it is offered-and-disabled rather than dropped when
the set is empty, because that is the *common* state (every tab at index 1 has only the console to
its left) and an item that comes and goes with the tab order is one the user has to hunt for.

**Back reopens a closed file through the closed-tab stack, and peeks rather than pops.** It used to
call `tab_open_file`, which mints one fresh single-pane editor and knows nothing about how the tab
left. Two things were wrong with that. A file tab's pane tree can hold a **live Claude session** —
splitting a File tab defaults to `NewClaude` — and `reinsert_tab` exists precisely to hand those
pane ids back to `paneHosts`. And it silently poisoned Ctrl+Shift+T: Back opened a fresh tab for X
while X's record was still on the stack, so the next press popped that record, found X open and
*consumed it anyway*, landing the user on an unrelated tab with the split gone for good.

`tab_reopen_file` therefore stats first, then **peeks** at the record and **takes it only when it
actually reinserts the tab**. The cost of each alternative, since both were tempting: popping would
hand Back whatever closed most recently rather than the file it has in mind, and consuming on a
mere activation would spend a record with nothing on screen to explain where it went — while
ignoring the stack entirely is the fresh-single-pane behaviour above, every time. A walk into a
file that is gone answers `gone` instead of minting a permanent tab reading *"This file could not
be opened"* with the path nowhere in the sentence; the entry is dropped so the next press does not
walk into the same wall, and `navHistory::abandon` puts the cursor back where the user actually is
rather than where the failed walk left it.

**A refused terminal open is no longer a place you have been.** `App.tsx` recorded the history
entry *before* `openFromTerminal`, so a path the user was shown and **declined** in the
out-of-project dialog stayed on the Back stack — where Back reached it again through a command that
by its own documentation enforces nothing. The entry is now written from the `.then()`. The
*origin* is still read before the call, through `pendingJump`'s closure: the open broadcasts from
inside the workspace lock, so the destination's editor can already have claimed `caretTrack`'s slot
by the time the promise settles, and reading the caret there would record the destination as its
own origin and leave Back with nowhere to go.

A Back entry's recorded position beats the per-file view memory, and that was confirmed rather than
assumed: `planRestore` refuses to restore while a reveal is parked for the path, `EditorSurface`
runs it *before* `registerReveal` spends the request, and `check:editor` pins that ordering. An
`UNKNOWN_LINE` entry parks nothing and correctly defers to the view memory, because that entry
never named a line.

**Not done.** None of it has been confirmed on screen — same reason as M14 and M15, and this batch
adds a pointer gesture, which is the kind that most wants a real window: `--audit-chrome` is worth
one run to confirm the caret changed no measured dimension. The strip **clips rather than scrolls**
(`.tabs` is `overflow: hidden`), so a tab past the right edge is neither painted nor hit-testable
and cannot be a drop target; the caret clamps to the last visible boundary. That is inherited, not
introduced — making the strip scrollable interacts with the awaiting marker's reserved box and is a
separate change. `tab_reopen_file` resolves its plan under one lock and reinserts under another, so
a second shell window opening the same file in between can produce two tabs over one path; that is
the same window `tab_reopen_closed` has always had. And the **bulk close stops at the first
unanticipated refusal** rather than marching on — a file that turns dirty between the one question
and the closes parks a dialog about itself, and the tabs after it stay open.

## Speed search in both sidebar trees (M15)

**Type into the explorer or the changes tree and it filters to rows whose name contains what you
typed.** Up and Down walk the matches in tree order, Enter opens the one you are on, Escape leaves,
and two seconds of silence clears it. A small box at the top-left of the panel shows the query and
`3 of 17`.

| key, while a query is armed | what it does |
| --- | --- |
| a letter, digit, `.`, `-`, `_` | extends the query — a bare keystroke *starts* one |
| Space | extends it, **once something has been typed**; a bare Space is still the changes tree's tick |
| ↑ ↓ | previous / next match, wrapping |
| Enter | opens the row and leaves |
| Escape, Backspace past the first character | leaves |
| Delete | **swallowed** — a bare Delete is *Move to Trash*, and a typo must not put a delete dialog on screen |
| ← → Home End, and every modified chord | leaves, then does exactly what it always did |

Nothing is taken from the keymap: `cide_core::keymap` binds no unmodified printable key, which
until M15 was a coincidence and is now `no_default_binds_an_unmodified_printable_key`. It had to
become an assertion, because the key gate resolves global bindings on a **window capture**
listener — a default of `r → git.pull` would eat the letter `r` in the file tree, the rename box,
the commit message, CodeMirror and every terminal, and speed search would go dead for that one
letter with nothing reporting it.

**One matching rule, in Rust, with two entry points.** `cide_fs::speed` decides what matches and
where; `fs_tree_match` walks the explorer's flattening and `tree_match_labels` takes a list the
changes tree supplies. That is the `picker_rank` shape rather than the `score.ts` shape, and one
fact decided it: the explorer's rows are not in the webview — it holds 200-row chunks of a
flattening Rust owns, so a match at row 40,000 exists only if Rust is the one looking. Once that
exists, a TypeScript copy for the other tree is a second answer to a settled question. The spans
come back in **UTF-16 code units**, because the only consumer is `String.prototype.slice`; a byte
offset agrees for ASCII and puts the highlight four characters early on any name with an emoji in
it. The *key* rule — which keystroke does what — is `ui/src/sidebar/speedSearch.ts`, pure and
import-free, driven row by row by `check:speed-search`.

**Only what is expanded is searched**, and the empty answer says so: `no match in the expanded
tree`, not `no match`. Expanding to reveal a match would re-flatten the tree and renumber every
index in the list being built, so each keystroke would invalidate its own results — and Ctrl+P
already searches the whole repository including collapsed directories. A capped list says `first
1000 matches` rather than answering short.

**Not done.** There is no way to reach speed search from the palette or a menu — typing *is* the
gesture, and nothing on screen advertises it. Matching is substring, not fuzzy, and not ranked. A
match list is dropped and re-issued when the rows move underneath it, so a burst mid-query costs
one round trip and a frame with no highlight.

## The Find usages popup, and the scratch type picker (M15)

**"It doesn't see where is filename and where is code."** The Find usages popup drew its file
headings with `.path` and its source lines with `.usageText`: same family, same weight, same colour
token, and **half a pixel** of font size between them — with the heading being the *smaller* of the
two. Three more defects were in the same three lines: `.path` ellipsises at the tail, and the tail
of `crates/cide-lsp/src/progress.rs` is the filename; `.group` is `flex: 1`, so a two-character hit
count claimed half a 620px card; and `.usageFile`'s `background: var(--chrome)` is exactly the
card's own ground, so the one rule written to make the heading stand out painted it the colour it
already was. The heading is now the UI face at 12.5px in `--w-bold`, front-truncated, with the
file's icon beside it — the search panel's treatment, because the two lists show the same thing.
Go to symbol had the same defect in a smaller form (two adjacent `.path` spans reading as one grey
run) and is fixed in the same pass.

**The gate that missed it is the finding.** `check-theme.mjs` has a table asserting exactly this —
"a name is the UI face, code is the mono face" — over three stylesheets, and
`src/overlays/Overlay.module.css` was not one of them. So the one overlay that grew a
heading-plus-source-line list in M14 was never swept. It is in the table now, plus assertions that
the heading is *larger* than the source line and carries a weight it does not.

**SQL is a real grammar, not a row.** `ui/src/editor/languages/sql.ts` is a `streamGrammar` data
module: `--` line comments, case-insensitive keywords (the flag exists for this one language), a
doubled quote rather than a backslash escape, and `capitalisedIsType`/`callSyntax` both **off**,
because `Users` is a table and `Sessions (` is a table followed by a bracket. The last of those was
found by the check rather than reasoned about — with `callSyntax` on, every table in a schema was
drawn as a function. There is no `@codemirror/lang-sql` in this project and adding one is the
100 KB-of-parser-tables decision `streamGrammar.ts` exists to avoid; `check:editor` would have
failed a bare `{ label: 'SQL', ext: 'sql' }` outright, which is the gate doing its job.

**The scratch picker has a filter field, and it is focused on the first frame.** `useLayoutEffect`,
not `useEffect` — `ModalShell` records the reason and it is a bug this project has already shipped:
the overlay is opened by a keystroke, and a frame in which the field is not yet focused is a frame
in which the next character goes to whatever had focus before, which is a terminal. The ranking is
`filterScratchTypes` in `editor/languages.ts` beside the list it filters (exact extension, then
extension prefix, then label prefix, then label substring), so `sql`+⏎ is the whole gesture. A query
that matches nothing says `No type matches ‘xyz’` and Enter does nothing — deliberately not
creating a scratch of the typed extension, which would be a free-text path into `check_ext` and a
separate decision.

## Autosave (M15)

**On by default.** A changed file is written when it loses focus, and after a minute with no edits.
*Settings ▸ Editor ▸ Save automatically*.

The signal is CodeMirror's own `focusChanged`, on the edge this listener never had, and one hook
covers everything: a click into another pane, into the file tree, into a terminal; a **tab switch**
(hidden tabs are `visibility: hidden`, never unmounted, so switching tabs is a blur); and the OS
window being deactivated, which is IDEA's frame-deactivation save arriving free. The idle timer is
a 60-second debounce **with a five-minute ceiling** measured from when the buffer went dirty,
because a restarting debounce is starved by continuous input — somebody typing steadily for twenty
minutes would otherwise never autosave, a trap `docSync.ts` and `gitCountStore.ts` had both already
written down.

**Every refusal, and how it is enforced.** All of them are `shouldAutosave` in
`ui/src/editor/autosave.ts` — pure, import-free, and driven as a truth table by `check:editor`,
because a feature that writes the user's files on a timer must not keep its rules inside a
`useEffect`.

| autosave refuses | because |
| --- | --- |
| the setting is off | and the toggle is *read*, which `check:editor` asserts — see below |
| the buffer is clean | a save is a `didSave`, which re-runs flycheck; otherwise every alt-tab is a `cargo check` |
| **the buffer is read-only** | External Libraries and toolchain sources; writing one corrupts a crate every project on the machine builds against |
| **the conflict bar is up** | it is the user's unanswered question, and autosave would answer it in the direction that discards whatever changed the file |
| **a Claude `openDiff` of this file is open** | `openDiff` blocks the agent's turn. Clicking the diff tab to look at it is a tab switch, is a blur, is a save underneath a proposal computed against the old bytes |
| focus is still inside the editor | the find bar is a CodeMirror panel inside `view.dom` |
| an overlay or context menu is open | Ctrl+P is not leaving the file |

Window deactivation is checked **before** the last two: `document.activeElement` does not move when
an OS window is deactivated, so testing "is focus still in the editor" first would veto the one
save this feature is best known for. An explicit Ctrl+S ignores all of it — that is the user
deciding.

**A failed autosave produces a sentence.** `void diag.log(...)` writes to a file nobody opens, and
a save that happened on a timer while the user was looking at a browser is the one that most needs
saying out loud — this is also the population where writes actually fail, because a root-owned
mode-644 file reports `writable: true` (the mode bits, not "can *you* write it"). The notice
dedupes by text, and the timers are not re-armed after a failure, so a full disk gives one toast
rather than sixty. The tab stays dirty either way, which is what keeps the close confirmation in
front of the user.

**The `sed -i` hole is closed, for autosave.** The conflict bar is raised by `cide://session-tool`,
which a `cargo fmt`, a `sed -i` or a `git checkout` never sends. Before autosave, clobbering one of
those took a deliberate Ctrl+S; with autosave-on-blur it would take *switching to the terminal,
running `cargo fmt`, and clicking back* — three things nobody decides to do. So `FileDoc` now
carries a `FileStamp` (mtime + length) and a background write hands it back as `ifUnchanged`;
Rust refuses with a **tagged** `CoreError::FileChanged`, and the pane turns that into the same
conflict bar. Subscribing the editor to `cide://fs-changed` was the alternative and is worse: our
own write emits one, so it needs a self-write suppression window, which is a race with a timer in
it. What remains open is the microsecond between the `stat` and the `rename`, and an explicit
Ctrl+S, which still forces.

**The dirty dot is unchanged**, and deliberately: its meaning is "this buffer differs from disk",
which stays exactly true, and it is load-bearing rather than decorative — `tab_set_dirty` is what
makes `close_tab` refuse without `force` and what `app_quit_requested` reports by name. There is no
save-on-quit either. The honest consequence is that the unsaved-at-quit dialog becomes rare, which
is the feature working; the guard stays for the cases autosave refuses, which are precisely the
cases where the user most needs to be asked.

**Migration is explicit.** `Workspace::CURRENT_SCHEMA` moves to **3** and `persist::v2_to_v3`
*writes* `settings.editor.autosave = true` into every existing document. `#[serde(default)]` would
have loaded a schema-2 file perfectly well and given the same behaviour — deleting the migration
changes nothing today. It is there for the day the default moves: "make it opt-in" is the obvious
next request, and a defaulted field would silently turn autosave *off* for everyone relying on it,
with nothing on their disk to explain it. Same argument as 1 → 2's proxy scope, applied to a
setting whose blast radius is the contents of files. A value the user already chose is left alone.

### Finding: five of the seven editor settings are wired to nothing

`tabSize`, `insertSpaces`, `showMinimap`, `wordWrap` and `trimTrailingWhitespaceOnSave` all
persist, survive a relaunch, and change nothing — `EditorSurface` hardcodes `tabSize.of(4)`,
`indentUnit.of('    ')`, an unconditional `minimap()` and an unconditional `lineWrapping`, and
nothing anywhere reads the trim flag. `tsc --noEmit`, `codegen --check` and `contract-check` are
all green over that, because every one of them is a *field nobody reads*, which no existing gate
can see. **Not fixed in M15.** The direct lesson was applied instead: the autosave toggle ships
with an assertion in `check:editor` that it is read, not merely offered.

## Three things that shipped and did not work (M15), and why no gate saw it

Each of these was built, verified and reported broken by the user. In every case the mechanism was
correct and something outside it — a dependency, a child process, an absent population — decided
the outcome. The second half of each entry is the more useful one.

**The mouse's thumb buttons did nothing, because wry got the press first.**
`wry-0.55.1/src/webkitgtk/synthetic_mouse_events.rs` connects its own `button-press-event` handler
to the WebKitWebView inside `WebviewWindowBuilder::build()`; for buttons 8 and 9 it returns
`Propagation::Stop` and spends them on `window.history.back()`, which in an SPA with one history
entry is a silent no-op. GTK3's `button-press-event` uses the true-handled accumulator, so the
*first* handler returning TRUE ends the emission — and `install_mouse_nav` runs after `build()` and
defers itself onto the GTK loop besides, so it was always second and **never ran**. The handler now
goes on the generic `event` signal, which `gtk_widget_event_internal` emits before any specific one
whatever the connection order (measured against the real GTK 3, both directions). The window-label
filter was checked first and exonerated: both sides derive from the same `label.as_str()`.
*The comments in `windows.rs` and `emit.rs` claiming WebKit flattens both buttons to `button === 0`
were false and are corrected in place* — a DOM-only implementation was possible all along, and the
wrong premise is what stopped anyone looking at wry. **No gate saw it** because every gate tested
one link of a five-link chain and the broken link was the dependency's: `check:keys` starts one
call downstream of the whole broken segment, `keymap.rs` proves the binding resolves,
`contract-check` records the event's *name*. Nothing asserted that a GDK button reaches a
`Decision`. The new gate is structural — the signal cide connects to, over comment-stripped source,
with the whole diagnosis in its failure message — plus `nav_action` extracted from the closure so
the press rules are drivable at all. A refused Back also went to `diag.log` and now goes to
`notify`, so an empty history can be told from a dead button; that indistinguishability is most of
why this took a milestone to notice.

**A ctrl+click on a path in a Claude pane opened the desktop file manager on the parent folder.**
Not cide's matcher — `(` and `)` are not body characters, so `Update(/home/…/Foo.tsx)` yields the
*file*, verified for every shape Claude Code prints. The gate returned early when nothing had
hovered yet, xterm's own always-on `mousedown` then wrote an SGR mouse report to the pty, and
`claude` answered it with `dbus-send … org.freedesktop.FileManager1.ShowItems`. In a Claude pane
the early return is the common case, not the exotic one: the alt-screen TUI repaints under a
stationary pointer, so no `mousemove` fires and no hover is ever recorded. The press is now claimed
**unconditionally** and resolved against the buffer afterwards, which also fixes the mirror-image
bug in the same three lines (a *stale* hover surviving a repaint). Claude Code stands down for
xterm.js hosts — `xtversionName?.startsWith("xterm.js")` — and cide failed that test only because
`@xterm/xterm` 6 implements no XTVERSION at all; it now answers `DCS > | xterm.js(6.0.0) ST`
truthfully, which is the only thing covering **alt+click**, a chord the CLI also claims and cide
deliberately does not. **No gate saw it** because `check-paths.mjs`'s own pin — *"a path parsed out
of terminal bytes reaches a workspace tab and nothing else — not the desktop opener"* — was true of
cide's source and false of the user's screen: a grep over cide cannot see a file manager opened by
a program cide forwarded the click to. *Anything cide does not swallow is a gesture cide has
delegated.* The rules moved into `ui/src/terminal/clickGate.ts`, whose `pressVerdict` is **not
given the hover state**, so the early return cannot come back; and the old pin turned out to name
only Rust command strings that could never appear in a frontend module, which a mutation found.

**Select opened file said "not in this project's file tree" about a `std` file on screen.** Nothing
was broken: the rustup sysroot is a population no part of cide knew existed. `cargo metadata`
reports the `Cargo.lock` graph — measured here, 547 packages, none of them `std`/`core`/`alloc` —
so there was no row for a reveal to find and `is_unlisted_library_path` never even asked the group
to resolve. The **second, unreported** half is worse: `…/library/core/src/option.rs` is mode 644 and
user-owned, so `writable` was true and Ctrl+S wrote into the toolchain every project on the machine
compiles against — bit for bit the bug the read-only rule was written for. `dependency_roots()` now
covers the whole `~/.rustup/toolchains` directory (textual, no toolchain-name resolution, no
syscall), and `cide-deps::sdk` adds one SDK row per unit from `rustc --print sysroot` with the
project's cwd — which is the *only* honest way to get the toolchain rustup would pick, and this
repo proves it: `rust-toolchain.toml` pins 1.92.0 while the machine's default is `stable`. **No
gate saw it** because both existing tests were self-referential: the end-to-end one is `#[ignore]`d
*and* builds its target out of `dependency_roots()`, and the predicate test took `caches.first()` —
neither can notice a *missing* root. The new ones name the rustup path shape from the outside and
sweep every root. One more gap was found by mutation while writing this: deleting `sdk::probe`'s
single call site left every gate green, which is this project's recurring defect appearing inside
the batch that was fixing three instances of it.

**Not done.** None of the three has been confirmed on screen (KDE will not raise a shell-launched
window); `CIDE_INPUT_PROBE=1 ./run.sh` is the one-command runtime confirmation for the thumb buttons
and must now print `button=8` on a press. The GTK signal choice is held by a source assertion, not
by a synthesized GDK event — an `--audit-windows` leg that injects one through `gtk_main_do_event`
and asserts `cide://mouse-nav` arrives is the check that would have caught the original bug for
real, and it would also be the first execution of `client.ts`'s label filter in any test. Go's
standard library stays **writable** unless `$GOROOT` is exported: deriving it needs
`canonicalize(which("go"))`, and `cide_core::toolchain` forks and `stat`s nothing on the per-open
path. A directory ctrl+clicked in a *detached pane* is offered no link at all, because that window
has no file tree.

## Switching, and the panel toggle (M14), and what is not done

Four keys, and one of them is a chord that moved.

| key | what it does |
| --- | --- |
| **Ctrl+Tab** / **Ctrl+Shift+Tab** | walk the active project's **tabs**, most-recently-used |
| **Ctrl+`** | walk this window's **projects**, most-recently-used (was Ctrl+Tab) |
| **Ctrl+1** | go to the pinned Claude console |
| **F4** | hide the left panel, or bring back the last one that was open |

**The two switchers are one implementation.** `ui/src/keys/switcher.ts` is the arithmetic over
opaque id strings — hold the modifier, press the key to walk, release to commit, Escape to cancel —
and `ui/src/keys/switcherStore.ts` holds *one* open walk, *one* capture-phase keyup latch and a
`SwitchTarget` that says what is being walked. That is not tidiness: the key gate has exactly one
`capture` hook, so a second store would give two walks of which only one could ever be consulted,
and the loser's popup would be immortal — swallowing nothing, closing never, while the winner ate
Tab for the rest of the session. `check:switcher` counts the `keyup` registrations and fails at two,
and it runs its whole scenario table twice, once per walk key.

**Ctrl+` is bound as `backquote`, not as the literal tilde**, although the tilde is what was asked
for. `ui/src/keys/chords.ts` derives a stroke from `KeyboardEvent.code`, and the key left of `1` is
`Backquote` on every layout whatever `key` reports — so a binding spelled `ctrl+~` would normalise to
a key no keystroke can produce and would be silently inert. `~` has been added to the alias table, so
a `keymap.json` that writes it still means the chord. Two more reasons the literal is wrong: it needs
Shift, so the whole walk would be a Ctrl+Shift hold, and the capture reads Shift as the walk's
*direction*. **`project.switcher.prev` ships unbound** — `ctrl+shift+`` is `terminal.splitBelow` —
and is reached from the palette, or by holding Shift while the popup is up.

**The tab order is a real MRU stack and it survives a restart.** There was no per-tab focus history
anywhere: `Tab` has no timestamp, `p.tabs` is insertion order, and `close_tab` picked the *left
neighbour*. M14 derived one per project in `ui/src/store/workspace.ts` from every
`cide://workspace-changed` snapshot and mirrored it to `localStorage` under `cide.tabMru`, with a
note in the code admitting that the argument for keeping it out of Rust was weak. **In M15 it moved
into `Project::tab_mru` and the `localStorage` key is gone** — see the M15 section above for what
settled it. The list contains **tabs**, console and settings included, which is the point: one
Ctrl+Tab from a file gets you back to the conversation about it.

**What Ctrl+Tab costs a user who already rebound it.** Nothing, and no migration: their
`{"key":"ctrl+tab","command":"project.switcher.next"}` has no `when` and the new default has one, so
the two coexist and the *last applicable* binding — theirs — wins everywhere. The one case that
changes is an **unbind**: `{"key":"ctrl+tab","command":"-project.switcher.next"}` now matches
nothing, is reported as `RemovalMatchedNothing` in Settings → Keymap, and Ctrl+Tab starts running
the tab switcher. `ctrl_tab_switches_tabs_and_an_old_keymap_json_still_wins` asserts both.

**F4 was the sixteenth thing built and reachable from nothing.** Hiding the sidebar has worked since
M3 — clicking the lit rail button does it — with no command, no binding and no palette row; the
mouse was the only way in and the only way back out. The new fact is *which panel comes back*, and
it lives in `ui/src/chrome/sidebarView.ts`, import-free so `check:sidebar` can execute it. The
settings view counts as **closed** (⚙ opens a workspace tab and has no panel), so F4 with it lit
opens the last real panel; `last` is never `settings`, or "bring back the last panel" would bring
back a state with nothing in it.

**It is not remembered across a restart, deliberately.** Every launch still opens on Files with the
panel showing, bit for bit as before. `Settings` is global and rides every snapshot to *every*
window, so a persisted "hidden" would mean F4 in one window closing another's sidebar in
`perProject` mode; and a Rust-owned flag arrives an IPC round trip after first paint, so the panel
would render and then be yanked away — the exact jump `chrome/sidebarWidth.ts` writes thirty lines
about avoiding. The two widths beside it stay persisted.

**What these keys cost, named rather than discovered.** All five bindings carry `when: shellWindow`,
so a detached pane or tab window keeps them — that is a small improvement on the old unconditional
`ctrl+tab`. In the shell window they are unconditional and the gate is a window *capture* listener,
so:

* **F4** takes `ESC O S` from every terminal pane. That is a real key to `mc` (Edit), `htop`
  (Filter) and any curses TUI. `{"key":"f4","command":"-sidebar.toggle","when":"shellWindow"}` in
  `~/.config/cide/keymap.json` gives it back — the `when` is not optional on that line, and the test
  named after it says why.
* **Ctrl+`** costs the pty nothing (xterm encodes no control byte for keyCode 192) and CodeMirror
  nothing. It costs legibility: Ctrl+` switches project and Ctrl+Shift+` splits a terminal below,
  one Shift apart, and it is VS Code's toggle-terminal chord.
* **Ctrl+1** costs nothing either — keyCode 49 is not in xterm's plain-Ctrl encode set.
* **Ctrl+2..9 are not shipped.** They are the browser convention and were not asked for; `tabs[1..]`
  are positional and shift under the user, where `tabs[0]` is an identity Rust enforces; and
  Ctrl+3..8 are *not* free, encoding ESC, FS, GS, RS, US and DEL. IDEA uses Alt+1..9 for tool
  windows and has no Ctrl+digit tab selection at all.

**Not done.** None of it has been confirmed on screen — the gesture needs a held modifier, KDE will
not raise a shell-launched window and the Wayland compositor exposes no capture protocol; the
coverage is `check:switcher`, `check:keys`, `check:sidebar` and `check:commands`. `tab.next` /
`tab.prev` still have the latent issue the new commands avoid: they carry no `shellWindow` clause,
so from a detached-tab window they would move the *shell* window's active tab. The palette's key
chip prints `f4` lowercase, because `chipLabel` upper-cases single characters only and has no f-key
glyph — `ctrl+f12` has always rendered `⌃f12`.

## Editing the keymap from Settings (M14), and what is not done

Settings → Keymap could read the keymap and nothing else. There was no write path anywhere in
the workspace — `keymap_report` was the only keymap command on the wire, and the only non-test
writer of `keymap.json` in `crates/` was a test helper. Genuinely absent, not the usual "built and
reachable from nothing".

**Every command has a row now, not every binding.** The screen used to map `report.bindings`, the
*resolved table*: 32 rows for a registry of 63 commands, so half the app was unbindable
from the screen whose job is binding — not disabled, not explained, simply absent. Rows are built
from `Bootstrap.commands` left-joined onto the bindings, in `ui/src/settings/keymapModel.ts`, which
is import-free so `check:keymap` can compile it and drive every rule.

**The file is still a diff, and that is the whole design.** An edit names a command, a context and
a key; `cide_core::keymap::apply_edit` works out the entries. Saving what the screen shows would
write every shipped binding into the user's file and freeze out every future change to a default
for anyone who had ever touched a shortcut. One gesture is routinely two entries:

```json
[{"key":"ctrl+p","command":"-picker.files"},
 {"key":"ctrl+shift+o","command":"picker.files"}]
```

…and the removal has to carry the `when` of the binding it removes, or it matches nothing and
reports `RemovalMatchedNothing` to a user looking at a screen that says the key is now free. The
`when` is therefore **copied from the row**, never re-derived, and never from `Command::when` — that
one gates the palette, and writing it into a binding would silently scope a chord the user asked to
be unconditional. Unbinding F4 produces exactly the one-liner this README prints two sections up,
`when` included; there is a test named after that.

**Restore default deletes, it never writes the default in.** Both halves of the rebind above go, and
the compiled-in binding reappears underneath. Rebinding a command back onto its own default chord
therefore empties the file rather than filling it.

**Conflicts warn; they never block.** `keymap.rs` states the policy — which of two commands should
win is a judgement only the user can make — and blocking would stop a user halfway through a
legitimate two-step edit. The box says what holds the chord, and *which kind* of collision it is:
over a **default** the new binding silently shadows it and nothing will be reported afterwards; over
another **user** entry both survive and the conflict banner will appear. It offers "bind and unbind
the other" as one atomic edit, because writing that removal by hand is the part a user cannot get
right. It also catches the two silent ones — binding `ctrl+k` when `ctrl+k ctrl+s` exists (the new
binding could never fire) and the reverse.

The preview is the one place the frontend knows something Rust does not. `normalize_key` renames no
keys by design, so `` ctrl+` `` and `ctrl+backquote` are two keys to it; `ui/src/keys/chords.ts`
folds them, and `clashesFor` compares through it. `conflicts()` still cannot see that pair.

**Recording a chord means owning the keyboard.** The key gate is a window *capture* listener, so a
dialog with its own `keydown` would never see Ctrl+P — the gate would have opened the file picker on
top of the box asking for a shortcut. So the recorder is the gate's `capture`, ranked above the
keymap, and while it is armed **every** stroke is consumed. `check:keys` sweeps the whole
modifier × key space a third time with it armed and asserts no command is ever dispatched.

What that costs, named rather than discovered: **bare Enter saves and bare Escape cancels**, so
those two spellings cannot be recorded from this screen (VS Code makes the same trade). Every
modified spelling still can, and a *bare* Enter or Escape is a binding this app must not have
anyway — the capture gate would swallow it in every text field and every terminal. A fumbled second
chord **replaces**; a two-stroke sequence is asked for with the "+ second stroke" button, so
`⌃⇧P ⌃⇧O` is never what a mistake produces. There is no live "⌃⇧…" preview while modifiers are
still held: bare modifiers never reach the capture, and the only way to get one would be a second
keydown/keyup listener beside the gate.

**Two-stroke sequences have never run in production and now can.** `defaults()` contains no
multi-stroke binding — `ctrl+k ctrl+s` appears only in a doc comment and a test fixture — so the
prefix machine has only ever been exercised by `check:keys`. It stays that way by default:
**`settings.keymap` is deliberately still unbound.** Binding it to `ctrl+k ctrl+s` would arm a
prefix on Ctrl+K globally, and Ctrl+K is readline's kill-to-end-of-line in every terminal pane in
every window. The recorder is what makes sequences reachable without cide taking that from
everybody.

**What happens to a `keymap.json` somebody wrote by hand.** Saying it plainly, because half of it is
a loss:

* **Comments were never legal and are not lost by this.** `load_user` is `serde_json::from_str` —
  strict JSON, not JSONC — so a `//` comment has always made the *whole file* fail to parse, and a
  file that fails to parse contributes **zero** overrides. That used to be a `tracing::error!`
  nobody reads; it is now a problem line on the screen that says the file is strict JSON.
* **A file that does not parse is never rewritten.** The edit command refuses and names the file.
  The one exception is **Reset all**, alone, which is the gesture that already means "throw away
  what is in there" and is the way out for someone who does not want to go and find it.
* **Formatting is lost. Entry order is kept.** The vector is re-serialised with `to_vec_pretty`, so
  indentation and the order of fields inside an entry become serde's. Order *between* entries is
  semantic — `apply_layer` walks the file in sequence and a removal only matches what is already
  accumulated — so the file is mutated in place and never rebuilt from the resolved table.
* **Unknown fields are dropped.** `{"key":…,"command":…,"note":"why I did this"}` parses today and
  the `note` is gone on the first rewrite. Nothing warns, because nothing parsed it.
* **The previous contents are kept as `keymap.json.bak`, on every write**, through the same
  `write_atomic` as the file itself. That is the answer to the two points above rather than an
  apology for them, and the screen says so under the table.
* **The mode becomes 0600** (`write_atomic` publishes by renaming a private temp file) and **a
  symlink is followed rather than replaced** — `keymap.json` symlinked into a dotfiles repository is
  how a hand-authored one usually arrives, and a plain rename would strand the repository copy.

**Every window is told.** A new `cide://keymap-changed` carries the resolved table, because
`applySnapshot` builds `{ ...current, workspace }` and keeps the old `keymap` — so
`cide://workspace-changed`, the one thing already broadcast to every window, is precisely the path
that cannot refresh a binding. The array identity matters as much as its contents: `gate.ts` caches
its indexed keymap against what `bindings()` returns.

**Not done.** None of it has been confirmed on screen; the coverage is `check:keymap`, `check:keys`,
`check:switcher` and the Rust suite, plus `./target/debug/cide-headless keymap` for the file itself.
The recorder caps a sequence at two strokes (a UI cap — the format has no limit). A command bound to
two chords in the *same* context is representable by hand and the screen shows two lines for it, but
*Restore default* on either line deletes both, because a reset matches on (command, `when`). Rust's
`conflicts()` still cannot see that `` ctrl+` `` and `ctrl+backquote` are one key; only the
pre-save preview can. There is no undo beyond `keymap.json.bak` and *Restore default*.

## Ctrl+hover and Find usages (M14), and what is not done

Hold Ctrl over an identifier and it underlines; click it and you go to the declaration — or, when
you are already **on** the declaration, you get its usages. IDEA's arrangement. `Alt+F7` and a
*Find usages* item in the code pane's context menu run the search unconditionally, from a reference
as happily as from a declaration.

**The underline is a promise about what the click will do**, so the two are one gesture and are kept
in agreement structurally rather than carefully. `ui/src/editor/codeIntel.ts::resolveWord` is the
only caller of `diagnostics_probe` in the app and both gestures go through it; both normalise the
pointer to the same word range, so both compute the same cache key and hit the same entry; both ask
`intent()` what the answer means, and `underlines()` is *derived* from `intent()` rather than being
a second list. Both handlers live in one file, `ctrlLink.ts`, on one CodeMirror extension — the
Ctrl+click that used to sit in `EditorSurface.tsx` moved there for exactly that reason. `check:editor`
counts the probe call sites and asserts the other three files never reach past it.

**The discriminator is "the definition is where I already am."** Ask `textDocument/definition` —
the request Ctrl+click already made — and compare the reply against the position asked about: same
file, caret inside the returned range, means the caret *is* the declaration. VS Code's rule verbatim.
On a reference, the common case, it is **free**: it is the one request that was already being made.
Only on a declaration is anything extra paid, and there it is a definition round trip in front of a
references search that dwarfs it.

Wrong in each direction, because it will be:

* **"Declaration" when it was a reference** needs a server to answer a non-declaration with the
  caret's own position. Neither shipped server does that short of a bug. Cost: a usages popup
  instead of a jump. One Escape, nothing moved.
* **"Reference" when it was a declaration** is real and reachable — `fn fmt` inside
  `impl Display for Foo` resolves to the *trait's* `fn fmt`, so Ctrl+click on it jumps to the trait
  rather than listing implementations. Cost: a jump nobody asked for. Which is why the jump goes on
  the Back stack, and why `navigate.usages` ships as its own bindable command that skips the
  discriminator entirely. **That command is not optional**; it is the only honest answer to "the
  guess was wrong".

**What a drag costs, measured.** `hoverPlan` is a pure function precisely so the budget can be
driven headlessly, and `check:editor` replays a synthetic drag through it with a clock. One second
of pointer motion across a line of eight identifiers — 120 samples at 8 ms — is **zero requests**;
stopping is **one**; a there-and-back drag over six identifiers is **one**, not one per identifier,
because the cache is keyed on the *word range* and not the pointer position; and a pointer that
settles on a comment, a keyword or punctuation is **zero**, because the local CodeMirror token
answers that without asking anybody. A naive `mousemove → invoke` would be 60–120 requests a second.

That budget is not politeness. `diagnostics_probe` is `spawn_blocking`, so a blocking thread is
parked per in-flight hover, and the outbound queue to a language server is `bounded(256)` and
**drops notifications** when it is full — a hover flood would drop a `didChange` and desync
rust-analyzer's copy of the buffer permanently, which is the exact failure `docSync.ts` exists to
prevent.

**One usage jumps, with no popup**, through `jumpTo` so it lands on the Back stack. Zero usages is a
sentence in the notice stack (`info`, not an error — "used nowhere" is a *result*), and it is a
different sentence from "there is no declaration under the caret": `textDocument/references` answers
`null` for the second and `[]` for the first, and collapsing them would tell a user who clicked a
keyword that their function is unused. Two or more opens the 620px card after a 150 ms grace, so a
fast answer never flashes a popup and a slow one says *"Finding usages of ‘parse’…"* with
rust-analyzer's own progress detail appended, rather than appearing empty and reading as "none".

**Escape genuinely cancels.** `Requester::cancel` releases the blocked thread *and* sends
`$/cancelRequest`, so rust-analyzer stops searching rather than merely being ignored — without the
second half, "Escape cancelled it" and "Escape stopped showing it" are indistinguishable on screen
and only one is true. A timeout now cancels too, which fixes a small pre-existing leak on the
five-second definition path.

**Nothing gates the request on a source being `Ready`.** That is the trap this repository has
already shipped once: rust-analyzer's indexing is a *sequence* of progress tokens and "nothing in
flight" is true in every gap between them — against this workspace it flapped eight times in the
first 0.9 s — which is what `READY_SETTLE` exists for. The status is used for the popup's prose and
nothing else; the timeout is the readiness answer. `check:editor` asserts the orchestrator never
reads a readiness flag.

**Ctrl+B is unchanged.** `navigate.definition` still means Go to definition, exactly, and so does the
menu item that carries its chip. Only the *mouse* gesture is the smart one. Making Ctrl+B also
switch behaviour would have left a command called "Go to definition" that sometimes does not, which
is a worse trade than one chord that does one thing.

**What is not done.**

* **No preview pane.** IDEA's Find Usages tool window has one; here it would be a second CodeMirror
  instance inside a modal, for a surface that exists to be dismissed in under two seconds. The
  preview is the row: the source line, clipped in Rust, with the occurrence marked through the same
  `splitHighlight` the search panel uses.
* **The list is capped** at 200 usages across 100 files and says so when it hits the cap. The
  alternative is reading four thousand files because somebody Ctrl+clicked `fn new`.
* **No usages panel**, and no grouping controls: the popup does not collapse groups, because a fold
  state nothing can save is a control that only ever costs a click.
* **The hover needs a focused editor to react to a Ctrl *keydown*.** `mousemove` carries the
  modifier, so the ordinary case needs no keyboard listener at all — but a pointer parked over an
  *unfocused* pane before Ctrl goes down gets its underline on the first pixel of movement instead.
  Fixing it means a window-level keydown listener beside the key gate, and this feature does not
  need one.
* **`supports_references()` is dead weight today.** Both shipped servers advertise the capability,
  so the fifteen lines that read it only ever prevent a future lie — a server built without the
  method would otherwise spend the whole twenty-second deadline and then be reported as "probably
  still indexing".
* **Read-only buffers opened outside the project** (`~/.cargo/registry`, `$GOMODCACHE`) were never
  `didOpen`ed unless a pane mounted them, so what rust-analyzer answers about them is whatever its
  own index holds. Unverified either way.
* **`Requester::cancel` is not reachable for the probe path**, only for usages. A superseded hover is
  dropped on arrival by a generation counter and the server finishes the work — which is acceptable
  because the ladder means there is almost never more than one in flight.

## Dragging rows into a folder (M13), and what is not done

Press a row, move four pixels, and the file tree picks it up — the whole selection when the row is
part of it, that row alone when it is not. A drop is a **cut and paste**, and deliberately not a new
operation: `fs_paste` in `PasteMode::Cut` is what runs, so every refusal, the collision plan and the
cross-device fallback are the ones Ctrl+X/Ctrl+V already had. There is **no `fs_move`** and no Rust
change at all in this feature.

**A collision asks the same question the paste asks.** `fs_paste_plan` runs first and writes
nothing, so a drop onto a name that is taken raises `PasteConfirm` — the same dialog, the same
*Keep both* default, the same promise that nothing has been written yet. One hazard, one dialog:
two would eventually disagree, and the one that would go wrong is the drag, which has no undo
anywhere and no keystroke to repeat.

**Every refusal is a sentence on the ghost, before the release**, because a drag that simply does
not land is indistinguishable from a broken feature. The rules are `ui/src/sidebar/treeDrag.ts`,
pure and executed by `check:tree-drag`; the pointer state machine is `useTreeDrag.ts`, which nothing
in this repo can run. A project root is not moved; a dependency source under *External Libraries* is
neither dragged nor dropped into; the *Scratches* drawer is refused as a destination although Rust
would accept it, because *New File…* and *Paste* are already refused there; a group header is a
heading, not a folder; and a folder cannot be dropped into itself or into anything under it — the
one that destroys a directory tree if it is merely unlikely rather than refused. A **scratch** is
draggable, which is how one leaves the drawer for the project.

Feedback is four channels and none of it is polish: the rows in flight dim, the destination folder
takes an accent ring, the rows *inside* it take a tint (it is usually scrolled off the top by the
time the pointer is over its files), and a ghost under the pointer names the load and the verdict.
Folders spring open after 600 ms — except under a refused drop, which is what stops resting the
pointer over *External Libraries* from launching `cargo metadata`.

**Divergences and what is not done.** Pointer events, not HTML5 drag and drop, for the three
reasons in `useTreeDrag.ts`'s header — so there is **no drag into cide from Dolphin or Nautilus and
none out**, the same missing desktop-clipboard flavours the tree's Copy already documents. Dropping
on empty space is refused rather than meaning "the project root", because a pointer released over
the editor resolves to no row either. Nothing retargets an **open editor tab** when its file moves —
`fs_rename` and Cut+Paste have always had that gap and this does not widen it. And none of it has
been confirmed on screen: the gesture needs a pointer, KDE will not raise a shell-launched window,
and the Wayland compositor exposes no capture protocol.

## Scratch files and Select opened file (M13), and what is not done

**Scratch files** (⇧⌥S, *New scratch file…*) are buffers that are not part of the project. They
live in `$XDG_STATE_HOME/cide/scratches/<blake3 of the project's canonical first root>/`, flat, one
drawer per project — the same keying `cide-git` uses for changelists and the shelf, and for the
same reasons. Not inside the project, because the walk is gitignore-aware and a scratch in the
working tree would either show up in `git status` or be hidden from cide's own file tree by the
user's own ignore file. A sibling `<key>.json` records which project a drawer belongs to, so an
orphan left by a moved project is findable by hand; **there is no garbage collection**, exactly as
for `repos/`.

Choosing the type first is what makes the file `scratch.rs` rather than an untitled buffer, and the
extension is the whole of how the editor decides the language. The offered list is
`SCRATCH_TYPES` in `ui/src/editor/languages.ts`, beside the table it must agree with; `check:editor`
asserts every offered extension resolves through that same table to that same label, so a type
cannot be offered that opens with no highlighting.

Scratches are a second synthetic group in the file tree, under *External Libraries*, and the only
one cide writes into: `ProjectFs::writable_paths()` is the roots **plus** the drawer, and it is
what the mutating `fs_*` handlers check against instead of the roots alone. So a scratch is saved,
renamed (which changes its language), cut, pasted and trashed like any other file. Nothing watches
the drawer — the group is re-listed by whatever changed it, which is also what emits the
`cide://fs-status` a second window redraws on.

**Select opened file** (⌃⇧E, and the ⌖ button in the Explorer header) scrolls the tree to the file
the active tab is about and selects it, for a project file, for a scratch, and for a dependency
source opened by Go to definition. That last case is the one that would otherwise be missing: the
*External Libraries* group holds no packages until somebody expands it, so `fs_reveal` resolves it
inline — once per project per process, gated on an I/O-free "is this path in a dependency cache"
test — rather than telling the user that a file they are looking at is not in the tree.

That gate is only as wide as `dependency_roots()`, which is how it kept saying exactly that for
**standard-library** files until M15: the rustup sysroot was in no root, no cache and no group. See
*Three things that shipped and did not work* above.

**Divergences and what is not done.** ⌃⇧E is *Recent Locations* in IDEA, where Select Opened File
has no default chord at all; cide has no Recent Locations, so nothing is lost, but the chord will
surprise somebody. ⇧⌥S costs every terminal pane the `ESC S` byte pair, unconditionally and in
every window (rebindable in one line of `keymap.json`). Scratches are **not** in Ctrl+P: the picker's
candidates come from the walk, and groups are not walked. *New File…* and *Paste* are refused
**inside** the drawer, deliberately — it is not a folder anybody organises, and the menu label would
be a blake3. A scratch renamed or deleted from one window reaches a second window's tree at once;
one *pasted* out of the drawer reaches it on that window's next refresh, because `fs_paste` has no
`AppHandle` to emit from. And none of this has been confirmed on screen — see the note under
*External Libraries* about the audits; the coverage is `cide_app::cmd::fs::scratch_tests`, which
drives the real command layer.

## External Libraries (M13), and what is not done

The file tree grows a second row source. `cide_fs::groups` is a **mechanism**, not a feature: a
group is a header row plus a lazily materialised subtree, drawn after the walked index's rows and
composed at the command layer (`cmd::fs::compose`). *External Libraries* is the first group;
Scratches is meant to be the second, and needs nothing new from that crate.

**Nothing happens when a project opens.** The first tree read runs a `has_marker` probe — a handful
of `stat`s to depth two — which decides only whether the header row exists. Resolution happens when
the user expands it: `cargo metadata --format-version 1 --frozen --manifest-path <m>` or
`go list -m -e -json all`, on a `cide-deps` thread of its own, with the group already showing
*Resolving dependencies…*. `--frozen` is the only cargo flag combination that resolves the full
graph **without writing `Cargo.lock`** — `--offline` was measured creating one — and go gets
`GOFLAGS=-mod=readonly` plus `GOPROXY=off`. Nothing under `~/.cargo/registry` or `$GOMODCACHE` is
ever walked, watched, indexed or offered to Ctrl+P.

**A group is never silently empty.** Every path ends in package rows or in a `Note` row the user
can read: `cargo` is not on PATH (with the install command), the lockfile is out of date (cargo's
own sentence, verbatim), a module go could not load (`-e` gives that per row), or *No external
dependencies*.

**The toolchain's own library is a row too (M15).** `Rust  1.92.0-x86_64-unknown-linux-gnu` and
`Go  go1.25.5` sort above the alphabetised crates, pointing at `<sysroot>/lib/rustlib/src/rust/
library` and `$GOROOT/src`. It is a `Package` with an `sdk` flag rather than a third group — a
group would need an id, a probe, state fields, a `STEMS` entry, an icon and a `view.*` command with
a dispatch case *or it is unreachable*, and IDEA puts its SDK node inside External Libraries too.
Neither resolver can report it (`cargo metadata` describes the lockfile graph; the Go standard
library is not a module), so it comes from `rustc --print sysroot` / `go env GOROOT` on the
resolution thread, independent of whether the dependency resolution itself succeeded — a stale
lockfile still gets a browsable `std` beside the sentence explaining its missing crates. **A
missing `rust-src` is an ordinary state, not an error**: the row stays, and says
`rustup component add rust-src`. `rust-toolchain.toml` joins the stamp, so editing the pin
re-resolves.

**Dependency sources are read-only, and that fixed a live bug.** Cargo unpacks a crate mode 644, so
before M13 a Go-to-definition into `serde` gave an editable buffer whose Ctrl+S wrote into the copy
every project on the machine builds against. `cide_core::toolchain::read_only_reason` now clears
`FileDoc::writable` for anything under a toolchain's dependency cache and `file_write` refuses it a
second time — unless the user opened that directory as a project root, which overrides. **M15 found
the same bug still live one directory over**: `~/.rustup/toolchains/…/library/core/src/option.rs` is
also mode 644 and user-owned, and the rustup tree was simply never enumerated, so a Ctrl+S in a
`core` buffer wrote into the toolchain `rustup update` then silently replaces. That directory is now
a root in full — sources, `bin/`, `lib/`, everything, since nothing under a rustup toolchain should
be written by an editor. `$GOROOT` joins it when it is set, which it usually is not; Go's std
therefore stays writable on most machines, and that is stated rather than hidden.

**Not done.** *Reveal in File Manager* is disabled for a row outside the project rather than
relaxing `fs_show_in_manager`'s containment check; the resolver has no timeout (`--frozen` and
`GOPROXY=off` make the network impossible, so the only unbounded wait left is cargo's own
package-cache lock); a project that gains its first `Cargo.toml` while open does not grow the group
until the next launch; and there is no default key binding — the palette's *Show External
Libraries* (`view.externalLibraries`) is the keyboard route — as is *Show Scratches*
(`view.scratches`) for the group below it, which is otherwise the last row of a virtualized tree.

## Language support (M12), and what is not done

Rust and Go get a tree-sitter symbol layer (`cide-lang`) and a language-server client
(`cide-lsp`). The honest state, because half of this is data with no surface on top of it yet:

**Works, and is checked.** `Ctrl+F12` (File Structure popup), `Ctrl+Alt+Shift+N` (Go to Symbol in
project), `Alt+Up`/`Alt+Down` (previous/next member), `Ctrl+G` (Go to line), per-file position
memory, the `getDiagnostics` MCP tool answering from a real store,
and the whole Rust pipeline underneath: extraction for both languages, a parallel project walk, the
merged diagnostic store, the LSP codec/session/supervisor.

**Keys into the editor, and what `Ctrl+G` cost.** Undo/redo (`Ctrl+Z`, `Ctrl+Y`, `Ctrl+Shift+Z`) is
CodeMirror's `historyKeymap` and is deliberately absent from `cide-core::keymap` — a binding there
is consumed by the key gate's *window capture* listener before any text surface sees it, and in a
terminal `Ctrl+Z` is SIGTSTP. Two tests keep it out. The find bar focuses its field on `Ctrl+F` and
bridges `F3`/`Shift+F3`/`Ctrl+G` into `search-panel` scope, so the keys work with the caret in the
box as well as in the buffer; the counter reads `3 of 12`, which is the only signal that the search
wrapped. `Ctrl+G` is Go to line, which **takes find-next away from that chord inside an editor** —
`F3` is find-next now, `Ctrl+Shift+G` is still find-previous, and one line of `keymap.json` —
`{"key":"ctrl+g","command":"-navigate.line","when":"editorFocused"}` — gives CodeMirror's `Mod-g`
back. The `when` is not optional in that line and the test named after it says why.

**Verified against real servers.** `cargo test -p cide-lsp -- --ignored` drives the real binaries;
**CI does not run it**, so run it by hand after touching that crate. All seven pass — `gopls`
reporting on a module, rust-analyzer indexing this workspace and reporting an introduced type
error, the shutdown ladder actually stopping a server, the on-disk-edit test below, a real
`textDocument/definition` resolving a reference to its declaration, and (M14) a real
`textDocument/references` listing the call site of a declaration and *not* the declaration itself.

Verified in the app, too, once: with a type error planted in `cide-core` *after* the build, a
launched binary logged `published … child_env.rs n=2 "mismatched types: expected u32, found &str"`
alongside an empty publish for the open editor tab — the full chain from spawn to store, and the
empty one is the `[]`-means-looked case the panel is built on.

A rustup caveat worth knowing, because it is not a cide bug and reads exactly like one:
`~/.cargo/bin/rust-analyzer` is a symlink to `rustup`, so it passes any "on PATH and executable"
probe and then fails at exec if the component is missing for the toolchain that project resolves
to. A repo pinned by `rust-toolchain.toml` gets the pinned one; a repo with no pin gets the default,
which may not have the component. `start_failure_reason` recognises the shim's own wording and the
panel says `rustup component add rust-analyzer` rather than reporting a crash.

Running them is what found the bug they now guard against. `Ready` was computed as "handshake done
and no progress token in flight", which is right for a server that indexes in one pass and wrong
for rust-analyzer, whose indexing is a *sequence* of tokens — so in the gaps between them the
source reported **Ready with an empty list**, i.e. a clean bill of health, eight times in the first
0.9 seconds against a workspace it had not read yet. The first version of the test passed anyway,
because it only asserted that `Scanning` appeared before `Ready` and `Session::new` makes that
trivially true. `READY_SETTLE` holds a `Ready` for a second before believing it, and the test now
asserts that no `Ready` is ever followed by a `Scanning`.

**Two bugs that a fully green gate did not see**, both found by running the thing and reading the
log rather than by any check, and both with the same shape — a feature that was built, registered,
reachable and never called:

- **No language server started on a normal launch.** `DiagnosticsRegistry::ensure` was called only
  from `project_open`, which a *restoring* launch never reaches, because the projects arrive from
  `workspace.json`. `lib.rs` already carried a paragraph about this exact omission costing the IDE
  servers their headline feature "on every launch but the first"; the language servers repeated it
  a hundred lines below. The symptom was an `unavailable` panel, `✗ — ⚠ —`, and `getDiagnostics`
  answering `[]` — each one indistinguishable from a clean workspace.
- **`getDiagnostics` was never wired to a store.** `IdeServers::ensure` set the source only if the
  diagnostics registry already held the project, guarded by a comment calling the order a race that
  "degrades to honesty". It was not a race: `project_open` calls the IDE `ensure` first, every
  time, so the guard was always false. The link is now made from both ends (`ide::link_diagnostics`)
  so the order genuinely does not matter.

**Document sync**, and why it is not optional. `didOpen`/`didChange`/`didSave`/`didClose` existed as
commands, were registered, and were exported in `client.ts` — with no caller. `cide-lsp`'s
`an_on_disk_edit_alone_never_refreshes_diagnostics` measures what that cost: flycheck runs once when
the workspace finishes loading, so a project **opens** with a correct list and looks fine, but
repairing a file on disk and telling the server nothing changes nothing, for as long as you care to
wait. The panel froze at the state the project opened in while the user edited underneath it, which
is worse than showing nothing. `ui/src/editor/docSync.ts` now sends all four, refcounted by path so
a split is one open document, and flushes the pending change *before* `didSave` so the server never
checks text from 300 ms ago.

**On screen and fed.** The Problems panel renders a live snapshot, the status bar's counts and the
⚑ rail badge are derived from that *same* snapshot (so the three cannot disagree about whether the
workspace is clean), and Settings ▸ Inspections drives all of it. The status-bar trail now carries
the caret's `mod › impl › fn` chain after the path.

**In the editor.** Squiggles and a gutter column, via `@codemirror/lint` — `text-decoration` in
theme tokens rather than the library's stock inline-SVG data URIs, which bake `#d11` and `orange`
and cannot read a custom property. The gutter glyphs are the panel's own `✗ ⚠ ℹ ·`, so margin and
panel share one alphabet. The per-editor highlighting level is three checked items in the code
pane's context menu; it is session-scoped over a persisted default, deliberately — a `none` set
three weeks ago and silently restored is a user concluding their language server is broken.

Go has a real grammar now (`func`, `chan`, `defer`, `select`, raw strings, rune literals,
non-nesting block comments) instead of borrowing Java's keyword table through `clike`. The outline
re-parses 300 ms after you stop typing, so the breadcrumb, the popup and the member walk follow the
buffer rather than the last save.

**Position memory: a file reopens where you left it.** Per file, cide remembers the caret and the
first visible **line** — lines and not pixels, because wrapping is on for every file under the 1 MB
limit and the code font size is a runtime setting, so a pixel offset is wrong after a window resize
or a font change rather than merely imprecise. It survives a tab switch, a project switch, a reload
from disk when the agent edits the file you are reading, a close-and-reopen, and a relaunch.

It is a separate store from `workspace.json` for the same reason `recent.json` is: the record you
most need is the one closing the tab has just removed from there. **A scroll gesture is not a
workspace mutation** and must never become one — `WorkspaceState::update` clones the tree twice,
re-validates it and broadcasts it to every window, per accepted change. The ladder is four rungs,
each an order of magnitude cheaper than the next: a `ViewPlugin` coalesces to one animation frame,
`EditorPane` trailing-debounces to one IPC call per 500 ms, `positions_state.rs` debounces to one
atomic write per 2 s, and `lifecycle::shutdown` flushes. `$XDG_STATE_HOME/cide/positions.json`, 0600,
256 files, LRU, **no TTL** — an expiry means the file you come back to on Monday opens at line 1,
which is the complaint. An explicit navigation always outranks a remembered position: `planRestore`
refuses while a reveal is parked for that path, so Go to definition into a file you had scrolled
lands on the definition.

**Mouse back / forward.** The thumb buttons walk a per-project navigation history. **It has to come
from Rust**, and that is not a preference: WebKitGTK's `buttonForEvent` maps GDK buttons 1–3 and
leaves the rest at `WebMouseEventButton::None`, which `MouseEvent`'s constructor reports as
`button === 0` — so both thumb buttons arrive in the DOM as an ordinary left click and cannot be
told apart there at all. A GTK handler on the `WebKitWebView` widget reads the raw button and emits
`cide://mouse-nav` to that one window. It also **fixes a phantom click that was already live**: a
Ctrl+thumb-press over a path in terminal output used to open the file, and over a buffer used to
fire Go to definition.

The handler names a *button*, never a command. `mouseback`/`mouseforward` are ordinary entries in
`cide-core::keymap`, resolved by the key gate's **third entry point** over the same `resolveStroke`
the other two use — so they get `when` clauses, composed modifiers, and rebinding for free:
`{"key":"mouseback","command":"-navigate.back"}` unbinds. `navigate.back` / `navigate.forward` are
in the palette and are **unbound to any key** by default; `ctrl+alt+shift+left` / `right` are free
in every layer if you want them:

```json
{"key":"ctrl+alt+shift+left","command":"navigate.back"}
{"key":"ctrl+alt+shift+right","command":"navigate.forward"}
```

IDEA's own `Ctrl+Alt+Left/Right` is not free here — `ctrl+alt+right` is `pane.split.right`, and on
KDE both are usually the compositor's virtual-desktop shortcuts — and `alt+left`/`alt+right` are
readline word-motion the window capture listener would take from every terminal.

**Only an explicit navigation is recorded**, and the list of what deliberately is *not* is in
`ui/src/editor/navHistory.ts`: typing and arrow keys, scrolling, find-as-you-type, the
`Alt+Up`/`Alt+Down` member walk (a held key, so ten presses would be ten entries), switching between
already-open tabs, edits, and a Back/Forward move itself. A history that records caret moves is what
makes Back useless.

**Unmet, and written here rather than left to be found.** The navigation history is **per JavaScript
realm and session-scoped**: a detached-pane window keeps its own (always empty, since such a window
never renders an editor) and refuses Back with a sentence rather than walking the shell window's
stack; it does not survive a window reload or a relaunch. Making it durable means a Rust-owned
history and an event per jump to every window, which is not worth it for gesture memory. Also
unmet: `WorkspaceState::flush_if_due` documents itself as "called from the app's tick and before
quitting" and **is called from neither** — there is no app tick — so `workspace.json` is in fact
written exactly once per run, at shutdown, and its 500 ms debounce is inert. The position store does
not inherit that: it runs its own flusher thread rather than assuming a tick exists.

**A context menu no longer scrolls the buffer to the top.** Right-click in an editor, move the
pointer over the menu, dismiss it, and the buffer used to jump to line 1 — but only if the pointer
touched the menu. Every step is WebKit's: a right-click leaves `.cm-content` focused, hovering an
item moves focus to a `<button>`, `FocusController::setFocusedElement` clears the document selection
on *every* focus change, and the `previous.focus()` on the way out then runs
`Element::updateFocusAppearance` on a root editable element whose frame has no selection — which
invents one at the start of the element and *reveals* it. Without the hover, focus never left, so
`Element::focus` took its refocus early return and nothing happened. `preventScroll` fixes the
scroll for every surface; the editor additionally supplies a `restoreFocus` calling
`EditorView.focus()`, because the `setSelection` runs before the reveal and `preventScroll` does not
suppress it — the caret would still be collapsed to the top and read back into state.

Two more found in the same twenty lines. The menu's `box.focus()` was a **no-op** — it ran in the
same commit as the measure effect, before React flushed `setPlacement`, so the box was still
`visibility: hidden` and WebKit refuses focus on such an element. Escape, the arrows, Home/End, Tab
and Enter were therefore reachable only after the pointer touched an item, and a keyboard-invoked
menu (Shift+F10, the Menu key) could not be driven at all. And the `scroll` capture listener that
dismisses the menu did not exempt the menu's own scroller, so arrowing past the fold of a long menu
would have closed it.

**Still not built.** "Fix with Claude" on a problem row, and Claude-authored inspections — the
`claude` source toggle exists and nothing produces findings for it. A row action needs the
host/pure split `GitPanelHost` uses, so the panel's SSR smoke test keeps working. Breadcrumbs are
drawn but **not clickable**: `StatusBar` receives a flat list and does not know where the path ends
and the symbols begin.

**Go to definition works, and it resolves rather than guesses.** `Ctrl+B`, `Ctrl+Click`, or the code
pane's context menu. It asks the language server `textDocument/definition` and does nothing else —
in particular it does **not** fall back to the symbol index, because jumping to whichever of the
eleven `fn new` in a workspace shares the identifier's spelling is right about one time in eleven,
and a confident wrong answer is worse than the disabled item it replaces. With no server running it
says which server is missing.

That meant building a request/response path in `cide-lsp`, which until now was strictly one-way:
notifications out, `LspEvent`s back. `Session::on_message` returns an empty effect vec for any
response whose id is not `initialize_id`, so a reply was parsed and dropped with no log line
anywhere. Replies are now intercepted in the supervisor pump *before* `Session` sees them and handed
to a per-request channel — not a new `LspEvent`, because `drain()` has one consumer and a command
reaching in to find its own reply would swallow the `Published` events the diagnostics pump needed.
Request ids start at 100: `initialize` is always 1 and `shutdown` always 2, **per life of the
server**, so a naive counter would eventually have a caller's request resolved by a handshake reply.
Ten unit tests cover the correlation, and
`cargo test -p cide-lsp -- --ignored` proves it against a real rust-analyzer.

**The click modifiers changed to match IDEA.** Ctrl+Click was CodeMirror's default multi-cursor
modifier on Linux (`clickAddsSelectionRange` is `browser.mac ? metaKey : ctrlKey`); it is now Go to
Definition, and adding a caret moved to **Alt+Click**. One facet override does both, because
`rectangularSelection` already claims Alt and its style consults that same facet to decide whether
to add or replace. The honest divergence: Alt+**drag** now adds its rectangle to the selection
rather than replacing it, where IDEA replaces.

An adversarial review of this work confirmed thirteen defects, nine in the new code, and every one
of them was invisible to the gate that had just gone green. Three were the same shape as the bugs
above — cancellation that read as complete and was not (`supervise` cancelled waiters *around* the
`run_once` call, so the stop-check that returns before it left a caller to sit out its five-second
deadline and then be told "still indexing" about a server that had been deliberately stopped); a
restart backoff that popped a queued request off the outbox and discarded it, cancelling nothing,
while also letting a single `didChange` skip the rest of the backoff and turn a crash loop into a
respawn storm; and a `.catch(() => {})` on `file.open` whose comment claimed `Failures` would show
the error anyway, when catching it is precisely what stops `unhandledrejection` from firing.

Two more were in the highlighting-level axis and predate this feature. `App.tsx` was applying the
per-editor level to the Problems panel, the status bar and the rail badge, so a default of `None`
produced `✗ 0 ⚠ 0` over a workspace full of errors — the confident zero this whole surface exists to
prevent, arriving through the one axis the design says must never reach it. And `EditorPane` read
`levelFor(path, 'all')` with the fallback hardcoded, which made the setting inert for every editor,
the one surface it is defined for. Both are fixed; the level is now applied exactly once, to the
buffer it belongs to.

**Still not wired**, and named here rather than discovered: `clearLevel`, `overrideFor` and
`reducedCount` in `highlightLevel.ts` are documented as feeding the context menu and the panel's
detail line, and are called only by the check script. There is no "reset this file to the default"
affordance and the panel does not say when an editor is showing less than it holds.

**Known limits of what does work.** No 100k-file repository has been indexed — the caps in
`cide_lang::Limits` are reasoned, not measured. Go to definition has three of its own, all
deliberate: a definition in a file no pane has open resolves against **on-disk** text, because
`didOpen` is only sent for buffers an editor mounted; a reveal is delivered to *every* editor
showing that path, so a split showing one file twice moves both carets; and a reveal requested from
a detached pane does not cross into the shell window, so a cross-window jump into a closed file
opens it at line 1. The lookup gives up after five seconds and says the server is still indexing —
which, for the first minute of a session on a large workspace, is the truthful answer.


## Verifying the Claude Code CLI

The IDE integration is reverse-engineered from a surface that is undocumented, unversioned and
self-updating. There is no protocol-generation field anywhere in the CLI, so the only question
that can be answered is not "which protocol is this" but **"is this a version anybody checked"**.

```sh
cargo xtask verify-cli              # rebuild, then check the installed CLI
cargo xtask verify-cli --no-build   # reuse what is already compiled
```

It preflights before spending anything. `claude` must be on `PATH`, and so must `script(1)`:
the CLI opens an IDE connection only from its interactive UI — a `-p` run opens no socket at
all — so the check needs a pty, and `script` is how it gets one. The binary it chose and what
`claude --version` says are printed *before* the run, so a hang has a version attached to it in
the scrollback.

Then one test: a real `claude` under a pty, which must find our lockfile, choose the WebSocket
transport, send the `x-claude-code-ide-authorization` header and complete the MCP handshake.
**It types no prompt.** No model is called and nothing is billed, which is what lets this be a
hard failure rather than a skip — the expensive test beside it,
`the_real_cli_round_trips_a_diff_three_ways`, spends a model turn and skips on anything it does
not recognise, and a skip-shaped version check is worth nothing.

**The handshake is checked before the version, and that order is load-bearing.** A green version
check on a build whose protocol had already broken would be a confident lie.

|                      | version in the record                  | version past it                                              |
| -------------------- | -------------------------------------- | ------------------------------------------------------------ |
| **handshake OK**     | pass                                   | fail — append the version, with this run as its evidence      |
| **handshake broken** | fail loudest — the record is a lie      | fail — real drift: read the transcript, fix `protocol.rs`, *then* append |

To record a new version, append it to `SUPPORTED_CLI` in `crates/cide-ide-mcp/src/protocol.rs`
and put the run's output in the module comment above it. That comment is the evidence trail, and
it is worth reading before assuming a release is harmless: 2.1.227 changed nothing in the IDE
protocol but *did* begin rejecting `--resume <id> --session-id <new>`, which broke resume until
`cide_claude::session` was corrected.

The record is **never enforced**. Refusing to run outside the range would break the app roughly
every fortnight to protect a feature that mostly keeps working across releases, so the strongest
thing here is a warning: a protocol change should degrade the diff view, never break the terminal.

### What a user sees

A green test says nothing about *someone else's* machine, and a test that wrote into the app's
settings would be recording a claim about a developer's laptop into a file a user then reads. So
the two halves are separate. At runtime the app writes whichever `claude` last completed a real
IDE handshake **on this machine** to `claude-handshake.json` in the state directory, and Settings
shows it with a badge saying whether that version is one this build was verified against.

Neither half reads `~/.claude/.credentials.json` and neither sets `ANTHROPIC_API_KEY`. The child
authenticates by inheriting the environment; that is the only supported path, and injecting a key
would silently bill a Console org for a user on a subscription.

## Quitting and coming back

cide runs **no background daemon**: quitting quits its Claude sessions. What survives is the
workspace — windows, tabs, splits, detached panes — and the conversations, which resume
because a `SessionId` *is* the value passed to `claude --session-id`.

Shutdown is a ladder rather than a kill: SIGHUP, then SIGTERM, then SIGKILL. A `claude` asked
to stop politely finishes writing its transcript; killed outright it may leave a conversation
unresumable, which is exactly what the restore half depends on. Signals are caught through a
self-pipe, because taking the workspace lock inside a signal handler deadlocks whenever the
interrupted thread already held it.

On relaunch, `app_restore_plan` marks each pane `Resumable` or `Fresh`, and exactly one pane
per project `eager` — its primary session. Every other Claude pane shows a resume splash and
spawns when asked, so reopening a six-pane project does not silently start six agents.

## Opening a file a pane printed, including one outside the project

Ctrl+click a path in any terminal pane and it opens as a tab, at the line and column the
producer named. A **directory** is shown in the file tree instead (M15) — the same `file.reveal`
Ctrl+Shift+E runs, so the sidebar comes to Files first and a path with no row says so. The bytes a
pane prints are attacker-influenced by definition — a build log, a tool result, an agent's
transcript — so `terminal_open_path` is the one command in the app whose path argument is
untrusted, and it is the only route from a pane to the tab list.

**The press is cide's the moment it happens**, before anything is known about what is under it.
That is not a detail: until M15 the gate waited for a completed hover, and in a Claude pane — where
the alt-screen TUI repaints under a stationary pointer and no `mousemove` fires — it usually never
came, so the press reached xterm, xterm wrote a mouse report to the pty, and `claude` answered a
ctrl+click by forking a desktop file manager. cide also answers XTVERSION now, truthfully, which is
how the CLI knows to stand down from ctrl+click and alt+click on an xterm.js host. Every claimed
press ends in an open, a reveal, or a sentence; none ends in silence.

Four guards sit on that path and they answer four different questions. Only one of them is about
the project boundary:

| guard | concern | can the user overrule it? |
| --- | --- | --- |
| absolute, no `..` | integrity — a path that resolves differently depending on who resolves it | no |
| inside a root (textually, then again after `canonicalize`) | confidentiality — any readable file on the machine | **yes, per click, by name** |
| a regular file | liveness — a FIFO parks a blocking-pool worker in `read_to_end` for ever; `/dev/zero` reports length 0 and is read until the process is OOM-killed | no |
| under the editor's size limit | junk — a 2 GB log becomes a tab and then an error | no |

The second one is a **question**, not a verdict: the refusal comes back carrying the canonical
path, a confirmation names it in full — and names the symlink target too, when the link resolves
somewhere else — and the answer that goes back to Rust is *that path*, not a boolean, so an
approval is an approval of a file rather than of a string. There is no "don't ask again": one
approval becoming a standing capability for every later line naming a sibling file is the thing
the gate exists to prevent, and the second line is written by whoever wrote the first.

A path is only offered as a link if it is really there. In-project candidates are answered from
the file index at no syscall cost; out-of-project ones get a real `stat` through `fs_stat_paths`,
because a linker error names `/usr/bin/ld`, `/dev/null` and half a dozen `.so`s, and underlining
all of them is how an underline stops meaning "cide can open this". **A relative path never
becomes an out-of-project candidate** — resolving `../../.ssh/id_rsa` against the pane's cwd
would produce a real private key from six characters on screen — so the property the
confirmation rests on holds: the full path was printed, and the user could read it before
clicking.

What such a tab then does: the status bar shows the whole absolute path (`pathTrail` falls back
to it), Ctrl+F12 and the member walk work, save works, and *Reveal in Files* says there is no row
rather than doing nothing. Diagnostics are the ragged edge — an out-of-project `.rs` is still
routed to the project's rust-analyzer by extension, which answers "file not included in module
tree" or nothing at all.

## The pane audit

The host registry is the one piece of this application that cannot be verified by reading
it: its whole job is that a terminal's DOM survives operations that would ordinarily
destroy it, and nothing about that shows up until a pane has been moved a few dozen times.

```
CIDE_AUDIT_PANES=1 ./target/debug/cide
```

It opens this repo plus a second Claude tab, then runs 100 cycles of split, close, focus,
maximize and tab-switch against the **real** Rust-owned tree — not a fake — asserting after
every cycle that `term.open()` has run at most once per pane, that no host was destroyed
while mounted, and that no terminal lost its element. A failure names the cycle and the pane.

Current result: `PASS — 103 panes, no terminal re-opened beyond its eviction allowance, no
host destroyed while mounted`.

## The chrome audit

M3's stated acceptance criterion is a screenshot diff against the design mock at 1440x900 in
both themes. Neither side of that diff is obtainable here — the mock is a template needing a
runtime this repo does not have, and KDE Wayland will not raise a shell-launched window for a
capture tool. What survives is the geometry, so the check became data:

```
CIDE_AUDIT=1 ./target/debug/cide
```

It resizes to a viewport of exactly 1440x900 (correcting for the compositor's invisible
window border, which is 52px on this desktop), renders a fixture covering every chrome state
a live app would not show on its own, and measures 48 dimensions against the mock's stated
values in each theme. An element that is missing reports as a **failure**, not a skip — a
check that silently disappears is indistinguishable from one that passes.

Current result: `PASS — 48 dimensions within ±2px of the mock` in both dark and light, every
delta exactly zero, with both self-hosted fonts confirmed loaded.

The half that is not automated is colour and glyph fidelity, which still wants a human eye.

## Layout

```
crates/
  cide-app/        THE ONLY crate that may depend on tauri. Glue by policy.
  cide-ipc/        Wire DTOs. serde + ts-rs. Zero logic, no tauri.
  cide-core/       The domain: workspace tree, settings, keymap, persistence.
  cide-pty/        PTY sessions: spawn, coalescing, backpressure, vt100 mirror.
  cide-claude/     Spawning and supervising `claude`: env, hooks, resume/fork.   (M7)
  cide-ide-mcp/    The Claude Code IDE-integration MCP server.                   (M6)
  cide-git/        Multi-root git, hunk/line staging, changelists, shelf.        (M10)
  cide-fs/         Gitignore-aware indexing and watching.                        (M8)
  cide-search/     Fuzzy pickers behind a Matcher trait.                         (M8)
  cide-lang/       tree-sitter: what a Rust or Go file declares.                 (M12)
  cide-lsp/        An LSP *client*: rust-analyzer and gopls.                     (M12)
  cide-deps/       What a project depends on, and where its source is unpacked.   (M13)
  cide-hook/       Second binary: bridges a Claude hook to the running IDE.      (M7)
  cide-headless/   Third binary: proves the core links without tauri.
ui/                React 19 + Vite 8 frontend. One document per window.
docs/adr/          Decisions that would otherwise be refactored away.
```

`cide-headless` is load-bearing architecture, not a demo: it links `cide-core`, `cide-ipc`
and `cide-pty` and must never be able to link `tauri`. If domain logic leaks into the app
crate, it stops building — cheaper than a code-review convention.

## Three decisions that shape everything

**One webview per OS window; panes are DOM.** Tauri's `unstable` multiwebview is the
obvious way to build a pane grid and is functionally broken on Linux: `tauri-runtime-wry`
packs child webviews into a `GtkBox`, and wry only honours `set_bounds` for `GtkFixed`
parents or X11 child windows. Splitter drags would be silent no-ops that still return
`Ok(())`. See ADR 0001.

**Sessions are owned by the Rust core, not by any window, tab or pane.** Panes hold a
`SessionId` and merely *attach*; closing one detaches a sink, it never kills a child. That
single decision is what makes "detach a pane into its own window", "sessions survive a
window closing", and the stack-projects-in-header-vs-one-window-per-project setting all
fall out as the same mechanism. Sinks are a *list*, so detach is gapless — the new window's
sink is live before the old one drops, and not a byte falls between them.

**cide registers as a real Claude Code IDE.** It serves the IDE-integration MCP server
(`openDiff`, `getDiagnostics`, `openFile`, `close_tab`) and sends `selection_changed` and
`at_mentioned`, so Claude's edits arrive as diffs in cide's own editor rather than as ASCII
in a terminal. That surface is undocumented and unversioned, so it lives behind one adapter
with a pinned known-good CLI range and degrades to plain-PTY-only if the handshake fails.

cide never reads `~/.claude/.credentials.json` and never injects `ANTHROPIC_API_KEY` — that
variable outranks subscription OAuth and would silently bill a Console org. Children inherit
their auth by inheriting the environment.
