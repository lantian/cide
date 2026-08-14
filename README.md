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
anywhere: `Tab` has no timestamp, `p.tabs` is insertion order, and `close_tab` picks the *left
neighbour*. `ui/src/store/workspace.ts` now derives one per project from every
`cide://workspace-changed` snapshot, over `Project::active_tab`, and mirrors it to
`localStorage` under `cide.tabMru` beside the project stack. It is *not* in Rust, and the honest
version of that argument is in the code: a keystroke that bumped `rev` would repaint every other
window, and it would be a schema migration — the domain-purity argument is weaker here than it is
for projects, because `active_tab` is already workspace state. The list contains **tabs**, console
and settings included, which is the point: one Ctrl+Tab from a file gets you back to the
conversation about it.

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

**Dependency sources are read-only, and that fixed a live bug.** Cargo unpacks a crate mode 644, so
before M13 a Go-to-definition into `serde` gave an editable buffer whose Ctrl+S wrote into the copy
every project on the machine builds against. `cide_core::toolchain::read_only_reason` now clears
`FileDoc::writable` for anything under a toolchain's dependency cache and `file_write` refuses it a
second time — unless the user opened that directory as a project root, which overrides.

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
memory, the mouse's back/forward buttons, the `getDiagnostics` MCP tool answering from a real store,
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
producer named. The bytes a pane prints are attacker-influenced by definition — a build log, a
tool result, an agent's transcript — so `terminal_open_path` is the one command in the app whose
path argument is untrusted, and it is the only route from a pane to the tab list.

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
