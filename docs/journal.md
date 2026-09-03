# The journal

cide was built in milestones, and each one was written up here as it landed. This file is that
record, in order, moved out of `README.md` when the README became a landing page. Nothing has
been edited: heading levels, wording and the "what is not done" / "what is not verified"
paragraphs are as they were written.

Two things follow from that, and both are the point of keeping it:

- **A statement here was true when it was written.** Some have been overtaken — a limit that was
  later lifted, a default that later changed. Where a later milestone corrected an earlier one,
  the earlier paragraph is left standing and the correction says so, because the mistake is
  usually the more useful half.
- **The unverified parts are named.** Most sections end by saying which acceptance criteria were
  never confirmed on a display. Read that before assuming a feature works end to end.

For the current shape of the project see [`../README.md`](../README.md); for what does and does
not exist off Linux see [`platforms.md`](platforms.md); for the decisions a refactor would
otherwise undo see [`adr/`](adr/).

## View commits after a pull (M37), and what is not verified

The pull's notice — *Fast-forwarded main 7 commits from origin · 12 files +230 −41*, ten commits
under *Details* — ended there. IDEA's *Update Project* notification carries a *View Commits* link
that opens the log filtered to exactly what was received, and that was the request.

### The range is the toast's list, not `old..new`

`FetchOutcome` gains `received`: the revspec `<pre-pull local tip>..<upstream tip>` over full oids,
spelled in Rust by the function that fills `commits`, because it is the same walk. Not
`old_oid..new_oid`: after a merge that range includes the merge commit cide has just made, after a
rebase it includes the user's own replayed commits, and neither was received. Full oids because the
log resolves a range with `revparse`, and eight characters are ambiguous in a repository the size
of the kernel — the toast would read fine and the link would open an `AmbiguousRev` refusal.
`tests/pull.rs` pins all three strategies against `git rev-list`, and asks `cide_git::log` itself to
walk the range, since that is the walk the link runs. The log's Rust did not change:
`LogRefs::Rev` took a two-dot range already.

### A notice can offer something to do

`Notice` gains `actions` — a label and a callback — drawn as small bordered buttons in the shape of
the log's *Clear filters and find it*, and not as a link inside the sentence, because the sentence
is selectable text and a control inside it would be dragged into the selection. Following one
dismisses the toast. `admit` changed with it: a repeat with the same words used to be declined
outright, which was right while a notice was its text and wrong for one that carries a link. Toasts
never dismiss themselves, so a second pull half an hour later with the identical headline would
have been dropped, and the surviving toast would have gone on offering the *first* pull's range. A
repeat that brings a different body or any action now replaces the shown notice in place — same
id, same position — and one that brings nothing new is still declined by returning the same array.

### The log takes a parked filter, and a filter names a repository

`gitlog/logRequests.ts` is the sibling of `requestLogReveal`: a parked, consume-once request with
the same TTL, in its own module because `LogTab.tsx` imports from `keys/dispatch.ts` and dispatch is
the producer. `LogFilter` gains `repo` — only this repository, or `null` — because a full oid is
meaningless without the object database it lives in: under the merged scope a multi-root Log tab
walks, every other root fails to `revparse` it and takes the page down with it. The Log tab narrows
its scope to `applied.repo` while it is set, the branch box reads *Range abc1234..def5678* (and
*… in app* when the project has several roots), and *Clear filters* drops it. Nothing in the bar
sets it. `findCommitFilter` and the ref chips produce a bare full-oid `rev` too and carry the same
latent multi-root failure; they were not changed here.

### One link per repository

`receivedLinks` gives one link per repository that received commits: *View commits* alone in a
single-root project, *View commits in app* otherwise, carrying the id and not the name because two
roots can share a basename. A single merged view of several ranges would need a `LogRefs` variant
and would have no graph. The links are offered in the shell window only: `git.pull` runs from a
detached window too, but the tool window is drawn by the shell window, and the parked request is a
store in the window that parked it. The status-bar branch popup's own *Pull* reports into its note
bar and not a toast, and got no link.

### What was found and not fixed

The first version of the range test committed six times inside one second, and once in a dozen
runs the log listed four commits for a range `git rev-list` gave three: the root, which is hidden
behind `mine`, came off the heap before `mine` did — a six-way tie ordered however the heap
happened to — and was emitted before the walk learned it was uninteresting. git's `limit_list`
tolerates that because it filters at output time; `cide_git::log` streams rows and cannot retract
one. Real histories are minutes apart, and the shape that ties — a hidden tip written in the same
second as an ancestor it hides — is a rebase's, where the rewritten commits are on the interesting
side and not the hidden one. So the test pins its dates (`TempRepo::commit_all_at`) and the walk
was left alone; this paragraph is so the next person who sees a range list one row too many knows
where to look.

### What is not verified

**Nothing here has been seen on a display.** The Rust range is covered against `git rev-list` under
all three strategies; `check:notices` drives `admit`'s refresh rule and pins dismiss-on-follow;
`check:log` drives the filter model and pins the seam's wiring; `check:branches` drives the link
derivation and the shell-window gate. What has not been seen: the buttons rendering in the toast at
all, whether *View commits* under an open disclosure crowds the 420px card, the branch box reading a
range, and whether the graph over a range of three commits looks like anything.

## The push dialog, and force push as a lease (M36), and what is not verified

`git.push` was a fire-and-forget gesture. It fanned out over every repository in the project and
sent each one's current branch to its usual remote, with no way to see what was about to leave the
machine and no force push anywhere in the stack — no flag on `cide_git::push`, no field on the
wire, no `+` on a refspec produced by anything in cide. IDEA's *Push Commits* is the shape that
was asked for: the commits, per repository, with a checkbox.

### The dialog is a promise, so the preview has to be the push

The whole risk in a dialog like this is that it is *internally consistent about a different push*.
There is no visible failure: the commits are real commits, the target is a real target, and the
button sends something else.

So `push::preview` makes none of the decisions itself. It resolves the remote with the same
`default_remote` that `push` calls — the function already shared with `fetch` and `pull` so three
menu items cannot disagree about which remote they mean — spells the refspec the same way, and
asks the same `route` whether the push will fork `git`. `a_preview_describes_the_push_that_would_happen`
pins it from the other end: it previews, then pushes, then asserts the outcome's remote, refspec
and count are the preview's.

**The count is not `BranchInfo::ahead`, and that is the load-bearing part.** That number is
measured against the branch's configured *upstream*. The dialog needs "what would this refspec
send", which is a different question the moment they are not the same ref — and `ahead` is **zero**
for a branch with no upstream at all, which is precisely the row a user is least sure about
(*publish this branch*). A dialog reading it would have offered to push a branch while reporting
that it had nothing to send. So the walk is done next to the walk `push` itself does, hiding the
remote-tracking ref where there is one and **every** `refs/remotes/<remote>/` ref where there is
not — the rule `unseen_by_remote` was already using for the count, extracted into `hide_remote_refs`
so the list the dialog draws and the number the notice reports cannot come from two walks.

`PULL_COMMIT_CAP` is 10 and was not reused. Ten is right for a four-line toast; this is the whole
content of a scrollable dialog whose one job is showing the set. `PUSH_COMMIT_CAP` is 100, matching
`REBASE_COMMIT_CAP`, with the remainder in `PushPreview::more` rather than silently dropped —
`outgoing()` is `commits.length + more`, so a row with 140 commits says 140 and lists 100.

### Three blocked states, not one

A repository with nothing to push has three different reasons and three different remedies: commit
something, check a branch out, add a remote. `PushBlock` carries which, and the row prints the
sentence. The alternative — a `pushable: bool` and a disabled row — draws a control that does
nothing with no explanation, which reads as a bug in cide rather than as a state of the repository,
and all three states are about ten seconds of work to leave.

They arrive as a `blocked` row rather than as an error or an omission, because the dialog draws a
project's repositories together: one unborn submodule must not blank the row beside it, and a
missing row is not an answer.

### Force push is `--force-with-lease`, and on one route cide performs the lease itself

`--force`, on a remote somebody else pushed to five minutes ago, deletes their commits and reports
success. There is no undo cide can offer: the commits are unreachable on the remote, and for
everyone who had already fetched them they survive only in a reflog they do not know to look in.
So the checkbox is a lease, and `PushRequest::force` is a boolean with exactly one meaning — a
value that could mean either would be a safeguard one typo away from not being one.

The two routes get there differently, and the difference is in the dialog's wording because it is a
statement about *which program is protecting the user*:

* **The binary route** appends `--force-with-lease`, **bare**. Not `=<dst>:<oid>`, which looks
  safer and is not: the lease has to be measured against the remote-tracking ref *at push time*,
  and an oid captured when the dialog opened is stale the moment anything else fetches — another
  window, a terminal pane, an agent. Pinning it to that oid would either refuse a push that is
  fine or assert an expectation nobody read.
* **The libgit2 route** has no lease at all. `git_push_options` carries no expectation, and the
  only force libgit2 understands is the `+` on a refspec, which is `--force` whole. So `check_lease`
  connects, reads what the remote actually advertises, and compares it against the remote-tracking
  ref; a mismatch is `GitError::PushLeaseStale` with both short oids and nothing sent. That route
  is only reached for a local or `file://` remote, where connecting is a directory read.

Three states, one of which is the one that reads as harmless: both sides have the ref and they
match (allowed), neither has it — publishing (allowed, exactly as git allows it), and **the remote
has it and we have no tracking ref**, which is somebody creating that branch since our last fetch.
Refused, with an empty `expected` saying we held no opinion.

`a_force_push_holds_a_lease_and_refuses_when_the_remote_moved` is both halves against a real bare
remote: a force push whose lease holds lands, then a second clone pushes, then the same force push
is refused and the other clone's commit is still there. Commenting out the `check_lease` call makes
that last assertion fail, which is the property worth having a test for.

### One road to the dialog, because two roads to one gesture is a bug this surface already had

M20 fixed *the panel's dim note line against the palette's red box* — two routes to one push
disagreeing about whether it worked. Three surfaces now raise the dialog: the palette command, the
branch popup's Push button, and the Git panel's *Commit and Push…*. All three call `openPushDialog`
and every send goes through one `pushPass`, which is where the `allSettled`-over-individually-caught
shape lives along with the aggregation `notices.admit` makes necessary — it dedupes toasts by text,
so five submodules each answering *"already up to date"* would otherwise show one toast silently
speaking for five.

`CommitBox`'s button has said **Commit and Push…** since it was written, with a comment beside it
saying the ellipsis is load-bearing — *this opens something rather than acting at once*. It opened
nothing. It now opens this, over the repositories that just committed, after the refresh.

`--set-upstream` stopped being decided at the call sites at the same time. It was the branch
popup's alone, worked out from `head.upstream === null`; the preview reads the same fact off the
absent remote-tracking ref, per row, and `pushPass` sends it. That is the M20 defect — *Commit and
Push* failing with *no upstream* on a branch the popup's Push button published fine — made
unrepresentable rather than fixed twice.

### The dialog's own rules

`ConfirmDestructive` could not draw this: its list is a flat `readonly string[]` and this is two
levels with an answer per root. So it is `ConflictsDialog`'s shape — `OverlayCard`, head, list,
foot, Escape on the card — and it keeps that component's rules by hand, which is the part a check
script exists to hold:

* **Cancel is focused and accented.** The dialog is opened by a keystroke, so the Enter already in
  flight has to back out. The focus is placed in an effect keyed on the pending question and not in
  a `ref` callback, because React runs a ref callback on every render — the callback version moves
  focus back to Cancel when you tick the force box, and the next space dismisses the dialog.
* **Force starts unticked on every open**, including a reopen after a dismissal. This inverts
  `ConfirmDestructive`'s house rule of ticking its option by default, and it inverts it harder than
  the pull dialog's *remember this choice* does: that component's rule is about an option that
  makes an act **recoverable**, and this one makes an act irreversible on a machine that is not the
  user's.
* **Force is disabled unless a ticked row has diverged.** A `--force-with-lease` over a
  fast-forward is a no-op, and a checkbox that reads the same whether it would overwrite a
  colleague's work or do nothing at all is one people learn to tick. It is also masked per row on
  the way out — the answer is the gesture's, the flag is each row's.
* **The count is on the button** — `Push 2 commits`, never a bare *Push*. `ConfirmDestructive`'s
  rule 1: the user is acting on a specific set and the size of it is the one thing they cannot
  check afterwards.
* `--red` appears on the Push button only while forcing, and it still means one thing.

`chrome/pushModel.ts` holds every one of those sentences and is import-free so `check:push`
compiles it standalone; `chrome/pushRun.ts` is the fan-out and lives outside `keys/dispatch.ts`
because `dispatch.ts` imports `openBranchPopup` from the branch popup, so the popup importing the
runner back out of it is a cycle — ESM tolerates one until somebody moves an import and gets an
order-dependent `undefined` with nothing at the failure site naming the cause.

### Not done

* **No remote picker.** `PushPreview::remotes` carries every remote, `origin` first, and
  `PushRequest::remote` is on the wire — the row draws the resolved one and does not yet let you
  change it. A repository with two remotes pushes to the one `default_remote` picks, which is what
  it did before.
* **No branch renaming on the way.** The refspec is `refs/heads/x:refs/heads/x`; `targetLabel`
  reads the destination out of it so a rename would draw correctly, and nothing produces one.
* **No progress.** `PushOutcome` arrives whole when the invoke resolves. A large push shows the
  panel's busy string and then a toast, with nothing in between.
* **`PushLeaseStale` is libgit2's only.** On the binary route git refuses first and its own text
  arrives as `GitError::Push`. Translating one into the other would mean parsing git's stderr to
  decide which error it was, which is exactly what `cide_git::push`'s header forbids.

### What is not verified

**Nothing in this milestone has been confirmed on a display.** The Rust half is covered against
real repositories — preview, the three blocked states, the publish case, divergence before and
after a fetch, and both sides of the lease — and `check:push` drives the model standalone and greps
the component for the four rules above. What has not been seen: the dialog rendering at all, the
tree's collapse toggles, the checkbox column at a `--ui-scale` above 1, the scroll cap on a branch
with eighty commits, and whether *Commit and Push…* opening a modal a moment after the commit lands
reads as helpful or as an interruption. The last of those is the one most likely to want changing.

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

## IDEA's change markers in the editor gutter (M35), and what is not verified

**A file could be modified according to git and the buffer you were typing in said nothing about
it.** The file tree painted a status letter, the Git panel listed the file, a diff pane would show
the hunks — and the editor, the one surface actually in front of the user, had no idea which of
its lines were new, which had been rewritten and where something had been taken out. IDEA answers
that in the gutter, and this is that: a green bar per added line, a blue one per rewritten line, a
red caret where lines were removed, and a card on click carrying HEAD's text with **Revert** and
**Copy**.

### The diff runs in the webview, and that is the whole design

The obvious implementation is `git_diff_file` with `DiffSide::Combined`, which already returns
hunks with `new_lineno` on every line. It is the wrong one, for a reason that has nothing to do
with cost: **every diff in `cide-git` reads the working tree from disk**, and the state the editor
is in whenever this column is most wanted is *unsaved*. A gutter fed from disk is blank for the
edit you just made and stays blank until you save — which is precisely backwards, because the
marker's job is to tell you what you have done since the last commit.

So the baseline — HEAD's blob, fetched once through `git_file_at_revision` — is diffed against the
**live CodeMirror document**, five times a second at most, with `panes/mergeModel.ts`'s patience
diff. That diff was written for the merge resolver and its header already justifies itself for
running "in a webview, on a keystroke"; nothing new was needed, and no new Rust was written for
this milestone at all. `FileDiff` would also have shipped `old_text` *and* `new_text` — up to
2 MiB each — per open tab per refresh, for information a gutter never reads.

HEAD and not the index, which is IDEA's "base revision": staging a hunk does not make its marker
disappear. The practical half is that HEAD moves only on a commit, checkout, pull or rebase —
exactly what `gitRefsMoved` already detects — so staging, the commonest git write there is, costs
no refetch at all, and a refetch that returns the same blob oid writes nothing and notifies
nobody.

### Blame is the template, and the three places it is *not* is where the reading is

`editor/changeBaseline.ts` is `blameStore.ts` with its machinery removed rather than copied:

- **No dirty-buffer registry.** Blame sends the buffer to Rust because Rust does the attribution.
  Here the only thing fetched is an immutable blob, so no copy of the buffer ever crosses the IPC
  wire, however fast anybody types.
- **No `loading` state, so nothing to strobe.** Blame needed `keepVisible` to stop its column
  emptying on every git write. Here there is no intermediate state to flicker through.
- **Every failure is silent.** Blame reports its reason because turning the column on is a
  *gesture*, and a gesture that does nothing is this project's named worst failure. These markers
  are ambient — nobody asked — so a file outside every repository, a file HEAD does not have, a
  binary blob, a truncated one and an unborn HEAD all resolve to the same "no markers", and **no
  error variant is inspected**. That last part is deliberate: `file_at_revision` rejects an unborn
  HEAD with `NoSuchCommit` and a missing path with `NotTracked`, and a taxonomy that knew about
  one of them would fall through on the other.

The same subtraction happens in the extension. `blame.ts` carries a paragraph on `touched`
overrides and a branch on `tr.isUserEvent('undo')`, both because blame cannot re-derive locally: a
marker that survives an edit keeps another person's name over text they never wrote, *for ever*.
This column re-derives from scratch every 120 ms, locally, for free — so there is no overlay and
no undo branch, and an undo is just another edit. An overlay would also have been **wrong**:
"the user typed here" is not "this differs from HEAD", and typing a line back to its committed
text must *remove* a bar, which an additive set cannot express.

### Three range sets, and the one that is never drawn

`bars` is one point per changed line, because `@codemirror/view` matches a marker to a line by
comparing the range's `from` against the line start — a single range spanning six lines paints
one of them. `carets` is separate because **a line can carry both**: a deletion immediately
followed by an insertion is two blocks whose markers land on the same line, and one set would
silently drop one of them.

`blocks` holds real ranges, is never rendered, and is what the card and Revert read. Mapped
through `tr.changes` like the others, so typing inside a block grows its range and the revert
replaces whatever is there *now* — which is the correct meaning of "put this block back", not a
compromise. A scan over the per-line points could not do it: an insertion inside a block leaves
its new lines unmarked until the next recompute, so the scan would stop early and revert half of
it.

### The bugs found by writing it down, the one found by looking at it, and one wrong fix

**A CRLF baseline splits on `'\n'` into lines that all end `\r`.** CodeMirror normalises the
buffer to LF at load — `lineEndings.ts` exists to put the file's own shape back on save — so every
line of a Windows file would compare unequal and the **whole file** would paint modified. Nothing
throws; it looks exactly like a file that really did change. `splitBaseline` splits on all three
break shapes, and it is the headline assertion of the new check.

**`highlightActiveLineGutter()` paints over the bar.** It puts `cm-activeLineGutter` on *every*
gutter element of the caret's line, background and all — so the marker would vanish from the one
line the user is actually editing, which is the line it is most wanted on. The bar rules are
written three classes deep to beat it *regardless of source order*, which is the same call
`.cm-blame-touched` already makes and for the same reason: relying on the cascade tie-break makes
a later reordering of the stylesheet a silent regression.

**A gutter's `domEventHandlers` do not stop propagation** — `@codemirror/view` installs them as a
plain `addEventListener`, and returning `true` from one calls `preventDefault()` and never
`stopPropagation()`. So the click that opens the card really does keep bubbling, and a dismissal
listening one level up would close it in the same gesture: the card would never appear, reading
as the marker simply not being clickable rather than as a bug in a dismissal rule.

The first fix for that was to exclude the gutter and the card by hand, and it was **wrong about
its own mechanism** — worth recording, because it would have left two guards and a confident
comment explaining a danger they were not the answer to. `EditorView.domEventHandlers` attaches
to **`contentDOM`**, and neither the gutter nor a tooltip is inside it, so the dismissal never
sees either event and both exclusions were dead code. What keeps the card open is the attachment
point, and the guard worth having is against somebody moving the handler to `view.dom` — which is
what the check pins instead.

**A phantom green bar on the last line of nearly every file in the repository.** `Text` keeps the
empty line after a file's final break — `"a\nb\n"` is *three* lines to CodeMirror — while the
baseline was split with the diff pane's rule, which drops it. That rule is correct where it lives:
`diffRows.splitLines` and Rust's `strip_newline` are lining up against **git's** numbering, where
a file ending in a break has no line after it. Here the comparison is against a CodeMirror
document, so the baseline came out one line shorter than the buffer for every newline-terminated
file — which is very nearly all of them — and the diff dutifully reported an addition at the
bottom of each. Copying the right rule from the wrong neighbour, in other words, and the result is
a green bar on files nobody has touched, invisible until somebody scrolls to the end. It is
**still** invisible: the screenshots below never reached the foot of a file, and this one was
caught by asking what `EditorState.create` actually does with a trailing newline rather than by
looking.

**Every deletion caret in the file was drawn in the same place.** The caret is an absolutely
positioned `::before`, and the stylesheet made the *column* the positioned ancestor rather than
the per-line cell — so all of them stacked in the gutter's top-left corner, off screen for any
line scrolled past. Nothing throws, nothing is missing from the DOM, and the only symptom is that
a deletion appears to paint no marker at all. **This one was found by looking at it**, which is
the whole argument for the paragraph below: a bar and a caret can be individually correct in
every check this repo can write and still be in the wrong place on a screen.

**A deletion at the end of a file reverts one line too high.** The caret anchors on the last line,
because there is no line below it to hang on; the *insertion point* is one past that line. Writing
the two as the same number puts the restored text above the final line, and it looks right in
every case where the last line happens to be blank.

### Revert is a document edit, and that is why it does not ask

The Git panel's *Roll back* opens `ConfirmDestructive`, because `git_rollback` writes the disk and
nothing undoes it. This one dispatches a single transaction with `userEvent: 'revert'`, so it is
one undo step and **Ctrl+Z puts it straight back** — asking would be ceremony over an action the
editor can already reverse. The restored lines are joined with `'\n'` and never with the file's own
break shape: a CRLF join would put literal `\r` characters *inside* CodeMirror lines and
`restoreLineEndings` would then add the file's ending on top, so every reverted line would end
`\r\r\n`.

The arithmetic lives in `changeModel.revertPlan`, whose `eat` field says which surrounding line
break a removal has to consume — the trailing one normally, the leading one at EOF where there is
no trailing break to take. `check:change-bars` round-trips every block shape through it: sixteen
add/delete/modify cases at the top, middle and end of a file, each asserted to reproduce HEAD
exactly. It is the only automated defence on the one operation in this feature that can destroy
the user's text rather than mispaint it.

### What is not verified

- **The three markers were confirmed on a display; nothing else was.** `run.sh` is
  profile-scoped now, so the `dev` instance starts beside the installed one this was written
  inside. A tracked file was edited on disk in all three ways and the instance restarted, and the
  screenshots show: a **green** bar on the added line, a **blue** bar on the rewritten one, and
  the **red caret** straddling the boundary where a line was removed. Two further things came out
  of the same screenshots — the line numbers do not move when a bar appears, and a bar on the
  **active line** survives `highlightActiveLineGutter`, which is the specificity rule above doing
  its job.
- **Everything reached by a pointer is unverified.** No synthetic-input tool is installed on this
  machine, so the card was never opened: whether it flips the right way up near the foot of a
  pane, whether `flushSync` measures it correctly, whether Revert lands the right edit and whether
  Ctrl+Z undoes it cleanly have all been reasoned about and none of them has been *done*. Revert
  is also the one gesture here that can destroy text, so it is the one that most needs a hand on
  it.
- **Nor is anything about typing.** Every screenshot was taken after a restart, against a buffer
  read from disk. Whether 120 ms reads as live while a key is held, whether the bars follow their
  lines through an edit above them, and whether the column keeps up during a paste are unmeasured.
- **The foot of a file was never looked at.** The phantom-addition bug above is proven fixed by
  the check, which compares against the line arrays a real `Text` produces, and by nothing on a
  screen — the file used for the screenshots is 343 lines and there is no way here to scroll one.
- The colours are unjudged: whether a `0.5ch` bar at 70% reads as IDEA's does, and whether the
  caret is findable at an 11px gutter without knowing to look for it.
- **The 120 ms tick has never been profiled on a large real file.** The argument is structural —
  the gutter is viewport-windowed, the patience diff trims its common prefix and suffix first, and
  the buffer is read with `iterLines` rather than materialised as one string — and the numbers are
  estimates. The line cap is 50,000 and the byte gate is `HIGHLIGHT_LIMIT_BYTES`, the same flag
  that already decides a buffer gets no grammar and no folding.
- **A renamed file loses its markers entirely.** `locate` gives the current path and HEAD has the
  old one, so the baseline read rejects. It fails to *no markers*, which is the safe direction,
  and it will read as the feature being broken on exactly the file somebody has just refactored.
- **A staged-but-uncommitted new file shows nothing**, because the baseline is HEAD and HEAD has
  no such path. IDEA paints the whole file green. This is a choice — a wall of green is noise
  rather than information — but it is a choice, and the first report about it will be legitimate.
- **Two panes over one file diff twice.** Correct, since they are two independent buffers, but it
  is two diffs per tick and nothing measures it.
- The revert round-trip proves the *arithmetic*, over arrays. Whether `changeBars.ts`' translation
  of that plan to offsets against a live `Text` is faithful — the clamps, the `+1` past
  `doc.length` — is source-pinned and nothing more.

## Moving a pane between rows and columns (M31), and what is not verified

**A pane's position was decided once, when it was made, and could never be changed.**
`pane_split` and `pane_add_row` put a *new* pane somewhere, `pane_close` took one away, and a
divider drag changed how much room a pane had but never which row it was in. Splitting the wrong
way round meant closing the pane and making another one — which ends the session it was holding.

Two gestures now move one:

| gesture | what it does |
| --- | --- |
| the **grab handle** in a terminal pane's corner cluster | press and drag; every other pane lights a band on the edge the drop would land on, release to move it there |
| **Ctrl+Alt+Shift+H/L/K/J** | move the focused pane past its neighbour to the left / right / up / down |

Both run one domain operation, `layout::move_pane`, and it is a real move rather than the swap
that was already in the tree. `layout::swap` exchanges two leaves' positions; it cannot take a
pane out of one row and put it in another, and it cannot collapse anything. It remains, and
remains unused.

**The row a pane leaves collapses, and gives its space back in proportion.** `move_pane`'s
removal is `close`'s, not `take_pane`'s: `removal_plan` is read before the surgery and written
after it, so four equal tiles minus one is three equal thirds rather than 50/25/25 with the
shape of the binary tree — invisible to the user — deciding which neighbour got the lot. Take
the last member out of a row and the row itself goes, its height spread over the rows that
remain. At the destination the pane is a newcomer and gets `1 / (n + 1)`, which is what a split
there would have given it.

**The whole operation runs on a clone and commits only on success**, and that is not defensive
tidiness. The destination's legality is only decidable *after* the removal — reordering a tile
inside a full ten-tile row takes it to nine and back, so a pre-flight count against the tree as
it stands would refuse the one gesture a full row most needs. Doing the removal first puts the
pane in `lift`'s return value, off the tree, where a refusal from `add_tile` would drop it on
the floor: a live session with no leaf and no `panes` entry pointing at it. Committing a clone
makes every refusal leave the tree bit for bit as it was, which is what
`a_refused_move_leaves_the_tree_bit_for_bit_untouched` asserts.

**The console's primary pane can be moved, and still cannot be closed or detached.** This is the
one invariant M31 relaxes, and the argument is narrow: `take_pane`'s refusal is about a pane
*leaving the tree* — its own note names the failure, a console tab holding no `Primary` for as
long as a detached window is open, permanently if the re-dock anchor goes stale. A move never
takes the pane out, so the leaf set at the end is the leaf set at the start and
`validate_console` holds throughout rather than being re-checked afterwards. `take_pane` is now
a role check in front of a private `lift`, so there is still exactly one removal path and
`close` is still `take_pane` rather than something kept in agreement with it.

**`pane.move.*` is `terminalFocused`, not `paneFocused`**, because the handle is drawn on claude
and shell panes only — one flag keeps the chord and the button describing the same gesture. An
editor or diff pane keeps the four-button cluster and cannot be picked up; it is still a
perfectly good *drop target*, so a terminal can be moved next to one.

**All four directions join the neighbour's row**, including up and down. Minting a full-width
row instead was the alternative and it loses on the case that matters: in a 2x2, moving the
bottom-left pane up would give three rows and promote its former row-mate to a full-width row of
its own, as a side effect of a keystroke aimed at another pane. Joining lands the pane at the
horizontal position it already had, and it is the only reading that can *reduce* the row count.
A new full-width row is still reachable — the drag's top and bottom bands, and
`pane.split.down`. At the edge of the tree the move is a no-op: `layout::navigate` answers
nothing there, and a move that wrapped would cycle a layout for ever with no fixed point.

**`Ctrl+Alt+Shift+H/L/K/J` and not the arrows**, which read better and are both spent.
`Shift+Alt+Arrow` is banked by the note on `alt+left` — *"a different stroke, so select-syntax
and move-line survive untouched"* — and taking it here would make that sentence false.
`Ctrl+Alt+Shift+Left/Right` are printed twice in this file as the binding a user should add for
`navigate.back`/`forward`, so a default there would collide with advice already in the manual.
The chords used are the `ctrl+alt+<hjkl>` focus block one modifier up.

**The session survives, by construction on both sides.** `move_pane` never touches
`Pane::session`; the pane is carried through `lift` into `add_tile` by value. In the webview the
pane's DOM lives in a module-level map outside React and `PaneSlot` *parks* it on unmount rather
than removing it, so the terminal, its scrollback and its child come back attached.

**Not verified.** *Nothing in this repo can move a pointer*, so the drag gesture itself — the
threshold, the capture, the band that follows the cursor, the drop — has never been executed by
a test and has not been confirmed on screen. What is covered is everything either side of it:
`check:pane-move` compiles `layout/paneMove.ts` alone and drives every decision it makes
(the four bands by normalised distance, the edge-to-axis mapping, and the no-op drop that stops
a tile being "moved" onto the edge it already borders), then greps the hook for the properties
no test can execute — that it uses pointer events and never HTML5 drag-and-drop, that its
hit test is rooted at the active tab's `[data-audit="paneTree"]`, and that every exit path
detaches its listeners. That last one matters more than it looks: an Escaped drag never sees a
`pointerup`, so a teardown hung off that event alone leaks the whole state machine.

The **tab-scoping** of the hit test is the bug that was found by reasoning and not by a test, and
it would have shipped silently: `TabContent` lays every tab out at full size and hides the
inactive ones with `visibility: hidden` — never `display: none`, which zeroes the measurements
xterm reads — so an unscoped `querySelectorAll('[data-pane-id]')` collects the panes of every
open tab at real, overlapping coordinates, and a drop would land in a tab the user cannot see.

Also unverified: that a moved pane's terminal comes back **painting**. A move re-parents the
pane into a different chain grid, and a chain root's `SplitId` changes when `prune` collapses
it, so React unmounts and remounts the slots of the affected rows — the park-and-re-adopt path a
project switch already takes, and the path the frozen-pane class `check:render-stall` exists
for. It should be a park rather than a release, and the watchdog in `layout/paneHosts.ts` should
catch it if it is not, but neither claim has been watched happen.

## Closing a mirror pane no longer kills the conversation it was mirroring (M18)

**It did, and it had since mirrors shipped.** `TerminalPane`'s mirror branch adopts the id of the
session it is mirroring — that is the whole of a mirror, a second sink on a child that already
exists — and `closePane` ended with `const session = peekHost(pane)?.sessionId; destroyHost(pane);
if (session) await sessionApi.kill(session)`. From the host map the two panes were
indistinguishable, so closing the copy killed the original's child, mid-turn, with no message
anywhere. Found by reading the pane-host ledger while planning M18's subagent panes, which open a
headless run into a pane by exactly this mechanism: "I closed the window I was watching it in"
would have terminated the run.

The fix is one field. `PaneHost.mirrored` means "this pane did not spawn what it holds", set beside
the id in the mirror branch, carried through the eviction ledger alongside the id it qualifies —
otherwise an evicted mirror pane comes back holding somebody else's session and no longer knowing
it — and cleared by `forgetSession`, because a restarted pane owns its new child. `closePane` reads
it and skips the kill; `destroyHost` still runs unconditionally, because the pane genuinely is
finished and clearing the ledger's id is what stops a late async continuation resurrecting it.
Detach and re-dock never needed it: they use `releaseHost`, which keeps the terminal, the buffer
and the id and kills nothing.

**Not verified on screen.** Nothing in `ui/scripts/` mounts React against a live backend — the
checks compile modules standalone or SSR them — so no gate observes a mirror pane being closed
with a child still running. This is argued from the two call sites and from the fact that they are
the only two, not from having watched the conversation survive.

## `.cide/`, a task tracker Claude can call, and subagents that actually run (M18)

M18 set out to turn the pinned Claude session into a **product owner**: decompose a goal into tasks,
hand each to a role-specialised subagent, watch the tasks resolve, validate, dispatch the next
round. That loop is now built end to end. A project opts in by writing `.cide/config.json`, defines
its roles as markdown files beside it, and the session in the console tab is told at spawn what it
has and handed eleven MCP tools to act with — six over the shared task tracker and five over the
agents themselves. A dispatch mints a run, takes a concurrency slot, ensures that role's git
worktree and starts a real `claude` inside it that no pane is showing; when the run hands its turn
back, one line is typed into the product owner's own terminal saying so. Everything a run is made of
is machinery that already existed: `SpawnSpec` → `PtySession` → `SessionRegistry` →
`lifecycle::watch_for_exit`, the same four steps a pane takes.

What has **not** happened is the last hop. **No subagent in this tree has ever been dispatched
against a real `claude`** — that needs a GUI and a person, and nobody has sat in front of one. Every
seam short of it is exercised, including the MCP server against a real unix socket with the real
bridge's header pinned as a test constant, and pause is exercised against a real stopped child; the
hop from a Dispatch button to a role editing files in a worktree is argued from those pieces and not
observed. The *Not done* subsection below is written with the same care as the rest of this section,
because that sentence is not the only one of its kind.

### `.cide/` is committed, and deliberately invisible to the file tree

Four things live there and only the last is ignored: `config.json` (the per-project orchestration
switch), `agents/<name>.md` (role definitions), `tasks.json` (the tracker) and `worktrees/` (one
checkout per agent). `.gitignore` used to ignore `/.cide/` wholesale, on the premise — written into
the line's own comment — that the directory held "runtime state cide itself writes". It never did:
`workspace.json` and everything like it live in `$XDG_STATE_HOME/cide`. That line is now the same
split `.claude/` already uses, ignoring `.cide/worktrees/` and committing the rest, with a comment
naming what changed, so this repository can commit its own task file.

**`enabled` is false in the absence of `config.json`**, which is the property everything else hangs
off: a project that has never heard of this feature cannot spawn anything after an upgrade. Every
read path in `cide_agents::config` funnels a missing, unreadable, truncated or unparseable file to
`AgentsConfig::default`, so reading cannot fail — and the *direction* of that failure is the point.
Guessing `true` on a file cide could not parse means unattended `claude` processes editing
somebody's repository and spending their quota; guessing `false` means a button does not work until
they read a `tracing::warn!` naming the file and the serde error. Nothing is mirrored into
`Workspace` either — this is committed configuration a teammate's commit or a `git checkout` can
change under the running app, and cide's own state file holding a stale copy of something git owns
is a bug with no upper bound on how long it lasts. So the file is re-read at every point that acts
on it, which costs a `read_dir` and a handful of small files.

**Three of the file's fields are deliberately not on the wire**, and `OrchestrationConfig` carries
the other three. `isolation` and `allow_dangerous_permissions` are dispatch-time facts with nothing
for a roster row to draw, and widening the DTO to carry them would put switches the panel cannot
render into the panel's vocabulary. The third is `nudge_orchestrator`, and it deserves its own
sentence: **it is the one that types into the user's own console.** It ships `true`, because the
loop it closes is the feature M18 was asked for and a loop whose last step is *and then the user
happens to notice* is not a loop — so the key is the way out, not the way in. Keeping it off the
wire means no webview gesture can turn it on, and no round trip through the panel can silently reset
a `false` somebody hand-edited; `AgentsConfig::apply` touches none of the three, and `write`
preserves what it did not change. It is also read fresh at the moment of each nudge rather than
cached at dispatch, because a `git checkout` can switch it off under a run that is already going and
a cached copy would go on typing into somebody who had already said no.

**A role definition is Markdown with restricted front matter, and that is the one place cide breaks
its own all-JSON rule.** The body *is* a system prompt: a multi-paragraph document a human writes,
rewrites, and their team reviews in a pull request. A system prompt inside a JSON string is one line
with `\n` between every sentence — decisively unreviewable in a diff, where a two-word change to
paragraph four shows up as the whole prompt replaced. The parser in `cide_agents::defs` is
hand-rolled, in `claude_cli`'s tradition, for three reasons in the order that decided it:
`serde_yaml` is deprecated; a full YAML parser is an enormous surface for eight flat scalars (block
scalars, anchors, merge keys, `NO` being a country); and a restricted grammar can name the file *and
the line*, which a YAML error generally cannot. What it does not understand it **refuses** rather
than ignores — indentation, block sequences, tags and flow mappings each get their own sentence —
because silently accepting a line the parser did not read means the file says one thing, the running
agent does another, and nothing connects the two. Every finding is an `AgentProblem { path, line,
message }` and a broken file never stops the others loading, which matters more here than in
`Filter::build` or `persist::load`: these files are edited by hand *and by models*, so one of them
being mid-edit is the directory's normal state.

**The tree cannot see any of it, and that is a four-way decision rather than a boolean.**
`cide_fs::filter` grew a fourth verdict, `Verdict::WatchOnly`, for exactly this directory:
`Filter::admits` says no, `Filter::watchable` says yes, and no other pair of answers is right. The
watcher has to report it — without `<root>/.cide` on `watched_paths` a teammate's `git pull` would
be invisible, because `admits` rejects a dot-prefixed component *before* any gitignore matcher runs
— while more than the file tree consults `admits`: `cmd::search` puts every content-search candidate
through it, `files`' symbol walk asks it, and `Index::rescan_dir`/`graft_subtree` ask it. Had
`.cide/` taken the git paths' `Verdict::Always` instead, task titles, task comments and agent system
prompts would have become `Ctrl+Shift+F` hits inside files the tree does not draw. The git half of
the list is told apart by a flag on the entry rather than by its name, so a write to
`.cide/tasks.json` does not raise `FsChange::git` and the branch readout does not refresh because
somebody ticked a task.

### The tracker is one file with one writer

`crates/cide-tasks` owns `.cide/tasks.json` and is the only thing in the process allowed to write
it. Three classes of writer read it — the primary session, every subagent it dispatches, and the
user through the panel — and that is not a lost-update problem for exactly one reason: **agents
never write the file.** They call `cide_task_*` MCP tools, those arrive over the agent-RPC socket,
and the app funnels every one into `TaskStore::update`, which mirrors `WorkspaceState::update` line
for line — snapshot, run the closure, validate, roll back on `Err` *or* on failed validation, bump
`rev`, `note_change`. The MCP-tool decision and the single-file decision are the same decision from
two sides: the moment an agent could `Edit .cide/tasks.json` directly, this crate would need real
file locking and the merge below would stop being a rare repair and become the hot path.

Four layers, and the third is the one a single actor cannot solve. A loader that repairs and never
fails, so a broken tracker is not a project that will not open. A `FileStamp` re-`stat`ed before
every write, because a `git pull`, a teammate's commit or a hand edit moves the file underneath:
when the stamp has moved the store re-reads and merges by rule — per task the higher
`updated_unix_ms` wins, tasks only on disk are adopted, tasks only in memory are re-added, comments
union — and `merge` is a pure function over two `TaskFile`s with a table-driven test precisely
because it is the most likely place in the feature for a bug to live. Refusing the write would
silently lose an agent's comment; last-writer-wins on the whole file would delete three tasks a `git
pull` had just added. Then debounce and an atomic publish, as `workspace.json`, differing in exactly
one respect and duplicated because of it. `persist::write_atomic` creates at 0600 deliberately — its
doc records the `screens.json`-at-0644 incident that put it there, and `workspace.json` can hold a
proxy password — but the tracker is **committed and shared**, so on a checkout two developers use,
0600 means the file exists and cannot be read and the failure looks like a missing feature rather
than a permission problem. `cide_tasks::write_shared` is therefore one line — `persist::write_atomic_with_mode(path, json,
persist::SHARED_MODE)` — over the same sibling-temp, `sync_all`, rename, directory-`fsync` dance
`write_atomic` performs at `PRIVATE_MODE`. The consolidation this paragraph used to flag as owed is
done; what survives it is the distinction between the two modes, which is not cosmetic. Neither is
a floor: the mode is passed to `create`, so the umask masks it, and 0644 here means "no wider than a
file the user created themselves" rather than "world-readable whatever your umask says" — the right
promise about something that lands in a repository, and the reason forcing the bits with
`set_permissions` afterwards was tried and reverted.

**A project that never used the tracker gets no tracker file.** `write_now` is the unconditional
write — project close and quit call it, so a change made in the last 500 ms is not lost to a
debounce that never elapsed — and it wrote an empty `{"schemaVersion":1,"rev":0,"tasks":[]}` into
every project that had merely been *opened*. A tracked file appearing in `git status` because a
panel was mounted is the same surprise the `absent` screen's own hint exists to prevent one step
earlier: *"Creating the first task writes a new file, which is committed with your code."* The guard
is two conditions and both are load-bearing — no tasks in memory **and** no file on disk. Dropping
the second would refuse to persist the deletion of the last task, which is a delete that does not
delete; dropping the first would refuse the write that creates the file for the first task. A file
that already exists is left alone: this stops one being created, it does not remove one.

The store owns no thread; `TasksStores::start_flusher` in `cide-app` does the ticking, modelled on
`PositionsState::start_flusher`, which exists because there is no app tick to hang it on. That tick
is also what notices a `git pull` moving the file under an open panel and broadcasts the merged
board. It is wired into `project_open`, `project_close`, `lifecycle::shutdown` **and the `setup`
restore loop** — that last is not optional, and `lib.rs` documents the identical omission twice for
two earlier registries, where a restoring launch came up with a whole feature missing and nothing
failing.

The shapes are cut down on purpose. `TaskStatus` is four states; `Blocked` lost because it is a
*reason*, not a place, and every tracker that ships it accumulates tasks parked there for weeks with
nothing saying why — a blocked task is `Todo` with a comment, which is a shape an agent can actually
write. `TaskId` is a short string, `t-17`, minted from the file's own high-water mark, because
agents quote ids **inside prompts and comments**, where a uuid costs tokens, gets truncated by a
model that is paraphrasing, and cannot be matched back. `TaskComment` is append-only, and there is
no wire shape that could edit one: `TaskEdit` has a `Comment` variant and no `EditComment` or
`DeleteComment`. That single restriction is what makes the tracker a channel *between* agents rather
than a scratchpad — an editable comment is one an agent can quietly rewrite after the fact, and
neither the user reading the panel nor the next agent reading the task would have any way to tell.
`task_delete` exists as a *user* command and is deliberately not a tool.

### Eleven tools, and the scope is the socket rather than the prompt

`cide_agents::tools` defines all of them once, in a module that is pure — names, schemas and
handlers over a `TaskSink`/`AgentSink`, with no socket and no filesystem in it. `tool::ALL` is the
six task tools (`cide_task_list`, `_get`, `_create`, `_update`, `_comment`, `_assign`);
`tool::ORCHESTRATION` is the five that drive the agents (`cide_agents_list`, `cide_agent_dispatch`,
`cide_agent_runs`, `cide_agent_stop`, `cide_agent_integrate`); `tool::EVERY` is the eleven, written
out by hand rather than concatenated so that a name lands in exactly one family on purpose. They
reach a child through three pieces. `crates/cide-app/src/agent_rpc.rs` binds one listener per
process at `$XDG_RUNTIME_DIR/cide-agents-<pid>.sock`, 0600, unlinked on drop. `cide-hook` gained an
`mcp` subcommand that is a **dumb pipe**: it proxies `initialize` and `tools/list` as well as calls,
so the vocabulary has exactly one definition and a `cide-hook` left over from an older install
cannot advertise a tool this build removed. And both spawn sites — `cmd::session` for a pane and
`ClaudeHarness::spawn_spec` for a run — attach it with `--mcp-config` carrying inline JSON built by
`serde_json` rather than formatted, so a worktree path with an apostrophe in it cannot produce a
config the CLI parses as something else.

**The scoping is structural, not prompted.** Before any JSON-RPC the bridge writes one header line
naming `CIDE_RUN` and `CIDE_SESSION` **read from its own environment** — the `spawned_as` rule, and
the reason it matters is that a caller who could name its own identity could sign a comment as the
user, or dispatch as though it were the product owner. `agent_rpc::resolve` asks the run question
*first* and the primary-session question second, and the answer decides the tool list for the whole
connection: a run this process dispatched is served the six task tools, scoped to its project and
signing every comment as that run's role; a project's `Project::primary_session` is served all
eleven, scoped to that project; anything else, including a `claude` a user started by hand in a
shell pane, gets a valid `initialize` and an **empty** list. Never a crash, and never another
project's tasks. Asking the run question first is part of the boundary rather than a
micro-optimisation: a connection that is a run can never be read as anything else. **That is the
whole of the answer to "may an agent dispatch an agent".** The five orchestration names are never in
a run's `tools/list`, and `call_tool` refuses a name the connection was not shown, so a run that
guesses `cide_agent_dispatch` is answered `METHOD_NOT_FOUND` before anything reaches the registry —
closed by construction rather than by asking a model not to. It is resolved **once**, at connect,
because `initialize` advertises `capabilities.tools.listChanged: false` and a client is entitled to
cache `tools/list` for the session; a scope re-derived per message would drift the moment
`workspace::bind_session` rewrote a project's primary session on a restart, and a long-lived
orchestrator would be told "no such tool" for something it can still see.

Two absences will be proposed as additions, and both are tested rather than merely written down.
`cide_agent_pause`/`cide_agent_resume`: pausing is a *user* gesture, and an orchestrator that can
`SIGSTOP` its own workers can wedge a project with nobody at the keyboard — `run_state_detail` says
so to the model's face, rendering a paused run as "frozen by the user, and only the user can resume
it". And `cide_task_delete`: the file is the shared record of what happened, and deletion belongs to
the person whose repository it is.

Three smaller decisions each close a failure this codebase has already paid for. `--mcp-config`
never goes **before the user's own arguments**: it is variadic and swallows every following token
that does not begin with `-`, which is the same wager `plan.args` refuses by going first, and every
token either spawn site writes after it begins with one. `--strict-mcp-config` is deliberately
absent: it would silently drop every MCP server the *user* configured in exchange for cide's one,
and attaching a tracker is not a reason to take somebody's own tooling away. And every tool's output
is wrapped by `tools::preamble` (or `agent_preamble`) inside a `<<<cide:project-data` fence telling
the model that the block is data written by the user and by other agents, never instructions
addressed to it — an acknowledgement of the injection surface, not a guard against it, and
`model_authored_text_can_never_be_the_closing_fence` is the structural half of the same worry.

That fifth row is **now done**. `claude_cli::INJECTIONS` carries `--mcp-config`, `ClaudeInjections`
carries `mcp_config`, and Settings → Claude sessions has the switch — so every argument cide adds to
a pane's command line is one the user can decline. Two things fell out of doing it that are worth
knowing. `--mcp-config` needed a `REFUSED_ARGS` row as well, because a test pins the two tables
against each other (*"cide injects `--mcp-config` and nothing refuses it"*), and that row carries
`because: Some(Injection::McpConfig)` so switching the injection off hands the flag back to the
user. And the orchestrator's roster paragraph turned out to assume the server was there: it is
nothing but instructions to call `mcp__cide__cide_agent*`, so ungated it would have been a system
prompt naming a vocabulary the session did not have. It is now behind the same switch.

### A run is a pane nobody is looking at

`crates/cide-agents/src/harness/claude.rs` builds the same `SpawnSpec` `cmd/session.rs` builds, and
the module header argues why at length: `-p --output-format stream-json` buys a machine-readable
result envelope cide already has from hook frames keyed on `CIDE_SESSION`, while an interactive PTY
buys two things with no substitute. A run headless for twenty minutes can be **opened into a pane**
with its whole transcript intact, because `PtySession` feeds its `vt100` mirror independently of
sinks. And a permission prompt is **answerable**, because there is a terminal for the answer to be
typed into; under `-p` a run either has `bypassPermissions` or it dies. So `AgentRegistry::start`
goes `harness.spawn_spec` → `PtySession::spawn` → `watch_for_exit` and `on_exit` → `SessionRegistry`
insert, in that order, with the watchers registered *before* the insert. **There is no second
process-hosting path**, which is exactly what makes the SIGHUP/SIGTERM/SIGKILL ladder, the orphan
arming and the close confirm cover a subagent for free, with no second implementation of any of
them. `LiveRun` therefore holds a `SessionId` and not an `Arc<PtySession>`: the registry is where a
child lives, and a second owner would be a second answer to "is it still running".

Liveness is likewise free and there is **no second state machine**. `Harness::observe` maps a hook
frame through `cide_claude::next_state` and `is_permission_request` — the same two calls
`hooks::decide` makes for a pane — so a run's phase dot moves for the same reason a pane's does. Two
rules the trait states and the implementation keeps: a `Paused` run is never moved by an
observation, because the freeze is a fact cide asserted with a signal and a frame already in flight
must not thaw the row; and a late frame must not resurrect a finished run, with `Observation::Exit`
the sole exception because it is the only observation carrying ground truth about the child.

The argv order is the third spawn site's, and it refuses the same wager the other two refuse: the
user's own arguments first, because `--add-dir`, `--mcp-config`, `--allowedTools` and `--tools` are
variadic and swallow every following token that does not begin with `-`, and every token cide writes
begins with one. Then `--session-id` through `cide_claude::conversation` rather than by hand, then
the role's system prompt through `fold_append_system_prompt` rather than a raw push, then `--model`,
`--effort`, `--permission-mode` and `--allowedTools` **only where the definition asked for them** —
the CLI's own defaults are the safe end of every one of those ranges, and a value invented here
would be behaviour the role's author never wrote and cannot find in their file. Then `--mcp-config`,
then `--settings`, then `-n "<role> · <task>"` so `/resume` and the terminal title name the work.

**`CLAUDE_CODE_SSE_PORT` and `CLAUDE_CODE_AUTO_CONNECT_IDE` are removed rather than merely unset**,
and that paragraph is the one to read before switching the IDE integration back on for a run.
`openDiff` blocks the agent's turn until a human answers a tab — a documented invariant of
`ide.rs::pump`, whose every early return must cancel the request for exactly this reason. A headless
run has no pane, so there is no tab and no human, and the turn would hang until somebody noticed a
row that had stopped moving, holding a slot and a worktree the whole time. Removed and not unset
because cide can *inherit* both: launching `./run.sh` from inside a `claude` pane hands this process
an `SSE` port that would otherwise reach every child it starts. The cost is one read — a subagent
gets no `getDiagnostics` and has to run the project's own build instead — and that trade is not
close.

**The opening prompt is one line, whatever the task says.** It is typed into the child's terminal
and terminated with `\r`, because that is what a terminal sends for Enter, so an embedded newline is
*another Enter*: a multi-line task body handed over verbatim would submit its first line as a whole
turn and feed every remaining line in as further turns, the agent answering a fragment before it had
seen the rest. So `cmd::agents::opening_prompt` **points a run at its task instead of handing it
over** — one flattened line naming the task id and telling the agent to read it with
`mcp__cide__cide_task_get`. The better half of that is the second consequence: the body reaches the
model through the tool, inside `tools::preamble`'s "project data, not instructions" fence, rather
than sitting raw in the prompt position where a task written by another agent would read as the
user's own instruction. `the_opening_prompt_never_contains_a_newline` is the test, and the rule
generalises — the nudge and the retry are both PTY writes and both inherit it.

### One worktree per task, and an integration that refuses before it touches anything

`crates/cide-git/src/worktree.rs` builds `.cide/worktrees/<name>` on branch `cide/<name>` from
whatever **name** it is handed. The unit was the *role* until a user asked, reasonably, why their
`max-concurrent: 2` role queued its second task: one checkout per role made the queue serial and
`effective_max_concurrent` clamped the declared number to 1 to protect it. The unit is the
**task** now — `cide_agents::checkout_name` composes `<role>-<task>` (the bare role for a
taskless dispatch; the task id sanitised through the worktree grammar, since `.cide/tasks.json`
is hand-editable and this string becomes a directory and a ref) — so a role's tasks parallelise
in their own checkouts up to the number its file actually declares, the merge unit is a task's
branch, and a re-dispatch of the same task lands on its earlier commits. What still cannot run
twice is two children in one directory, and that moved from a per-role number to a per-checkout
fact: admission's **checkout gate** holds a queued run while any run whose child may be standing
in the same checkout is on it (`Idle` deliberately excepted — its parked claude is wound down at
the moment the checkout is claimed, by a wind-down now scoped to that checkout and not the
role). The gate reads a `checkout` stamped on the run at dispatch, like the limits, and stamped
`None` under shared isolation, whose N-runs-one-directory meaning is unchanged. A role may also
opt itself out: **`worktree: false`** in its frontmatter puts that role's runs in the project
root, on the user's own branch — right for a role that only reads (a reviewer, a reporter),
wrong for anything that edits, which is why the safe posture is the default and the file has to
say otherwise (a mangled value greys the role rather than guessing — this switch decides
whether an unattended child edits the user's checkout, which is `permission-mode`'s class of
stakes). The flag rides `AgentDraft` as well as the definition, because `defs::render` writes
only the fields the draft models and a switch the draft did not carry would be silently deleted
from a hand-written file by the next panel save. `ensure` is
idempotent and called before **every** dispatch, not once, because the
half-made states are all reachable: a valid registration is returned, a registration whose directory
was deleted is pruned and rebuilt (`rm -rf` is one keystroke), a registration pointing somewhere
else is refused, a non-empty directory with no registration is refused because cide deletes nothing
it cannot prove it made, and a branch left by a previous run is **reused and never recreated** — its
commits *are* the agent's work, and re-pointing the ref at today's `HEAD` would abandon them where
only the reflog remembers. An agent name that is a path is refused **before anything is joined**,
because a role file's stem is a string out of somebody's repository and `.cide/worktrees/../..` is
one `git worktree add` outside the project.

`integrate` merges one of the role's branches into whatever the project root has checked out —
`cide/<role>-<task>` when the task is named (the ordinary case now; `cide_agent_integrate` and
`agents_integrate` both take the task, and the Agents panel's role-level button, which names
none, targets the taskless base branch — a merge control on the task's own card is the named
follow-up) — and computes the
whole merge **in memory** first: `merge_commits` produces an index nothing on disk has seen, and
`Index::has_conflicts` is asked before a single file is written. A conflict returns
`Integration::Conflicts { paths }` **having changed nothing** — auto-merging when a task turns
`Done` was the tempting wrong move, because a conflict would then surface as a broken checkout the
user did not ask for with no task explaining it. A refusal they can read beats a working tree they
have to repair.

**A project that is not a git repository is refused with the reason and the way out.**
`cmd::agents::worktree_refusal` names the root, says cide cannot give an agent its own worktree
there, and offers both exits — `git init`, or `"isolation": "shared"` in `.cide/config.json` — and
it is checked in two places: at `agents_config_set`, so enabling fails loudly rather than silently
falling back to a shared tree, and in the panel's `disabled_hint`, so the sentence is on screen
before the button is pressed. A silent fallback is how two agents clobber one file with nobody told.
Two consequences are written down rather than discovered: `claude` files its transcript under the
directory it started in, so a run's cwd *is* its resume identity and normalising it would orphan
every transcript the role had accumulated; and paths an agent prints are outside the project root,
so `terminal_open_path`'s containment gate asks about them, which is correct behaviour and not a
bug.

### The slot is taken at admission, and that is the whole of finding 9

`AgentRegistry` holds `runs`, a `VecDeque` per `(project, agent)`, and two counters — `agent_slots`
and `project_slots`. A pass over the queues (`admit_a_pass`) considers only the **front** of each
agent's queue, which is what makes an agent serial structurally rather than as a property that falls
out of an iteration order somebody could change; the fronts are then ordered by a monotone `seq`, so
admission is FIFO across roles as well as within one. `take_admissions` repeats the pass until one
admits nothing, so a role under `Isolation::Shared` with room for three gets all three in one call.

The interesting decision is that the slot is a **latch on the run** (`LiveRun::slot`) rather than a
count derived from the current phases, and the reason is a real window rather than a
micro-optimisation. `SessionStart` reaches `SessionState::Idle`, so a freshly spawned run reports
idle for the milliseconds between its child checking in and its opening prompt being processed. A
registry that recomputed "how many runs are working" from the phase would see that gap and admit a
second run of the same role — into the same worktree, since worktrees are per agent. So the slot is
taken at admission and released on the **edge** `Running | AwaitingPermission → Idle`, or on
`Finished`/`Failed`; `Starting → Idle` is deliberately not that edge, and
`the_idle_a_starting_child_reports_releases_nothing` is what keeps it closed. `agent_limit` and
`project_limit` are recorded on the run at dispatch for the same class of reason: a config edited
mid-queue must not release a slot that was never taken.

That leaves one thing true and worth naming: an `Idle` run has given its slot back but its `claude`
is still sitting in the role's only checkout. `idle_children_of` winds those down **at the moment
the checkout is next needed** and not on a timer, so an idle run that nothing is queued behind stays
open to be read.

**`RunState` gained `Idle` because `Finished { code: 0 }` was a lie.** The mapping this variant
replaced wrote an exit status for a process that had not exited, and the number it wrote was
byte-identical to a clean exit — so every later reader of `code` was reading a zero no child
produced. The motive behind the old mapping was sound and is preserved: a run left `Running` at an
idle prompt holds its slot and its worktree until the child dies, and the queue stalls with nothing
on screen explaining it. What it got wrong was conflating "this run has stopped working" with "this
child is gone", and only the second licenses a `code`. `Observation::Exit` is now the sole producer
of `Finished`, so a code appears exactly when the reaper has one.

### The roster paragraph, and the flag that would have stopped the pane starting

A project's primary console pane is told what it is. `cmd::session::orchestrator_paragraph` gates on
`is_primary_console_spawn` and on `enabled`, reads `.cide/` fresh on the spawn thread, and hands
`roster_paragraph` the roles; a pane that is not a project's product owner, or whose project never
opted in, gets an argv byte for byte what it was before M18. The paragraph names the roles **and**
the tool that re-reads them, and says which of the two is current: `cide_agents_list` is better and
stays live, but a session told "you have roles" with no names has no reason to spend a tool call
finding out, and this is the only channel that arrives before the first turn. Every tool it names is
spelled `mcp__cide__cide_agent_dispatch` and not `cide_agent_dispatch`, because the CLI namespaces
an MCP server's tools as `mcp__<server>__<tool>` — measured, not assumed — and a paragraph naming
the bare form would be describing tools the model cannot see under that name. An empty roster is
*said* rather than omitted, naming `.cide/agents/<name>.md` and that only the user can add one,
because a session handed nothing would call the tool, get an empty answer and have no way to tell a
failure from the truth.

It is folded in with `fold_append_system_prompt` and never pushed, since a second
`--append-system-prompt` silently deletes the first. And it is **skipped entirely** when the user's
own launch configuration carries `--append-system-prompt-file`, which is finding 5 and the reason
`carries_append_system_prompt_file` exists as a whole-token check at this call site rather than as a
table lookup. The CLI refuses the two flags together outright, so adding ours would stop the pane
starting — a regression M18 would introduce into a field that works today.
`fold_append_system_prompt` cannot repair it: it folds two occurrences of *one* flag, and these are
two mutually exclusive flags with nothing to fold into. So the call site degrades rather than
refuses — cide adds nothing, the user's file is read in full, the pane starts, and the roster
reaches the model through the tool descriptions, which are what actually make it call
`cide_agents_list` — and one `tracing::warn!` says so. The `WARNED_ARGS` row that promises this on
the Settings screen and the condition here are kept together by
`the_warned_row_and_this_file_still_describe_the_same_degradation`, because the behaviour must not
depend on a table entry staying put: deleting the row would silently turn the degradation back into
a pane that fails to start.

### The nudge: one line typed into somebody's own conversation

A run reports back only through `.cide/tasks.json`, and there is **no out-of-band channel into a
running `claude`** — nothing that can hand a live session a message which is not a keystroke. So
when a run hands its turn back, `agent_rpc::note_run_idle` fires on exactly the edge that released
the slot, and one line is written into the project's primary session's PTY, through the same
`Harness::deliver` a dispatch uses and therefore ending in `\r` with no newline in it. Three
constraints, each a bug if dropped. It is **coalesced** — a 2 s trailing window with a 10 s ceiling
— or six agents finishing together type six prompts and get six answers. It fires **only when the
orchestrator is `Idle | AwaitingInput`**, checked *after* the burst settles rather than before,
because the two seconds a burst spends settling are two seconds in which the product owner may have
started a turn of its own; a busy owner means the nudge is **dropped and never queued**. And it is
behind `nudge_orchestrator`, read off disk at the moment of each nudge.

### Pause and resume, and the SIGCONT the shutdown ladder needed

`cide_core::child_env::signal_group` was extracted out of `lifecycle::deliver` for this: pause needs
`SIGSTOP`/`SIGCONT` from a crate that may not depend on `cide-app`. `deliver` keeps only the
`Rung`→signal-number mapping, and `Rung` deliberately did **not** grow `Stop`/`Cont`, because it is
one rung of the shutdown ladder and such a variant could be handed to `stop_children`. The **group**
is the point: `SIGSTOP` to the leader alone leaves the agent's `bash` invocations and its stdio MCP
servers running while the model process is frozen, and `signal_group` keeps `deliver`'s two rules —
`kill(-pid)` with a `kill(pid)` fallback, and a refusal for `pid <= 1`, because negating that turns
one signal into a broadcast.

**The order is load-bearing and it is mark-before-signal.** `take_freezes` does the whole of the
mark first — the project goes into `paused_projects`, each run records its pre-freeze state and
takes `RunState::Paused { since_unix_ms }` — and only then does anything get signalled.
Freeze-then-mark leaves a window in which the queue writes a prompt into a stopped child's PTY,
where it *succeeds* into the kernel buffer and is read on resume, out of order with whatever the
model was mid-turn on. `the_queue_is_shut_before_any_child_is_signalled` pins it. Resume mirrors it:
`plan_thaws` reads, `SIGCONT` goes out, `finish_thaws` clears the marks and restores each run's
remembered state, and the post-thaw phase is then re-read from the hook server rather than
fabricated.

**`SessionState::Paused` carries no payload**, because the frontend needs one fact from the wire —
this is frozen — and not the pre-freeze state, which is remembered in Rust where it is used.
`is_live()` answers **true** for it: nobody pauses an idle prompt, so a paused session is by
construction one the user would mind losing, and quitting over a freeze is strictly worse than
quitting over a busy session because the turn was parked deliberately.

**A frozen turn that may have timed out is a suspicion, offered and never acted on.** cide cannot
see the model request; it sees a process that was stopped and continued. A run is at risk only if
its pre-freeze state was `Running | AwaitingPermission` and the freeze lasted `STALE_FREEZE_MS` (60
s); after `SIGCONT` a `THAW_WATCH` of 20 s looks for any evidence of life — a hook frame, or a byte
of PTY output through a sink that detaches itself — and only silence raises `AgentRun.stale_turn`.
It is **a field on the run and not an event**, because a window opened after the resume has no
history to derive it from and would show nothing where another window shows a warning. `retry_turn`
spends quota and clears it; `ack_stale_turn` clears it and spends nothing. Automatic re-dispatch was
never on the table: it would double-bill a turn that in fact survived.

**And the shutdown interaction, which would otherwise ship broken.** A `SIGSTOP`ped process does not
act on `SIGHUP` or `SIGTERM` — they go pending — so `stop_children` would spend `hup_grace +
term_grace`, 2.25 s, doing nothing and then `SIGKILL`, and a `SIGKILL`ed `claude` leaves the
half-written transcript the ladder exists to prevent. `AgentRegistry::thaw_for_shutdown` drains
every frozen session and `SIGCONT`s it, and `lifecycle.rs` calls it immediately before the ladder
runs. `a_shutdown_thaws_before_it_signals` is the test. Pause does not survive a restart, and that
is stated rather than attempted.

### Opening a run into a pane, and the second door on the mirror bug

Opening a headless run is `SplitIntent::Mirror` — a second sink on a session that already exists,
which is what a mirror has always been — and `PaneKind` stays `claude`. An `agent` kind would need
arms in six modules to express a fact the run already carries, and would silently flip
`claudePaneFocused` to false, taking `claude.fork`, `claude.mirror` and `claude.restart` away from a
pane where they all still make sense.

The gesture lives in `ui/src/sidebar/AgentsPanel/openRun.ts` and issues **no `invoke` at all**: it
re-checks `canOpen` (the roster can move between paint and click), reveals an existing pane if one
already shows that session, and otherwise calls `addRow` on the pinned console tab with `{ kind:
'mirror', session }`. That it is a frontend gesture is the fix for finding 10, which is the second
door on the bug the section above this one records. `PaneHost.mirrored` — the flag that stops
`closePane` killing a session a pane does not own — is set in `TerminalPane`'s **spawn-plan**
branch, the one reached when `takeSpawnPlan(paneId)` answers. A pane created by a Tauri command
arrives over `cide://workspace-changed` instead, so `takeSpawnPlan` never runs, the flag is never
set, and the pane adopts its session through the `sessionIsLive` path — and **closing that pane
kills the child**, which for an agent run means closing the window you were watching it in silently
terminates it mid-turn. So `agent_open_pane` was written, found to open exactly that door, and
**deleted**: from `generate_handler!`, from `contract/commands.json`, and from `client.ts`, where
the space it occupied now carries the note saying why there is no wrapper. `addRow` calls
`rememberSpawnPlan` *before* `hydrate()` — an ordering its own comment calls load-bearing — which
makes the flag correct by construction rather than by a second ownership signal.

The general rule that leaves, and it is worth more than the command that was deleted: **any future
route that hands an existing `SessionId` to a new pane must go through the spawn-plan path, or carry
ownership some other way it can defend.** "Mark every adopted session as mirrored" is *not* that
answer — a re-docked detached pane also adopts through `sessionIsLive` and genuinely does own its
child, so marking it would leak the process instead of killing it. The two cases differ only in how
the pane came to exist, which is exactly what the spawn plan records. Two behavioural details of the
deleted command are kept here in case anyone revives it: it appended the row at the *bottom* of the
console rather than directly after the console pane's row, and it validated the caller's
`WindowLabel` — a validation the frontend path gets for free by resolving the console from this
window's own mirror.

### Three things measured against the real CLI, which is why they are written here

These were run against the installed `claude` 2.x on this machine rather than remembered, and each
of them will go stale.

**`--mcp-config` accepts an inline JSON string, end to end.** `--help` documents *"Load MCP servers
from JSON files or strings"*, and the string form was verified by passing a minimal stdio probe
server and watching the model actually call its tool — the sentinel came back in the transcript.
That is the linchpin of the whole design: no file is written into the user's project and nothing is
left behind if cide is killed. Tools arrive **namespaced** as `mcp__<server>__<tool>`, so calling
the server `cide` is what makes the vocabulary land as `mcp__cide__cide_task_list`; the bare
`cide_task_list` never appears on the model's side of the wire, and that is the spelling an
`--allowedTools` line has to use. Also learned, and worth knowing before somebody wastes an hour on
it: `claude mcp list` ignores the global `--mcp-config` and reports the user's own configured
servers instead, so it is not a usable probe — only a real turn is.

**Two `--append-system-prompt` flags do not error; the first is silently dropped.** Verified
*positional* rather than content-dependent by reversing the two values and watching which token
survived. No error, no warning, no log line. Because cide appends its own arguments **after** the
user's — the ordering that is load-bearing for the variadic flags — a roster paragraph pushed raw
would take a user's own prompt away with nothing on screen saying so.
`cide_core::claude_cli::fold_append_system_prompt` is the answer: it concatenates the user's text
and cide's into one value, user's first with a blank line between, and emits exactly one occurrence.
A `WARNED_ARGS` row was the alternative and loses twice — a warning the user has to read is not a
fix for a prompt that vanishes. Two measured details shape the parser. It matches whole tokens
through `RefusedArg::matches_as` rather than by prefix, because a `starts_with` would fold
`--append-system-prompt-file`; and the two-token form takes the next token **unconditionally**, so
`--append-system-prompt --append-system-prompt-file notes.txt` makes the literal string
`--append-system-prompt-file` the prompt with no error — the opposite of the `!starts_with('-')`
swallowing rule `user_args` applies, so a fold that stopped at `-` would silently change what the
child is told.

**`--append-system-prompt` and `--append-system-prompt-file` are mutually exclusive.** Measured on
2.1.235: passing both is refused outright with `Error: Cannot use both --append-system-prompt and
--append-system-prompt-file. Please use only one.` So there is nothing to fold into, and the fold
deliberately leaves the file form alone. What makes that acceptable is that it is *loud*: a pane
that dies with that sentence in its own transcript is a bug report that writes itself, where a
prompt that vanishes is not. The call site that adds the roster paragraph degrades instead, and a
test holds that call site and the `WARNED_ARGS` row that explains it to the same story — a row
promising behaviour the code does not have is worse than no row, because a user who reads it
concludes their file is safe.

### The panels and the gates

Both panels ship whole and both are mounted: `App.tsx` renders `AgentsPanel` and `TasksPanel`, feeds
the activity rail a live count and an `awaitingPermission` badge, and holds the
`cide://agents-changed` and `cide://tasks-changed` subscriptions **itself** rather than inside
either panel, because the rail badges must stay live while the sidebar is shut and a subscription
inside a panel goes stale the moment the panel unmounts. The two events differ deliberately.
`agents-changed` carries the whole roster and **no `rev`**: it is derived from one in-process
registry with one writer, so the last emit is by construction the newest, and a revision here would
make every window rehydrate its workspace because an agent started a tool call — it is coalesced in
Rust at 120 ms with a 1 s ceiling instead. `tasks-changed` carries a `rev`, because
`.cide/tasks.json` genuinely has several writers — two cide windows and the agents — so a snapshot
can arrive out of order and the receiver must drop anything older.

The Tasks panel does **not** depend on subagents being enabled, which is stated in three files so
nobody "fixes" it: a committed task list is useful on its own, orchestration is off by default, and
gating the tracker would make the first thing a curious user clicks say "turn on a feature you have
not read about". Its board carries a **fourth** `unknown` arm the plan did not have, and so does the
roster: both reach their stores through `pendingCommand` with a `null` fallback, so there is a real
interval in which nobody has looked, and with three states the panel would spend that interval
rendering "No task tracker in this project" — a confident claim about somebody's repository, one
frame before the truth arrives. The wire enums keep three arms, because this is a frontend fact
about whether a round trip finished, not something the backend can report. `newerBoard` is the
rev-drop rule as a pure function and returns the **identical object** when it drops a snapshot, so a
store reader does not re-render. `agentChip` has three renderings for three facts — a live run lit
with its phase dot, an assigned role dim, nothing at all — because a chip that looked the same
either way would say an exited agent is still working. A comment is **markdown** (M27), rendered by
`TaskMarkdown.tsx` through the AST and React's escaping — never a `string -> HTML string` into
`dangerouslySetInnerHTML`, which is the road the old blanket refusal was really about, and a link is
drawn rather than followed. It parses with `softBreak: 'break'`, so a lone newline is a line: a
comment is a message, and the markdown *preview* pane keeps CommonMark because a `.md` file is not.
The sentence here said "never rendered as markup" for a milestone after that stopped being true, and
so did the MCP input schema agents read — so runs were told their reports would not be formatted and
wrote flat paragraphs. The Agents panel's disabled state is designed rather than
defaulted, printing the full `.cide/config.json` path *before* the enable button, because a feature
toggle that quietly adds a committed file is a surprise commit.

`check:agents` compiles both panels' import-free `model.ts` standalone with the TypeScript in
`node_modules` and pins the failure classes. It slices `TaskStatus`, `RunState` and `Harness` out of
`crates/cide-ipc/src/{tasks,agents}.rs` as source text and asserts each equals the model's frozen
list as a set, so **adding a status in Rust fails the build until the panel's tables know it**; it
pins the two copies of `LIVE_PHASES` equal, with `queued` out and `paused` in. Every member of every
vocabulary must yield a non-empty *string* — `check-problems.mjs`'s `ROGUE` lesson made structural,
where a table miss returns `undefined` and a prototype key returns `Object.prototype.constructor`, a
function React refuses as a child and `className` stringifies into the whole source of `Object` —
and `'constructor'`, `'toString'`, `'__proto__'` and `''` are each pinned as non-members. And
`canDispatch` must return **exactly one** of a green light and a non-empty sentence, never both and
never neither; it has two non-ready sentences rather than one, because saying `OFF_FOR_THIS_PROJECT`
when the truth is `ROSTER_NOT_READ` is the confident-empty-list failure in sentence form.
`check:agents-render` SSRs both panels through Vite and digests every story: `ready-queued` renders
**zero** Open elements rather than a disabled one, the stale-turn bar carries exactly `Retry turn`
then `Leave it`, `unreadable` offers only Reveal and Retry and zero writing controls, a live-run
chip uses a different class set from an assigned-role one, and `unclassed === 0` everywhere — which
is the only thing in the build that can see a `styles.typo`, since a CSS module is `Record<string,
string>`, so it type-checks, evaluates to `undefined`, and React drops the attribute in silence.

**Two things that shipped on screen and were invisible to every gate**, both found from one
report — *"the Delete button on a task's comment does nothing"*. The per-comment Edit and Delete are
drawn only when their handlers are passed, which is the right rule and left a hole: **no story
passed either**, so the render gate digested a comment log with no controls on it while the running
app drew two on every line. They are in the fixture now and the check counts them, including the
*rows* that hold them — because the second half of the report was the layout, and the layout is the
likelier cause. Both controls were spliced into the comment's head beside the timestamp, as a pair
of 40x13 targets 4px apart, revealed only on hover, at the end of a line the eye is reading rather
than aiming at. They now sit on their own right-aligned row under the comment, at rest rather than
on hover: the hover rule was argued for a 320px sidebar drawing four comments at once, and the card
has been a 620px modal since the read-only posture landed.

The other half is the one that made *any* of this present as silence. `Failures` — the toasts that
exist precisely so a rejected command is not indistinguishable from a control wired to nothing — was
`z-index: 40`, under the overlay scrim's `60`. So a command that failed **while a dialog was open**
reported into a layer the dialog was painting over: the toast rendered, behind the veil, greyed. It
is `90` now, above the switcher and the context menu as well, on the general rule that every layer
above a report is a surface the reported-on gesture can be made *from*. `check:picker` compares the
two literals rather than pinning either, because the numbers may move and their order may not — and
nothing else in the suite can see it, since no check mounts a dialog and a toast together and
neither `tsc` nor `vite build` has an opinion about paint order. Worth noting for the next reader:
`App.tsx` renders `<Failures />` last in the tree and used to say that was what put it on top. It is
not, and has not been since `OverlayCard` started portalling to `document.body` — a portalled dialog
is a later sibling than anything the React root contains, whatever order it is written in.

### Not done, and it is still a long list

**No subagent has ever run against a real `claude` in this tree.** That is the single most important
sentence in this section. Every seam short of the last hop is tested — argv construction against a
real `SpawnSpec` with no `claude` on `PATH`, the worktree against real repositories in `/tmp`, the
hook-driven state machine, the MCP server against a real unix socket with the real `cide-hook mcp`
header captured verbatim as `A_REAL_HEADER`, a run's connection being refused `cide_agent_dispatch`,
a real stopped child making no progress until it is continued, a real child's exit finishing its run
— and the hop from a Dispatch button to a role editing files in a worktree needs a GUI and has not
been performed. There is still no `crates/cide-agents/tests/real_mcp.rs`: the `#[ignore]`d real-CLI
test that would spawn a `claude` with the inline config and assert `mcp__cide__cide_task_list` was
actually called is the gate on this whole design, and it has not been written.

**The reported comment delete was not reproduced, and the honest state of it is written here rather
than closed.** The chain was measured end to end after the report: a real click dispatched at the
button's own centre through a real DOM reaches the handler (nothing covers it, and it is not
obscured by anything at that point); the host, the store and `client.ts` emit
`task_edit { edit: { kind: 'deleteComment', id } }` verbatim; that exact JSON deserialises into
`TaskEdit::DeleteComment`; and `TaskStore::edit` applied to a copy of the very file the user
reported against tombstones the comment and clears its text. So the wiring is right in this tree and
the failure was not seen. What *was* found is why it would have looked like silence either way — the
toast layer under the scrim, above — and a layout that made the control easy to miss. Both are
fixed; if it recurs, the reason now reaches the screen.

**Pause and resume were built and reachable from nothing, and that is case #21.** They are wired
now; what is worth recording is that no gate caught it. The four commands were registered, in the contract, unit-tested and
clippy-clean, and *nothing could invoke them*: no wrapper in `client.ts`, no handler passed to the
panel, no palette row. `contract-check` compares `generate_handler!` against `contract/*.json`, so it
saw no drift; `check:commands` walks `cide_core::commands`, which these had no row in. A command
wired to nothing satisfied every check in the repository. They now have wrappers, per-run controls,
a project-scope pair in the panel header, and `agents.pause`/`agents.resume` in the palette — and
`check:agents` grew the assertion that closes the class: every `agents_*`/`task*_` entry in
`contract/commands.json` must appear as a literal in `client.ts`, and every `invoke` in `client.ts`
must name one the contract lists.

**`cide_agent_integrate` is a gesture as well as a tool now.** `agents_integrate` merges
`cide/<role>` into the branch the user has checked out, and the Roles section offers it per role,
confirm-on-second-click — `TaskDetail`'s delete pattern, chosen for the same reason: the act is
recoverable (the agent's branch keeps its commits, and `worktree::integrate` refuses without writing
a byte when it would conflict), so a modal is heavier than it is worth and a single bare click is
lighter. On conflict the paths reach the user through the notice's foldable `detail`, because a
refusal reported without them is a dead end.

**`OpencodeHarness` exists now**, and both halves of what was owed came with it. The role's system
prompt travels in `OPENCODE_CONFIG_CONTENT` — a whole configuration, agent definition and cide's own
MCP server included, with no file written into the user's project and nothing left behind if cide is
killed — because opencode has no `--append-system-prompt`. `Delivery::Respawn` is implemented rather
than refused: `opencode run` is one turn per process, so a follow-up is a new child with
`--session <captured>` in the same worktree, keeping the same `RunId`, the same slot and the same
checkout, with the rebind done *before* the old child is killed so the stale exit belongs to no run.
`defs::implemented` needed no edit — it asks `harness::for_kind`, so the registry entry answered for
it — and the earlier sharp edge is gone with it: a role naming opencode on a machine that has the
binary is now dispatchable, and on one that does not it is greyed with the PATH sentence rather than
cutting a worktree first and failing in `start_child`.

Three things about that harness were measured rather than assumed, and two of them corrected a
design. **`sessionID` is on every event including the first**, which deleted the three-step
id-capture ladder the plan assumed and makes runs resumable from the first line. **Not every
`step_finish` is the turn** — a turn with tool calls emits several, the intermediate ones carrying
`reason: "tool-calls"`, and reading those as the boundary would release the run's slot mid-turn and
start the next queued run of that role into the same checkout; only a `step_finish` with any other
reason answers `RunState::Idle`. And **the inline agent's `prompt` does reach the model**, which had
been recorded as unconfirmed on the strength of a probe that told a role to answer only `ZETA` and
watched it answer the question instead. That probe could not tell delivery from compliance, so it
measured neither; a prompt carrying facts the model could not otherwise know came back verbatim, and
`opencode agent list` shows the inline role registered. An `#[ignore]`d test pins that against the
installed binary and needs no account, network or quota, which makes it the cheap check to re-run
when the schema moves.

The liveness story is the one real asymmetry with claude, and it is structural: no hook cide can
install reaches this CLI, so `CIDE_SESSION` buys nothing and is not set. The `--format json` stream
is the only channel, which is why `AgentRegistry::watch_stream` attaches a sink for a
`SessionBinding::Harness` run and claude runs attach nothing. Writing that turned up a trap worth
knowing repo-wide: **a sink must never call back into its own session from `deliver`**, because
`broadcast` walks the sink list under its own mutex and `parking_lot::Mutex` is not reentrant — the
coalescer parks, and every terminal in the process stops painting with no panic and no log line.
`PtySession::ack` is the one that gets reached for, since a sink consuming bytes is exactly the
thing that wants to return credit. The contract now says so on `Sink` itself, where the next
implementer will look, rather than only at the call site that learned it.

**Nothing in `ui/scripts/` mounts React against a live backend**, and this feature adds no
exception. That the panels repaint on `cide://agents-changed` and `cide://tasks-changed` is argued
from the call sites — the two subscriptions in `App.tsx` and the stores' `adopt` — and from the fact
that they are the only ones, not from having watched a second window's board move. The same holds
for Rust's 120 ms coalescing actually holding on screen, for two windows racing the tracker
(`newerBoard`'s drop rule is pure and checked, the interleaving is not and cannot be from here), and
for the two rail glyphs rendering on a target machine's fonts, which no check can see. Nor has a
mirrored subagent's transcript been watched *paint* — real PTY, real xterm, real WebGL. Only
`--audit-panes` touches that machinery and it still knows nothing about agents; adding an agent-pane
cycle to it is worth doing and is not claimed until then.

**Agents are now prevented from editing `tasks.json` behind the store's back**, and the enforcement
is a `PreToolUse` deny in `cide-hook` rather than anything in `cide-tasks`. The single-writer
property — one process holding the only writer — was a convention until this landed: the
stamp-checked merge *recovers* from a direct write, but recovery on a 500 ms debounce is not
prevention, and an edit that lands between a read and a write is still lost work.

The deny is local and needs no socket, which is what lets it keep `cide-hook`'s standing rule that
every failure is `exit 0`: the payload already carries the tool name, the `file_path` and the
session's `cwd`, so the decision is made from stdin and printed on stdout while the reporting half
still goes to the socket and forgets. The path is **resolved** before it is compared — an absolute
path, a `..`-relative one from a subdirectory, and a `.` component all reach the same file and are
all caught, where a string compare would have been a hole. Exactly one file is covered:
`.cide/agents/*.md` and `.cide/config.json` are the user's to hand-edit, and denying more would be
cide claiming a directory it only owns part of. And the refusal **names the alternative** —
`mcp__cide__cide_task_update`, `cide_task_comment`, `cide_task_assign` — because a refusal that does
not is one an agent simply retries.

The other half the plan named is there too: `harness::TRACKER_PREAMBLE`, one definition folded into
the role's system prompt by both harnesses — claude through a second `fold_append_system_prompt`
(so the argv still carries exactly one flag, and the user's own text still comes first), opencode
into its inline `agent.prompt`. It is gated on `hook_bin`, because a prompt naming tools the session
does not have is the bug the orchestrator's roster paragraph had to fix. A test runs both harnesses
and compares the two values, since one paragraph reaching two binaries through completely different
machinery is the only property worth pinning — a role told different things about concurrency
depending on which CLI ran it is this whole rule failing in miniature.

The fence is what *holds*; the preamble only saves a turn. Without it an agent learns the rule by
being refused, which burns a turn and leaves a transcript in which it tried the obvious thing and was
slapped. The paragraph's last sentence is the load-bearing one and is not about writing at all — it
says the task's comments are how a run reports back, because whoever dispatched it reads the task and
not the transcript, and a run that finishes without leaving one has reported nothing.
(`tools::preamble` is a different thing — it fences *tool output* against injection.)

**`.cide/` cannot be opened from the file tree**, by design and not by omission:
`Verdict::WatchOnly` means the tree does not draw it, `Ctrl+Shift+F` does not search it, and the
symbol walk skips it, so a user who wants to read `tasks.json` or edit a role file has to reach it
from outside cide — or, for a task, through the panel. Whether it should be visible — behind a
default-on setting, the way *Show hidden files* works — is an open question and nothing has been
built for it. What *did* land is the other half: `cide-headless tasks <root>` and
`cide-headless agents <root>`, which read the tracker and the merged roster through the real
loaders, so both new crates are now linked by the binary that must never be able to link a webview
and the no-tauri proof covers them. It is also the only way to see a role file's problems — its
file and its line — without a GUI, which is worth more than it sounds for a directory the file tree
refuses to show.

**`LIVE_PHASES` is `WORKING_PHASES` now**, and the rename was the smaller half. The name said "the
child exists", the doc said "how many agents are working", and what it selected was "holds a queue
slot and its role's worktree" — three meanings that agree only for `running`. `idle` is out (turn
handed back, slot released, child alive) and `paused` is in (frozen mid-turn, still holding both),
which is exactly where the three came apart. All three files moved together, as they had to:
`AgentsPanel/model.ts` defines it, `TasksPanel/model.ts` keeps a deliberate second copy (both stay
import-free so `check-agents.mjs` can compile them standalone), and the check destructures both *by
name* to pin them equal.

**Both consolidations that were owed are done**:
`cide_tasks::write_shared` is now one line over `persist::write_atomic_with_mode`, and
`claude_cli::INJECTIONS` has its fifth row, so nothing cide adds to a pane's command line is beyond
the user's reach any more. What is *not* symmetric, deliberately: a **subagent run** still gets
`--mcp-config` unconditionally, because a run reports back only through the tracker and one without
it is a run nobody can see.

## The loop closed: assignment starts the agent, @mentions, and a roster that follows the disk (M27), and what is not verified

M18 built the machinery and left the loop open at four joints; M27 closes them.

**`.cide/` changes reach the panels without a restart.** `cide_fs` always watched `<root>/.cide`,
and nothing in Rust consumed the events — so a role file an orchestrator wrote with its ordinary
Write tool (the documented road: there is deliberately no `cide_agent_create` MCP tool) was
invisible until relaunch, and a `git pull` moving `.cide/tasks.json` sat unmerged until this
process happened to write. `cide_app::dotcide` is the router: `.cide/agents/**` and
`.cide/config.json` mark the run registry's coalescer (whose flush already rebuilds the roster
from disk), `.cide/tasks.json` goes to the new `TaskStore::refresh_from_disk` — `reconcile`'s
stamp compare, so the store's own debounced writes are not events — and **the first line is the
worktrees filter**: `.cide/worktrees/<agent>` is under the watched prefix and each worktree is a
whole checkout with its own `.cide/tasks.json`, so a subagent's `cargo build` would otherwise
storm the router and its checkout's tracker would be mistaken for the project's. The global agents
directory is outside every project watcher and deliberately stays uncovered — every read path
re-reads it, and `agents_save` (the in-app editor for it) already emits.

**Assigning a task starts the role.** `Task::agent` was pure bookkeeping; now
`cide_agents::autodispatch::trigger` (pure, table-tested) decides and `cide_app::task_triggers`
acts: an assignment edge — `cide_task_assign`, `cide_task_update`, the panel's `<select>`, a
creation with an assignee — or an `@role` mention in fresh text dispatches through the same
`agents_dispatch` the button uses, so the refusal table and the one-line opening prompt are never
re-derived. The rules that earned their place: **only the user and the orchestrator trigger** (the
tools are served to runs too, and an ungated trigger would let a subagent spawn a subagent through
the side door the two-vocabulary split keeps shut); only `todo`/`doing` tasks start work; a
mention always starts the role but adopts only an unassigned task; mentions are scanned only in
the text the mutation introduced and filtered against the loaded catalog, so a typo'd `@develoepr`
can neither spawn nor rewrite the assignee field; re-assigns dedupe against open runs
(`has_open_run` — `Queued` and `Idle` count, only `Finished`/`Failed` reopen the pair); and
unassigning stops nothing — `agents_stop` stays the only kill. A started run moves its task
`Todo → Doing`, and only that hop, which is what keeps a respawn idempotent and a reviewer
dispatched onto a `review` task from yanking it backwards. `agents.autoDispatch` in
`.cide/config.json` (disk-only, default on, read fresh per burst) is the way out.

**The orchestrator now hears every ending, and a busy one hears it late instead of never.** The
nudge fired only on the handed-back edge and was dropped if the product owner was mid-turn — false
comfort ("the next run will nudge again") for the *last* run of a burst, and an `opencode` run,
whose normal end is process exit, never nudged at all. Now `Finished`/`Failed` nudge too (the exit
path delivers through `watch_exit`'s production wrapper, so the observation stays app-free and
test-drivable), the line says *how* it ended — "finished (exit 0)", "failed", "handed its turn
back" — and a nudge composed against a busy session is **held** in the coalescer and re-armed on
that session's next `Idle`/`AwaitingInput` edge, re-checked at delivery, capped at 64 turns
dropping oldest. The done-workflow convention rides every prompt: a run that finishes sets its
task to `review` and comments what it did; `done` belongs to the reviewer.

**The UI joins the two stores.** `TasksPanelHost` finally subscribes the agents roster — the
assignee dropdown had been hardcoded empty since the store landed, a wiring bug at the one seam
neither check mounts, and `check:agents-render` now source-greps the subscription so un-wiring it
fails. The dropdown carries a sentence (`assigneeHint`) when the roster is legitimately short, the
card's live-run strip got its three handlers, and `@` in any task textarea opens a mention popup —
`mentionModel.ts` (import-free, compiled standalone by `check:agents`, its opening rule
deliberately the Rust parser's) under a `MentionTextarea` that keeps the comment composer
uncontrolled and renders its open list in normal flow, where the render check can see it. The
orchestrator's system-prompt paragraph became an operating manual: role-file format and the fact
that writing one takes effect live, assignment/mention semantics, the queue-not-refuse concurrency
model (`cide_agents_list` prints each role's cap and where its runs stand — since worktrees went
per-task the declared `max-concurrent` is the true number under both isolations, and the list's
header says which checkout a task's work and its integrate branch live in), the review
convention, keep-the-plan-on-the-board, and end-your-turn-
with-a-question to summon the user through the existing awaiting badges.

**Verified the hard way, once.** The first live workstream (an opencode-harness role in a real
project) found what no gate here could: **claude ≥ 2.1.245 detects pastes by length**, so a
nudge- or opening-prompt-sized chunk written to the PTY with its `\r` in one write lands in the
composer as a paste with the Enter eaten — two nudges stacked unsubmitted in the product owner's
input box, and (probed, not just argued) an opening prompt a claude-harness run would have idled
on for ever, holding its slot and worktree. The fix is `agents::type_submitted_line`: the text
now, the lone Enter after the session leaves `Spawning` plus a 250 ms beat, used by all three
live-typing paths (nudge, retry-turn, opening prompt) — measured against the shipped binary
three ways, long-chunk/short-chunk/split. The `/bin/sh` `read` test keeps the byte shape honest
and *cannot* see this class; only typing at the real TUI can.

**And the permission answer for unattended children, which that same run decided.** The opencode
road died on the CLI's headless permission auto-reject — not refusing one tool but **ending the
turn** at it, exit 0, task never reported — and a claude run hitting a prompt parks in
`AwaitingPermission` holding its slot and its role's only worktree until somebody opens its pane.
So `agents.skipPermissions` (in `.cide/config.json`, disk-only like its three neighbours, **on by
default**, live-editable through the fs router) sends every child out promptless: `--permission-mode
bypassPermissions` for a claude run whose role names no mode of its own — the author's word always
wins, restrictive modes included — and `--auto` for an opencode run, whose module header now
records the measurement that reversed its never-pass-`--auto` stance. While it is on, the
`allowDangerousPermissions` two-acts gate stands down (refusing a role for writing down the
project's own default would be a refusal about nothing) and bites again the moment a project sets
`"skipPermissions": false`. The blast-radius containment for an unattended child is what it always
actually was: worktree isolation.

**And an opencode run's pane reads as a transcript now, not as ndjson.** The same live run
reported the raw `--format json` stream as unreadable — which it was, and the harness header had
accepted that cost knowingly. The reconciliation is `cide_pty::LineRender`: an optional per-line
hook applied once, at the top of the coalescer's output arm, so the vt100 mirror, every sink, the
frame splitter and the choked-sink catch-up all agree on one rendered stream — and the app's
stream observer moved *into* the hook, reading each raw line before returning the rendering,
which deleted wholesale the three hazards the old sink's doc agonised over (the ack-credit debt,
the re-entrant sink-list lock, the Weak-vs-Arc cycle). `opencode.rs::render_event` does the
prose: `● bash ls -la` with a budgeted output block, the model's words whole, reasoning dimmed, a
rejected permission **red and verbatim** — that line is the answer to "why did this run die", and
it used to be findable only in opencode's own database. CLI prose passes untouched; unknown event
types become a dim marker instead of a screenful of JSON. `opencode serve` + `attach` stays the
named upgrade path to a *live* TUI.

**The tracker reads and writes markdown now.** Agents write `**bold**`, fences and lists into
task bodies and comments all day, and the card drew the syntax raw — reported with screenshots,
alongside two icon regressions the same screenshots caught (the card head printed the literal
name `circle-dot`, the live-run strip printed `loader-circle`: both were `model.ts` glyph-table
strings rendered as text instead of through `Icon`/`asIcon`, which is exactly the drift those
tables' "narrow at the render site" comments warned about). Rendering reuses the **parser** the
markdown pane already owns (`editor/markdown/blocks.ts` — import-free, no HTML node in its
grammar, so model-authored text still cannot reach the DOM as markup) under a new small renderer
(`TaskMarkdown.tsx`): headings capped at card scale, uncoloured fences, tables that scroll in
place, task-item boxes drawn as icons rather than `<input>`s (the read-only-at-rest count is a
render-check assertion), and **links inert by decision** — accented, destination in the `title`,
following nothing, because the card calls no IPC and a model-authored link that navigated the
IDE is the surface the old no-markup rule was guarding. The "styling tools when creating" half
is a GitHub-style toolbar on every markdown-bearing textarea (`MentionTextarea`'s `tools`, so
the compose body, the card's body editor, the comment composer and the comment editor all carry
it): eight buttons whose transforms live in `markdownTools.ts` — import-free, compiled
standalone and driven by `check:agents`, every inline tool a toggle, every transform total. The
render check pins that the markdown story produces real `<strong>`/heading/list elements with no
marker surviving into the prose, and counts eight tools per textarea. This batch also taught
`AgentsPanel/model.ts` the **`interrupted`** phase the Rust side had already grown (tone and
placement argued in the tables; `WORKING_PHASES` deliberately unchanged — an interrupted run
holds no slot), which un-broke the vocabulary pin in `check:agents`.

**And runs survive a restart now, as `Interrupted` rows a Resume continues.** The registry was
memory-only, so quitting cide erased the run list and a halted workstream restarted itself as
nothing — `thaw_for_shutdown`'s doc even recorded "pause does not survive a restart, stated
rather than attempted"; the snapshot is the attempt. The registry's durable half (run, role,
task, prompt, the claude session id, opencode's `ses_…`, the recorded limits, the paused set)
is written to `$XDG_STATE_HOME/cide/agent-runs.json` on every coalesced flush, and the teardown
writes it once more and **seals it** — machine-local state, never `.cide/`, which is committed.
The seal is not tidiness; the first live restart found the race it closes: the shutdown ladder's
kills reached the reaper, the runs went `Finished`, and the coalescer's next flush (120 ms,
comfortably inside the ladder's 2.5 s of graces) overwrote the teardown's snapshot with the
shutdown's own kills recorded as outcomes — two paused runs restored as *history*, and Resume
had nothing to resume. At launch the rows come back
as **`RunState::Interrupted`** (a new wire variant: not `Failed`, whose conversation is spent,
and not `Paused`, which claims a frozen live child) and paused queues come back paused. Resume —
the row's, or the project header's — requeues them **through ordinary admission**, because an
interrupted run holds no slot and its role's worktree may meanwhile belong to a newer run; on
admission the continuation is built per harness: a claude run rebinds its *own* previous
`SessionId` and respawns with `--resume` (the shutdown ladder's graces exist precisely so that
transcript got written; `ClaudeHarness` grew the `respawn_spec` it used to refuse), an opencode
run passes `--session ses_…`, and a run that was still queued at quit starts fresh with its
original prompt — nothing happened, and billing a fresh start as a continuation would be a lie.
Each resumed child's first line names its task and says cide restarted, and Stop on an
interrupted row means "do not resume this" — it closes into Recent with the conversation still
on disk.

Opening a **finished** run's transcript had a multi-window hole, found live: a mirror pane's
spawn plan lives in the *clicking* window's JS realm, so a pane rendered by a different window
falls through to `TerminalPane`'s domain-session gate — which adopted a **running** session (Open
on a live run worked, by accident of that very fallback) and refused a **finished** one, so the
pane spawned a fresh `claude` over the transcript the user clicked Open to read. The gate now
asks `sessionIsHeld` — running, *or exited with the screen `report_exit` retained* — and adopts
both; the missing `mirrored` flag costs nothing on the exited arm, because closing the pane
`kill`s a process that is already gone. (`reaping` and `unknown` still refuse: one is a
mid-reap corpse the plan-carrying window never races, the other is the restore path's business.)

The first live restart sharpened the same slice twice more. **The panel's tail is a history,
not a per-process scratchpad**: the snapshot's original filter dropped `Finished`/`Failed`
rows, so a workstream that *completed* before the quit restored as nothing — now terminal runs
ride the snapshot with their states and restore verbatim (the section reads **History**;
`forget_old`'s 50-per-project cap bounds the file by construction), with their dead `session`
dropped so no Open can aim at a process the old cide took with it — the same reasoning made
`canOpen` withhold Open from `Interrupted` rows, whose session id is real but whose process is
not, and whose Open used to land on a pane-resume splash instead of anything readable. And
**a resumed run keeps its screen**: teardown writes each live run's `vt100` screen to
`$XDG_STATE_HOME/cide/run-screens/` (teardown-only — a crash keeps the runs and loses the
pictures, the right half to keep; the directory is pruned to the snapshot's runs), and the
resumed child's mirror is seeded from it through the same `SpawnSpec::preload` a restored shell
pane uses, under a dim "— resumed after a cide restart —" rule, so opening the continued run
shows what the old child last showed instead of starting blank.

**The card answers when, and who moved what.** Three reports from reading the card daily, each
now a pinned behaviour. Comment text and the body went up a rung (`--fs-ui-12` and `-13`; the
provenance heads stay at 11 — metadata under content). Every timestamp on the card reads
`14:32:07 (4m ago)` — the wall clock first, the age in brackets — because two `4m ago`s are
indistinguishable and a bare clock time is ambiguous across days; `model.ts::clock` is the pure
half, asserted by shape since the render check runs in arbitrary timezones. And **status changes
are recorded now**: `TaskEdit::SetStatus` appends `{from, to, by, at}` to a new
`Task::history` vector (`#[serde(default)]`, so every existing tasks.json parses; a no-op set
records nothing), with `by` stamped from the connection's identity exactly as a comment's author
is — so the autodispatch `Todo → Doing` hop reads as the Orchestrator's, and an agent cannot
record a move as the user. The merge unions both sides' rows structurally (immutable rows need
no id — the two hazards that broke the comments' structural union cannot arise), and the card
draws the log as a native `<details>` under the segment, **collapsed by default** — the record
you need rarely and must trust absolutely when you do. The render check pins collapsed-by-default
(a stray `open` would ship every card unfolded), scrambled-fixture ordering, and the
no-disclosure-when-empty rule.

**The first workstream filed its own debug report, and its five findings are behaviour now.**
The orchestrator left `CIDE_AGENTS_DEBUG.md` behind in its project — a subagent's-eye account of
what the loop felt like from inside — and each finding earned code. **Closing a pane no longer
kills the run it showed**, the report's one fatal: the multi-window adoption fix above lets a
plan-less window adopt a *running* session, necessarily without the `mirrored` flag (the plan
carrying it lives in another window's JS realm), so closing that pane `kill`ed the child
mid-turn — exit 129 from a SIGHUP, the precise signature the report logged. `session_kill` now
asks the registry first and refuses any session belonging to a run that has not ended
(`Interrupted` included: that is the state Resume depends on); registry-owned children die only
through their owners — Stop, the idle wind-down, a respawn, the shutdown ladder — which call the
session's `kill` directly and never pass through the command. **Every run writes a transcript to
disk as it goes**: the stream hook tees each raw line to `$XDG_STATE_HOME/cide/run-logs/<run>.log`
before the display rendering, 5 MiB per run *lifetime* (a respawned child appends under the same
cap), one truncation notice, dead-latched on any write failure so a full disk cannot stall the
coalescer thread, and pruned by the snapshot's own retention — so "what did this run actually
do" no longer dies with the screen. **A run that dies without finishing says so on its task**:
an abnormal end — `Failed`, or `Finished` with a non-zero code — appends one
Orchestrator-authored comment naming the run and its exit code, latched so it speaks exactly
once and stays silent when a sibling run of the same role is still open on the task; and the
Tasks panel draws the orphan a dead run leaves — a `doing` task with an assignee and no run
engaged renders a **stalled** chip, unlit with attention ink (`queued` and `idle` suppress it,
the system is mid-stride there; `interrupted` deliberately does not, because nothing moves until
a human presses Resume), where before it was indistinguishable from "assigned, quietly", which
is the invisibility the report named. **Re-assigning revives**: picking the same assignee again
— the dropdown, `cide_task_assign` — is an assignment *gesture* now, decided at the collection
site where the edit's shape is still in hand, so it dispatches even though the field did not
change; a status flip or comment that merely carries the assignee along still fires nothing, and
`has_open_run` still refuses to stack a second run on a live pair. **And the prompts teach
checkpoint discipline**: both injected halves now tell a run to comment its plan before starting
and to commit each coherent step — the plan comment plus the branch's commits are the only
record if the run dies mid-turn, which the report's lost work demonstrated — and
`cide_agent_dispatch`'s description states that `instructions` is one line *by design* (it is
typed into a terminal); anything longer belongs in the task body, which the run reads in full.

The report's second visit added a sixth finding, and it was the sharpest: **a dispatch to a
busy role appeared to preempt the role's active run** — run 06202dd6, sixteen minutes into a
live smoke test, died at the exact moment the dispatch that had queued behind it for thirteen
minutes flipped to `starting`, which is precisely what queue-not-refuse promises never happens.
The chain: opencode's observer read a mid-turn `step_finish` whose finish reason was not
`tool-calls` as `Idle` (the stream carries nested sessions' final steps, and the SDK has
`length`/`error`/`unknown` reasons that end a step mid-work), `Idle` released the slot, the
queued run was admitted, and admission's idle wind-down killed the "idle" child to reclaim the
role's worktree — the CLI trapped the signal, cleaned up its process tree (the report noted the
cleanup was flawless) and died as if TERMed, exit 143. The fix deletes the misreading at its
root: **no opencode line answers `Idle` any more** — `run` is one turn per process and exits
within milliseconds of its true last step, so the exit *is* the turn end, releases the slot
then, and the queued run starts into a checkout whose previous occupant is genuinely gone.
Being late costs those milliseconds; being early cost a working agent its life. The idle
wind-down is now in practice a claude-only event, which is the case it was designed for — a
parked interactive `claude` is the one idle child that never leaves by itself.

Two more findings came from the user watching the panels rather than from the report. **Pause
told a half-truth**: the run said *Paused* while the user's local model visibly kept inferring —
because a SIGSTOPped child cannot cancel the model call it already had in flight, the provider
finishes it server-side, and the bytes wait in the kernel's socket buffer to be read on resume.
cide cannot stop that call (only the child could abort its own request, and the child cannot run
an instruction; killing the connection is Stop's semantics, not Pause's), so the fix is the
sentence: `phaseHint` rides a paused row's `title` with exactly that mechanics, pinned in
`check:agents` as the one phase whose one-word label is not the whole truth. And **a role set to
`max-concurrent: 2` queued its second task**, which is what dissolved the one-worktree-per-role
design — the per-task worktree section above carries the whole argument, and the loop-closed
operating manual, `cide_agents_list` and the integrate surfaces all moved with it.

The next pair of notes closed two more gaps. **History showed "about 10 lines" of a
twenty-minute run**, and the amputation was mechanical: every snapshot path emitted `vt100`'s
`contents_formatted` — the viewport — while the mirror sat on 5,000 lines of scrollback nothing
ever read, and a headless run's mirror ran at the default 80×24 until a pane attached. A dead
session's snapshot is now `PtySession::full_state`: every retained scrollback row and then the
final screen's rows, emitted as flowing `\r\n` lines rather than a screen dump (whose leading
clear-screen would erase the just-replayed tail — the eviction-replay path survives its own
clear only because what it erases is exactly what it repaints, and here it would not be). The
same composition now feeds the teardown's `run-screens/` files, so a run resumed across a
restart scrolls back past the restart instead of starting from its last two dozen lines. Live
sessions deliberately keep the one-screen snapshot — their scrollback story belongs to the
eviction-replay machinery, and a full replay there would prepend a second copy of history to a
terminal that already holds one. And **`worktree: false` exists now** — the per-role opt-out
described in the worktree section above — because not every role needs a checkout, and the
user asked for the choice.

**Still not verified end to end**: no assignment in this tree has yet auto-dispatched a **real**
`claude` and run to `review`; the fs router's hookup runs only
in the app (`classify` is table-tested, but nothing headless drives a real watcher event through
`try_state` to a broadcast); the mention popup's DOM behaviour — caret tracking, key
claiming, the uncontrolled write — is story-rendered and source-grepped, not driven; the
rendered run pane has been driven only by `/bin/sh` printing captured event lines, not yet by a
live opencode child; and the restart round-trip is proven registry-to-registry (snapshot →
restore → requeue → admission with the right continuation, both harnesses) but has not yet been
watched across a real quit with a real child resuming. The debug-report batch is in the same
state: the kill-guard, the death comment and the revive gesture are truth-table-tested
(`the_registry_owns_a_session_until_its_run_ends`, `a_death_speaks_once_and_only_with_no_successor`,
`an_assign_gesture_dispatches_and_a_carried_assignee_does_not`) but no real pane close over a
real live run, and no real abnormal child death, has yet been watched through them; the run log
is exercised at a tiny injected cap, not by a full 5 MiB stream; and the stalled chip is
SSR-pinned in both checks, not yet eyeballed on a live board. Per-task worktrees are proven at
the registry (`two_tasks_parallelise_and_one_checkout_serialises` drives the checkout gate's
whole table, and `checkout_name`'s sanitiser has its own) but no real role has yet run two tasks
in two checkouts side by side, and no orchestrator has yet integrated with the task named; the
worktrees a finished task leaves behind also accumulate until someone removes them — a prune
gesture for integrated tasks is a named follow-up. The full-transcript snapshot is driven by a
real 200-line child in `an_exited_sessions_snapshot_carries_its_whole_scrollback` but has not
yet been eyeballed on a real opencode History row, and `worktree: false` is parse- and
gate-tested with no live opted-out run watched yet. The markdown surfaces are in the same state: markup-verified under SSR,
not yet eyeballed in the running card — the toolbar's selection round-trip (apply, selection
lands, second press toggles) is driven as a pure model but its DOM half
(`setSelectionRange` after React's commit) only runs live.

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

## Claude Code subagents as roles cide can run (M30), and what is not verified

cide read role definitions from two directories — `<root>/.cide/agents/*.md` and
`~/.config/cide/agents/*.md`. Claude Code has had its own convention for the same idea for
longer and on far more machines:
[`.claude/agents/`](https://code.claude.com/docs/en/sub-agents), project and user, YAML front
matter over a markdown system prompt. A user who had already written subagents there saw none of
them in cide's roster and could not hand one a task without copying the file and rewriting its
front matter. M30 reads both, badges them, edits them without damaging them, and dispatches them
by **naming** them rather than by re-deriving what they mean.

**Four directories, one merge, one catalog.** `defs::scopes()` is the list, lowest precedence
first — `~/.claude/agents` → `<root>/.claude/agents` → `~/.config/cide/agents` →
`<root>/.cide/agents` — and `load_from` takes a slice of them instead of the two paths it used to
take. Two rules composed: *project beats user*, which both formats already say about themselves,
and *cide's own directory beats Claude Code's*, which this milestone had to invent and which goes
that way because `.cide/agents/` is the only one of the four that can state how **cide** should
run a role (`max-concurrent`, `worktree`, a harness that is not Claude). One keyed catalog rather
than two lists, because a role's identity is its name and that name becomes a worktree directory,
a `cide/<role>-<task>` ref, a `Task.agent` and an `@mention`; two lists keyed by name would be two
answers to *who is `reviewer`*. Shadowing is never silent — `LoadedAgent::shadows` already
recorded the replaced file, and a **cross-family** shadow now also raises a warning naming both
paths, because "I edited my subagent and nothing happened" is the afternoon this costs when it is
not said out loud.

**The parser grew a dialect, not a twin.** A real subagent's front matter is real YAML: `hooks:`
is a block mapping, `skills:` and `mcpServers:` are block sequences, and the keys are camelCase.
cide's hand-written reader refuses nesting and block sequences *by name with a line number*, which
is right for a format cide defined and fatal for one it did not — a subagent with a `hooks:` block
was not degraded, it was **dropped**, because `read_definition` turns a `FrontMatterError` into a
problem and returns `None`. So `frontmatter::parse_with` takes a `Grammar`, and exactly three
things differ: a continuation line is appended to the previous key's raw span instead of refused,
a key may carry capitals, and an unquoted value that opens a YAML construct is carried rather than
rejected. Everything else — the fence, the BOM strip, the comment rule, the quoting rule, the
duplicate-key refusal, body-is-verbatim — is one implementation, because those are the rules that
took the arguing and a second copy would drift silently, each dialect being exercised only by
files of its own kind.

**`AgentDraft` stopped deleting what it could not read, and that reverses a paragraph this file
used to argue at length.** The old rule was that a save re-renders from the keys cide models and
drops the rest, on the grounds that re-emitting an unread key would be cide vouching for a line it
does not understand. That holds for a directory cide *owns* and collapses for one it does not:
deleting somebody's `hooks:` block because they fixed a typo in a description is not a defensible
thing to do to a committed file in somebody else's format. Every unmodelled key now rides
`AgentDraft::extras` — key plus **raw** value, which may be multi-line with its indentation intact
— is written back verbatim by `render`, and is shown in the Settings dialog as an editable **Meta**
row, because preserving text the form cannot show would make the form lie about what the file
contains. What is still dropped is a front-matter *comment*, and a **value** cide's vocabulary
cannot hold (`harness: bogus`, `max-concurrent: lots`): a value cide could not read is one it
cannot vouch for. Reporting did not move with preservation — an unknown key in a `.cide/agents/`
file is still an error or a warning against its line, now *reported and kept* rather than reported
and deleted.

**A subagent is dispatched by name.** `harness/claude.rs` asks one question — is this role's scope
`is_claude_code()` — and four things follow. `--agent <name>` is pushed in step 4's position
(non-variadic, so it disturbs nothing after it); the role's body is **not** folded into
`--append-system-prompt`, because `--agent` makes it the child's system prompt outright and a
second copy would say the brief twice; and `--model`, `--effort`, `--permission-mode` and
`--allowedTools` all stand down, because the CLI reads them from the definition itself and a copy
on the command line would *win* — a `permissionMode` its author wrote could be overridden by a
value cide re-derived, and `--allowedTools` would silently drop the `disallowedTools` half cide has
no flag for. The one survivor is the unattended-permission fallback: a subagent naming no
`permissionMode` still gets `--permission-mode bypassPermissions` under `agents.skipPermissions`,
for the reason it has always got it — a headless run cannot answer a prompt, and one that hits a
prompt parks in `AwaitingPermission` holding its slot and its checkout until a human opens its pane.
`skills`, `hooks`, `mcpServers` and `maxTurns` reach the child because the CLI applies them; cide
has no vocabulary for any of the four and, deliberately, does not acquire one.

**Discovery needs no materialising step, and that is luck worth pinning.** `--agent` resolves a
project subagent by walking *up* from the child's working directory, and a run's cwd is
`<root>/.cide/worktrees/<role>-<task>` — inside the project root — so the walk reaches
`<root>/.claude/agents/` on its own. Move worktrees to a sibling or a temp directory and every
project-scoped subagent stops resolving with **no error cide would ever see**: the run would simply
come up as the default agent, doing plausible work under the wrong brief. `cide-git`'s
`discovery_containment` is the test that stops that being quiet. (A *committed* `.claude/agents/`
also appears inside the worktree at the revision the run's branch points at, and being closer it
wins — same content, worth knowing when a stale one confuses somebody.)

**Names are relaxed, creation is not, and a rename is an edit.** `valid_claude_name` drops cide's
32-character cap and its stem-must-equal-`name:` rule, both of which are cide's alone; the
path-safety whitelist stays whole, with `:` refused by name because it is Claude's plugin-scoping
separator and can never be a path component. Because the stem need not agree, a Claude scope is
addressed by declared name and the file behind it is **found** by reading the directory —
`definition_path` composes for cide's scopes and searches for Claude's, and composing there would
create `code-reviewer.md` beside the `reviewer-v2.md` the user opened, leaving two definitions
claiming one name. So editing `name:` rewrites the key **in place**: no move, nothing to strand.
Creating into `.claude/agents/` is refused with a sentence naming `/agents` and `.cide/agents/`,
and moving a definition *between* the families is refused too — the field sets differ, so the file
that arrived would be a translation nobody wrote.

**What the panels say.** `AgentDef` gained `scope`, and a `is_claude_code()` row draws a **Claude
Code** chip after its name. Only those two of the four are badged: a chip on every row would say
nothing, and what a subagent's row needs to convey is a real difference in kind — no harness to
choose, a definition applied by the CLI, a file `claude` reads outside cide entirely. Both Claude
scopes draw the *same* mark, because which of the two matters when you go to edit it and not when
you are reading a roster. In Settings the Harness control **disappears** for a subagent rather than
greying — the others grey because the question is meaningful and the answer fixed; here the
question does not exist — while Scope greys, so the row still says which file is open.
`rowsFor`'s shadowing generalised from `scope === 'global'` to a rank table, which is the change
that would otherwise have drawn a shadowed subagent as the one that runs.

**Watching, and the two directories that are deliberately not watched.** `<root>/.claude/agents`
joins `watched_paths` recursively — `openspec/`'s bargain, a bounded subtree of documents, and
emphatically **not** `<root>/.claude`, which also holds transcripts and session state and would be
a storm with no upper bound. `dotcide::classify` routes `.claude/agents/**` to `Route::Agents` and
everything else under `.claude` to `Nothing` **by name** rather than by falling through, because a
fall-through is one refactor away from rebuilding the roster once per assistant token. `admits`
still says no, so nothing new appears in the file tree, in `Ctrl+Shift+F` or in the symbol walk.
`~/.claude/agents` is not watched, exactly as `defs::global_dir()` is not: every read path re-reads
it and `agents_save` emits.

**What is not verified.** No dispatch of a Claude Code subagent has ever been run against a real
`claude` from this tree — that needs a GUI and a person, as M18's own paragraph said of the loop it
built. Two facts in particular are argued from the CLI's documentation and its `--help`, not
observed. **That `--append-system-prompt` composes with `--agent`** rather than being ignored or
replacing the agent's own prompt: cide folds its tracker paragraph in that way and a run that
never hears about `mcp__cide__cide_task_*` is a run that reports to nobody, so this is the one
finding that would send the design back to translating onto cide's own flags. And **that a
project-scoped subagent resolves from a worktree cwd** — the containment is pinned by a test, the
CLI's walk is not. One further consequence is written down rather than worked around: Claude's
`initialPrompt:` is auto-submitted as the first turn under `--agent`, and cide types its own
one-line opening prompt as well, so a subagent carrying one gets two turns.

## Specs the agents work from: OpenSpec in cide (M28), and what is not verified

M27 closed the loop and left cide running unattended agents against requirements that live only
in a chat transcript. [OpenSpec](https://github.com/Fission-AI/OpenSpec) is the convention that
fixes that — `openspec/specs/<capability>/spec.md` for the requirements as they stand,
`openspec/changes/<name>/` for each piece of proposed work (a proposal, a `tasks.md` checklist,
and delta files stating `## ADDED | MODIFIED | REMOVED | RENAMED Requirements`), and an *archive*
step that merges a finished change's deltas into the specs. It is MIT, tool-agnostic, and read by
thirty other editors. **cide is a frontend to it, not a fork of it**: the brand is shown, the real
directories are named on screen, and everything a user learns here transfers to the CLI and to a
teammate's editor.

**The two systems join at one field, and nothing is mirrored.** `Task.change` names a change
folder. OpenSpec owns content; the tracker owns orchestration — status, assignee, the dispatch
trigger, the review conversation. `cide-ipc/src/tasks.rs`'s header rejects six proposed fields on
the test *"a field an agent must be taught to fill and a column the panel must draw, and neither
changes what anybody does next"*; this one changes what a dispatched run reads, what the panel
draws, and what accepting the task does. A task without it renders exactly as it did before —
which is asserted rather than claimed, because `check:agents-render`'s existing digests are
unchanged.

**Nothing in cide parses OpenSpec's markdown.** `cide-spec` shells out to the CLI's `--json`
surface. ADR 0012 has the argument in full; the short version is that the grammar carries
code-fence masking, case-folded name matching and a *schema-driven artifact set* — `config.yaml`
picks a workflow schema whose artifacts are declared with globs, so even `tasks.md` is not a
filename cide may assume. The one exception is the write path, which the CLI has no command for:
`cide_spec::block` addresses a requirement as a **byte range** and the writer splices into it,
copying every other byte, so a committed file keeps its front matter, its comments, its BOM and
its CRLF. That is the corpus round-trip test, and it caught a real bug — a byte-order mark made
every heading in a Windows-authored file invisible to the scanner.

**cide supplies buttons; upstream supplies the prose.** `openspec init` installs its workflow
into the project's own Claude session, so `init` runs `--tools claude` and never `--tools none`,
and an empty board offers *Propose a change* and *Explore first* rather than a tour cide wrote —
each typing the matching command into the pinned tab. **Which commands exist, and how they are
invoked, is read from the project directory before anything is typed** (`cide_spec::claude`),
because a table in cide was wrong twice. The first version offered `/opsx:onboard`, which is in
the CLI's templates and is *not* installed by the profile `init` uses, so Claude answered
`Unknown command` and nothing in cide could explain it. The second was the whole prefix: OpenSpec
moved the same workflow from slash commands (`.claude/commands/opsx/`, typed `/opsx:propose`) to
Claude Code skills (`.claude/skills/openspec-propose/`, typed `/openspec-propose`), and every
button in the panel then refused on every correctly set up project — with a sentence recommending
`openspec update`, which on such a project answers *"all tools up to date"* and writes nothing.
Both surfaces are enumerated, newest first, so a project set up by an older CLI keeps working;
the board carries the resolved line so the composer previews exactly what it sends; and the
refusal names what is installed. The prefix was not the only thing that moved: four of the six
commands were **renamed** with the surface (`apply` → `apply-change`, `archive` →
`archive-change`, `sync` → `sync-specs`, `update` → `update-change`), so each row carries a
rename table and cide's own handle is always the current name — a table that templated one handle
into both rows answered *this project has no such command* on the older surface for exactly the
commands a dispatch types.
`SPEC_PREAMBLE` is six sentences of cide's own rules — tick as you go, correct an artifact when
implementing proves it wrong, set `review` when done, **never archive** — and then names
`openspec instructions apply --change <c> --json`, saying in as many words that its answer
outranks the preamble. A project that installed OpenSpec's apply workflow gets one sentence more,
naming `/openspec-apply-change` and saying it outranks that summary too; a dispatch into a *live
conversation* types the command itself, with cide's task rules as its argument.

**A conversation that was already running does not have a skill written after it started.** Claude
Code reads a project's skills and slash commands once, at launch — so enabling OpenSpec from the
panel wrote `.claude/skills/`, and the very next *Propose* typed `/openspec-propose` into the
console pane that came up with the window and got `Unknown command`, with nothing anywhere saying
why. `SessionRegistry::started` stamps every fork, `cide_spec::claude::installed_at` reads the
command file's mtime, and `spec_run_command` refuses the send when the file is newer, naming the
restart — which resumes the transcript, so the fix costs seconds. The same comparison is
*advisory* on the dispatch road, which falls back to prose rather than failing.

**An archived change is still readable, and the CLI cannot read it.** `openspec archive` *moves*
the directory to `openspec/changes/archive/<YYYY-MM-DD>-<name>/`; `show` resolves
`openspec/changes/<name>/proposal.md` and nothing else, so from the moment work was accepted the
task that did it said *"this change could not be read; it may have been archived"* — a refusal at
exactly the point the record is worth keeping. `cide_spec::archived_change` reads the directory:
the deltas come back through the same byte-range scanner the write path uses, the documents come
back as a directory listing, and `SpecOrigin::Archived` labels the answer. The **checklist and the
verdict do not**, and that is the design: which file is the checklist is the schema's business and
`openspec validate` cannot see a moved directory, so both are neutral and no surface draws them —
an empty progress bar over finished, merged work, or a green *Valid* nobody ran, would each be a
worse claim than the refusal this replaced. The date stamp is stripped **as a date** rather than
as "anything before the last match", because `ends_with("-auth")` is true of `fix-auth` and a
change called `auth` would otherwise be answered by somebody else's archive.

**The same rule reaches the session row, which it did not at first.** *No surface draws a claim it
cannot support* was applied to the bar and the badge and stopped there, so a task whose work had
gone to a conversation drew, directly under its own *Archived* line, a **Working** chip, the
sentence *"It is working. The checklist above ticks as it goes."* — beside a card that draws no
checklist on this arm, by the paragraph above — and **Resume** and **Hand it elsewhere**, each of
which dispatches against a change directory `openspec archive` has moved. `primaryAction` had
refused the arm since M28; the row was the second door onto the same gesture and carried none of
that reasoning. `sessionState` and `sessionHint` therefore ask `archived` **first** and override
all three live states, because whether a pane still holds the conversation decides whether work can
*continue* in it and there is none left to continue. **Open** stays, and is why the row stays at
all: which conversation did the work is the record worth keeping, and reading it back writes
nothing.

**A task acquires a change by proposing one, never by being created with one.** The compose dialog
offered *New change from this task*: it called `spec_propose`, which scaffolds a directory with a
stub proposal, **no delta specs and no checklist** — a change `openspec validate` refuses and
whose board row reads `0/0` for ever. Writing a proposal is work and needs the codebase, so the
gesture moved to after the task exists: the card offers *Make a proposal*, `spec_propose_for_task`
runs `/openspec-propose` in a conversation, and the line tells that conversation to link the
change back with `cide_task_update` — cide cannot name the folder in advance, because the workflow
chooses it. The picker now only ever links a change that is already there.

**That link is also how the finished proposal gets on screen.** A conversation writing a proposal
takes minutes, and the gesture used to end with the change existing and nothing saying so — the
user had to notice the board row appear and open it. `spec_reveal` reads the same mutation burst
`task_triggers` does and opens the change's tab on one transition: a task that *existed without a
change* acquires one, written by somebody who is not the user. That is the propose road's shape
exactly, and nothing else's — a creation naming a change is agent bookkeeping, the card's own
control is a `User` edit made inside a modal the tab would open behind, and a task that already had
a change reads `Some` in `before` and declines. It needs no ledger for the same reason
`spec_triggers`' review hop needs none: unset-to-set happens once, and unlinking by hand is the only
re-arm. Watching `openspec/changes/` for a new directory was the alternative and is wrong — it fires
on a `git pull`, a branch switch and a checkout of somebody else's proposal.

**The three CLI facts the code turns on**, each a silent bug the other way and each pinned by an
`#[ignore]`d test against the real binary: a `--json` failure exits **0** and reports itself in a
`status` array; root resolution **walks ancestors**, so an agent's worktree without its own
`openspec/` would report the parent repository's progress as that run's; and a bare `init`
**prompts**, which on a command worker is a hang rather than an error.

**Finding the binary is its own problem.** `openspec` is installed by `npm -g`, into a directory a
shell rc file puts on `PATH` and a desktop launcher never does — while `toolchain::extra_dirs`
is `~/.cargo/bin` and `~/go/bin`, and its header forbids widening that list. So the Node rungs
live inside `cide-spec`, and `child_env::run_filter_with` hands the chosen directory back to the
child, because `openspec` is a `#!/usr/bin/env node` script and without it `execve` *succeeds*
while the shebang dies with `env: node: No such file or directory`. There is no `npx` rung: it
would download a package with nothing on screen saying so.

**Split work: one change, a task tree, and a question before anything starts.** A change's page
offered one gesture — *Start work*, which makes a single task carrying the change link. That is
the single-agent shape, and it left the checklist a proposal already carries doing no work. The
second button asks the project's Claude conversation to read that checklist with
`openspec instructions apply --change <c> --json`, create a **main** task carrying the change and
one `subtaskOf` child per coherent unit of it, look at the roster with `cide_agents_list`, say
which role should take which piece — or, on a project with no roles, offer to write them rather
than quietly doing the work itself — and then **stop and ask**. Nothing is dispatched by pressing
it: the prompt says to leave every task unassigned, because `autodispatch` fires on the assignment
edge and a helpful model filling that field in would start N subagents on a plan nobody has read.
The line is cide's own prose and not an upstream command, since OpenSpec has no opinion about
`.cide/tasks.json`, `subtaskOf`, or who may dispatch; the checklist is upstream's and is read as
data. **Exactly one task may carry `change`**, and the prompt says so with its consequence
attached, because `spec_triggers::consider_one` finds a change's task by that field and on more
than one match moves *neither* — a split that put the change on every subtask would switch the
finished-checklist hop off, silently, for the life of the change. cide refuses the same case at
the command, naming the task that already tracks it. The target is unconditionally the **primary**
session: `agent_rpc::ProjectTools::names()` serves the roster and the dispatch only to
`Project::primary_session`, so a split typed anywhere else would reach the sentence about the
roster and find no tool to look with.

**The progress bar moves while an agent works**, which needed one hole cut in the worktree storm
filter — `.cide/worktrees/<agent>/openspec/**` routes to the spec board and everything else in
that checkout still routes to nothing. `dotcide::classify` takes the project root now and strips
it first, since `openspec` is an ordinary word that can appear at `src/openspec/`; that anchoring
also fixed a pre-existing miss where a vendored dependency's own `.cide/` fired the router.

### What is not verified

- **Nothing here has been driven in the real app.** Every claim above is held up by tests —
  `cargo test -p cide-spec -- --ignored` runs eight of them against the installed CLI — and by
  `cide-headless spec <root>`, which is the only way to see the discovery ladder answer on a
  launch whose `PATH` is not a shell's. The panel, the card's spec block and the compose picker
  have been type-checked, SSR-rendered and digest-asserted; they have not been clicked.
- **The accept gesture has never run end to end.** `spec_accept` integrates and then archives, in
  that order, and its refusal table is unit-tested — but no test drives a real merge followed by a
  real archive, because that needs a worktree with a committed branch and a change whose deltas
  are worth merging.
- **Split work has never been driven in the real app.** The prompt is pinned by unit tests — one
  line, every tool namespaced, the three rules present, a length budget — and the button by
  `check:openspec` and `check:openspec-render`. What has not happened is somebody pressing it and
  watching a conversation build the tree: whether the grouping it chooses is sensible, whether it
  really leaves everything unassigned, and whether it stops to ask rather than dispatching are all
  claims about a model's behaviour that no test here can make.
- **The Review hop has not been observed firing.** `spec_triggers` is tested over path batches and
  the status gate is tested in isolation; what has not happened is an agent ticking the last box
  in a worktree and the card moving on its own.
- **The requirement editor has not been typed into.** It exists — a pencil on each delta card
  opens name, behaviour and per-scenario fields, with `+ Scenario` seeding the canonical shape and
  WHEN/THEN/AND chips inserting a clause at the caret — and its model and markup are gated by
  `check:openspec` and `check:openspec-render`. What has not happened is somebody editing a real
  requirement in the running app and watching the file change.
- **A scenario body is a textarea, deliberately, and that is a decision worth disagreeing with.**
  Rigid WHEN/THEN fields would need a parser *and* a serialiser that round-trip every scenario
  anybody ever hand-wrote; the first that did not would silently rewrite a committed file a
  reviewer had approved. The chips teach the shape without owning it. If that trade turns out to
  be wrong, the place to change it is `OpenSpecPanel/editModel.ts`.
- **`--tools claude` assumes the harness.** A project whose roles all use `opencode` still gets
  the Claude slash commands installed and nothing of its own.

## The tab strip: drag to reorder, Close to the left, and Back into a closed file (M16)

| gesture | what it does |
| --- | --- |
| **drag a tab** | move it along the strip; an accent caret shows where it will land |
| **right-click → Close to the left** | close everything before this tab, asking about unsaved work once |
| **Back / Forward** (the mouse's thumb buttons) | reopen a closed file *as it was* — split, pane ids and all |

`navigate.back` and `navigate.forward` have **no key binding on Linux or Windows** — the thumb
buttons are it. On **macOS they are ⌘[ and ⌘]**, added to `keymap::platform_layer` rather than to
`defaults()`, because `ctrl+[` *is* the ESC byte on every terminal and the key gate is a window
capture listener: the Linux spelling would take Escape-equivalent from every pane in every window.
The cost on macOS is `defaultKeymap`'s `Mod-[`/`Mod-]`, which are indent and dedent — the
capability survives on Tab and Shift+Tab (`indentWithTab`), which is what IDEA-on-mac binds anyway,
and `check:editor` pins that by command identity.

`Ctrl+Alt+←`/`→` are not them and must not be written down as if they were: `ctrl+alt+right` is
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
separate change. Half of it has since been answered: the strip's right end now carries a `▾` that
lists the tabs currently out of view and activates the one picked (`chrome/tabOverflow.ts`,
`check:tab-overflow`), so a clipped tab is **reachable** again. The rest of the sentence still
stands — a clipped tab is still not a drop target, and activating one from that list switches the
pane without bringing its tab back into view, because the strip still does not scroll.
`tab_reopen_file` resolves its plan under one lock and reinserts under another, so
a second shell window opening the same file in between can produce two tabs over one path; that is
the same window `tab_reopen_closed` has always had. And the **bulk close stops at the first
unanticipated refusal** rather than marching on — a file that turns dirty between the one question
and the closes parks a dialog about itself, and the tabs after it stay open.

## The find bar spans the file, and the path trail is clickable (M16)

Two reports about the bottom and the top of an editor pane, and both of them are about a control
that is drawn and does nothing.

### The cluster steps below the find bar, instead of the find bar stopping short of the cluster

The pane's ⊞ ⛶ ⧉ × cluster floats in the top-right corner, and a CodeMirror top panel is
`position: sticky; top: 0` across the editor's full width — so the find bar lands on exactly those
pixels, and the cluster wins the paint order (`.body` is a stacking context at `z-index: 0`, which
was added deliberately after the *reverse* shipped and buried all four buttons). The M15 answer was
to reserve horizontally: `margin-right: var(--pane-corner-clear)`, 221px on an editor pane. The user
rejected it — a search field is the width of the thing being searched — and it was also failing on
its own terms, because `.findBar`'s own controls need about 217px, so under roughly 440px of pane
the reserve started clipping the buttons it was protecting.

So the bar keeps its width and the **cluster moves down by exactly one bar**. Where it rests:

| state | find bar | cluster `top` |
| --- | --- | --- |
| nothing pinned above the buffer | — | `0` |
| find bar open | full width of the file content | `--h-findbar` (33px) |
| conflict bar up | — | `0` |
| both | full width, *below* the conflict bar | `0` |

The cluster moves in exactly one state, and it is the common one. In *both*, the find bar is no
longer the topmost strip, so none of it is under the cluster and nothing needs to move.

**The conflict bar keeps its reserve, and that is not an inconsistency — it is the rule.** A strip
whose height is a constant can be stepped over; a strip whose height depends on its content cannot.
`.conflictText` is `flex: 1; min-width: 0` with no `white-space`, so *This file changed on disk while
you had unsaved changes* wraps and the bar grows a line on a narrow pane, and a step sized for the
unwrapped case would land the cluster in the middle of **Keep mine** — which is the exact bug M13
fixed there, arriving back by a different route. That bar therefore goes on reserving
`--pane-corner-clear` horizontally, `EditorPane.tsx` marks it `data-pane-strip="fluid"`, and the
frame's rule stands down while it is up. Both bars are fully clickable in every one of the four
states above.

**"Full width" means the width of the file content, not of the pane.** The bar stops at
`var(--w-minimap)`, which is where `.cm-scroller` stops: `.cm-panels` is `z-index: 300` and the
minimap is a sibling canvas at `right: 0`, so a literal 100% would paint over its top 33px and take
its clicks. The bar's `border-bottom` now meets the minimap's `border-left`.

**A too-short pane.** `MIN_RATIO` is 0.1 and splits nest, so a pane under 60px tall is reachable by
dragging, and `.frame` has no `overflow: hidden` — an unclamped 33px would paint the cluster over
the pane *below*. It is `clamp(0px, var(--pane-top-strip), calc(100% - var(--h-panetitle)))`, and
the comment there is honest that below ~59px there is no correct resting place at all: the clamp
only chooses *inside my own frame, over my own find bar* rather than *outside my frame, over
somebody else's pane*. When the maximum goes negative the spec yields the minimum, which is `0px`.

**Terminals do not move, and that is the same rule returning zero** rather than an exception: the
cluster sits at the top of the pane's *content*, and a terminal pins nothing above its transcript.
Forcing every kind down 33px for symmetry would park four buttons over line 2 of every transcript
for ever, and the reveal is `.frame:hover`, so two clusters are essentially never on screen at once
and there is no misalignment anybody could observe.

The mechanism is `:has()` on the frame, which is this codebase's first — custom properties inherit
*downwards*, so nothing the editor declares can reach a box that is its ancestor's sibling, and
`:has()` is the only selector by which a descendant's existence reaches an ancestor's computed
style. The five alternatives that lost (including CSS anchor positioning, which is literally the
feature for this problem and falls back to `top: 0` on any WebKitGTK older than Safari 26 — and
`top: 0` *is* the bug) are written up in `PaneTitleBar.module.css`. `check:rows` pins the whole
thing: that the panel does **not** read `--pane-corner-clear` any more, that both ends of the step
name `--h-findbar`, that the token's value re-derives from the bar's own padding and control
height, that exactly one rule sets the step and it is scoped and excludes the fluid strip, that the
conflict bar declares itself, that the `top` stays clamped — and that the find bar is still the
**only** top panel in `src/editor/`, since a second one would stack inside the same box and leave
the cluster resting on it.

**Not verified by a gate:** `:has()` invalidation cost with a live terminal in a sibling pane. The
argument is that `.cm-panels-top` never appears in xterm's mutations so WebKit should not
invalidate, but nothing in `ui/scripts/` runs a browser; the four states and a deliberately short
split want `./run.sh --audit-panes` and a pair of eyes.

### Every segment of the status bar's path trail that leads somewhere is clickable

`crates › cide-core › src › lib.rs › impl Parser › parse` is drawn from `editor/statusReadout.ts`.
Three things were wrong with it and they are separable.

**It only rebuilt when the buffer was focused.** The claim stack was moved by exactly two things —
mounting and DOM focus — and a tab switch is neither: `TabContent` never unmounts an inactive tab,
so every open file holds a live claim for as long as the project does, and `file_open` is
open-*or-activate*. Switching to an already-open file therefore moved nothing until the user clicked
into the buffer, and on restore every tab mounted at once in `file_read` completion order and **the
last one to land owned the bar**, whichever tab was in front. This is *not implemented* rather than
implemented-and-unreachable — but it is a near miss of the usual kind: `TabContent.renderTree`
has taken `(tab, active)` since M4 and documents that flag for exactly this class of consumer, and
`App.tsx` wrote `renderTree={(tab) =>` and threw it away. The flag now reaches `EditorSurface`,
where a mount behind another tab is inserted at the *bottom* of the claim stack and a tab coming
forward calls `focus()`. Two further rebuilds were missing: the **outline arriving late** (the parse
is asynchronous, so a freshly opened tab had a path and no symbols until the caret moved) and, worse,
the late-*root* repair published the path alone and so **truncated** a symbol tail that was already
on screen. There is now exactly one `setTrail` call in that file, and `check:editor` counts it.

**A third bug, found on the way:** `segments` is memoized on `[path, root]` while the build effect
is keyed on `[path, reloadKey]`, so a root arriving late gave the update listener a stale array —
the repair landed and the very next caret move published the absolute path back over it. It reads
through a ref now.

**Which segments are clickable.** A segment is revealable exactly when its absolute path is at or
under a *reveal root* — a path the file tree can hang a row from. That is `rootOf`'s longest-match
containment, the same test `Groups::owner_of` applies in Rust, and the list is `fs_reveal_roots`:
the project's roots plus **each group's top-level children**. So on a dependency source,
`serde-1.0.229 › src › de › mod.rs` is live and `home › u › .cargo › registry › src ›
index.crates.io-…` is not, which is the report. The list has to come from Rust: those are *package*
directories, not the caches above them, and the SDK row's directory comes out of
`rustc --print sysroot`, so a depth rule guessed on the frontend would mark
`lib › rustlib › src › rust › library` clickable and every one of those clicks would fire the notice
this feature exists to stop showing.

The rule is `sidebar/rowPaths.ts::crumbTargets` — pure, import-free, and driven by
`check:tree-status` over all three populations (a project file, a library source, a file under
nothing) plus a scratch, a nested second root, and the degradations. It takes `pathCount`, the
number of leading crumbs that are path rather than symbol, because without it `parse` classifies as
`…/lib.rs/impl Parser/parse`, comes out inside the project root, and is drawn live.
`StatusBar.tsx` used to say so itself: *"deliberately does not know where the path ends and the
symbols begin… no crumb is clickable yet"*.

**An inert segment gets no notification, because it is not a control.** It is drawn inert instead:
the revealable run is `--dim` against the trail's `--faint`, so the boundary is legible at rest with
no badge and no icon, and only the live ones underline under the pointer. The cursor is `default`
and never `not-allowed` — a forbidden cursor is a refusal, which is the same message the user
rejected with a different renderer. A live crumb runs `runCommand('file.reveal', { path })`, the
same command ⌃⇧E, the palette row, the Explorer's ⌖ button and a Ctrl+click on a directory in
terminal output run; there is no second reveal path. In a window with no sidebar the handler is
withheld entirely, so the whole trail is inert rather than live-and-refusing.

**Where it is still only a necessary condition.** Containment is all a pure function can know:
inside a reveal root, whether a *particular* directory has a row also depends on the project's
ignore rules, so a gitignored file's crumbs are drawn live and report themselves the way ⌃⇧E
already does for the file itself. That residue is uniform — if the file is ignored, so is every
ancestor crumb of it — and closing it would mean an IPC round trip per crumb. And a library crumb
is inert until *External Libraries* has resolved: `fs_reveal_roots` deliberately never triggers a
resolution (a `cargo metadata` inside a status bar's render is not a trade worth making), so the
crumbs light up on the `cide://fs-status` the resolver emits. The store re-asks exactly when the
composed row count moved, which is provably every burst that can grow the list and no ordinary one.

**No tab stops.** `BranchSelector` is the bar's one focusable control and six to ten new focus stops
for a path trail would cost more than the gap they close; the keyboard route to the same place is
⌃⇧E and is bound. Crumbs carry `role="link"` and stay inline `<span>`s — `.path` needs
`text-overflow: ellipsis`, which requires inline content, so a `<button>` would silently coarsen the
ellipsis to whole-crumb granularity. Clicking the *last* crumb to open File Structure at its
siblings, which is IDEA's other breadcrumb gesture, is still not done.

## Which `claude` is launched, and with what (M16)

**Settings → Claude sessions now names the binary, extra arguments and extra environment**, and
every Claude pane, plus the one-shots behind *Generate commit message* and *Explain selection*, is
spawned from it. Before this the program was the literal `claude` in five places. Nothing was
un-stranded: grepping for every shape of it (`cli_path|claude_path|binary_path|extra_args|extra_env`)
across `crates/` and `ui/src/` returned zero hits, so this is **not implemented**, not
built-and-unreachable.

### The binary is checked at save, and the check is a warning far more often than a refusal

A bare name stays a bare name. `claude` is passed to `execvp` unresolved so the OS resolves it at
each spawn — the CLI self-updates, and pinning the answer at launch would keep panes on a version
that no longer exists. cide resolves it only to *check* it and throws the path away.

Only three verdicts are hard, and they are the three the OS cannot `exec`: blank, a bare name on no
`PATH` directory (`~/.cargo/bin` and `~/go/bin` included, because a desktop-launched cide has a
different `PATH` from a terminal-launched one), and a path that is not an executable file. Everything
else warns. `mise`, `asdf`, `direnv` and a plain shell wrapper are all legitimate ways to name a
`claude` and none of them answers `--version` in a shape worth refusing over — the project already
paid for the opposite arrangement once, with `~/.cargo/bin/rust-analyzer`, which is a symlink to
`rustup` and passes any on-PATH-and-executable probe before failing at exec.

**The parenthesis in that paragraph was a half-truth until M17**, and it is worth naming because
it is the shape of bug this whole area produces: cide accepted a `claude` in `~/.cargo/bin` that
was not on `PATH`, then passed the bare name to a spawn whose child searched `PATH` only, and the
check and the exec disagreed. `base_env` now appends `toolchain::extra_dirs()` to the child's
`PATH`, so the two consult the same list and the bare name is kept in every case. See *Finding
`claude` from a Finder-launched `.app`* under Platforms.

The verdict is unlatched and per-binary, which is a change: `version::check_once` describes whichever
binary was probed first in the process, which was free while `claude` was a constant and became a lie
the moment it was a setting — a user who fixes a typo would read back the verdict for the binary that
answered ten minutes ago. `run_headless` still calls the latched one for its one-line-per-process log
warning, and its comment now says which binary that line is about.

A binary that cannot be run also fails at *spawn*, as `SessionError::NoClaudeBinary`, whose written
sentence `spawnFailureText` prefers verbatim and `TerminalPane` writes into the failing pane's own
transcript. It is deliberately outside `isRecoverableSessionError`: retrying cannot help, and a pane
that retried would spin.

### What is refused, and where the refusal is legible

Seven arguments and seventeen variables. The spellings were read out of `claude --help` on 2.1.233
and every variable name was checked against the strings in the installed binary, rather than
remembered.

| refused | why it is not merely discouraged |
| --- | --- |
| `--session-id`, `--resume`/`-r`, `--fork-session`, `--continue`/`-c` | the uuid **is** the pane's `SessionId`; a second one makes every hook frame name a session this process never heard of |
| `--settings` | duplicates cide's inline hook payload; if the user's wins, every hook dies silently |
| `--bare` | authentication becomes strictly `ANTHROPIC_API_KEY` or `apiKeyHelper` — an auth failure in every pane for a Max subscriber |
| `--print`/`-p` | turns an interactive pane into a one-shot that exits |
| `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` | a key outranks subscription OAuth, so it would silently bill a Console org — and the sentence already on this screen promising cide never sets one would become a lie |
| `CLAUDE_CODE_SSE_PORT`, `CIDE_HOOK_SOCK` | cide's own; a wrong value binds a pane's `claude` to another editor's lockfile |
| `TERM`, `COLUMNS`, `LINES`, `TMUX` | cide sets these and this list is folded *after* them, so an accepted value would win |
| the four `CLAUDE_CODE_*` names above | there is a switch for each; two controls writing one variable is how the one you can see loses |
| the four proxy names | the proxy screen has three modes and three scopes so "no proxy" and "do not interfere" stay distinguishable, and it is applied *after* this list |
| `CLAUDE_CONFIG_DIR` | `lifecycle::claude_projects_dir` reads it from **cide's own** environment to find a transcript; set for the child alone, every Resume button vanishes |

`ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX` and `--safe-mode` are
**warned, not refused** — they work, they mean something serious, and they are the user's decision.
The line is whether *cide's own* behaviour becomes wrong.

**Nothing is rejected on save.** `useSettings.ts` sends its patch fire-and-forget with
`.catch(() => {})`, so a `settings_set` that returned `Err` is invisible — the field snaps back and
says nothing. Everything is therefore stored verbatim and the readout says what the child actually
gets: a refused token stays in its row, struck through (a strike, not a colour: a dim monospace input
against a dim placeholder is a distinction nobody makes at a glance and one that vanishes entirely
for a colour-blind reader), absent from the resolved argv below it. The proxy screen already made
this argument about a password and answered it the same way.

Enforcement is at the spawn as well, and that is not belt-and-braces: `workspace.json` is
hand-editable and `settings_set` is one `invoke` away from being bypassed, so a UI-only filter would
let a hand-edited file cost somebody their `--resume`. `apply_patch` makes the same argument three
times over for its clamps.

### The two orderings that are load-bearing

**The user's arguments go first.** `--add-dir`, `--mcp-config` and `--tools` are variadic and collect
every following non-flag token, and every argument cide appends begins with `-` — so a user flag
placed first can never swallow cide's session id, and one placed last would swallow whatever cide
wrote. `cide_claude::headless::argv` refuses the same wager for the same reason.

**The hooks decision is made before the binary is substituted.** `program_is_claude` matches on a
file name, and its own note predicted this exact failure: *"the moment anything passes an absolute
path as the program — a configurable CLI location in Settings, say — this returns false, no
`--settings` is attached, and every session runs with no hooks."* It is closed by ordering rather
than by teaching that function about paths: the decision is made from what the frontend asked for,
which is still the bare string `claude`, and the substitution happens downstream of it. Teaching it
about the setting would still answer `false` for `~/.local/share/claude/versions/2.1.233`, which is
the value a user pins when they want a specific version.

### Two deliberate asymmetries, stated so they are not "fixed" later

* **The extra environment reaches `claude` panes and the one-shot lane, and not shell panes** — where
  the four `CLAUDE_CODE_*` switches above *do* reach. Those names are inert to `bash`; an arbitrary
  `NODE_OPTIONS` or `PATH` is not, and a field labelled *the environment claude is spawned with* must
  not quietly become the user's shell's.
* **The extra *arguments* do not travel to the one-shot lane at all.** `headless::argv` is cide's own
  machinery — `-p --output-format json --no-session-persistence --tools` — and a user `--model` or
  `--tools` folded into it does not customise anything, it breaks the parse of a reply that never
  arrives. `ProxyScope::claude` covers both lanes because a proxy is one thing to a user; an argument
  vector is not.

**`ClaudeSettings` lost `Copy`** (a `String` and two `Vec`s cannot be) and `ClaudeCli` implements
`Debug` by hand, redacting values and keeping names — `Settings` derives `Debug`, so a
`tracing::debug!(?settings)` anywhere would otherwise print a token the user typed into the env
editor. Two tests guard it, modelled on `ProxySettings`'s, one of which reads the field list off
`serde_json` so it cannot fall behind either.

**Fixed while here:** `cide_claude::version::probe` built a bare `Command` and did not scrub, while
the identical probe in `cmd/app.rs` did and said why. Under the AppImage it ran a Node with
`PYTHONHOME` pointing inside the bundle — the ADR 0007 environment — so the version cide reported
came from a process launched in a way no pane is ever launched in.

**Not done.** No end-to-end confirmation: nothing in `ui/scripts/` mounts React against a live
backend, so "typing a bad path makes every pane print the same sentence" is argued, not observed. The
refusal list is a list of somebody else's flag names and **will go stale**, exactly as `SUPPORTED_CLI`
does — it belongs in `version.rs`'s table of what a patch release can retract, and the honest gate is
an `#[ignore]`d real-CLI test putting these shapes in front of the installed binary, which is not
written. `CLAUDE_CONFIG_DIR` is refused rather than supported; teaching `claude_projects_dir` to read
the setting is the better answer and is a change to the resume path, not to this screen. And there is
no way to spell *remove* in the environment editor, deliberately: a removal control would let a user
delete the `ANTHROPIC_API_KEY` their own login environment carries, which is the only credential some
Console customers have.

### The arguments cide adds are switchable, and renameable (M17)

**Settings → Claude sessions → *What cide adds to the command line* has one row per argument cide
injects** — the session id, the resume, the fork flag and the hook settings — each with a switch and
a spelling. It exists because a pane is the obvious place to drive a different harness (`opencode`,
or a wrapper around Claude Code) and cide always added `--session-id` and `--settings` regardless of
the configured binary: `program_is_claude` answers from what the *frontend* asked for, which is the
bare string `claude` for every Claude pane however the binary is set, and that ordering is correct
for the reason above and is not going to change.

**Defaults preserve today's behaviour exactly**, and that is the one property with no runtime symptom
if it breaks. `bool::default()` is `false` and `ClaudeCli` carries `#[serde(default)]`, so a
*derived* `Default` on `ClaudeInjection` would load every `workspace.json` already on disk with all
four injections off — no hooks and no resume, for every user, on the launch after an upgrade, from a
screen they never opened, with the pane starting perfectly well and simply reporting nothing. The
`Default` is written by hand with that paragraph beside it, and pinned from three sides:
`cide-ipc`'s old-JSON round-trip test, `cide-core`'s `the_default_injects_exactly_what_this_build_injected_before_the_switch_existed`,
and `check-claude-cli.mjs`, which reads the `impl` out of the Rust.

**What each switch costs is stated on its own row**, not in a note at the foot of the section, because
every one of them silently disables a feature that gets reported later as something else:

| off | what stops |
| --- | --- |
| `--session-id` | no transcript is filed under the id the pane is saved with: **resume stops working**, and the Resume button over a dead pane goes away. *Not* the status bar — token figures and busy/idle chrome ride on the hooks and on `CIDE_SESSION`, which cide sets separately |
| `--resume` | every restored pane starts fresh; cide stops *offering* resume rather than offering a button that quietly starts a new session (`plan_restore` and `session_resumable` both read the switch) |
| `--fork-session` | *Split → fork* continues the conversation instead of branching it |
| `--settings` | **no hooks at all**: no token or cost figures, a close confirm that cannot tell busy from idle, buffers reloading on a poll instead of on a tool write, no finished-turn notification — and no `theme`, so Claude Code draws itself dark inside a light pane |

**The refusal table and the injection table are one fact.** `REFUSED_ARGS` refuses `--session-id`,
`--resume`, `--fork-session`, `--settings` and `--continue` *because cide passes them*; stop passing
one and the refusal has to lift, or the screen disables a feature and then forbids the replacement.
Rather than two lists, each row carries `because: Option<Injection>`, and a rename moves the refusal
to the new spelling: rename the session id to `--sid` and `--sid` becomes refused while
`--session-id` becomes the user's to pass. `--continue` carries `Injection::SessionId` and not a
variant of its own — it is illegal *beside* an injected session id — while `--bare` and `--print`
carry `None` and stay refused whatever cide adds. `every_injected_flag_is_refused_and_every_injection_refusal_is_injected`
and `disabling_an_injection_relaxes_exactly_its_own_refusals` are the guards, and
`check-claude-cli.mjs` compares the `because` column across the two implementations the way it
already compares aliases and `takes_value`.

**A rename is validated by two rules, both of which discard rather than reject.** It must begin with
`-`, because `claude`'s first positional argument is a *prompt* and a bare `sid` would start every
pane by asking the model something; and it must not be another injection's spelling, defaults
included, because one flag written twice is the silent breakage this whole area is about. Either way
cide falls back to its own spelling and Rust's sentence appears under the row.

**The degraded argv shapes are the risk, and they are unit-tested.** `--resume <parent> --session-id
<minted>` without `--fork-session` is rejected outright by 2.1.227 — every Resume click failing
before a pane appears — so a fork whose fork flag is switched off falls back to the plain resume, and
a resume whose resume flag is off degrades to a fresh session rather than emitting a lone
`--fork-session`. **Those shapes have never been put in front of the real binary**: `tests/
real_session_args.rs` is `#[ignore]`d and drives the shipped defaults only.

**Not done.** Nothing here has been run against `opencode` or any other harness — the honest claim is
"cide stops adding flags", not "another harness works". The headless one-shot lane keeps its own
fixed argv (`-p --output-format json …`) and is deliberately not configurable, so *Generate commit
message* and *Explain selection* fail against a binary that is not Claude Code whatever is set here;
the screen says so. And no automated check forks a `claude` and reads a status bar, so "with
`--settings` off the pane loses exactly the four named features" is argued from where the hooks are
consumed, not observed.

## Ctrl+P can reach External Libraries (M16)

**⌥L while the picker is open — or the footer's `⌥L Libraries` chip — widens Ctrl+P to the source of
this project's resolved dependencies.** Off at every launch. This is **not implemented** rather than
stranded: every supporting piece existed and was reachable, but library files were candidates
nowhere, because `Index::build`'s sink is the only thing that ever fills the file matcher and it only
walks project roots.

### It cannot be a filter, and it is not one

The exclusion is a *structural* property, not a rule: `cide_fs::groups`'s header lists the four
couplings a grafted synthetic root would trip, and line 17 names this exact cost — *"the walk's sink
is the picker's injector | 593 crates' worth of files in Ctrl+P"*. **That decision is untouched.**
`Index` is not modified; library candidates go into a *second, opt-in matcher* on `ProjectFs`, so
`dir_paths()` (the watcher's list), `Filter::build`, `show_roots` and `path_of` are the project's
alone. "Never watched" stays true because there is still no code that could watch them.

### Measured on this repository

| | |
| --- | --- |
| external packages `cargo metadata --frozen` resolves | **593** (0.21 s, 3.1 MB of JSON) |
| files under them, with cide's exact ignore settings | **31,865** (36,643 entries including directories) |
| the project's own index | 875 files |
| walk, 593 roots at `threads: 1`, warm | **130 ms** |
| the same loop at `BuildOptions::default()` (`threads: 0`) | **1.24 s** |
| the whole of `~/.cargo/registry/src` | 127,055 files |
| `~/go/pkg/mod` | 347,777 files |

The last two rows are the point: *index the libraries* and *index everything this machine has ever
built* differ by two orders of magnitude, and only the first is bounded by the project the user has
open. The `threads: 0` row is the trap — the cost is not the walking, it is spawning and joining 32
walker threads 593 times over directories that hold 60 entries each — and it has a source assertion in
`check:picker`, because nothing else in the suite can see a 20× slowdown.

### Lazily, and visibly

Nothing is walked until the scope is switched on for the first time in a process. `picker_index_
libraries` claims the walk and returns; the overlay watches it fill through the poll it already runs,
so `running` stays true and the counter climbs — the same shape `symbols_index` and `symbol_query`
have had since M12. If nobody has opened *External Libraries*, the walk asks
`ProjectGroups::resolve_now` for the packages, blocking on `cargo metadata` on a pool thread: the
same call `fs_reveal` already makes for *Select Opened File*, on the same argument — the gesture
explicitly asked for this, it happens at most once per project per process, and the alternative is
not "faster", it is an empty answer.

A `cargo add` or `cargo update` invalidates it. The lockfile stamp already notices (it is a stamp and
not a watcher subscription because `Cargo.lock` is gitignored in plenty of repositories and no
`cide://fs-changed` would ever mention it), and that path now calls `forget_libraries` *before*
spawning the resolver — the stale set is wrong now, and the resolver runs for a quarter of a second
during which Ctrl+P would otherwise keep offering it. It does not re-walk; the next query does.

### Two matchers and a merge

Forced, for the reason `symbol_matcher` is — `Matcher::query` holds one query per matcher — plus one
specific to this: **nucleo is append-only**, so a toggle-off could not un-inject 32,000 candidates,
and post-filtering a 200-row frame would break both the row limit and the `6 of 2,418` counter, which
come from the snapshot.

Merging is exact and free. `Pattern::indices` **returns the score** and `frame()` was already calling
it for the highlight offsets and throwing the return value away; `frame_scored` stopped throwing it
away. The rows are already sorted within each side, so it is a two-finger merge, `matched`/`total` are
sums, and `running` is the OR. **Ties go to the project**, and that decides the common case rather
than an edge one: an empty query scores every row identically, and `lib.rs` typed in full matches this
project's and `serde`'s identically too.

### Telling the rows apart

A library row draws `serde-1.0.229/src/lib.rs` — the package directory prefixed for free by
`Visitor::item`, so a user can narrow to one crate by typing its name — and a right-aligned dim chip
carrying `serde 1.0.229`. That is one nullable `PickerRow.source` and not a flag beside a label, so a
row can never be marked as a library with nothing to show for it. The version is in the chip and not
in the matched column: it is a label you read to tell two rows apart, not a thing you search for, and
in the haystack it would make every crate's files match every other crate's version digits. `.source`
is `flex: none` and `.path` is `flex: 1`, so the path ellipsises first — a path is reconstructible
from a name and a package, and a package name cut in half is not.

### The chord, and why it is a command

`⌥L` is bound in `cide_core::keymap` under a new `filePickerOpen` context flag, not handled locally
in the overlay. A local `if (ev.altKey && ev.key === 'l')` would be unrebindable, unlisted and
undiscoverable — the shape that produces this project's recurring defect. The clause is
`filePickerOpen` and **not** `overlayOpen`, which is true for any of nine overlays: the gate is a
window *capture* listener, and ⌥L is `ESC l` to a shell (readline's `downcase-word`), so the wider
clause would have taken it from every terminal pane in every window whenever a menu happened to be
up. `ctrl+alt+l` was the obvious alternative and is KDE's Lock Screen on a stock install, so on the
development platform it never reaches the app.

The footer chip is an `aria-pressed` button (the app's precedent is `SearchPanel`'s; nothing in the
overlay language uses a checkbox) that commits on `mousedown` with `preventDefault` — a click blurs
the input between the two events and `ModalShell`'s `selectionchange` listener repaints the caret,
after which typing stops working. **One element documents the chord and is the mouse target.**

The scope is window-session state in the overlay store: it survives closing and reopening the picker
within a run, and resets on relaunch, so "disabled by default" stays true in the sense a user checks
it. Persisting it would make that true exactly once in a user's life and would silently change what
Ctrl+P costs on every project thereafter.

**Not done.** No end-to-end confirmation on screen — the merge, the chip and the toggle are asserted
in Rust and in `check:picker`, and nothing in `ui/scripts/` mounts React against a live backend. Go
projects are **untested at this scale**: `go list -m all` bounds the set the same way, but a Go
module's directory is the whole module rather than a package, and the numbers above are cargo's alone.
`.a` and `.so` files and trybuild `.stderr` fixtures are indexed like everything else the walk admits
(2,829 `.a` files in this dependency set), because a second ignore rule beside `Filter` is a thing
this codebase resists on principle. The registry is machine-global and immutable, so two open projects
sharing 500 crates each pay their own 130 ms and ~12 MB; a process-global cache keyed by package
directory is the obvious later move. And `Matcher::dirty()` is still an orphan — declared,
implemented, and called by nothing in the workspace, not even a test. That is pre-existing and
unrelated, and it is recorded here because it was found on the way through.

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
| **F7** / **Shift+F7** | next / previous change, in a diff or the conflict resolver (M25) |

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
`tab.prev` still carry no `shellWindow` clause, but the latent issue that used to imply — from a
detached-tab window they would move the *shell* window's active tab — was closed when detached-tab
windows became creatable: their dispatch arm answers `unmet` in one ("this window shows one tab").
The palette's key chip prints `f4` lowercase, because `chipLabel` upper-cases single characters only
and has no f-key glyph — `ctrl+f12` has always rendered `⌃f12`.

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
sentence in the notice stack (`warn`, not an error — "used nowhere" is a *result*), and it is a
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
the editor resolves to no row either. And none of it has
been confirmed on screen: the gesture needs a pointer, KDE will not raise a shell-launched window,
and the Wayland compositor exposes no capture protocol.

## An open editor tab follows its file (M25)

Until M25 nothing retargeted an **open editor tab** when its file moved, and the second half of that
was worse than the first. A `TabKind::File` tab is an absolute path, and `cide_core::document::write`
canonicalizes before it writes — so a rename left the tab showing the old name over a buffer that
**could not be saved at all**: `ENOENT` in the log, a dirty dot with no gesture that could clear it,
and an autosave notice on a timer. The tree was right and the editor was silently stranded.

`cide_core::workspace::retarget_paths` is the rule, called by `fs_rename` and by the **cut** half of
`fs_paste` once the disk agrees, never before. It matches a tab's path exactly *or* as a directory
prefix — renaming `src/` moves every tab beneath it — and it rewrites the editor panes' headers with
it, in the tab's own tree and in `project.detached`, exactly as `retarget_diff` does for a diff torn
into its own window. It reaches **every project**, not the one the gesture came from: the path is
absolute, `TabKind::File`'s own documentation makes the point that nothing there claims the file is
even in the project, and the file moved for whoever has it open. `rev` moves only when a tab did, so
the common rename — of a file nobody has open — tells no window anything. `PositionsState::rename`
moves the remembered scroll with the same rule, so renaming a file you are reading at line 400 does
not throw you to the top of it.

**Unsaved edits survive it.** A rename copies nothing, so the bytes on disk are the buffer the user
had already edited past; re-reading them would replace their work at a moment when nothing on screen
said a buffer was about to be thrown away. `EditorPane` carries the live text across instead and the
tab stays dirty, so the next Ctrl+S writes it to the new name. The **undo history does not survive**:
`EditorSurface`'s build effect is keyed on `[path, reloadKey]` and has to be, because the language is
resolved from the extension — `notes.txt` renamed to `notes.md` is a different grammar.

**What is deliberately not retargeted**: a `Diff` tab (a `ClaudeMcp` one is holding an agent's turn
open on a fetch key, and a `Git` one spells its paths repo-relative), and `Revision` and `Merge` tabs,
which are repo-relative and name blobs a working-tree rename does not touch. **Not confirmed on
screen** — the gesture needs a pointer, for the reason above. The rule is covered by
`cargo test -p cide-core workspace`, including the folder-prefix case and the near miss beside it
(`src/network.md` is not inside `src/net`).

## Project Notes (M17), and what is not done

**Project Notes** is one pinned row at the **top** of the file tree — above *External Libraries*
and above *Scratches* — that opens one markdown file per project in an ordinary editor tab. Double
click it, press Enter on it, right-click it and choose *Open*, or run *Open Project Notes*
(`file.projectNotes`) from the palette; all four go through the one command, which is why they
cannot come to behave in four ways.

**Where the file is.** `$XDG_STATE_HOME/cide/notes/<blake3 of the project's canonical first
root>/notes.md`, beside the scratch drawer and keyed identically — `cide_core::notes` reuses
`cide_core::scratch::key` rather than writing a third copy of the hashing rule. Not inside the
project, for the reasons a scratch is not, plus two that are specific to a pinned row: cide would
be creating an untracked file in somebody's repository on a single click, and the file would be
drawn **twice** (once as the pin, once as a walked row) or — once gitignored, since the walk is
gitignore-aware — as the pin and never as itself. A sibling `<key>.json` records which project the
directory belongs to. A multi-root project has **one** notes file, keyed by `roots[0]`, which is
the identity `RecentProject` and the scratch drawer already use.

**Nothing is created at project open.** The row is drawn from a constant label; the first click
runs `fs_notes_ensure`, which creates the directory and the file. That call is idempotent and uses
`create_new(true)`, so the second and hundredth clicks cannot truncate what the user has written —
that is the one line in the feature that could destroy data, and `cide_core::notes` has a test
whose only job is to prove it.

**The tab is an ordinary File tab**, deliberately and with no new tab kind: save, the dirty marker,
undo, find-in-file and the markdown grammar all work because nothing about it is special.
`file_read` and `file_write` apply no containment check — only the dependency-cache read-only rule,
which this path is not under — so the editor saves it.

**The row is not a file, and everything that assumes a row is a path excludes it.** It is a new
`TreeRowKind::Pin` whose `path` is the same `cide://group/<id>` sentinel a group header carries, so
it is not absolute and `cide_fs::ops::check_within` refuses it: rename, delete, drag, drop, *Copy
Path*, *Reveal*, the clipboard and the trash confirmation all decline it by rules that already
existed. `rowVerbs('pin')` grants `openable` and nothing else. The fs watcher never sees it — the
file is outside every root, so it is never walked, never watched, never a Ctrl+P or content-search
candidate.

**The write/rename asymmetry is deliberate.** The editor saves the file; the tree will not rename,
cut or trash it, because the notes directory is **not** in `ProjectGroups::writable_dirs`. A rename
would leave the pin pointing at nothing and the next click would create a second, empty `notes.md`
beside the user's real notes. Adding the directory to `writable_dirs` to "make it consistent"
re-opens that bug; the comment there says so.

**Divergences and what is not done.** The notes cannot be committed or shared with a team, and a
project that is moved or renamed gets a new key and appears to have lost its notes — the same cost
scratches, changelists and the shelf already pay, unmitigated beyond the breadcrumb. If
committable notes are ever wanted the honest shape is a setting with two values, not a change of
default: `cide_core::notes::file_for` is the only thing that decides the path. There is no default
binding (the palette and `keymap.json` are one step away), no garbage collection, and no watcher —
an edit made by another program is noticed by the editor's own conflict machinery on the next tab
activation, not by the tree. **And none of it has been confirmed on screen.** The coverage is
`cide_core::notes`, `cide_fs::groups`, `cide_app::notes` and `check:notes`; no check script can
mount CodeMirror, so that the tab really highlights markdown and that the row really draws the
markdown glyph above *External Libraries* are claims only a manual `./run.sh` can settle, and
neither audit covers this feature.

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

**And a read-only buffer could not be focused at all, which killed the entire editor keymap in
it.** (M16) Reported as *"Ctrl+F opens no find bar in a std-library file"*; the find bar was never
the problem. `findExtensions()` is unconditional in `EditorSurface`'s extension list, and
right-click **Code ▸ Find…** opened the bar in those buffers the whole time. The cause is one
missing attribute: `EditorView.editable.of(false)` gives `.cm-content` `contenteditable="false"`,
such an element with **no `tabindex` is not focusable**, and CodeMirror registers every DOM handler
— `keydown` included — on `contentDOM`. Events bubble up, so the listener never fired. Clicking the
buffer focused `.cm-scroller` (contentDOM's *parent*, which carries `tabIndex = -1`) instead, and
every `view.focus()` in the codebase was a no-op against it.

So it was never one chord. `Mod-f`, `F3`/`Shift-F3`, `Mod-d`, `Mod-Shift-l`, `Escape`, `Alt+Enter`
(*send lines to Claude*) and **the whole of `defaultKeymap`** — arrows, Home/End, PageUp/PageDown,
`Mod-a` — were dead in every library and toolchain buffer. It read as alive because `.cm-scroller`
is the focused overflowing element, so those keys still scrolled the pane; what was actually missing
was the **caret**, which the base theme hides outside `.cm-focused`. And `view.hasFocus` is
`activeElement == contentDOM`, so it was permanently false — which is why the update listener's two
slot re-claims never fired, and why Ctrl+G, Ctrl+F12, ⌥F7, Ctrl+B and Back all acted on whichever
*editable* file was touched last while the status bar went on naming it.

The fix is `EditorView.contentAttributes.of({ tabindex: '0' })` beside the `editable` facet, which
merges over the computed attributes and leaves `contenteditable="false"` alone. Dropping
`editable.of(false)` would also work and loses: it makes `.cm-content` a root editable element
again, which is exactly what `codeMenu.tsx`'s focus-restore note relies on read-only buffers not
being, and it puts an IME and a native caret in a document that cannot change. Binding `ctrl+f` in
`cide-core::keymap` fights `nothing_binds_the_find_bars_f_keys` and would have fixed one chord out
of twenty. `check:editor` asserts the pairing inside the `if (readOnly)` block, on
comments-stripped source, because that file now argues about tabindex at length.

**Still unmet:** the same shape exists on the `a` side of `panes/DiffPane.tsx`, where it is
*deliberate* — "a click does not place a caret in a document that cannot be changed" — so Ctrl+F in
a split diff still only works after clicking the right-hand side. That is a separate decision and
was not folded in here. And the end-to-end confirmation (open a toolchain path, click, assert
`document.activeElement` is `.cm-content` and `.cm-editor` carries `cm-focused`) needs a display and
belongs in the `CIDE_AUDIT_PANES=1` harness; no check script anywhere constructs an `EditorView`.

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

**And what `Alt+Up`/`Alt+Down` cost was a comment, not a compensation.** `keymap.rs` has claimed
since M12 that "`EditorSurface` re-homes `moveLineUp`/`moveLineDown` to `Mod-Shift-Arrow` — IDEA's
own chord for the same thing — which is free in both layers, so a capability moves rather than
disappearing." Both halves were false and it took until M16 to check: `grep moveLineUp ui/src`
returned nothing, so move-line-up/down had simply been gone from every buffer since the member walk
claimed Alt+Arrow — with no palette row and no menu item either; and `Mod-Shift-Arrow` is *not* free
on macOS, where `standardKeymap`'s `{ mac: 'Cmd-ArrowUp', shift: selectDocStart }` claims ⌘⇧↑. The
binding exists now, spelled `Mod-Shift-Arrow` off macOS and `Mod-Alt-Shift-Arrow` on it, and
`check:editor` **computes** the freedom of both — expanding the composed CodeMirror keymap the way
CodeMirror expands it, `shift:` sub-bindings and `mac:` overrides included — rather than trusting
the documentation, since trusting the documentation is what produced the sentence being replaced.
`check:keys` holds the other end: the re-homed chord is read out of `editorKeys.ts` and asserted
**unbound in `cide_core::keymap`**, on both layers, because the gate is a window capture listener
and a `ctrl+shift+up` added there next year would delete the capability a second time in exactly
the way it was deleted the first — with no conflict visible to either side alone. The binding sat
inline in `EditorSurface.tsx` until M17 moved it into `editorKeys.ts`; see *Ctrl+D duplicates a
line* for what that was worth.

**Still chord-only.** Move line up/down has a key and nothing else: no palette row, no *Code* menu
item. `moveLineUp` is a `@codemirror/commands` function, not a `cide-core::commands` id, and giving
it one means an id, a `when`, a dispatch arm and a route from `keys/dispatch.ts` into a specific
pane's `EditorView` — which is the seam `paneHosts.ts` exists to keep closed. Named here rather than
half-built, since naming a cost and then not paying it is the failure this whole section is about.

**Verified against real servers.** `cargo test -p cide-lsp -- --ignored` drives the real binaries;
**CI does not run it**, so run it by hand after touching that crate. All eleven pass — `gopls`
reporting on a module, rust-analyzer indexing this workspace and reporting an introduced type
error, the shutdown ladder actually stopping a server, the on-disk-edit test below, a real
`textDocument/definition` resolving a reference to its declaration, (M14) a real
`textDocument/references` listing the call site of a declaration and *not* the declaration itself,
and (M18) the four that back the section after next: the flycheck kick clearing a diagnostic an
on-disk edit alone would not, gopls re-diagnosing a file it was only *told* had changed, and
definition-versus-implementation giving two different answers at one caret in Go and the concrete
`impl` in Rust. Run against rust-analyzer 1.92.0 and gopls v0.21.0.

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

## Diagnostics that follow the disk, and a button to re-run them (M18)

Reported as *"rust-analyzer doesn't catch every change — after Claude fixes some hints I still see
them, and clicking one goes to some comments rather than the line, and I have no manual button to
re-run it in the Problems panel."* Three separate defects wearing one sentence.

**Nothing carried an out-of-editor write to a language server.** Document sync (above) covers the
buffer the *user* is typing in. It says nothing about the file an agent just rewrote in a tab
nobody opened — which, in an editor whose centre of gravity is a Claude session, is most of the
writes. The watcher's events reached the file tree, the picker and `cide-lang`'s symbol index and
stopped there. `FsEvents` now has a third method, `files_changed`, and `ProjectDiagnostics::
files_changed` decides what each server needs:

- **gopls gets `workspace/didChangeWatchedFiles`, at once.** It has no watcher of its own — it
  registers watch patterns through `client/registerCapability` and then waits — so this
  notification is how it learns anything the editor did not tell it. That also means the client
  capability has to be declared, and it now is, **for gopls only**.
- **rust-analyzer gets `rust-analyzer/runFlycheck`, debounced.** Deliberately *not* the
  `didChangeWatchedFiles` capability: declaring it transfers responsibility for file notifications
  to the client, and cide's watcher is gitignore-filtered and confined to the project's roots,
  where rust-analyzer's own watcher additionally sees path dependencies and `~/.cargo/registry`
  sources. Narrowing what it sees would trade a stale-diagnostics bug for a wrong-answer bug. Its
  problem was never file notification anyway — semantic analysis already followed the disk; it is
  *flycheck*, the `cargo check` behind every `E0308`, that only ran on a save. The kick is
  coalesced (500 ms trailing, 5 s ceiling) because a `cargo fmt` over four hundred files must
  produce one `cargo check` and not four hundred.

`an_on_disk_edit_alone_never_refreshes_diagnostics` is unchanged and still asserts the negative —
it sends nothing, so it stays true. `a_flycheck_kick_refreshes_diagnostics_an_on_disk_edit_alone_
would_not` is its exact inverse and is the only gate on the extension still existing: a
notification produces no reply, so a server that stopped implementing it would drop the kick in
silence.

**A stale row kept its old line number, and said nothing about it.** That is the "clicking it goes
to comments" half, and it is true for a window even with the above fixed. `Diagnostic` now carries
`stale`, set in `DiagnosticStore::snapshot` from a `(source, path)` dirty set the watcher marks and
a publish clears. Keyed by source and not by path alone, because tree-sitter re-parses the open
buffer on every keystroke while rust-analyzer's flycheck may be twenty seconds away, and a
path-keyed set would let the first un-mark the second's rows — under-reporting, which is the defect
itself. The row stays clickable and says *"changed since it was checked"*: a jump that may be a few
lines off beats a dead row, as long as it is labelled.

**The panel had no controls, and one of them already existed.** `diagnostics.restart` shipped in
M12 fully implemented, unit-tested, registered in `contract/commands.json` and wrapped in
`client.ts` — with **no caller anywhere in the app**. So did `snapshot.sources`: on the wire since
M12, converted by `adapt.ts` on every emit, rendered nowhere, which meant *"rust-analyzer is not on
PATH"* — the one sentence that explains an empty list — crossed the IPC boundary and was thrown
away. The panel now has a footer listing every analyser with its status, a **Restart** button on
the two that are processes (tree-sitter and Claude get none, because `restart` returns immediately
for them and a button that does nothing is the failure this panel exists to avoid), and a
**Re-run** button beside it. Re-run is the cheap one — seconds, no re-index — and is also
`problems.refresh` in the palette. **No default binding**: IDEA and VS Code ship none for their
equivalents, and a chord is taken from every terminal pane in every window.

## Go to implementation, which is a different question from Go to definition (M18)

Reported as *"goto for golang goes to the interface declaration, but should go to the
implementation."* gopls is answering correctly: `textDocument/definition` on a call through an
interface resolves to the interface's method, because that is where the callee is declared. cide
simply never asked the other question.

**Ctrl+Alt+B**, IDEA's own chord, on a new id `navigate.implementation`, scoped `editorFocused` for
the reason every other member of that group is — the key gate is a window *capture* listener, and
Ctrl+Alt+B is `ESC ^B` to a shell. Ctrl+B and Ctrl+click are untouched and still mean "definition".

Teaching Ctrl+B to try implementation first and fall back was the obvious fix and it regresses
Rust: rust-analyzer answers `textDocument/implementation` on a struct name with its `impl` blocks
and on a trait with its implementors, so Ctrl+click on an ordinary type name would stop opening the
declaration. Changing the most-used gesture in one language to fix a complaint in another is the
trade `navigate.usages` already refused once, for the same reason.

It reuses the Find usages popup wholesale — same 0/1/≥2 rule, and ≥2 is the *common* case here
because an interface with many implementors is the normal shape — with one difference and one
correction. The difference: **0 falls through to Go to definition**, so the command always does
something wherever a caret is. The correction: the popup's prose branches on which question was
asked, because telling a user who pressed Ctrl+Alt+B *"no usages of ‘Reader’ outside its
declaration"* claims their interface is unused, which is not what they asked and not true.

`gopls_goes_to_the_implementation_rather_than_the_interface` asserts **both** halves at one caret —
definition landing on the interface's method, implementation landing on the concrete one — because
a test that only asked the second would pass on a build where the separate command had become
pointless. It also caught something worth knowing: gopls answers `textDocument/implementation` on a
position that is not a type with a JSON-RPC **error** (*"s is a var, not a type"*) rather than with
`null`, so `RequestError::Failed` is folded into `NotFound` — the server's words go to the log, and
the user gets the definition they were reaching for instead of a sentence that reads like a fault.

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
KDE both are usually the compositor's virtual-desktop shortcuts — and `alt+left`/`alt+right` have
since been spent anyway, on `pane.navigate.left`/`right`: the readline word-motion cost this
paragraph once refused to pay for a navigation nobody had asked to put there was later paid for a
pane move asked for by name, and the trade is recorded at length on the binding in `keymap.rs`.

**Only an explicit navigation is recorded — and, since M16, a far pointer click.** The list of what
deliberately is *not* is in `ui/src/editor/navHistory.ts`: typing and arrow keys, scrolling,
find-as-you-type, the `Alt+Up`/`Alt+Down` member walk (a held key, so ten presses would be ten
entries), switching between already-open tabs, edits, and a Back/Forward move itself. A history that
records caret moves is what makes Back useless.

The pointer was the **one input device that moved the caret and wrote nothing**, which left Back
empty for anyone who navigates by pointing and Forward empty for everyone — Forward only fills once
you have gone Back. `navHistory.ts`'s header had predicted the complaint and written down why the
entry was missing; the objection was that the rule would have to live inside `EditorSurface`, which
is documented as pure. It was answered by moving the *rule* rather than the component:
`recordsClick` is in the import-free module `check:editor` already compiles standalone, and
`navRecorder.ts` is a `ViewPlugin` that reads four facts off a `ViewUpdate` and asks. A click
records when it is a **single empty selection**, from a `select.pointer` transaction, with no
document change, landing **more than 25 lines away or in another file**. So a drag records at most
the one entry its `mousedown` earns; a double-click, a triple-click, a shift-click and an Alt+click
multi-caret record nothing; and clicking about inside the function you are reading — by far the most
common click there is — is not going anywhere. Twenty-five must exceed the merge distance of three,
or `record` would collapse every entry the rule produced.

Two orderings make it work and both compile backwards. CodeMirror runs plugin updates **before**
update listeners, which is what lets the recorder read `caretTrack`'s slot before `EditorSurface`'s
listener hands it to the editor being clicked into — so a click from one pane into another records
the pane you left. And `jump.ts::recordClick` **takes** the origin rather than reading one: read it
a listener later and the origin is the destination, `near` merges them, and the feature is a silent
no-op. The recorder falls back to `update.startState` whenever the live caret is already in this
file, so if that CodeMirror ordering ever changes the cost is a *missing* entry, never a wrong one.

Two consequences, stated rather than left to be met on screen. **The first click after Ctrl+Tab
records the tab you came from**, however little it moved the caret: `caretTrack`'s claims are
released on unmount and an inactive tab is never unmounted, so the slot still names the old file
until something in the new tab takes focus. The switch alone still records nothing — what records
is a pointer landing in a document the caret was not in, which is the rule applied exactly, and it
is what makes Back after a tab switch go somewhere. And **clicking to and fro between two panes of
a split fills the stack with the alternation**, one entry per crossing, because two different files
are never `near` each other; `NAV_CAP` is 50 and that is now a number a session can actually reach.
Both are IDEA's behaviour. The alternative — take the origin from the clicked pane's own previous
caret instead of the slot — is more consistent with "a tab switch is not a jump" and loses twice:
it breaks the split case the feature is for, and it makes Back from a click disagree with Back from
Go to definition about what an origin is.

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


### And the one position where Go to definition asks the other question

The above shipped as a second binding, and the report came back unchanged: *"goto for golang works
not as i expected - it goes to interface declaration, but should go to implementation."* A feature
on a chord the person who asked has not been told about is the same non-delivery as a feature
behind a default they never saw.

So Ctrl+B and Ctrl+click now redirect — from **one** position, decided by parsing the file the
answer points at rather than by looking at the language. `cide_lang::interface_method_at` asks
whether the definition landed on a method inside an `interface { … }` block. Only that answers
yes, which is exactly the case the report is about and exactly the case where the declaration is
useless. Everything else is untouched, and the two obvious regressions are both unreachable by
construction:

* a Rust struct or trait name lands on its own declaration, never inside an interface body, so
  Ctrl+B never turns into a jump to an `impl` block;
* a Go interface **type** name — `io.Reader` — lands on the type spec, which is *outside* the
  brace block holding the methods, so the declaration still wins.

`navigate.implementation` keeps Ctrl+Alt+B, because "what implements the thing I am standing on"
is a question worth asking from the positions the redirect deliberately never fires on. The two
gestures share `interfaceMethod` on the wire so Ctrl+click and Ctrl+B cannot drift apart, and the
redirect is *injected by the caller* rather than called from inside `goToDefinition`: that
function is `goToImplementation`'s own empty-answer fallback, so wiring it the other way round
would make an interface nobody implements bounce between the two for ever.

## Code completion, and the handshake key that turns it on (M25)

Typing in a Rust or Go buffer now opens a popup of what the language server thinks can go under
the caret. **Tab and Enter both accept.** Ctrl+Space opens it on demand, including on an empty
line. Function completions arrive as snippets — accepting `push` gives
`push(value)` with the argument selected and Tab walking the fields — and accepting an unimported
symbol brings its `use` line with it.

Nothing about it is language-specific. `cide_lsp::discover::Server` is a registry rather than an
enum, so one `textDocument/completion` path serves rust-analyzer, gopls and any server an
extension declares; a contributed one gets completion by existing. Two settings live under
**Settings → Editor**: whether to suggest at all, and whether the popup opens while typing or only
on Ctrl+Space.

### The assumption this was built on was backwards, and a test found out on day one

The plan said: declare no `completionItem.resolveSupport`, and servers will compute import edits
eagerly and ship them inside each item, so accepting `HashMap` costs no second round trip. That
reasoning is what the capability *means*, and it is wrong for rust-analyzer in the direction that
deletes the feature.

Measured against 1.92: with `resolveSupport` absent, the completion list at a position naming an
unimported `HashMap` contains `Vec`, `String`, `Option` and 114 other in-scope items, and **no
`HashMap` at all**. rust-analyzer gates flyimport on the client being able to resolve
`additionalTextEdits` lazily, because computing an import path for every candidate in std is work
it will not do up front. Declaring nothing does not buy eager edits — it buys no auto-import.

So cide declares `resolveSupport: ["documentation", "additionalTextEdits"]` and asks
`completionItem/resolve` for the row the user commits to.
`crates/cide-lsp/tests/real_servers.rs::a_real_rust_analyzer_offers_an_import_edit_with_the_item`
is the standing proof, end to end through cide's own conversion, and it asserts the *absence* of
the naive reading — because dropping that key reads like deleting an unused option.

`documentation` is deferred for an unrelated reason and it is the one that pays for itself:
rust-analyzer sends it eagerly otherwise, it is markdown, and `HashMap`'s alone is six kilobytes.
One 118-item reply measures **44 KB** with it deferred. `detail` is deliberately *not* deferred —
it is the dimmed type the popup draws on every row, and it costs short strings rather than
kilobytes.

### The three things that are wrong in ways nothing shows you

**The document is synced first.** `docSync.ts` is a 300 ms trailing throttle, so the server's copy
of the buffer is routinely behind it — and a completion computed against text that does not
contain the caret is not late, it is about a different file. `syncNow` sends the exact text
CodeMirror's immutable `context.state` holds and resolves when the notification is queued;
`diagnostics_did_change` pushes into the same bounded outbox the request will use, so the ordering
is a property of the channel rather than a race that was won.

**`label` is the filter text and `displayLabel` is the label.** CodeMirror matches typing against
`Completion.label`. LSP's `label` is a *display* string — rust-analyzer sends `push(…)`, ellipsis
included — and `filterText` is the match target. Feeding the display string to the matcher scores
`push(…)` against `pus` and produces the complaint that the popup "stops narrowing when you type",
with every row still visibly present.

**A failed resolve refuses the accept.** Inserting `HashMap` and silently not adding its `use`
line leaves a file that does not compile and looks exactly like a successful completion. Not
completing at all is recoverable in one keystroke and says so out loud.

### What it does not do

- **No documentation panel.** The payload is deferred and dropped in `convert::completion`; the
  honest way back is one `completionItem/resolve` for the selected row, not a thousand.
- **No signature help.** `textDocument/signatureHelp` is still asked nowhere.
- **No palette command.** Ctrl+Space, Tab and Enter are editor-local CodeMirror bindings, as
  Ctrl+F and F3 are, and `nothing_binds_the_editors_completion_chords` keeps them out of
  `cide-core::keymap` — the key gate is a window *capture* listener, so a binding there would
  swallow Tab in every text input in the window and Ctrl+Space's `NUL` in every shell pane.
- **Nothing above 1 MB.** `diagnostics_did_change` refuses to sync a buffer that large, so the
  server's copy is permanently stale and any answer would be a confident lie. The same judgement
  the grammar, bracket matching and folding already make about the same buffers.
- **No commit characters.** Declared `false`: an accept that fires as a side effect of typing
  punctuation — `(` completing the function you were only half looking at — is a third, invisible
  accept key on top of Tab and Enter, and it is the behaviour people turn off first.

**Tab-only shipped first, and was wrong.** Enter was deliberately left to insert a newline, on the
argument that Enter-accepts eats a line break whenever the popup is open and the user had not
noticed it. That hazard is real; it is also not what anyone expects on first meeting a completion
popup, and the first report this feature drew was that Enter "does nothing". Both accept now. The
fallthrough is what makes it safe enough to be the default: `acceptCompletion` returns `false`
with no popup open, so Enter reaches `insertNewlineAndIndent` untouched in every buffer where
nothing is being suggested.
- **Snippets degrade rather than mangle.** CodeMirror reads `#{` as a field opener as well as
  `${` and its placeholder default is `[^{}]*`, so a nested LSP placeholder has no representation
  at all. `lspSnippet.ts` falls back to plain text in both cases — a completion with no
  placeholders is a small loss, one that inserts `${1` into a user's file is a bug.
- **Not confirmed on screen.** `check:completion` (111 checks) drives the decisions and pins the
  call sites, and the two real-server tests prove the protocol path — but nobody has yet watched
  the popup appear, accepted a snippet, or seen an import land. `cargo test -p cide-lsp --
  --ignored` is not in CI.

## Hidden and ignored files in the trees (M18)

The walk was `hidden(true).git_ignore(true).ignore(true)`, so a dot-prefixed entry and anything a
`.gitignore` covered were simply absent — including `.claude`, which is a directory a user of this
particular IDE has every reason to want to open.

Both are now settings, **and both default to on.** The cost of showing ignored files is real and
asymmetric — dotfiles add tens of entries, while `target/` and `node_modules/` add a very large
number of rows to walk, hold, offer to Ctrl+P and *watch*, and one `Filter` serves the tree, the
picker, content search and symbol indexing alike, so it lands on all four at once. That cost is
why the setting shipped `false`, and shipping it `false` was the wrong call: the feature exists to
answer "cide doesn't show gitignored files - but should", IDEA shows them with nothing configured,
and a feature that stays off until the person who asked finds a switch has not been delivered. The
expensive behaviour is the one that was asked for; the cheap one is a toggle away. `.git` itself
stays hidden either way.

Correcting it needed a schema rung, which is the interesting part. `ExplorerSettings` carries
`#[serde(default)]`, so documents written *before* the field take the new default for free — but
those are exactly the users who do not have the feature yet. Every workspace written by the build
that shipped it contains `"showIgnoredFiles": false`, put there by a constant rather than by a
person. So `persist::v3_to_v4` overwrites `false`, and only `false`: a `true` already agrees, an
absent key is serde's to answer. It is the only rung in the ladder that overwrites anything, and
the exception is bounded to a value that can be shown never to have been a choice. Ignored rows are drawn in a muted tone from the token palette rather than a literal
colour copied from IDEA, so they read as present-but-excluded in both themes.

Two things worth knowing, neither of them hidden in the code: the walk and the *watcher* share the
same matchers, so both halves move together or the tree and its events disagree; and flipping
either toggle rebuilds the `Index`, which is where folder expansion state lives, so the tree comes
back collapsed.

## A find bar in terminal panes, and what Ctrl+V now means (M18)

Ctrl+F opens a find bar over a Claude or shell pane, driven by `@xterm/addon-search`, which was
already a dependency and previously unused.

The scope is the honest half. A full-screen TUI runs on the **alternate** buffer, which *is* the
visible screen — there is no scrollback under it, and `cide_pty`'s vt100 mirror holds the same
alternate screen, so the transcript a user remembers reading is genuinely not being kept by anyone
while Claude owns the display. Search there still works, it simply cannot see one line further
than the user can, and the bar says so (`visible screen only — this program is drawing a
full-screen view`) rather than quietly returning nothing for a word that is plainly ten lines up.
Writing `\x1b[?1049l` to reach the scrollback was rejected: taking the child's screen away in
order to look at it is the `layout/paneHosts.ts` class of bug one layer down.

The cost is deliberate and named in `terminal/keys.ts`: plain `^F` (0x06) no longer reaches the
child, so readline's `forward-char` and vim's page-forward are gone in a terminal pane, on the
same argument that already applies to Ctrl+C and Ctrl+V.

Ctrl+V changed meaning in a Claude pane. It used to pass `^V` straight through so that Claude's own
paste — including **image** paste, which cide cannot do — handled it. That works at Claude's main
prompt and nowhere else: a numbered-choice prompt has no paste handler, so the keystroke vanished
and the only way in was the context menu.

The ordering is what makes the fix non-obvious. xterm's custom key handler is **synchronous**, and
its return value is the only thing that can stop `^V` reaching the pty — but "does the clipboard
hold text?" is an `await`, so the decision to intercept has to be made before the answer exists.
So the keystroke is now *always* intercepted, and re-emitted as `\x16` to the CLI when the
clipboard turns out to hold no text. Text pastes through cide; an image still reaches Claude's own
handler, one round trip later. Both live in the single implementation in `terminal/clipboard.ts`
that the keystroke and the context menu share.

## Ctrl+G edits the plan in cide, and why it used to say VS Code (M20)

> *"Why in claude session i see: `ctrl+g to edit in VS Code` but we here have integration with
> cide IDE, why ctrl+g not opens plan in cide and opens it in vscode?"*

**Ctrl+G was never part of the IDE integration**, and the integration was working the whole time.
`cide-ide-mcp` publishes `ideName: "cide"` into `~/.claude/ide/<port>.lock`, a real CLI reads it,
picks the WebSocket transport and completes the handshake — that is the channel carrying
`openDiff`, `openFile` and `getDiagnostics`. The external editor is a different mechanism that
happens to be reachable from the same session, and it never asks the IDE anything.

What it does instead, read out of the 2.1.238 bundle: resolve `$EDITOR`; failing that, take the
first of `code`, `vi`, `nano` that is on `PATH`; look the result up in a table
(`{code: "VS Code", cursor: "Cursor", vi: "Vim", …}`) for the hint; then `spawnSync` it with a
file, **block the agent's turn until that process exits**, and read the file back off disk. cide
set no `EDITOR` and `/usr/bin/code` existed, so the hint said *VS Code* on a machine where nothing
about VS Code was otherwise involved. It would have said the same on one with no VS Code
integration at all — the sentence was never evidence about which IDE the CLI was talking to.

### Why this could not be `openFile`

That is the obvious fix and it does not work. The CLI does not want the file *shown*; it wants **a
process whose exit means the human is finished**, because what it does after the wait is read the
file. An MCP call returns as soon as the tab is on screen and contains no such moment, so routing
Ctrl+G through the IDE socket would hand Claude Code an unedited file and call it a success.

So the `cide` binary grew a second mode. `cide --wait <file>` opens the file in the running
application and does not exit until the tab is closed, which is `code -w`'s contract and the one
the CLI already knows how to consume. `main.rs` dispatches it **before the graphics ladder**: this
mode creates no window, needs no display, and every line below that branch is setting an
environment variable for a webview it never builds.

### A third socket, and why it is not either of the two that existed

`$CIDE_EDIT_SOCK`, at `$XDG_RUNTIME_DIR/cide-edit-<pid>.sock`, `0600`, unlinked on drop — the
lifecycle is copied from `hooks.rs` and `agent_rpc.rs` line for line. What is not copied is the
socket itself, because both of the others are the wrong shape:

| socket | shape | why not this |
| --- | --- | --- |
| `CIDE_HOOK_SOCK` | write-and-forget, one connection per frame, every frame serialised through one applier thread | a verb that blocks for as long as a human is editing would occupy that thread and take every session's busy/idle chrome down with it |
| `CIDE_AGENT_SOCK` | MCP, and its header line is an **authorisation** — it decides which project's tasks a model may write | `cide --wait` is not a model and asks for no tools; the one place that answers *what may this caller reach* should not also answer something else |

### What "finished" means, and the three cheaper answers that are wrong

The wait ends when **no tab anywhere in the workspace is showing that path**.

* *Watch the `TabId` we opened.* A tab detached into its own window and re-docked comes back with a
  **fresh id**, so tearing the editor out into a window would release the CLI's turn mid-sentence.
* *Wait for a save.* People save several times and close once.
* *Resolve from the close paths, the way the `openDiff` broker does.* That is edge-triggered and
  needs every close path enumerated — `tab_close`, closing a project, closing a window, the quit
  ladder — and a path added next year silently stops resolving. Missing one does not cost a stale
  tab; it costs a `claude` turn that never ends.

So the check is level-triggered and therefore a poll: one uncontended mutex and a walk of a few tab
lists every 250 ms, for as long as one person has one file open. It rides on the socket's own read
timeout, so the syscall that paces the loop is also the one that notices the client dying — `Ok(0)`
is EOF and only EOF, because the client writes one line and then waits.

Which project the tab lands in is four questions in order: the client's `CIDE_SESSION` (a Claude
pane has one, and it is the only fact here that is not a guess — the plan file itself is under
`/tmp` and inside no project at all), then the client's cwd (a *shell* pane gets no `CIDE_SESSION`,
so a `git commit` in one has nothing else), then the file's own path, then whatever the user is
looking at. The session is read from the client's own environment and never from an argument — the
rule `cide-hook`'s `forward` states and all three sockets now depend on.

### Three refusals in `child_env::editor_env`, each avoiding something worse than the default

* **No socket, no `EDITOR`.** Both variables come out of one function so a spawn site cannot ship
  one without the other. `cide --wait` with nowhere to report can only fail, and an editor that
  fails every time is strictly worse than the `code` guess it displaced.
* **A path with whitespace in it, no `EDITOR`.** The CLI splits this variable on `" "` with no
  quoting and no shell, so `/Applications/My App.app/…/cide` names a program called
  `/Applications/My`. macOS-shaped, unfixable from this end, and a `warn` rather than a silence.
* **An `EDITOR` the user already set is never overwritten.** `EDITOR=nvim` is an answer someone
  already gave, for `git commit` as much as for Ctrl+G. The reported case is the *unset* one, which
  is exactly the case where the CLI guesses. A user's `.bashrc` still wins over cide either way,
  since it runs after the environment is inherited.

Set for **every** pane, shell as well as Claude, for the reason `CLAUDE_CODE_SSE_PORT` is: a
`git commit` in a cide shell wants the same editor a `claude` there would get.

### Not done

* **Nothing here has been run against a live `claude`.** The unit tests cover the placement and the
  wait condition; `crates/cide-app/tests/edit_wait.rs` drives the real binary against a socket the
  test owns and pins the half that is a process — the argv, the cwd-relative path resolution, that
  it does *not* exit while the answer is outstanding, and the exit code. What no test here reaches
  is the CLI actually pressing Ctrl+G, because that needs a restart of the running instance.
* **The CLI will enter the alternate screen while you edit.** It classifies an editor as *GUI* by
  substring-matching the command's basename against `["code", "cursor", "windsurf", "codium",
  "subl", "atom", "gedit", "notepad++", "notepad"]`; `cide` matches none, so it is treated as a
  terminal editor and the pane blanks until the tab closes. Cosmetic, and not fixable without
  naming the binary after somebody else's.
* **A dispatched subagent's pane does not get it.** `cide-agents`' harnesses compose their own
  environment and are not passed the socket, so a run's `claude` falls back to the CLI's guess.
* **Quitting cide mid-edit ends the wait as a failure.** The connection dies, the client exits
  non-zero, and the CLI reports that rather than reading the file back. Honest, and noisier than a
  clean answer would be.
* **`ServerEvent::OpenFile` is still a `tracing::debug!` and nothing else.** The IDE socket's own
  `openFile` — a different caller with a different contract — remains unwired; it was unwired
  before this and is not what Ctrl+G goes through.

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

### The pane's id and the CLI's conversation are two different things (M17)

`SessionId` is cide's handle for a pane's child: it keys the registry, the webview addresses
panes by it, and it is stable for the pane's whole life. It is **not** reliably the id the CLI
is running under. On 2.1.224, `claude --resume <parent>` runs under a *fresh* conversation id
rather than announcing the parent's, and `/clear` mints another mid-session. The code assumed
otherwise in two places, and both assumptions were load-bearing:

* Hook frames were routed by the payload's `session_id`, so every frame from a resumed pane
  named a uuid no pane held and its effects were emitted to nobody. Measured on a live
  workspace: 2 session ids arriving from hooks, 12 held by panes, **zero overlap**. That is
  the whole of "the finished-turn notification never fires" — it worked in a freshly opened
  pane, where the two ids agree, and nowhere else. Four attempts read the chain and found every
  link correct, because the defect was in which id the links were keyed by.
* Resume named the pane's own id, whose transcript is still on disk after a `/clear`. So a
  restart kept resuming the conversation the user had cleared — "it always restarts with almost
  the first session that was in this panel".

The fix stops inferring identity and carries it. A Claude child is spawned with
`CIDE_SESSION=<the id cide filed it under>`; `cide-hook` reads that from its own environment
and puts it in every frame as `spawned_as`; `HookFrame::owner()` is what effects are keyed by.
The CLI's id is not discarded — it is recorded on the pane as `Pane.conversation` and is what
`restore_for` resumes, so a restart picks up the conversation the user was last looking at.
`Pane.conversation` stays `None` for a pane whose CLI never diverged.

Two consequences worth keeping in mind. `run.sh` builds `cide-hook` as well as `cide-app`,
because the app resolves the hook binary as `current_exe().parent()/cide-hook` and a stale one
reports in an old frame shape **silently** — no error, no log line, just chrome that never
updates. And the hook log now prints `session` and `cli` separately; when they differ, only
`session` addresses a pane.


## Images, and the four things that would have failed silently (M18)

Opening a `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `ico` or `svg` shows the image rather than
its bytes. The interesting parts are all refusals.

**The bytes go over Tauri's asset protocol, not the IPC control plane.** `emit`/`listen`
string-interpolates JSON into an `eval`'d script and is control-plane only; a 32 MB PNG marshalled
that way is the pathological case that argument exists for. The protocol needs three things to
line up, and any one of them missing fails *silently* — a blank pane, no error anywhere: the
`protocol-asset` cargo feature, `assetProtocol.enable` in `tauri.conf.json`, and
`img-src … asset: http://asset.localhost` in the CSP. All three were already at HEAD.

The static `assetProtocol.scope` is **empty, and stays empty**. A scope wide enough to serve a
repository is a scope wide enough to serve `~/.ssh`, so instead `image_read` grants
`asset_protocol_scope().allow_file()` for the one path the user opened — and grants it *after*
the size, type and header refusals have run, so a directory, a FIFO or a 2 GB tarball never
reaches the protocol on the strength of somebody having clicked something.

**`MAX_IMAGE_BYTES` is 32 MB and is checked against the editor's own cap**, so the two cannot
drift into a state where a file is too big to view and small enough to open as text.

**SVG renders through `<img>` and is never inlined.** An SVG is a document that can carry script,
and inlining one into the DOM is a straightforward XSS vector for a repository you cloned. `<img>`
does not execute script, which is what makes rendering an untrusted one safe at all. The cost is
recorded here rather than hidden: `.svg` no longer opens as editable XML in CodeMirror, which it
used to. That is a real capability loss for anyone who edits SVG source by hand, and it is the one
part of this feature that may need an *Open as text* affordance if it turns out to matter.

**A file that is not what its extension claims fails with a sentence.** `cide_core::image::sniff`
reads the header and says so, rather than mounting a pane that draws nothing. `tiff`, `avif`,
`jxl` and `svgz` are deliberately absent from the extension table: WebKitGTK either cannot decode
them or decodes them only in some builds, and a format that works on one machine and not another
is worse than one that works nowhere — a refusal from Rust is at least readable.

## Three things the commit tool window got wrong about a changelist

Reported together, and they turn out to be one story:

> *"on commit changes that was moved from Uncommited to custom list goes into Uncommited again,
> but should stay in custom list. When commiting - i should be able to commit only selected
> (checkbox) change lists. Also i see some 'Partial' state - i don't need it."*

### A commit of one changelist unstaged every other one

ADR 0004 makes the index a derived artifact, and `cide_git::commit::rebuild_index` is where that
decision acts: reset to HEAD, write the selections, commit. That is what makes *"commit one
changelist and the other is untouched"* true of the **tree**. It was not true of the **index**,
and there is one class of file for which the index is the whole of what makes it a change at
all — a new file that has been `git add`ed is tracked because it has an index entry and for no
other reason.

The chain, every link of it invisible: commit `Changes`; `rebuild_index` resets the index and the
added file's entry goes with it; the next status walk reports the path `Untracked`;
`status::repo_changes` builds its `live` set from paths that are neither untracked nor ignored;
`Sidecar::reconcile` drops every assignment outside that set. A file the user had deliberately
filed into a list of its own reappeared under `Unversioned Files` after a commit that never named
it, with nothing on screen saying why. It was even written down as correct — a *"What it costs,
per ADR 0004"* paragraph in `useGitPanel::trackPaths` explained it as the model working as
intended. It was a bug, and the paragraph now says so.

`commit` saves the entries the rebuild is about to erase and puts back every one the commit did
not consume. `staged_entries` is the `HEAD → index` diff, so it is a handful of paths rather than
the whole index — this runs on every commit, and a repository can have a hundred thousand tracked
files. Entries are replayed **from memory**, never re-added from the working tree: `add_path`
would stage whatever the file says now, which for a file staged and then edited again is not the
content the user staged. A staged *deletion* is saved as an absence and restored as one.

The refusal path was the same bug with a worse ending. `write_commit` can fail after
`rebuild_index` has already run, and the index was then left as the rebuild made it — a silent
`git reset` of everything staged in every other list, delivered as the answer to *"nothing to
commit"*. `rewind_index` puts HEAD's tree back and replays the saved entries over it, which *is*
the pre-commit index by construction, and it is guarded on the mode: in staging-area mode nothing
was rebuilt, and rewinding there would throw away an index the user built by hand.
`committing_one_changelist_leaves_the_others_staged_files_staged` and
`a_commit_refused_after_the_rebuild_puts_the_index_back` are the two tests, the second of which
reaches the post-rebuild refusal by unsetting `user.email` — any failure from there on would do,
and that is the one a test can arrange without reaching inside the function.

### The tick did not follow the file into its new changelist

A file row's id is its repository and its path — `model.ts::fileRowId` — and filing a change into
another list changes neither. So the tick stayed exactly where it was: filing a file **out** of
the changelist that was about to be committed left it ticked, and the commit took it anyway. That
is the panel answering *"commit only the lists I ticked"* with *"and also the file you just filed
away"*, and it is not visible — the tick is 12px, several rows down, inside a changelist whose
whole point was to be left alone.

`ticksAfterMove` is the rule and it lives in `model.ts`, where `check:git` can run it. A moved
file adopts the answer its destination is already giving: joins the commit if that list has
anything ticked, leaves it otherwise. Reading the **destination** rather than the active flag is
what keeps both intentions alive — a user who has ticked a non-active list is building a commit
out of it, and unticking each file as it arrived would make that gesture undo itself. A list that
does not exist yet (*New changelist…* mints one and moves in a single gesture) holds nothing and
so takes the second answer, which is what moving files somewhere new is nearly always for.

All four routes into filing go through it: the drop, the chooser, *New changelist…*, and the
`Unversioned Files` drop that stages first. The chooser used to run its own copy of the same
command, which is exactly how a fifth route would quietly keep the old behaviour, so it now calls
`movePaths` like everything else.

### `partial` meant two things, and one of them was not true

The tri-state box had a second meaning: a ticked file whose index held only part of it — `staged`
and a dirty worktree both — was drawn `–` as well, with a bordered `partial` chip beside its name,
and the partiality climbed to its directory row and its changelist. A list ticked whole therefore
read `–`, and **no gesture in the panel could make it read `✓`**: the box was reporting a property
of `.git/index`, while the only thing a click on it can change is the ticks.

It was also a claim about the commit that the commit does not honour. In changelist mode
`cide_git::commit` resets the index to HEAD and writes the selections in, so what the index
happened to hold for a file has no bearing whatever on what lands — a ticked file is committed
whole however it was staged. The thing that genuinely does narrow a commit is a hunk selection
held by the diff pane, and that already has its own line above the tree, which can say how many
files it is about and offer to clear them; neither is something a `–` in a 12px box can do.

So `checkState` is now a function of the ticks alone, and `isPartiallyStaged` is gone. The
fixture's half-staged file is deliberately kept: `check:git` and `check:render` now pin the
opposite behaviour from the same data.

## Markdown preview (M20), and what is not done

A `.md` pane draws in one of three layouts — the buffer, the buffer and a rendering side by side,
or the rendering alone — switched from a three-button cluster that floats in the pane's top-right
corner beside `⊞ ⛶ ⧉ ×` and is revealed the same way. IDEA's arrangement, and the report it
answers is the ordinary one: opening `README.md` in cide gave you its bytes.

The layout is remembered **per file**. It rides `ViewPosition` in `positions.json`, the record
that already carries the scroll offset, the caret and the folds, and it arrives the way `folds`
did one milestone earlier: one `#[serde(default)]` field, no schema bump, no migration. The two
reasons that store exists rather than `workspace.json` both apply harder here than they do to a
scroll — *lifetime*, because the record you most want back is the one closing the tab has just
removed from the workspace, and *cost*, because every workspace mutation bumps `rev`, clones the
tree twice and broadcasts it to every window. A layout is a click, but it must not become one of
those. `a_positions_file_written_before_the_markdown_layout_loads_unchanged` is the test, and the
half it is really about is the absent field: without the `default`, every entry in an existing
user's file fails to deserialise, `load_positions` answers empty, and everybody loses their place
in every file they have ever opened, on the launch after an update.

### The renderer is written here, and it produces React elements rather than HTML

There is no markdown dependency in this repository and this did not add one. Every renderer in
the ecosystem answers `string → HTML string`, and consuming one means `dangerouslySetInnerHTML`
plus a sanitizer — a pair this project has twice written comments to avoid, most plainly in
`sidebar/TasksPanel/TaskDetail.tsx`: *"rendering model-authored markup inside the IDE's own chrome
is an injection surface bought for nothing."* A `.md` in a cloned repository is exactly as
untrusted as a task comment, and the same answer applies as the one *Images* below records for
SVG: the fix is not to filter the markup, it is not to have a path for it.

So `editor/markdown/blocks.ts` and `inline.ts` parse to a tree with **no node that can carry
markup**, and `MarkdownPreview.tsx` maps that tree to elements. `<script>alert(1)</script>` in a
document renders as those characters. `check:markdown` walks every node of nine hostile inputs
and fails on any kind outside the safe set — which is not a filter but a proof that no filter is
needed, and the assertion to look at if a node is ever added to `types.ts`.

The second argument for a tree is that it can be *checked*. `ui/scripts/check-*.mjs` are this
project's only frontend tests, and what they do is compile an import-free module and assert on the
values it returns. A pure `parseMarkdown` is exactly that shape; an HTML string is assertable only
by matching substrings of itself.

**It is a subset, not CommonMark.** In: ATX and setext headings, thematic breaks, fenced and
indented code, blockquotes with lazy continuation, bullet and ordered lists nested arbitrarily,
tight and loose, GFM tables and task items, link reference definitions, and the whole of the usual
inline vocabulary including reference links and autolinks. Out, and listed under *what is not
done*.

**Both scans are bounded, and the bound is not new.** `languages/markdown.ts` already paid for
this and wrote the measurement into its header: an unbounded `\[[^\]]*\]\(` "scans to the end of
the line for every `[` that does not open a link and then backtracks over the whole scan one
character at a time — so a line of brackets is quadratic. Measured at 3.8 s for a
160,000-character line of `[`, against 0.13 s for the same line with this bound." The parser uses
the same 512, deliberately the same number rather than a second opinion, and emphasis resolution
runs over a **doubly linked list** — `Array.prototype.splice` shifts everything after the cut, so
`*a*` repeated *n* times, which is an ordinary paragraph rather than a pathological input, is
O(n²) on an array. `check:markdown` measures nine inputs at 20,000 and 160,000 characters and
fails on anything superlinear.

### Fences are coloured by the buffer's own grammars

```rust
fn main() {}
```

— coloured by the same `languages/rust.ts` table that colours a `.rs` buffer, through the same
`highlight.ts`, because in split mode the two are eight pixels apart and two highlighters that
could disagree would be seen disagreeing. `REGISTRY` now holds each language's *grammar* and
`loadLanguage` wraps it, so `markdown/fenceTokens.ts` can drive `token()` over a string directly;
and `createTokenType`'s rule — the token-name → `Tag` mapping CodeMirror does not export — has
moved out of `check-editor.mjs`, where it called itself "the smallest possible restatement", into
`highlight.ts::tokenClassFor`, which the check now pins instead of duplicating.

**The preview therefore colours code the buffer does not**, and that is worth naming because
`languages/markdown.ts` says the opposite about itself: *"Code inside a fence is left uncoloured
rather than dispatched to the fence's language. Nesting one stream parser inside another needs
`StreamLanguage`'s nesting support, which it does not have."* That is a constraint on CodeMirror,
not on markdown — out here the fence's text is a string and the grammar is a function.

### Scroll sync is anchored on lines, and has a latch

The naive version keeps the two halves at the same *fraction*, and it is wrong in the way that is
most annoying: fifty lines of fenced code are tall in both halves, and fifty lines of paragraph
are fifty lines in the buffer and four wrapped lines in the rendering, so the two drift further
apart the further down you read. Every top-level block therefore renders with its source line on
it, and `scrollSync.ts` interpolates between those anchors — lines and not pixels, for the reason
`positions.rs` gives about the record itself.

Nothing new crosses into the editor to make it work. `viewTracker.ts` already publishes the first
visible line at most once an animation frame, which is exactly a follower's cadence, so the
preview follows the buffer on a signal that was already there; the other direction is one new
`EditorSurface` prop, `onScrollHandle`, in the shape `onSaveHandle` has and **not** in the build
effect's `[path, reloadKey]` dependency array, which `check:markdown` asserts as well as
`check:editor`. It scrolls without moving the selection, which is the difference between this and
`revealRequest.ts`: a caret that walked down the file while somebody read it would be a selection
they did not make, in a buffer they are about to type into.

The latch is the part that is invisible until it is missing. Scrolling either half moves the
other, and moving the other fires *its* scroll event; whoever moved first therefore owns the
gesture for 150 ms and the follower's own events are ignored. Last-event-wins is the feedback
loop with extra steps, because a programmatic scroll fires events indistinguishable from a
person's.

### The pane's corner was already spoken for

`layout/PaneTitleBar.module.css` floats the pane controls in the top-right and publishes
`--pane-corner-clear` — `--w-minimap` plus `--pane-corner`, 221px on an editor pane — as the band
a pane's own content must keep clear. `panes/EditorPane.module.css` carries the write-up of the
once a strip reserved 125 instead: both of its buttons were drawn *inside* the cluster's live hit
box, hovered, and answered the ×'s click. The switch reserves the published sum, and below 320px
of pane it moves to the bottom-right instead of being squeezed — where `panes/ImagePane.tsx`
already puts its one toggle, for the same reason. `check:markdown` re-derives the 221 from the two
tokens and fails if the threshold stops covering it.

Below 520px, `split` is overruled to **preview** — the pattern `panes/GitDiffPane.tsx` solved once
for side-by-side diffs, `data-overruled` and an explanatory `title` included. The fallback differs
because the question does: a diff falls back to unified because that is the shape its per-line
staging is expressed in, and a reader who asked for split asked to see the rendering.

### Three things that are cheap only because something else already paid for them

**The buffer is never unmounted.** In preview mode the `EditorSurface` is still in the tree, still
laid out at the pane's full size, and merely `visibility: hidden` with the rendering painted over
it — so unsaved edits, undo history, find state, the LSP document and the autosave timer all
survive, and switching text ⇄ preview costs no reflow at all. `display: none` is the trap, and it
is the same one `layout/paneHosts.ts` states for hidden terminal tabs: measurements read zero, and
a CodeMirror asked to lay a wrapped document into zero columns re-wraps every line of it twice.
`MarkdownFrame` also keeps the surface inside **one** element in all three layouts and changes
only its class, so switching text ⇄ split ⇄ preview does not re-parent the editor at all. (The
markdown-or-not branch above it does re-parent, and that is fine: only a rename flips it, and
`EditorSurface` rebuilds on a path change in any case.)

**Local images go through `image.read`**, the same two calls `ImagePane` makes, which is what
grants `asset_protocol_scope().allow_file()` for one path *after* the size, type and header
refusals have run. A `![](../../.ssh/id_rsa)` is refused on its header, in Rust, before anything
reaches the protocol. Remote images cannot load — the CSP's `img-src` has no `https:` and will not
get one — and both refusals draw the alt text with the sentence on hover rather than nothing,
because a document with a silently missing diagram looks fine and reads wrong. 64 images per
document, because each one is a standing grant and a document is a thing an agent can write.

**No `href` is written anywhere in the preview.** An `<a href>` in this webview is a way to
navigate the application itself away from its own document, which is the hazard `terminal/xterm.ts`
closes for OSC 8 links: until that handler existed, "any program in any pane could emit
`ESC ] 8 ;; https://… ST` and a single click on the text it wrapped would navigate part of the
application to a URL that program chose." A middle-click or one forgotten `preventDefault` is all
it takes, so the attribute is simply never written and there is nothing to forget — the link is a
`role="link"` span, put back on the tab order by hand. Activating an external one **copies the
address and says so**, which is the terminal's *"Copy the address and open it in a browser"*
carried out rather than recited; a local one opens a tab; a `#fragment` scrolls the preview.

### What is not done

- **There is no command, no default binding and no palette row.** The floating switch is the only
  route to the feature. Adding one is a `Command::new(… VIEW)` and a `case` in `keys/dispatch.ts`
  — but note that `editorFocused` is true for every buffer, so a correct gate wants a new
  `markdownTabActive` context flag, and `cide-core::commands` records at length what happens when
  a flag is added without a supplier that really derives it.
- **The layout is per file, not per pane.** A split showing one `.md` twice restores the same
  layout into both, and switching in one does not switch the other until the next mount. Exactly
  the limitation folds have, for exactly the same reason.
- **Out of the CommonMark subset**: HTML blocks and inline HTML (text, on purpose), footnotes,
  definition lists, math, YAML front matter — which renders as a paragraph of `key: value` lines
  rather than as the table it deserves — reference-style images, and nested tables.
- **The divider position is session-global and is not saved.** One number shared by every markdown
  pane in the window, gone when the window closes. Persisting it would mean either a second wire
  field on a per-file record for something nobody asked to keep, or a settings write per drag.
- **A task item's checkbox is not clickable.** Ticking it would have to edit the buffer, and a
  preview that writes to the document the user is editing is a second author of that file.
- **Nothing here has been confirmed on screen.** The standing reason plus the same one M19 had:
  the author of this change is running inside the user's own instance, and `run.sh` stops any
  instance already running. What is covered is more than usual — `check:markdown` is 231
  assertions over the parser, the sync arithmetic, the layout rules and the fence tokenizer, and
  it drives the real grammars. What no check in this repository can speak to is the picture: the
  hover reveal, the type scale, whether the two halves land where a reader expects.

## Extensions, and a marketplace that is a git repository (M22), and what is not done

Four things are contributable from outside the tree: **languages**, **language servers**, **UI
panels** — a button on the activity rail, or a tab in the bottom tool window — and **logic over
open files**. A *marketplace* is a git repository listing extensions; cide clones it, reads its
index, and installs one at a pinned commit.

`/home/…/work/cide-marketplace` is the first, and it ships two extensions: **SQL** and **YAML**.
Both supersede a grammar cide already has, which is the point rather than a coincidence — see
*What the two demo extensions are for*, below.

### Extension code runs out of the window's realm (ADR 0010)

The obvious implementation is what every comparable product does: dynamically import an
extension's module into the window and hand it React and the DOM. It loses twice.

It loses on ADR 0001, which is not a preference but the shape of this application: one webview per
OS window, **one JavaScript main thread serving every pane**, and PTY bytes coalesced in Rust to
≥8 KiB or 8 ms specifically so that thread stays free. Extension code there competes with xterm
for it, and the failure is not a slow panel — it is a terminal that stutters while somebody else's
loop runs.

It loses again on the seam. `ui/src/ipc/client.ts` is the only file allowed to import
`@tauri-apps/api`; an extension in the same realm reaches `window.__TAURI__` directly, so a
capability system would be a convention rather than a mechanism.

So: **a dedicated Web Worker per extension**, with no DOM, no Tauri and no `invoke` — genuinely
absent, because a worker is a different realm. Everything it can do is a fixed table of requests,
and every one is checked against the granted capabilities in `ui/src/ext/host.ts`, on the main
thread, where the extension cannot reach the check. That last clause is the whole design.

**A panel is a view model, not a component.** An extension posts a tree, a list, a table or some
prose, and `ui/src/ext/ExtPanelView.tsx` draws it with cide's own components and tokens — so
`check:theme` and `check:ui-scale` are true of a contributed panel whose author has never heard of
either, and a broken extension cannot unmount the React root. The cost is stated rather than
hidden: an extension cannot draw something cide has no component for. A chart, a canvas, a custom
gesture: not possible, and not by omission.

### The language registry stopped being closed, and that is most of the work

`ui/src/editor/languages.ts` was a closed `LanguageId` union feeding four parallel tables — a
loader map, an extension map, an exhaustive `Record<LanguageId, FoldSpec>` and a `SCRATCH_TYPES`
array. Adding a language cost five TypeScript edits and adding a *server* for one cost five more
in Rust.

Now there is one registry with two sources. The tables come from **`cide_ipc::lang::builtins()`**,
generated into `ui/src/editor/builtinLanguages.ts` by `cargo xtask codegen`, and an extension's
languages merge into them. That is `cide-core::commands`' argument for commands and keys being one
table, applied one floor down: two tables would let a language be highlightable but unfoldable, or
foldable under a name the status bar does not use, and both would drift in silence.

What did **not** move is the tokenizer. A builtin's grammar contains a *function* — Rust's
lifetimes, Go's rune literals, Markdown's headings — and no JSON can express one, so it stays in
`ui/src/editor/languages/<id>.ts` behind its own dynamic import. A contributed grammar gets
`rules` instead: a regex, a tag name, and whether it is tried only at the head of a line. That is
deliberately less than a hook and deliberately enough for the shape of hook that turns up over and
over.

**`at: "lineStart"` is not a refinement.** `data.ts` records the measurement that made it a field:
YAML's key pattern scans to the end of the line before its lookahead can fail, so tried once per
token a 200,000-character line took 4.8 s against 20 ms at the line head. A manifest author will
not know that, which is exactly why it is a field with two values rather than a comment asking
them to be careful.

### `cide_lsp::Server` stopped being an enum, and one line was the reason

`cide_app::lsp::server_for` derived a language server from `cide_lang::Lang`. `Lang` is closed
because every variant costs a statically linked C parser table — so *"a language cide can
outline"* and *"a language cide can run a server for"* were forced to be the same set, and a YAML
server would have needed a tree-sitter grammar for YAML. That is an absurd price for a `--stdio`
flag.

`Server` is now a `Copy` index into a registry installed at startup: the two builtins plus
whatever the enabled extensions declare. Staying `Copy` and `Ord` is what kept the change to a
rename at the ninety-odd call sites that hold one in a map key or compare two. `server_for` asks
the language registry for a `languageId` and the server registry for whoever claims it.

The field that turned out to matter most is **`args`**, which did not exist: `server.rs` built
`Command::new(binary)` and spawned it, and both builtins happen to speak LSP on stdio with no
flags, so nothing noticed. `yaml-language-server` needs `--stdio` and says so in its own README.

`DiagnosticSourceId` stopped being an enum for the same reason and keeps its four wire names
unchanged, so every stored source filter still works.

### What the two demo extensions are for

cide already highlights `.sql` and `.yaml`. An extension that only added a language cide had never
heard of would prove the easy half of the contribution path and leave the interesting half — **a
builtin losing to a contribution** — untested, which is indistinguishable from a broken extension
host until somebody notices nothing changed on screen.

So both supersede a builtin, every displacement is recorded on the wire (`LanguageBinding::supersedes`),
and the Extensions panel names the source that won. Disabling one puts the builtin back.

Between them they exercise every contribution kind: a declarative language, a declarative language
server *with arguments*, worker logic over open files, a left-sidebar panel and a bottom-panel
tab. YAML's grammar is a rule-for-rule translation of the builtin's `hook`, which is the check
that the rule language is sufficient rather than merely plausible.

### The panel is a catalog, and an extension has a page

The Extensions panel has a search box and seven filter chips — **All, Installed, Enabled,
Disabled, Updates, Not installed, Problems** — each one a question somebody actually asks about an
editor's extension list, and each one press. They are not a matrix of toggles: combining two
constraints is what the search box is for, because a filter matrix is a control nobody uses
correctly on the first try.

Each chip carries its count **over the unfiltered set**, and that is the whole reason the counts
are computed before the query rather than after: a chip reading `0` only because some *other* chip
is selected would be a control that lies about what pressing it does. No chip is ever disabled,
including at zero — a chip you cannot press is a count you cannot confirm, and pressing an empty
one shows the sentence that explains it.

A search that matches nothing says so, naming the text and — when both are narrowing — the filter
too, because *"no results"* under two constraints is ambiguous about which one to relax. What the
query does **not** touch is the Languages and Problems readouts: those are facts about the whole
registry, and hiding *"why is my `.sql` coloured like that"* while somebody is searching would take
the answer away exactly when they are looking for it.

**Clicking an extension's name opens its page** — a workspace tab with its README, its version, the
permissions it asks for, and Install/Enable/Remove. A tab kind and not a file tab on the
`README.md`, because that file is not in the project: an installed extension lives under
`$XDG_STATE_HOME` and a listed one lives in a marketplace clone, both outside every root, which is
exactly what `cmd::file`'s refusals exist to keep out of an editor. It also has to draw more than a
file — somebody reading a README is deciding whether to install, so the button is on the same page
as the prose.

The README goes through cide's own `parseMarkdown`/`MarkdownPreview`, which render no raw HTML at
all, so a third party's markdown is prose, links and code and nothing in it becomes markup. Links
go through `ext_open_link`, which accepts `http` and `https` and refuses everything else **in
Rust** — a scheme check that lived in the component rendering the link would be one `invoke` away
from being bypassed, which is the argument `cmd::settings`' clamps already make about a number
input.

### A third extension, because a demo inside a tool is a tool with a demo in it

`showcase` contributes no language and no server. What it contributes is one of every `ViewBody`
kind in a sidebar gallery, a bottom panel that sends **one of every host request** and prints what
came back — refusals included, verbatim — and a README written as a reference. It asks for all five
capabilities so that what each one answers can be shown rather than described.

It exists because SQL and YAML are *tools*, and demonstrating what a panel can draw inside one of
them would have made it a tool with a demo in it. It is also the honest place to put the two
mistakes worth warning about: doing work while your panel is hidden, and assuming a request
succeeded.

### `sqls` reports no problems, and that is not a configuration mistake

Driven by hand over stdio, `sqls` advertises no `diagnosticProvider` and sends no
`publishDiagnostics` at all — not for a syntax error, not for anything. It is a completion, hover
and go-to-definition server. cide was faithfully showing nothing because there was nothing.

That is a bad outcome even though it is correct: a `.sql` file with an unterminated string showed
a clean Problems panel, which is a *claim* rather than a silence, and it is the same failure the
panel's `null`-versus-`0` discipline exists to prevent one level up.

So the SQL extension reports four findings of its own — an unterminated string, block comment,
dollar-quoted body or bracket, each on the line that opened it. They cost nothing: the statement
splitter already tracks all four, because it has to in order to avoid cutting a statement in half.
Every dialect agrees they are errors, so there are no false positives, and anything beyond them
needs a parser and a dialect, which is the line that extension has already decided not to cross.

**A contributed server now starts in an already-open project.** It did not, and that was three
symptoms of one cause: `sqls` had no row in the Problems footer, published nothing, and therefore
offered no Restart either. `ProjectDiagnostics::sync` is additive — a server already running keeps
its handle — so enabling a YAML extension costs no rust-analyzer re-index, which was the objection
to the simpler "stop everything and start again".

The Restart button had a second bug behind that one, and it would have survived fixing the first:
`RESTARTABLE` in `ProblemsPanel/model.ts` was `['rustAnalyzer', 'gopls']`, a complete list right up
until an extension could contribute a server — and `isDiagnosticSourceId` was a matching closed
list that would have swallowed the click in silence. Both are rules now. A closed list over an
open set fails by doing nothing, which is the failure mode with no symptom.

### An extension may contribute settings, and they go in Settings

`contributes.settings` is a list of `{ id, label, description, kind }`, and `SettingKind` is four
members — `toggle`, `text`, `number` with an optional band, and `choice`. They appear under
**Settings ▸ Extensions**, drawn with cide's own `controls.tsx` rows: the same bargain a
contributed *panel* makes by being a view model, and it buys the same three things — correct in
both palettes, covered by `check:ui-scale`, and an extension cannot ship CSS. The cost is the same
too, and stated: an extension cannot have a setting cide has no control for.

**The worker reads a value with no fallback and no type check.** cide sends the complete, coerced
set — the defaults with the user's changes layered on, each one the declared type, a number already
clamped to its band and a choice already known to be in its list. A hand-edited `extensions.json`
holding `"rows": "lots"` arrives as the default and one holding `9000` arrives as the maximum,
because `SettingDef::coerce` is the single place a value is decided and every road goes through it
— the control, the hand edit, and an update that narrowed a range. Values arrive on `ready` and
again as a `settings` note, always the whole set, so there is nothing to merge.

Only the **differences** from the defaults are stored. Writing the whole resolved set would pin
every existing user to the defaults that were current when they installed, having never chosen
them — so an author who improves one in an update reaches everybody who left it alone.

Adding the section found a hole worth recording: `renderSection` is a `switch` returning
`ReactNode`, `undefined` is a valid `ReactNode`, and a `SettingsSection` variant with no `case`
therefore compiled and opened a **blank page** with no error anywhere. It has a `never` guard now,
the same one `chrome/TabStrip.tsx` puts on `TabKind`.

### Trust is a list the user was shown, not a list the manifest asked for

`extensions.json` records `granted` — the capabilities the install sheet displayed — and an
install whose manifest asks for more is **refused, not upgraded**. Without that a marketplace
could add `process:spawn` in a commit and every machine that ran a refresh would grant it
silently. The check runs again on every load, because that file is one a user may hand-edit.

The consent line is drawn beside the button and not behind one more click, in words rather than
capability strings: `process:spawn` tells a user nothing, *"run programs on your machine"* tells
them what they are deciding.

An unknown capability **greys the extension** rather than being ignored — a restriction cide
cannot name is one it cannot enforce, and running it anyway would run it under less restriction
than its author declared. An unknown *key* only warns. That asymmetry is `cide-agents`', and so is
everything else about how a bad manifest is handled: one broken manifest never stops the others
loading, a defect greys a row rather than hiding it, and every refusal carries a path and, where
it has one, a line.

### Not done

- **Four bugs found the first time it ran in a real window, and all four are fixed.** A worker's script
  URL must be **same-origin** — no header relaxes that, and it is the rule that makes `new Worker`
  different from `importScripts` — so `new Worker('cide-ext://…')` from a `tauri://localhost` page
  was refused at construction. Workers are now built from a same-origin `blob:` shim whose only
  statement imports the real module, which keeps relative imports inside an extension resolving
  against `cide-ext://`. The second bug was the first one's symptom: a module worker that fails to
  *load* fires a plain `Event` with no `message`, and the panel printed `YAML: undefined`. The
  error path now names the URL when there is nothing else to name. A third, found while reading:
  a worker started *after* a file was open was never told about it, so its panel sat on "open a
  .yaml file" over a `.yaml` file until somebody typed — which is what enabling an extension looks
  like. A new worker is now seeded with the active editor.

  The fourth was the worst, because everything about it looked fine. `Capability` carried
  `rename_all = "camelCase"`, so the **wire** said `editorRead` while `Capability::as_str` — what
  a manifest is parsed against — said `editor:read`. The frontend compared against the manifest
  spelling, so `capabilities.has('editor:read')` was false for every extension ever installed: no
  worker was ever told about an open file and **every host request was refused**. Both panels sat
  on "open a .sql file" over an open `.sql` file. There is one spelling now — serde is told to use
  `as_str`'s — and the check that should have caught it was comparing the frontend's list against
  `as_str`'s table, which is both sides of a two-sided agreement and neither of them the wire. It
  compares against `generated.ts` now.
- **None of it has been confirmed on screen.** Every gate below passes and the whole install road
  is exercised end to end by `cargo test -p cide-ext` against the real sibling marketplace — but
  that test drives `ExtStore`, not a window. Nobody has yet watched a worker start, a rail button
  appear, or a `.sql` file change colour. Same reason as M14, M15 and M16: `./run.sh` stops the
  running instance.
- **`yaml-language-server` has not been run** — it is not installed on this machine, so what has
  been checked is that it is registered with the right arguments (`--stdio`) and not that it
  answers. `sqls` **has** been run, by hand, and the finding is in the section above.
- **Enabling a language extension does not recolour an open editor.** The fold table has to be
  present at first paint (`foldSpecFor` runs inside the mount dispatch and cannot await), so the
  language registry rides `Bootstrap`. A panel appears immediately; a buffer follows on its next
  mount. The panel says so rather than pretending otherwise.
- **There is no update-all, and no version constraint.** An extension declares a `version` string
  that is compared for equality. A marketplace that ships a breaking change to a cide that cannot
  run it is caught by `schema`, and nothing finer exists.
- **A worker cannot `fetch`.** `connect-src` deliberately omits `cide-ext:`, so an extension that
  needs data ships it as a module it imports. That is a real limit and it is the conservative
  direction to have picked first.
- **No extension pane.** A `PaneKind` is a closed Rust enum in `workspace.json` and adding one
  would put a derived fact in the persisted tree — the argument `PaneBody.tsx` already makes for
  refusing `PaneKind::Image`. Contributions get a sidebar panel and a tool-window tab.

## Updating a project, and resolving a conflict (M20), and what is not done

Ctrl+T was bound to `git.pull` and the command was titled **Pull (fast-forward only)**, which is
what it did: `cide_git::branch::pull` fetched, and the moment the branch had diverged it returned
`NotFastForward` with both counts and a sentence telling the user to merge or rebase in a
terminal. That is the IDE's headline key sending you to a shell for the most ordinary thing that
happens on a shared branch.

It is now IDEA's **Update project**: fetch, fast-forward where it can, and otherwise merge or
rebase — asking which, once, when nothing has said.

### The ladder, and where the answer is stored

`branch.<name>.rebase` → `pull.rebase` → cide's own `Settings › Git` default → ask.

The repository's own configuration wins over the application's, because `pull.rebase` is a fact
about a project's workflow — often set by whoever set the repository up — and a global preference
that silently overrode it would make cide disagree with the `git pull` typed into a pane two lines
below. A user who has already run `git config pull.rebase true` is therefore never asked.

The dialog's *remember this choice* box writes `pull.rebase` into that repository's **local**
`.git/config` — `Config::open_level(ConfigLevel::Local)`, which is explicit rather than
load-bearing: a bare `set_bool` already lands there for an ordinary checkout, and differs only
under `extensions.worktreeConfig`, where it would scope the answer to one worktree. `~/.gitconfig`
is never touched. It starts **unticked**, which inverts `ConfirmDestructive`'s house rule for a
checkbox — that rule is for an option that makes an act *recoverable*, and this one makes an
answer permanent in a file nothing outside Settings will remind anyone about.

Parsing `pull.rebase` reproduces `builtin/pull.c::rebase_parse_value`, and three clauses of it
are not guessable: git's boolean set is `true`/`yes`/`on`/any non-zero integer against
`false`/`no`/`off`/`0`/empty; a **valueless** key — `[pull]` then a bare `rebase` line — is
`true`, and `Config::get_string` cannot see the difference between that and a missing key, so
`ConfigEntry::has_value` is the only way to ask; and `preserve` or a typo is treated as
*unconfigured* with a warning, because git dies on it, cide cannot die, and quietly choosing
merge would be a lie about the user's own file.

### The decision that reverses a written one

`crates/cide-git/src/replay.rs` had rejected libgit2's stateful `revert`/`cherrypick` — and by
extension `merge` and `rebase` — for two reasons. The first, that `.git/index` is derived from a
changelist under ADR 0004, is answered directly: `cide_git::commit` now takes the staging-area arm
whenever it is concluding an operation, because on a merge **the index is the truth** — it was
built by the merge and edited by the resolutions, and `rebuild_index` would `read_tree(HEAD)` over
it and commit HEAD-plus-selected-hunks under a message saying the branches were merged.

The second was the load-bearing one: *the moment `RepositoryState` is non-clean,
`operation_in_progress` starts refusing every other action in this crate — from a panel with
nothing on it that can finish or abort the operation.* That panel now exists, so the decision
reverses with its reason. `docs/adr/0009-real-sequencer-state.md` records it, because reverting
this by reflex from `replay.rs`'s header is the likeliest way it gets undone.

Real git state is what makes a conflict resumable across a restart, legible to `git status` in a
terminal pane, and abandonable with `git merge --abort` by somebody who would rather not use the
resolver at all.

### Resolving

A conflicted pull is **not an error**. `FetchOutcome.conflicts` comes back non-empty and the
gesture succeeded; the user now has work to do, and an `Err` would route it to the red toast
`chrome/Failures.tsx` draws for refusals.

The commit panel already had a *Merge Conflicts* group — `FileState::Conflicted` and
`RepoChanges.conflicts` have been on the wire since M10 and `model.ts` draws the group first — and
every write verb refused on it with *"Resolve the conflict first"*. There was no verb that
resolved. There are five now: *Resolve…* opens the three-pane tab, *Accept ⟨side⟩* takes one whole,
*Un-resolve* puts a file back, and a `MergeBar` above the tree carries *Resolve simple*,
*Continue* and *Abort*. `GuardBar`'s argument for a bar rather than a modal holds one step
stronger here: the honest default of a conflict is *do nothing yet*.

**`ours` and `theirs` are index stages, not the words on the buttons.** During a *rebase* git
swaps them — stage 2 is the branch being rebased onto and stage 3 is your own commit — so
`cide_git::conflict::side_labels` decides the wording and the panel only draws it. A resolver
that hardcoded *Accept Yours* over stage 2 would hand the user the other branch's work under
their own name.

### The three panes, and the merge they compute

`@codemirror/merge` ships a two-pane `MergeView` and a `unifiedMergeView`, and no three-way
primitive at all, so the panes are three plain `EditorView`s — left `ours`, centre editable, right
`theirs`, with line numbers in all three.

**The centre opens as the base revision**, which is IDEA's model and its documentation's own
words: *"Initially, the contents of this pane are the same as the base revision of the file, that
is, the revision from which both conflicting versions are derived."* Every difference either side
made is a block you take or reject.

This reverses an earlier decision, and the reversal is the interesting part. The resolver first
**parsed git's conflict markers**, on the argument that git had already merged the non-conflicting
hunks and re-deriving them risked disagreeing with the index cide was about to commit. The
argument is sound and it produced the wrong tool: the hunks git had already applied were
**invisible and unrevertable**, so a merge tool could show you two decisions out of thirty and
call the rest settled. What that argument was really protecting is preserved from the other end —
**the resolved text is written to the working tree and staged verbatim**, so what the user sees is
what gets committed, whatever any algorithm thought.

So the merge is computed here, from the three index stages, by a **patience diff** over lines.
Patience rather than a dynamic table because that is `O(n·m)` cells — 144 million for two
12,000-line files, in a webview, on a keystroke — and because it anchors on lines unique to both
sides, which produces the hunks a person expects rather than matching braces and blank lines
across unrelated blocks. It is what `git diff --patience` does.

**A chevron per block, in the gutter of the side it belongs to** — `»` in the left pane, `«` in
the right, with IDEA's `X` beside it to reject. **Both disappear once that side is answered**,
which is IDEA's behaviour: a chevron that stayed after being used looked like it had not worked,
and clicking it again did something else entirely. A side has three states, not two — asking,
accepted, rejected — and *rejected* is what a toggle cannot express.

Rejecting **keeps the base**; it does not delete the block. Deletion is what you get by accepting
a side that deleted it, which is a decision somebody made rather than the by-product of two
clicks.

**Both sides, in the order you click them.** A block's answer records `taken` as an ordered list,
because the commonest real conflict is two people adding a function, a test or an import at the
same line — where the answer is neither side but both, and the order is the order of the resulting
file. Two insertions at one base line are both zero-width there, so they overlap and become **one**
block offering both. JetBrains describes the same case: *"the change on the other side will remain
open… to combine the changes from both sides, you can choose to accept them both."*

**A highlight means work you have not done.** A block is lit while it is still asking and goes
dark the moment that side is accepted or discarded — in all three panes, by one rule. Two tones,
not four: red for an undecided conflict, **blue for an undecided one-sided change** (a change, not
a problem). Painting the answered blocks as well leaves a merge of thirty with thirty coloured
bands and nothing to separate the two you have not read from the twenty-eight you have; the colour
stops being a signal at exactly the point it is needed.

It is not a record of what was decided. The result column's *text* is that record, and the `↺` in
the gutter marks every side that was answered, in the pane it was answered in.

The side panes ask this **per side**, so answering the left half of a conflict quietens the left
pane while the right stays lit — which is what tells you the half that is left. The result asks it
per region, because a block is only finished there once both sides are.

The decorations are a `StateField` rather than a fixed set, so an edit in the centre pane moves
them with the text instead of throwing a range error.

*Apply non-conflicting* takes the one side of every block only one side touched, and
`Settings › Git › Apply non-conflicting changes automatically` runs it on open — **off by
default, as it is in IDEA**, because applying two thirds of the blocks before the user has looked
undoes the reason for showing them. The default changed within this milestone, and a changed
default does not reach a value that is already stored — so `persist`'s ladder grew a **schema 4 →
5** arm that deletes the key. That is the inverse of what `v1_to_v2` does two functions above it,
which *writes* a default down so a later change reaches new installs and nobody else; the
difference is whether the stored value was ever a decision. This one never was: no released build
offered the setting, and everyone who ran an intermediate one got `true` without being asked.

The side panes are **read-only but selectable**: `EditorState.readOnly` alone, deliberately
without `EditorView.editable.of(false)` beside it. `DiffPane` uses both and is right to; here the
second one takes the caret away and with it keyboard selection, Ctrl+A and Ctrl+C — and these
panes are where the text somebody wants to copy *into* the result lives. A pane you cannot copy
out of is a pane you have to retype from.

All three panes carry the editor's own syntax highlighting — the same `cideHighlightStyle` over
the same `TOKEN_ROLES`, so a merge does not show the file in different colours from the tab beside
it. Wiring the highlighter turned out not to be enough: the `.cide-tk-*` colours lived in
`EditorSurface.module.css`, **scoped under that module's `.body`**, so they painted inside an
`EditorSurface` and nowhere else. The resolver got a highlighter, got tokens, got the class names
onto its spans and got plain text, with nothing in the build failing. They now live in
`editor/highlight.css` — a plain stylesheet, because CodeMirror writes the literal class name and
a module would hash it — loaded by `main.tsx`, because `check:editor` runs `highlight.ts` under
node where a `.css` import cannot resolve. The grammar arrives through a `Compartment` once its chunk lands, for `EditorSurface`'s
reason: it is a dynamic `import()` that is not available when the editor is built, and rebuilding
to add it would take the scroll position and selection with it.

Inside a lit line, the part that actually moved is marked — a **word-level diff** against the
base, split on identifier boundaries rather than characters, because a character diff over
`alpha` → `beta` marks a scatter of shared vowels. The three panes **scroll together**, anchored
on block boundaries rather than on a scroll fraction: the documents are different lengths, so 40%
down `ours` is not 40% down the result, and the drift grows with every block taken. Two details
there are worth the words, because the first version of it had both wrong and the symptom was
*scrolling takes you back to the top*. `scrollTop` is **not** a document-relative height —
CodeMirror measures blocks from the top of the document, which sits at `view.documentTop` — so the
conversion goes through that. And the re-entry guard is a set of marks rather than a timer: a
programmatic `scrollTop` fires its own `scroll` event a frame or more later, so a flag released on
the next frame was already down when the echo arrived, and the three panes converged on line one. **Ctrl+Z**
walks back through the block decisions, which the centre pane's own CodeMirror history does not
cover — a block accepted by a chevron is not an edit anybody typed.

*Apply* is gated on **every block** being answered — not only the conflicting ones — and on no
conflict marker anywhere in the text. It used to light up as soon as the conflicts were done, on
the argument that leaving a one-sided change keeps the base and is a valid outcome. It is a valid
outcome and a terrible default: the user is looking at a chevron and an `✕` still sitting in the
gutter, has not decided about them, and the tool is telling them they have finished. Rejecting is
one click and *is* the answer for a change you want the base for.

The marker check is not redundant with that.
The centre pane is a real editor, a marker can arrive by typing, and
staging marker soup is the worst thing this surface could do — it commits cleanly and breaks the
build for everybody. It checks only `<<<<<<<` and `>>>>>>>`; a line of seven `=` is a Markdown
setext heading, and a resolver that cannot save a README is one people work around. An unanswered
*one-sided* change blocks nothing — leaving it means keeping the base, which is a valid outcome.

### One file at a time, and the merge commits itself

A merge of four conflicted files is a sequence, not four unrelated gestures. Resolving one closes
its tab and brings the list back with that row ticked and the rest still to do; closing a resolver
without answering does the same, rather than leaving somebody in a repository that is mid-merge
with nothing on screen saying so. **When the last file is answered the merge commits** — a merge
with every conflict resolved has exactly one remaining action, and `git reset --hard ORIG_HEAD`
covers anybody who disagrees.

### Notices, and four defects fixed on the way past

A pull already reported well. A **push** reported nothing at all on success — `keys/dispatch.ts`
was a bare `void Promise.all(...)` — and had no `.catch` either, so a *failed* push toasted the
single word `push` (`GitError::Push`'s detail is an object, so `notices.describe` fell through to
the tag). Both are fixed, `PushOutcome` carries the counts, and multi-root pushes aggregate into
one notice the way pulls do — `notices.admit` dedupes by text, so five submodules each saying
*"already up to date"* would otherwise show one toast speaking for five.

* **`branch.rs` was the only mutating module in `cide-git` that never called
  `changelist::record_index`.** A `checkout_tree` makes libgit2 rewrite `.git/index`, so the
  sidecar's fingerprint described an index that no longer existed and the *next* commit refused
  with `IndexChangedExternally` — pointing the "staging changed outside cide" bar at cide's own
  act.
* **`branch::pull` checked neither `operation_in_progress` nor `conflicted_paths`.** A pull run
  during a rebase spent a network round trip and then failed inside `checkout_tree` with a raw
  `GitError::Git`. Both checks now happen *before* the fetch.
* **`behind == 0` with `ahead > 0` was reported as a divergence.** The test was `ahead > 0`
  alone, so a branch with one unpushed commit and a quiet remote was told it had diverged and
  handed two counts explaining why it could not fast-forward. `git pull` says *Already up to
  date*.
* **A push libgit2's route could not deliver reported success.** `git_remote_push` returns `Ok`
  when the *transport* worked, whatever the remote decided about the refs; a server-side
  rejection arrives only through `push_update_reference`, which nothing was listening to. And a
  refused push from *Commit and Push* landed in the panel's one dim note line while the palette's
  Push, on the same rejection, put a red box on screen — two routes to one gesture disagreeing
  about whether it worked. Both now toast.
* **An open editor did not follow a pull.** `cide://session-tool` covers an agent's edits and
  nothing else, so pulling a branch that changed a file you had open left the pane showing the
  old text. It now re-reads on `cide://git-status` and **compares the stamp** before reloading —
  that event fires on every stage and unstage, and a rebuild per stage would take the scroll
  position, selection and undo history with it.
* **`git_push` had no `AppHandle` and broadcast nothing**, so ahead/behind counts went stale in
  every other window until something unrelated refreshed them. And the panel's *Update project*
  button, disabled since it was written with the tooltip *"needs a pull command"*, is finally
  passed one — `git_pull` had existed since M10.

### Not done

* **No `--rebase=merges` and no interactive rebase.** A merge commit among the commits a rebase
  would replay is **refused** (`RebaseWouldDropMerges`) rather than flattened: `git rebase` drops
  it silently and the conflict resolutions recorded inside it go with it, which is a content
  change on a branch about to be pushed with no undo surface. The dialog's other button merges.
* **No autostash.** A pull that would overwrite local changes refuses by naming them — the same
  refusal a checkout makes, computed by the same `blockers_against_tree`.
* **Two documented differences from `git rebase`**, both *reported* rather than hidden, through
  `FetchOutcome::skipped`. libgit2 has no `--cherry-pick` patch-id pre-pass, so a commit whose
  patch is already upstream but whose replay would conflict stops the rebase where git would have
  dropped it before trying. And libgit2 answers `GIT_EAPPLIED` for a commit that was **empty to
  begin with**, which `git rebase` keeps — it offers no way to force one through mid-sequence, so
  cide follows libgit2 and counts it.
* **`REBASE_COMMIT_CAP` is 100**, refused up front and never truncated. A truncated rebase is a
  rewrite that lost commits.
* **A file resolved outside cide mid-merge** — `git checkout --ours` in a pane — is picked up on
  the next refresh, but a resolver tab already open on it goes on showing what it read.
* **The merge tab is not remembered by *Reopen closed tab*.** It is a query about a transient
  state: by the time somebody presses the chord the merge may be finished, aborted, or resolved
  differently in a terminal.
* **Nothing here has been confirmed on screen.** The Rust side is covered by
  `crates/cide-git/tests/pull.rs` (differential against the real `git` binary, including the merge
  message byte for byte) and `tests/conflicts.rs`; the frontend by `check:pull-strategy` and
  `check:merge`. The three-pane resolver has not been driven by hand.

## The git tool window: log, graph, file history and blame (M19)

A bottom panel — IDEA's *Git* tool window — opened from a new button pinned to the foot of the
activity rail. It holds a **Log** tab and one closable **History** tab per file, and it brings a
**blame** gutter to the editor. `cide-git` had shipped the whole *commit* half of git since M10
and none of the *investigate* half: the only `Revwalk` in the workspace was private, capped at ten
commits, and existed to populate a pull toast.

### `git2::Revwalk` could not be used, and that is the load-bearing finding

The obvious implementation is `Revwalk` with `Sort::TIME` and a page limit. It does not stream.
`revwalk.c:762` is `if (walk->sorting != GIT_SORT_NONE) walk->limited = 1;`, and `:660` then runs
`limit_list` — a `while (list)` loop draining the **entire reachable set**, inside the first
`next()`. Only `Sort::NONE` streams, and its order is documented as arbitrary. So a scan budget
over a `Revwalk` would bound our loop and nothing else.

`crates/cide-git/src/log.rs` therefore drives its own walk from an explicit `BinaryHeap` frontier.
The performance is not the reason — this repository is 242 commits, where `limit_list` costs
nothing. The *features* are: a serialisable frontier is what lets graph lanes continue across a
page boundary without recolouring rows already on screen, a per-frontier-entry tracked path is
what makes `--follow` coherent through a merge, and one shared heap is what makes a merged
multi-root log a k-way merge rather than a second algorithm. The oracle is the real `git` binary —
the emitted oid list is asserted equal to `git log --format=%H` under each mode, which is the
discipline `patch_props.rs` already sets for the other dangerous code in that crate.

### Paging is a cursor, and `budget` is not the end of history

A parent is pushed with `key = min(committer_time(parent), key(child))`, so the popped key
sequence is non-increasing *by construction* and keyset paging is exact. Every page reports why it
stopped, and the six reasons are all reachable in the UI. **`Budget` — the walk inspected its
whole allowance without filling the page, which on a filtered search over a deep history is the
expected outcome — renders as "Searched 20,000 commits — keep looking?" and not as an end of
list.** A list that drew it as the end would silently lose the answer the user was looking for.

### What the graph does and does not draw

Lanes are never compacted: a freed column is reused by the next new lane rather than shifting its
neighbours left, which is what collapses a crossing line to "enters and leaves in the same column"
and makes every row self-contained. Colour is a property of the lane instance, not of the column,
and **first arrival wins** — re-pointing a lane when the mainline child turns up would recolour
rows already on screen. The property that makes it verifiable: a history walked in one page of 500
and in five of 100 produces byte-identical rows, checked over 40 seeds × 4 caps × 4 page sizes.

The graph is **off**, with the reason said out loud, whenever the row set is not closed under a
parent function: an author or text filter (as IDEA does — inventing a "nearest surviving ancestor"
edge would assert a reachability nothing checked), a merged multi-root scope, and full-history
mode. A path filter under the default simplification *keeps* its graph, because there the parents
are rewritten — which is why `git log --graph -- path` works at all.

### Blame, and the two things libgit2 does not do

`BlameOptions`' four `track_copies_*` setters are **inert**: `blame.h:36-64` marks every one of
them *"not yet implemented and reserved for future use"*, so calling them costs nothing and does
nothing — the exact shape of the dead `repoOpen` flag. `git blame -M`/`-C` therefore shells out to
the binary, as an explicit request variant rather than a flag, and reports what it *actually* ran
in `BlameFile::follow` rather than what was asked for. Whole-file rename following is free the
other way: `blame_git.c:470` runs `find_similar` with `GIT_DIFF_FIND_RENAMES` unconditionally.

The payload is runs plus a deduplicated commit table — a 5000-line file is ~800 runs and ~300
commits, about 60 KB instead of 500 KB per line — and the table is built with **no `find_commit`
at all**, from the hunk's own signature. `check_runs` asserts the runs are ascending, gapless and
cover every line, on **every** route: a run set with a hole paints a gutter that is silently one
line off for everything below it, and every line still carries a label, so nothing downstream can
notice.

A dirty tab hands its text over so the answer is right from the first frame; live edits after that
are mapped through CodeMirror's own `tr.changes` rather than re-fetched, and any line a change
touches is drawn uncommitted whatever marker it still carries — an edited line's anchor survives,
and keeping the previous author's name on it is a per-line falsehood rather than a stale view.

### The commit actions refuse rather than force

Revert, cherry-pick, reset (soft/mixed/hard), amend, tag and branch-from-here. Three of the six
needed no new backend at all — `branch::create` already took a start point, `commit::commit`
already amended and already kept the original author.

libgit2's *stateful* `Repository::revert`/`cherrypick` are deliberately not used: they write
`REVERT_HEAD`, set `RepositoryState` and leave the repository half-finished, at which point
`repo::operation_in_progress` starts refusing every other action in the crate from a panel that
has no *Continue* button. Instead `cherrypick_commit`/`revert_commit` are asked for their
**in-memory index**, which writes nothing at all — so "would this conflict, and where" is answered
before anything moves, and a conflict is reported with its paths. `nothing_leaves_a_sequencer_state_behind`
asserts it after every successful action.

Reset re-records the index fingerprint on **both** the success and the failure path, because
`git_reset` writes `.git/index` for mixed and hard and a stale fingerprint would make the very
next commit refuse with `IndexChangedExternally` — pointing at cide's own act. The guard fires for
mixed and hard and not for soft, and in `use_staging_area` mode it cannot fire at all, which is
correct because there the index is the user's; the dialog's Mixed wording differs per mode for
exactly that reason. Amend carries the oid it believes it is amending, checked against the
repository rather than against the row the menu was drawn on, because a `git commit` in a bash
pane can move HEAD in between.

The three reset kinds, revert and cherry-pick are compared against the real `git` binary over
randomised working trees — HEAD, `ls-files --stage` and a hash of the whole worktree, byte for
byte, a thousand cases clean.

### Reading a commit: a range, a file as it was, and blame in the diff

Ctrl+click a second row and the details pane becomes a range — `Comparing d4e5f60 … a1b2c3d`,
the changed files with their `+41 −9`, and a **⇄ Swap**. Which end is newer is decided by the
log's own order and not by click order, so clicking bottom-then-top does not invert the diff;
Swap therefore cannot be an exchange of the two slots, because the re-sort would undo it and the
button would visibly do nothing. It records a flip instead, and any gesture that changes *which*
commits are selected clears it — a flip carried onto a new pair would silently invert a diff
nobody flipped. *Compare with…* resolves a typed name through `git rev-parse` and stores the oid
it resolved to, because a tab holding `main` would name a different tree tomorrow.

**A file as one commit left it** is its own read-only tab (`log.rs @ a1b2c3d`) with a breadcrumb
back through the revisions that led there — `working tree ← a1b2c3d ← d4e5f6a`, each crumb
truncating the chain rather than growing it. *Annotate previous revision* lands here rather than
on a diff, which is what the button means: `blame_with`'s `newest` blames the file **as it was at
that revision**, so the point of the hop is to read that file with its own gutter. The diff it
used to open was worse in two concrete ways — `git_diff_revision` answers `NoSuchChange` when the
parent did not touch the path, so a hop through a merge showed a *refusal* where a file was asked
for, and its old side was the parent's own first parent rather than anything the walk chose.

The pane keeps `path` as the real path — so the language detection, the find bar and the status
trail are all unchanged — and hands `EditorSurface` a separate **`identity`**, git's own name for
the object: `a1b2c3d:src/log.rs`, exactly what `git show` takes. That is what keys
`registerReveal`, `viewTracker`, `ctrlLink`, `navRecorder` and `claimCaret`. `registerReveal` is
the one that made it necessary rather than tidy: it delivers to live receivers and only parks when
there are none, so a second buffer registered under the real path made a search-result click into
that file silently stop moving the caret. `claimStatusReadout` deliberately keeps the real path,
because the status bar is about the file the user is looking at rather than about which registry
slot it occupies.

The prop defaults to `path`, so no existing caller changed and `tsc` proves it. The build effect's
key stays `[path, reloadKey]` and must: an identity is contracted to be fixed for the life of a
mount, which every caller satisfies structurally rather than by care — Rust keys
`TabKind::Revision` on `(repo, path, rev)`, so another revision is another tab and therefore
another mount. Everything ever added to that array has cost somebody their scrollback, their undo
history or their unsaved edits, and `check:editor` and `check:blame` both pin it letter for letter.

The pane also withholds `project`, which turns off Ctrl+click — go-to-definition sends a line
number to a server reading the file *as it is now*, and a position from a forty-commit-old buffer
names a different symbol.

The git diff pane has a **blame column** too, on the new side, off by default and toggled per tab.

### The changed-file list is a tree, and for two milestones it only looked like one

Reported as *"changed files in commit view (bottom panel) currently has wrong tree — each folder
is a row, but this should be a real tree, like file tree."*

The list has had a **grouped** reading and a **flat** one since it grew the toggle, and the grouped
one bucketed files by their whole directory path and drew one row per bucket. That is
indistinguishable from a tree in a commit touching two directories, which is what every fixture in
the suite had, and stops being one at four: `ui/src/panes` and `ui/src/sidebar/GitPanel` were two
unrelated top-level rows spelling `ui/src/` twice, with no shared parent to fold and nothing nested
inside anything, in a pane whose whole job is *which areas did this touch*. Nesting was not merely
missing — a bucket key has no parent, so it was **unrepresentable**.

It is now the same `DirNode` tree the Git panel builds, walked the same way and with the same three
rules: single-child chains compacted so `crates/cide-git/src` is one row rather than three;
directories before files at every level; and otherwise the input's order, never alphabetical,
because the backend returns a commit's files in the order the diff produced them and re-sorting
would make the grouped and flat readings disagree about which file is first. A compacted row keeps
the **deepest** path as its id, which is what lets a fold outlive the commit it was made in, and
folding a parent takes its nested headings with it. A heading now carries the same `depth * INDENT`
margin a file row has always carried, because it is a row *in* the tree rather than a label pinned
to the left edge.

**Why the whole suite stayed green through it**, which is the part worth writing down: a flat list
of directories and a real tree emit the *same labels in the same order* for any commit whose
directories are each a single chain from the root — and both fixtures were. Only the margins differ.
So the fix came with a `nested` story shaped to be the one that can tell them apart — three
directories under one shared parent, one of them a chain worth compacting *below* a heading, one
compacted *at* the root, and a file at the repository root that must land last rather than vanish —
and `check:log-render` now asserts on each heading's indent, not just on its text. `check:log`
drives the same shape through the model directly, including a fold aimed at a nested row.

### The actions are reachable, and every refusal is a sentence

Right-clicking a commit offers Revert, Cherry-pick, *Reset here…*, *Tag…*, *New branch from
here…*, a detached checkout and *Copy revision number*. Reset opens the three-mode dialog with the
files it would discard **named**, not counted, and a shelve-first box checked by default — the
recoverable answer should be the one a reflexive Enter produces, which is the same instinct that
puts focus on Cancel. A merge revert that arrives without a mainline opens a picker built from the
parents the refusal carried, rather than guessing 1. A detached checkout is attempted with
`mode: 'refuse'` first, always, and only offers to stash once git has said what is in the way.

Every one of the fifteen new `GitError` variants has a sentence in `branchModel::explain`, and
`check:log-actions` drives all fifteen with real `{kind, detail}` objects, asserting that none of
them falls back to the bare tag and that each differs from the same error with its detail stripped.
A `GitError` is an object, so `String(error)` is `[object Object]` — the bug `check:branches` was
written for, one surface over.

### *Amend…* reaches the commit box, and a reword is one of the things it can do

The act is `git_commit` with `CommitRequest.amendOf` set, but the *control* is the Git panel's
commit box — it owns the message, the ticked paths and the Amend checkbox — and a second amend
implementation would be a second place where "the original author is kept" can stop being true.
So the log's *Amend…* does not amend. It fetches the message and parks it through
`ui/src/chrome/panelRequests.ts`, which reveals a panel from outside React: `App.tsx` fills the one
slot, `keys/dispatch.ts` no longer carries a `showSidebar` closure, and every reveal in the app now
goes through it. The panel comes up with **Amend ticked**, the message prefilled from
`CommitDetail.message` whole (body included), and the oid on the wire — so `require_amend_head`
refuses rather than rewriting the wrong commit when HEAD moved between the menu opening and the
confirm. The parked request expires after ten seconds (`AMEND_TTL_MS`), because a prefill that
fires minutes after the click names a commit that may no longer be the tip.

A box that already holds a draft is **not** overwritten. `planAmend` opens the house
`ConfirmDestructive` naming both messages — the draft that would go and the commit's that would
replace it — and Cancel drops the whole request rather than ticking Amend and leaving the draft,
which would arm a rewrite of HEAD with a message written for something else. The draft is quoted in
the *body* and `files` is empty, because the dialog runs each file entry through `basename`, and a
draft reading `fix: handle a/b paths` would be drawn as the file `b paths` in the directory
`fix: handle a` — the dialog misquoting the text it is asking permission to destroy.

**A reword — an amend whose only change is the message — turned out to be blocked in three
places, each of which looks like the whole fix on its own.** Nothing is ticked in that case, so:
`cide_git::commit`'s `selections.is_empty()` guard answered `NothingToCommit`; the Commit button
was disabled; and `commitUnits` yielded no units, which `commit` returns early on. Fix only the
first and the button stays dead. Fix the first two and the button lights up on a click that
silently does nothing — the worst of the three states, because it is the one that looks like it
worked.

The Rust guard is **widened, not deleted**. What it is actually for is refusing an *ordinary*
commit of an empty changelist **before** `rebuild_index` clears the index to HEAD rather than
after, having already destroyed it; `an_ordinary_commit_with_nothing_selected_is_still_refused`
pins that, and fails if the amend term becomes an unconditional skip. The reword is then right by
construction — `rebuild_index` with no selections leaves the index at HEAD's tree and `head.amend`
writes it under a new message, so unstaged edits and untracked files survive, asserted.

The two frontend halves read **one** value, `useGitPanel::rewordRepo`, rather than each deriving
"is this a reword" for itself: `model.ts::canCommit` lights the button from it and `commitUnits`
mints the file-less unit from it. Two derivations of one fact disagree eventually, and the way
they disagree here is exactly the silent no-op above. It is non-null only when a repository is
**named** — `amendOf.repo` from the log, or the sole root of a single-root project — because the
bare Amend checkbox means "HEAD", and in a monorepo there are several HEADs with no honest way to
pick one; a reword of an unnamed repository would rewrite whichever root sorted first. The unborn
check is against that named repository and not `canAmend`'s "some repository has a HEAD", which is
the right question for the checkbox and the wrong one here. Both rules live in `model.ts` so
`check:git` can compile and run them, including the case where an amend *does* tick something and
must not also pick up a unit for a repository nobody selected in.

### Two watcher bugs found on the way, and fixed

Neither was caused by this work. `refs/heads` was watched **non-recursively**, so a commit on a
branch whose name contains a slash — `feature/login` — produced no event: it lives in
`.git/refs/heads/feature/`, which nothing watched. It appeared to work in the session that created
the branch, because the watcher picks up newly-created directories, and stopped silently after a
restart. And a **linked worktree** was only half-watched: `HEAD` and `index` are per-worktree while
`refs/**` and `packed-refs` live in the common dir, so a ref change in one was invisible — which
matters here, because agent worktrees live in `.claude/worktrees/`. `refs` is now recursive,
`packed-refs`/`ORIG_HEAD`/`FETCH_HEAD` are watched, and `repo::watch_dirs` returns both directories.

A `GIT_NEVER` guard landed with them, before anything needed it: `is_git_path` is a `starts_with`
over the watched list, so the moment a whole gitdir joins that list `.git/objects/**` becomes
admissible and a fetch's 256 fanout directories each get an inotify watch — the exhaustion
`watch.rs`'s header exists to prevent.

**No new event, and no new watcher.** `FsChange.git` already existed, already covered `HEAD`, the
index and the refs, and already rode `cide://fs-changed`; it simply had no consumer that read it.
The log and the blame gutter are its first. (The Git panel and the branch readout joined them in
M17 — see *The Git panel followed cide and not the disk*, which is that doc comment being made
true several milestones after it was written.)

### Full width, and a refresh that does not throw the page away

The panel is a **sibling of `.body`**, not a child of `.content`. `.body` is the horizontal row —
rail, sidebar, splitter, panes — so anything inside it is bounded by the column it sits in; placed
between that row and the status bar, the panel runs the whole window width with the rail and the
sidebar ending above it. That is IDEA's default. The first version put it inside `.content` so the
sidebar would keep its full height, which is IDEA's *widescreen* variant — an option there, and
not what was wanted here.

**A refresh keeps what is on screen.** The fetch effect fires for two reasons that want opposite
treatment: a new question — another project, scope, path or filter — must throw the old answer
away, because it is about something else; a refresh is the same question asked again, and
clearing there is what made the panel appear to flicker and lose its selection about once a
second. It now compares a `questionKey` against the last one and only resets on a genuine change.
`logStatus` already had the other half — *Reading the log…* is reachable only when `rows === 0` —
so a soft reload is invisible until it lands and then swaps in one paint, and React reconciles an
unchanged row to nothing.

The selection is kept across it and pruned afterwards rather than dropped up front: an amend or a
rebase does replace the oid, so a row that is gone is deselected once the new page is in, each end
of a pair independently, with a collapse onto the surviving end rather than a range with one side.

Why it fired so often is worth recording, because it is a case of a guard being defeated by the
workload it was written for. `gitRefsMoved` exists so that a `git add` does not restart a frontier
walk — but it returns true for any **truncated** burst, since a dropped path list cannot be shown
not to contain a ref. A tree with several agents writing to it never goes quiet, so `cide-fs`
flushes on `max_wait`, every burst is truncated, and the predicate says yes every time. The
coalescing window went from 120 ms to 500 ms with that written down; a commit typed into a bash
pane now appears an eighth of a second later, which is not perceptible, and the walk it triggers
is cancelled the moment anything supersedes it.

### A superseded walk stops, and a cancelled page is not an error

Typing in the filter box starts a walk per keystroke. `LogRegistry` in `cide-app` is
`SearchRegistry` again — the same two load-bearing details copied with their comments, a separate
compare and insert so two windows asking the same question join one job, and the old job cancelled
only *after* the new one has replaced it — but keyed `(ProjectId, ToolTabId)` rather than per
project, because the Log tab and any number of History tabs are live at once and one job per
project would make opening a History tab cancel the Log's walk.

The seam into `cide-git` is a borrowed `&AtomicBool`, which is `cide_search::content`'s choice for
the same reason: a `&dyn Fn() -> bool` would need `Send + Sync` threaded through every frame of the
walk and buys nothing a flag does not already give. `log()` keeps its old signature and wraps
`log_cancellable` over a **local** flag — not a `static`, which would be one stray store away from
stopping every uncancellable walk in the process. The poll is the first statement of the loop body,
before the budget check and before the heap pop, so nothing expensive runs after the flag is seen.

It is threaded through `seek` as well, and that is the half worth naming: `MAX_SEEK` is twice
`SCAN_MAX`, so the cursor path is the longest loop in the module and a cancel that could not reach
it would be a control that does nothing for exactly the request that runs longest. Its `false`
therefore had to stop meaning two things — the `CursorLost` arm is now guarded on the flag, because
without that, typing a character during a reveal would tell the caller its cursor was gone and send
the list back to the top of history.

**A cancelled page resolves, with `cancelled: true` and the rows the walk had reached.** Painting a
supersession as a failure is how a fast typist gets a wall of toasts; the rows are correct as far as
they go and there is nothing to act on. The generation counter the caller already keeps stays the
authority on *which* answer to paint — `cancelled` is the backend agreeing, and is what makes the
abandoned walk stop costing anything.

Two things cancel: the next `page` for the same tab, and `git_log_cancel`. The command is **not**
`async` and not on the blocking pool, which is `diagnostics_usages_cancel`'s argument — queueing a
cancel behind the very walk it is cancelling makes it arrive after the answer. It fires from the
tab's close button, from `LogTab`'s unmount (hiding the panel, switching tabs, switching project and
closing the window all unmount without closing anything), from `fs_close` beside the existing
`searches.cancel`, and from `lifecycle::shutdown`. The unmount cleanup is deliberately its own
effect with an empty dependency list rather than the fetch effect's cleanup: the latter would fire
on every keystroke, racing a cancel against the `page` that was about to supersede the walk anyway.

**Blame cannot be cancelled at all**, and says so rather than being given a control that does
nothing — libgit2 exposes no hook inside `git_blame_file`. `MAX_BLAME_BYTES` is the honest version
of that bound.

### Not done

- **Only *Tag…* has a palette row.** *Revert*, *Cherry-pick* and *Reset here…* each need a commit
  the palette cannot name, and unlike `git.stageSelected` there is no honest default — resetting
  to HEAD is a no-op, and a bare *Revert* in the palette beside the Git panel's *Rollback* is a
  real ambiguity that IDEA has and that confuses people. All three are one right-click away in the
  log; four dead palette rows would have been four dead rows. Ids are API and can be added later.
- **No merged multi-root graph**, and no true topological order or `--simplify-merges`: both need
  the whole graph before the first row, which is incompatible with paging by construction.
- **Blame is not offered on the old side of a diff**, in the split view's left column, or in the
  `@codemirror/merge` pane that answers Claude. The git diff pane *does* have the column, on the
  new side, off by default and toggled per tab — a working diff blames the working file, a
  historical one blames `new_rev`. The old side stays refused because it is a different document
  and needs a second walk at `old_rev` for a column nobody reads while staging, and the merge pane
  stays refused because its unit is the whole file rather than the line. The **staged** side is
  refused too, and for a harder reason: its new text is the index, `cide_git::blame` can only
  produce a commit, the working file or a supplied buffer, and blaming the working file instead
  would be wrong by a line or two on exactly the files that have unstaged changes on top —
  silently, with every row still carrying a plausible oid.
- **The diff pane keeps its own blame state** rather than using `editor/blameStore.ts`, and the
  reason is the store's key: `toggleBlame` resolves a repository through `git_locate` and so needs
  an **absolute** path, while a diff tab holds a repo-relative one and a `RepoId` — and the key has
  no room for the revision a historical diff blames at. One file annotated at HEAD in an editor and
  at `a1b2c3d` in a revision tab would be one entry with two meanings. The part that must not
  diverge — `collapseRuns`, which decides what is drawable and how old a line is — is shared.
- **The tool window is not drawn in a detached-pane window**, which is what keeps a project's panel
  from being shared between two windows.
- **Nothing here has been confirmed on screen.** Same reason as M14, M15 and M16 — KDE will not
  raise a shell-launched window. The coverage that does exist is `check:toolwindow`, `check:log`,
  `check:log-render`, `check:blame`, `check:diff-render`, `check:commands`, `check:menu-model`,
  `check:log-actions`, `check:toolwindow-render`, and 308 tests in `cide-git`, of which the reset,
  revert and cherry-pick suites are differential against the real `git` binary over a thousand
  randomised working trees. What that coverage cannot speak to is whether any of it is legible on
  screen.

## The Git panel followed cide and not the disk (M17)

Reported as *"need autoupdate git tree when got new changes in files."*

The panel had two triggers, `cide://session-tool` and `cide://git-status`, and
`crates/cide-app/src/cmd/git.rs::refreshed` is the **only** emitter of the second. So between them
they answered *Claude did it* and *cide did it*, and nothing else: a `git add` typed into a bash
pane, a `git pull`, a `cargo fmt`, an editor outside cide. In an IDE whose centre of gravity is a
terminal and an agent, that is most of what happens to a working tree, and the panel sat there
until somebody pressed ↻.

**Three comments already said this worked.** `FsChange.git`'s own doc in `cide-ipc` — *"The branch
readout and the git panel refresh on this"* — `client.ts`'s note on `onGitStatus`, and
`BranchSelector`'s *"and so does the watcher's view of `HEAD` and the refs, which is how a `git
checkout` in a bash pane moves the label."* All three described a mechanism that exists, attached
to a subscription that did not. It is the `moveLineUp` shape exactly: a compensation written down,
never built, and read past for milestones because the paragraph reads as evidence.

The visible symptom was the tell and nobody chased it: `chrome/gitCountStore.ts` **did** subscribe
to the watcher, so the change count in the panel's own header stayed correct while the tree beneath
it went stale. The header said 5 and the tree listed 3.

**The fix is two subscriptions and no new plumbing** — `cide://fs-changed` already carried
everything, and every other git-adjacent surface already read it. What is worth recording is that
the two new subscribers filter it **differently, and oppositely**:

* The **panel** refreshes on *any* burst for its project. `change.git` is wrong here — a plain
  working-tree write raises no flag and is exactly what turns a file from clean to modified — and
  `gitlog`'s `gitRefsMoved` is wrong more sharply still, since it answers `false` for an index-only
  burst, i.e. for `git add`. The burst's paths are not inspected either: that judgement belongs to
  `cide_fs::filter`, and re-deriving it from path strings in the frontend would be a second, worse
  copy of it. One coalesced `git status` is the call that actually knows.
* The **branch readout** gates on `change.git`. A label only moves when `HEAD`, a ref or
  `packed-refs` moves; without the gate every file save costs a branch walk of every repository for
  an answer that cannot have changed.

**The coalescer had to change with it, and that was not tidying.** `useGitPanel`'s `schedule` was a
*restarting* debounce — it cleared and re-armed on every trigger — where every other git surface in
the app is a throttle, `gitStatusStore.ts` having written down two milestones earlier why: *"A
debounce that restarts can be starved indefinitely by a steady stream of writes."* That was
survivable while the only trigger arrived in gaps. Feed it the watcher and the steady stream becomes
the normal case, because an agent editing files is what the app is *for* — the panel would have
frozen for exactly as long as anything was happening, which is a worse bug than the staleness being
fixed and would have passed every gate. It is a throttle now, at the same 120 ms as its four
siblings.

`refresh` also gained a **generation guard**. `git status` on a real repository takes longer than
the coalescing window and `invoke` promises resolve independently, so the slower *older* walk can
land last and install a tree computed before the edits that triggered the newer one — and if the
user has stopped typing there is no next event to correct it. Checked after both awaits: the
merge-state pass is a further round trip per repository and is the half most likely to be overtaken.

**Gated by greps, and that is written down as the weak choice it is.** `check-git-tree.mjs` compiles
`model.ts` and runs it; it cannot reach `useGitPanel.ts`, whose own comment already records that a
defect in it was *"invisible to all 29 gates"*. The new block asserts on source text — three
subscriptions, the throttle's one distinguishing line, both generation checks, and the branch
readout's `change.git` gate — because extracting that wiring out of a 2264-line hook is a larger
change than the one being gated. Each assertion was confirmed by mutation.

## Ctrl+D duplicates a line, and what it cost (M17)

`copyLineDown` is `@codemirror/commands`' own and is IDEA's *Duplicate Line or Selection*; the whole
of the work was that the chord was taken. `searchKeymap` binds `Mod-d` to `selectNextOccurrence`,
and `findExtensions()` is added to `EditorSurface`'s extension list **before** the surface's own
keymap — so a `Mod-d` entry appended there would have been offered the stroke second, behind a
command that returns `true` whenever the caret is in a word. It would have read as wired and fired
almost never.

So the capability **moves** rather than being deleted: `selectNextOccurrence` is filtered out of
`searchKeymap` by command identity and re-homed to **Alt+J**, IDEA's own chord for it, free in both
layers. That freedom is *computed* by `check-editor.mjs` and not asserted from this paragraph —
which is the lesson of the `Mod-Shift-Arrow` comment that was wrong for four milestones.

**Not a `cide-core::commands` id, and the `when` clause would not have saved it.** A binding in
`keymap.rs` is resolved by the key gate's window *capture* listener before any text surface sees the
event, and plain `ctrl+d` is a byte a pty wants — `^D` is EOF. An id would take EOF from every
terminal pane in every window, which is why `gitlog/LogView.tsx` already refuses to register *its*
Ctrl+D. And `editorFocused` is a **pane-kind** flag, so it is false in the diff and merge panes,
which are two of the three surfaces this had to work in. The cost is the same one move-line-up and
move-line-down pay and it is stated rather than discovered later: **no palette row, no menu item.**

The bindings live in `ui/src/editor/editorKeys.ts` as data, because `find.ts` imports a CSS module
and `EditorSurface.tsx` imports React — neither can be `require`d, so bindings inside them can only
ever be regexed. `check-editor.mjs` loads that module, folds both arrays into the composed-keymap
expansion it already runs, resolves the chords **by identity**, and then *drives* `copyLineDown`
against a real `EditorState`: the line lands below the original, the caret rides to the copy, a
multi-line selection duplicates as a block, and a read-only state refuses. That last one is
load-bearing rather than tidy — `DiffPane` and `MergePane` install the array on their read-only
sides too, sharing one array on exactly that guarantee.

**One thing that expansion found on the way.** `claimedOn` built its chord map with a bare `Map.set`,
so a chord claimed twice reported the *last* binding — while CodeMirror's `runHandlers` stops at the
*first*. The two answers are opposites, and they differ in precisely the case the expansion exists
to catch: a chord that is bound and shadowed. First claim wins now. It changed no existing answer,
which is the other half of why it was safe to have been wrong.

**Move line up/down rode along, and gained two surfaces doing it.** Ctrl+Shift+Up/Down (⌘⌥⇧↑/↓ on
macOS) lived inline in `EditorSurface.tsx`, which made move-line true of exactly one of the three
editable surfaces `editorKeys.ts` exists to keep in step: the diff pane's proposed side and the
merge pane's result pane are both places a user edits text, and both had Ctrl+D and no way to move
a line — *duplicate* was the only line command in the pane. Both chords are in `lineEditKeymap` now
and all three surfaces get both. It costs nothing extra to put them there: `moveLine` guards on
`state.readOnly` and returns `false` exactly as `copyLine` does, which is the same guarantee that
already let one array serve `DiffPane`'s read-only side and `MergePane`'s two conflict panes.
`check-editor.mjs` drives it rather than asserting about it — the line ends up one line over with
the caret riding it, a multi-line selection moves as a block, the line count is unchanged, and both
directions refuse a read-only state — and computes the chord's freedom against an **upstream-only**
expansion, because asking the full composed map whether `Mod-Shift-ArrowUp` is claimed now answers
"yes, by us" for ever and would hide the very collision the expansion exists to catch.

**Then the chord the report was actually about.** `Ctrl+Shift+Arrow` is the compensation the app
keymap owes, but the chord a hand reaches for to move a line is **⌥⇧↑/↓** — IDEA's spelling on
every platform — and in `defaultKeymap` that is `copyLineUp`/`copyLineDown`, so it *duplicated* the
line above or below instead. `editorKeys.ts` said in as many words that IDEA's spelling "loses,
because trading one capability for another is not a re-homing". That was true while duplicate-line
had no other chord and stopped being true the moment Ctrl+D landed: duplicate-below is Ctrl+D, and
duplicate-above is re-homed to **Ctrl+Alt+D** — free in both layers and unbound in
`cide_core::keymap`, both computed rather than asserted from the paragraph. So `Shift-Alt-Arrow`
moves a line now, in all three surfaces, shadowing `defaultKeymap` rather than deleting it: the
array is spread ahead of it and CodeMirror stops at the first command that returns `true`.
`check:editor` writes that up the other way round from the re-homed pair — upstream **must** still
claim the chord, or the shadowing is decoration — and `check:keys` now reads *every* literal
binding in the file, not just the two with a `mac:` spelling, and asserts each one unbound in
`cide_core::keymap`. Its regex had required `run: x }`, so the three bindings carrying
`preventDefault` had been skipped in silence.

**And a comment in `MergePane` that was false in the hardest way to notice.** Its Ctrl+Z handler
says *"the centre pane's CodeMirror history owns typing, and it must go on owning it"*, and steps
aside for any Ctrl+Z from inside an editable surface. There was no CodeMirror history: the file
imported nothing from `@codemirror/commands` and installed no `history()`, so the chord was handed
to a `StateField` that did not exist and Ctrl+Z in the result pane did nothing at all. It mattered
more once Ctrl+D put a document-editing command in that pane — shipping an edit into a surface whose
undo is inert is how a hand-merged block gets lost with nothing to reach for. `history()` and
`historyKeymap` are on the editable side now.

## Code folding (M19), and what it costs to have no parse tree

Collapse and expand a block, from the caret or from a chevron in the gutter, in every language
the editor knows plus the ones it does not. IDEA's chords, IDEA's keypad included:

| | |
| --- | --- |
| `Ctrl+-` / `Ctrl+=` | Collapse / Expand the block at the caret |
| `Ctrl+Shift+-` / `Ctrl+Shift+=` | Collapse all / Expand all, **at every level** |
| `Ctrl+Alt+-` / `Ctrl+Alt+=` | Collapse / Expand recursively |
| `Ctrl+.` | Toggle |

All ten bindings are scoped `editorFocused`, and that scope is load-bearing rather than tidy:
the key gate is a window **capture** listener, and xterm encodes 0x1f for a plain `Ctrl+-`, so an
unscoped binding would silently remove that byte from every shell in every window. They are also
spelled `minus`, `equal` and `plus` and never `-` or `+` — `strokeFromEvent` reads
`KeyboardEvent.code` first, so one `ctrl+minus` covers the main row *and* `NumpadSubtract`, while
`Equal` and `NumpadAdd` are two different physical keys and Expand therefore needs two lines.
A binding written `ctrl+shift+-` would parse in Rust and be inert for ever, because no keystroke
ever produces the token `-`.

### There is no parse tree, and this is what that costs

`streamGrammar.ts` has said since M9 that folding was the price of its design:

> What is knowingly given up: anything that needs structure. […] there is no folding or
> indentation beyond the bracket heuristic.

That decision is not reversed here. Every language is still a `StreamLanguage` over a data
table, there is still no `@codemirror/lang-*` in this project, and CodeMirror's usual route —
`foldNodeProp` over a Lezer tree — is still unavailable. So folding comes from a `foldService`
over the *text*, and `ui/src/editor/foldRanges.ts` is the bracket heuristic written out with
enough of a tokenizer to know that the `{` in `println!("{}")` is not a block.

Four sources, one linear pass:

- **Brackets** — `{ [ (`, skipping strings and comments, nested `/* /* */ */` where the language
  nests them, and Rust's `r#"…"#`, which is where an unbalanced brace actually lives.
- **Indentation** — Python's suites and YAML's mappings, as a monotonic stack rather than a
  forward search per line. The forward search is O(n²) on a deeply nested file and is the
  quadratic this directory has already shipped twice; `check:editor` asserts the ratio.
- **Headings and fences** — Markdown, because indentation folding is wrong for prose.
- **Explicit regions** — `// region` / `// endregion`, `#region`, and IDEA's own
  `<editor-fold>`, in every language with a line comment. The one fold a person writes on
  purpose.

**Crossing ranges are dropped.** Nothing makes four independent sources agree, and a `// region`
opened outside a block and closed inside it produces a pair that overlaps without nesting. Both
cannot be offered: a fold is a `Decoration.replace`, and two overlapping replacements leave the
text between them belonging to neither placeholder. The earlier, wider one survives.

The honest limit is the same one the tokenizer has. An apostrophe in a language where `'` opens
a string still ends that line's scan, an unterminated `"` in a language whose strings may span
lines still swallows what follows, and a `{` inside a construct no `FoldSpec` describes is still
a block. What is different from the highlighter is the *direction of the default*: a quote is
assumed **not** to cross a line unless the language opts in, because a mis-coloured tail is
cosmetic and a mis-scanned brace deletes folding for the rest of the file.

### The fold table is a second copy, and a gate makes it one

`FoldSpec` mirrors six fields of `GrammarSpec` and is written out again in `languages.ts` rather
than read off the grammar — because a grammar arrives through a dynamic `import()` and the fold
restore has to run inside the editor's **mount dispatch**, one transaction with the remembered
scroll. Two dispatches would lay the document out at its unfolded heights, scroll to a line, and
*then* collapse several thousand lines above it, landing the user somewhere they have never been
— once per restore, on every file they had folded.

So `grammar()` now hands its input back as `.spec`, and `check:editor` compares every mirrored
field of every language against the grammar it must not disagree with. The failure it prevents is
specific and silent: a `lineComment` reading `#` for Rust makes every `#[derive(…)]` a dead line,
half the braces are never seen, and the outer `impl` quietly stops being foldable.

### Folds persist, as start lines

They ride the per-file view memory that already existed — `ViewPosition` gained a `folds:
Vec<u32>`, so a fold reaches the disk through the same four-stage write ladder a scroll does
(`positions_state.rs` has the ladder). Nothing new crosses the wire and there is no new Tauri
command; folding is entirely client-side.

**Start lines and not offsets**, and that is the whole design. A record is written against one
version of a file and applied to another — the agent rewrote it, a `cargo fmt` shortened it. A
line that no longer names a foldable range is dropped in silence, which is the ordinary case and
not an error; a stale *offset* would collapse a range of text nobody chose. `clampView` therefore
**filters** folds where it **clamps** the caret: landing a caret on the last line of a shortened
file is a disappointment, and collapsing whatever block now sits there is a piece of the document
silently missing, in the file that has just changed under the user.

`remember_position` sorts, dedupes and caps the list at `MAX_FOLDS` (256) in Rust rather than
trusting the webview, for the reason `touched_at` is stamped there: a `ViewPlugin` that appended
instead of replacing would otherwise grow one entry of `positions.json` without bound. The sort
is not only hygiene — `sameView` compares the lists element-wise, so an unsorted list read back
from disk would look different from the identical one the editor holds and note itself again on
every mount.

### What is not done

- **Nothing here has been confirmed on screen.** The standing reason plus a new one: the author
  of this change was running inside the user's own installed AppImage, and `run.sh` stops any
  instance already running. What *is* covered is more than usual — `check:editor` compiles
  `folding.ts` in a second pass and drives all seven commands against a real `EditorState` with
  the real extension installed, including the persistence round trip and a fold surviving an edit
  above it. What no check in this repo can speak to is the picture: the chevrons, the hover
  reveal, the placeholder, and whether the scroll lands right after a restore.
- **There is no "fold selection".** IDEA's `Ctrl+.` is Fold Selection and creates an ad-hoc
  region; here it is Toggle, and an ad-hoc fold would be a second kind of record in
  `positions.json` — a range, not a line — with the offset-rot problem the line-based design
  exists to avoid.
- **`.cm-gutters` reserves the fold column unconditionally**, including in a `.txt`, which is why
  a file with no known language folds by indentation instead of not folding at all. The
  alternative was a `:has()` reservation like the blame column's, and it moves every line number
  sideways when you switch from a `.md` tab to a `.rs` one.
- **Folds are per document, not per pane.** A split showing one file twice restores the same
  folds into both, and collapsing in one does not collapse in the other until the next mount.
  Same shape as the remembered scroll position, and the same argument: the record is keyed on
  the file.

## Sending a selection to a named conversation (M19), and what is not done

Right-click in a code pane and *Send lines 12–20 to Claude* is no longer a button. It is a row
with a `›` on it, and hovering it opens the project's Claude conversations:

```
Send lines 12–20 to Claude      ›   │ 1: agents
                                    │ 2: git-details
                                    │ 3: claude-code-ide-rust
                                    │ 4: cide : claude
```

The number is the **position** of the pane, counted the way it is drawn — tabs left to right,
and inside each tab the split tree in reading order, detached panes last. It renumbers when a
pane closes, which is why it is only ever shown: the row carries the `PaneId`, and the number
carries nothing.

The name is the CLI's, not cide's. `/rename` in Claude Code writes it into
`~/.claude/sessions/<pid>.json` beside the `sessionId`, `cide-claude`'s `roster` module reads
that directory, and `claude_session_names` puts it on the wire. A conversation nobody has named
falls back to the pane's own title — `4:` above — which in practice makes the list say which
panes have a live `claude` behind them, because a record only exists while one is running.

Picking a row **@-mentions the range into that conversation and takes you there**: the tab is
activated, the pane focused, the terminal scrolled to the bottom and given the keyboard. Nothing
is submitted. The mention is the first half of a question and typing the second half is the next
act, which is the whole reason focus moves at all.

### What is not done, and what it cost

- **Nothing here has been confirmed on screen**, for the standing reason — KDE will not raise a
  shell-launched window. The submenu's *decisions* are covered (`check:menus` resolves parents,
  refuses to let one both open a list and run, and drives `placeSubmenu`'s flip; `check:editor`
  drives the ordering, the numbering and every way a name can be absent). What no check in this
  repo can speak to is whether the box lands beside the row, because there is no DOM in the
  harness.
- **The automatic destination left the mouse route.** A parent row cannot also be a button — one
  click, two meanings, and one of them unreachable — so *whichever Claude can receive*, which is
  what the row used to do, is now `Alt-Enter` in the buffer and the palette's
  `claude.mention.file`, and nothing else. Picking a row is an **exact** send: it addresses that
  pane and refuses if its `claude` is not on cide's IDE server, rather than quietly delivering
  the lines to a different conversation.
- **The names are fetched, not pushed, and can be one gesture stale.** There is no event: a
  `/rename` is typed into a CLI that tells cide nothing. The refresh happens as the parent menu
  is built and the submenu is built later, when the row is hovered, so an ordinary open has the
  answer in hand — but a submenu hovered in the same few milliseconds shows the previous one.
  Only the labels are affected; the row still sends where it says it does.
- **`~/.claude/sessions/` is an internal Claude Code detail**, like the transcript path
  `lifecycle::transcript_exists` depends on, and is treated the same way: read-only, never
  written or swept, `CLAUDE_CONFIG_DIR` honoured, and every way of being wrong answers *no name*.
  A CLI release that moves or renames that directory costs the submenu its labels and costs the
  feature nothing.

## The `claude` behind a wrapper: attributing a connection to a pane (M29)

*Send lines to Claude* refused, from a Mac, with one Claude pane open and a live conversation in
it:

> cide has no Claude bound to any pane in this project. One session is connected to the IDE
> server, but nothing identifies which pane it belongs to — run /ide in the pane you meant to
> send to.

`/ide` did not help, and the same cide on Linux could not reproduce it. Neither fact is what it
looks like.

**The join was pid equality.** `pane_bind_session` records the pid of the process cide forked
into the pty; the CLI announces itself over the IDE socket with `ide_connected {pid:
process.pid}` — that is literally what it sends, `async function
HUe(e){await e.notification({method:"ide_connected",params:{pid:process.pid}})}` — and
`cide_ide_mcp`'s `pane_of_pid` matches the two. A *broadcast* (`selection_changed`) goes to every
connection; an **addressed** notification (`at_mentioned`, which is what this gesture is) goes
only to a connection the map claims. So the whole feature rests on the process cide forked being
the process that opened the socket.

On the reporting machine `claude` is a **wrapper** — `some/path/bbin agent claude` — that spawns
the real CLI rather than `exec`ing it. cide holds the wrapper's pid, the CLI announces its own,
and the join finds nothing. On the Linux box the same user's `~/.local/bin/claude` is a symlink
straight to the executable: one `execve`, one process, pids equal, feature works. Nothing in this
path is `cfg`-gated and the platform was never the variable — the *shape of the install* was.
The same failure has been in the code for shell panes since it was written, and
`cide_ide_mcp::server` says so in its own notes: a `claude` typed into a cide shell is a
grandchild of `$SHELL` and received no addressed notification, ever.

**The fix is an ancestry walk.** `cide_core::proc::owning_ancestor` climbs from the announced pid
and stops at the first pid this project has a pane for; `ide::resolve_unbound_connections` asks
the server which connections nothing claims (`unbound_pids`) and binds what it finds. Nearest
owned ancestor wins, which is what makes a `claude` inside a shell pane belong to the shell pane
and not to whatever pane is above it. A chain with **no** owned ancestor is left unattributed
rather than guessed at, so somebody's `claude` in a Terminal window that found this project's
lockfile is exactly as unaddressable as it was before.

It runs twice, and the second time is not belt-and-braces: on `Connected`, and again in
`claude_send_lines` before it concludes a send has nowhere to go. The pane binding is a workspace
mutation the *frontend* makes after a spawn resolves, so a connection that beats it would find
nothing to match against — a feature whose correctness depends on the order of two independent
round trips is a feature that works on the machine it was written on.

### What is verified, and what is not

- **Every rule that can be got wrong is pure and runs on any host** (`cargo test -p cide-core
  proc`): nearest owned ancestor wins, the hop cap, stopping at `init`, refusing a cycle, and an
  unowned chain answering nothing. The walk takes its parent lookup as a closure precisely so
  that those are testable without a process tree.
- **The routing half is driven over a real socket** (`cargo test -p cide-ide-mcp`): a connection
  announcing a pid nothing bound is reported by `unbound_pids`, is not addressable, and becomes
  addressable when bound — which is the whole of the reported bug and its fix, from the server's
  side.
- **The Linux `parent_of_pid` is checked against `getppid(2)`**, and the derived-binding
  bookkeeping — reaping the wrapper must forget the CLI it spawned, because `report_exit` has
  never heard of that pid — has its own test in `cide-app`.
- **The macOS arm has never run.** It is `proc_pidinfo(PROC_PIDT_SHORTBSDINFO)`, and the only
  thing said about it from here is that it compiles, via the Darwin type-check in
  *Type-checking for macOS from Linux* below. That check earned its place immediately: the first
  version used `sysctl(KERN_PROC_PID)` and `libc` defines no `kinfo_proc` for Apple at all, so it
  was a hard error on a platform no machine here can build.
- **A `claude` in a shell pane is now *attributed*, and is still not a mention *target*.** The
  ancestry walk answers "which pane is this process running in", which is what diff credit and
  the routing table want. Which panes an `@`-mention may be *sent* to is a different decision,
  `cmd::file::mention_candidates` makes it, and it offers Claude panes only — unchanged here on
  purpose, because widening it is a feature and not a bug fix.
- **Nothing has been observed end to end on the reporting machine.** What should be visible there
  is a log line — *attributed a claude to a pane through its ancestry* — naming the pane, the
  announced pid and the pid it was derived from; and where it still fails, the warning that
  replaces it prints the chain it walked and the children the project has, which is the sentence
  a report of this error was missing.

## Typed links between tasks: related, blocked-by, subtask-of (M30), and what is not verified

Tasks referenced each other only in prose — a `t-14` quoted in a comment, which is what short
ids exist for and which nothing could act on. M30 makes the edge a field: `Task::links`, three
kinds, each stored **once on its canonical side** (`blockedBy` on the blocked task, `subtaskOf`
on the child, `related` on whichever end made the gesture) with the other reading derived from
the board at render. `cide-ipc/src/tasks.rs`'s module header used to cut `blocked_by` and
parent/child by name; the header now carries the reversal, and the argument is one sentence:
**`blockedBy` is not advisory** — auto-dispatch skips a task whose blockers are not done
(`autodispatch::trigger`'s new early return, table-tested), and an explicit dispatch of one is
refused in `plan_dispatch`, the single funnel the panel's button, `cide_agent_dispatch` and
auto-dispatch all pass through, with the blockers named in the sentence. `Done` alone
satisfies; `Review` still blocks, because review work can bounce back to `Doing`.

The pieces, each with its gate:

- **The wire**: `LinkType`, `TaskLink` (tombstone + stamp), `TaskLinkSpec`, `TaskEdit::{Link,
  Unlink}`, `TaskNew::links` — additive, `#[serde(default)]`, no schema bump; a pre-M30 file
  parses byte-identically (`cargo test -p cide-ipc`).
- **The store**: per-edge merge (`union_links` — newer stamp wins per `(link, target)` key, so
  an unlink survives a merge whose *scalars* the stale side wins, and a re-link reverses an
  unlink because the tombstone is a toggle, not the comments' one-way flag); self-links and
  duplicates refused in `validate`; **dangling targets and cycles deliberately legal in the
  file** and refused only as gestures — `validate`'s doc carries both arguments, and
  `a_merged_or_hand_edited_cycle_does_not_freeze_the_tracker` is the invariant as a test.
  `repair` re-points link targets through its re-mint map, which its old doc said was
  impossible for references — true of prose, false of fields, and rewritten to say so.
- **The MCP surface**: `cide_task_link`/`cide_task_unlink`, in `tool::ALL` — a run decomposing
  work records edges; the author gate and the dispatch preflight are the boundary, not the
  vocabulary split. `cide_task_list` carries the blocking pair both ways (`blocked by t-3` /
  `blocks t-7`), `cide_task_get` a full `links:` block with targets resolved and dangling
  marked `(deleted)`; a task without links renders byte-identically to M29
  (`cargo test -p cide-agents`).
- **The panel**: a Links *section* on the card — chips that navigate, ✕ on the edges the card
  may write, and a picker whose target is a **search input**: id-prefix first, then title
  word-prefix, then substrings (`model.ts::linkTargetOptions`, the mention popup's tier idea),
  with picking a row as the commit — and the same search in the compose dialog's links row,
  because a create
  naming an assignee dispatches immediately and a `blockedBy` attached one call later is a gate
  the trigger never saw. `TASK_FIELDS` stands at four with the argument written beside it;
  `check:agents` pins `LINK_KINDS` to `pub enum LinkType` and drives the derivation;
  `check:agents-render`'s new stories assert the chips, the derived direction, the gone mark,
  and that every pre-M30 digest is unchanged.

What is **not** verified:

- **Nothing has been driven through the GUI.** The whole loop — link `blockedBy` on the card,
  watch the assignment record intent instead of dispatching, see the refusal sentence from an
  explicit dispatch, mark the blocker done, re-assign and watch the run start — has run only as
  unit tests either side of the seam. `./run.sh` and two tasks reproduce it in a minute.
- **Two writers toggling one edge inside clock skew** resolve by wall clock (`union_links`'s
  stated cost); the merge tests pin the rule, not the skew.
- The `#[ignore]`d spawn tests have not been re-run — links change no spawn path, but the
  preamble a dispatched run reads now includes link lines nothing live has read yet.

## The orchestrator can define a role: `cide_agent_create`/`cide_agent_update` (M33), and what is not verified

The product owner could hand work to a role and could not make one. The paragraph
`cmd::session` puts in its system prompt said to write `.cide/agents/<name>.md` with ordinary
file tools, which is true and is how it was done — and it is a grammar nobody can see from
outside: a definition that spells `max_concurrent` with an underscore, or names a
`permission-mode` from another CLI's vocabulary, loads, is greyed by `read_definition`, and is
never dispatched. The model finds out from a dispatch that refuses, or does not find out.

Two tools, in `tool::ORCHESTRATION` — so the product owner's session has them and a dispatched
run never sees them, which is the same structural split `agent_rpc` already draws for dispatch.
Both go through `cide_agents::defs::validate` and `defs::save`, the writer the Agents panel's
form uses, so `AgentDraftProblem`'s field lands in the sentence: *`systemPrompt`: a role needs a
system prompt…*, and **nothing is written**.

- **`cide_agent_create`** takes a name, a description and a system prompt, plus the eight
  optional keys, and writes `.cide/agents/<name>.md`. `original: None` is what makes it a
  create rather than a rename, so a name a file already holds is refused rather than
  overwritten — the file it would replace is somebody's system prompt.
- **`cide_agent_update`** is a **patch**: `AgentSink::definition` reads the draft, `Field<T>`
  folds each argument onto it (absent keeps, `null` clears, a value replaces), and everything
  the caller did not name — the prompt above all, and a subagent's unmodelled `hooks:` block —
  comes through untouched. A serde struct of `Option<T>` would have collapsed *absent* and
  *null* and silently cleared every field a caller did not mention.

Four narrowings are decisions rather than gaps, each with the argument in `tools.rs`'s header or
beside the refusal: **a create is always the project scope** (a global role is the user's *other*
projects, changed by a model, in a directory no review looks at — `cide_agent_update` does take a
scope, because editing what exists is bounded by what is there); **no rename** (the name is what
every assignee, every `cide/<role>-<task>` branch and every worktree is keyed on, and `name` on
an update is refused rather than ignored, because a dropped one is a rename the caller believes
happened); **no `cide_agent_delete`**, for `cide_task_delete`'s reason one turn further; and
**`harness` is refused on a Claude Code subagent**, the one modelled key that is not in that
dialect (`defs::canonical_key`'s single `!claude` arm), because written there it is a line
nothing reads, in a file cide does not own, preserved for ever as an extra.

`cide_agents_list` now prints the **scope** beside every role rather than marking only the two
Claude ones, because that word is `cide_agent_update`'s `scope` argument and a roster that named
half of them would leave a model guessing at the other half. An update with no scope resolves to
the definition *in effect* — the roster's row, so a project file shadowing a global one is the one
that gets edited — and every answer names the path it wrote.

What is **not** verified:

- **No MCP client has called either of them.** Every refusal, every patch and the whole
  create → load → update road run as unit tests, the last against a real directory
  (`a_role_written_by_a_tool_is_a_role_the_loader_runs`, which asserts through
  `defs::load` rather than through the writer). What has not run is the socket: a `claude`
  calling `mcp__cide__cide_agent_create` and then dispatching what it just defined.
- **`RegistrySink::write_definition` goes through `cmd::agents::agents_save`** — for the
  emission, since `$XDG_CONFIG_HOME/cide/agents/` is watched by nothing and a global role would
  otherwise never appear in an open window. It is `dispatch`'s and `stop`'s shape (`block_on`
  into an async command that awaits `blocking`), which is proven in production, but this
  particular call has not been run live.
- **The system prompt is written blind.** No tool in the vocabulary shows a caller the prompt a
  role currently has — `AgentDef` carries it and `cide_agents_list` deliberately does not print
  it — so `systemPrompt` on an update replaces bytes the model may never have read. The
  description says so, the answer echoes the line count and the opening line back, and the
  honest fix is a read the vocabulary does not have.
- Nothing has been driven through the **GUI**: a role written by the orchestrator appearing in
  the Agents panel of a second window is the emission path above, unobserved.

## The web languages, and the rung that finds an npm server (M34), and what is not verified

Before this, a `.html`, `.css`, `.vue` or `.svelte` file opened as Plain Text, and builtin
JavaScript/TypeScript was syntax-only — Ctrl+Alt+F answered *No formatter for TypeScript*,
because no server claimed the `typescript` id. The fix is one marketplace extension plus two
cide-side repairs the extension exposed.

**The `web` extension** (`../cide-marketplace/extensions/web`) is the marketplace's first purely
declarative one: no `main.js`, `process:spawn` only. Seven contributed languages — html, css,
scss, less, vue, svelte, xml — and five servers, two of which bind **builtin** ids:
`typescript-language-server` to `typescript` and `vscode-json-language-server` to `json`. That
asymmetry is the design: the builtin TS and JSON grammars are hook-based and better than
anything the rule form can express, so the extension deliberately re-declares neither, and the
richer-when-installed experience comes entirely through the LSP road `cmd::format` already
resolves (filter → server that claims the language → builtin). Markdown fences follow for free —
`fenceTokens` picks a grammar by extension, so ```` ```html ```` colours in the preview the
moment the extension claims `.html`. Grammar warts are documented in the extension's README
rather than hidden: a stateless lexer paints `a:hover`'s `a` property-coloured, `<script>`
bodies get the host language's rules (cide has no nested grammars), and `.sass` is unclaimed on
purpose.

**The hint rung** (`cide_lsp::discover`). Every one of those servers is an npm-installed
`#!/usr/bin/env node` script, and the discovery ladder searched `PATH` plus `~/.cargo/bin` and
`~/go/bin` — so a desktop-launched cide could not find them, and the marketplace README's claim
that `extraPathHints` dirs are "appended to the child's PATH" was simply false. cide had already
solved this exact problem once, for `openspec`: the Node-directory enumeration moved from
`cide-spec` into **`cide_core::node_dirs`** (same extraction, same reason as `which`'s), and
`locate` now probes the manifest's `extraPathHints` (~-expanded) plus those directories after
`PATH` misses. A hit rides the ladder as `Provenance::HintDir` — configured like `SystemPath`,
which the config gate's new negative assertion pins — and carries `child_path_dirs`: the
directory the binary was found in, plus the one `node` lives in when `PATH` reaches none,
handed to `child_env::prepare_command_with` at the spawn. Without that second half the failure
is the documented one: `execve` succeeds and the shebang dies with `env: node: No such file or
directory` from a process cide never mentions. Precedence is pinned by ladder tests: the
override still wins alone, `PATH` beats a hint, a hint that yields nothing **never refuses**
(a hint is a manifest author's guess about somebody else's machine; the override is a
developer's instruction about their own), and `ServerBinaryChoice::System` does not skip the
rung.

**The didOpen label** (`Server::language_id_for`). `notify_document` sent every document the
server's *first* `languageId`, which no server with one language could ever notice —
`vscode-css-language-server` declares `["css", "scss", "less"]` and picks its dialect parser
from that label, so an SCSS document labelled `css` would earn an error on every nested rule.
The document's own resolved language now labels it when the server owns that language; the
first-declared id stays the fallback for a caller that could not resolve the document.

Two smaller repairs in the same change: the manifest's *"claims a languageId this extension
does not contribute"* warning now counts the builtins as known (it fired on exactly the two
bindings that are the extension's point), and `cide-ext`'s marketplace test grew the positive
assertion that a `main`-less extension installs and contributes — the shape the format always
promised was first-class and nothing exercised.

What is **not** verified:

- **The whole feature on a display.** `cargo test -p cide-ext --test marketplace` runs the real
  connect → install → resolve road against the sibling checkout, and a real
  `typescript-language-server` reaches Ready under
  `cargo test -p cide-lsp --test real_npm_server -- --ignored` — but no window has drawn a
  `.vue` file, no Ctrl+Alt+F has formatted a `.ts` buffer through the newly bound server, and
  the Extensions panel has not been watched installing `web`.
- **The desktop-launch case itself.** The rung exists for a `PATH` with no node on it, and that
  is precisely what a test process cannot arrange (`set_var` is unsafe; the parent's `PATH` is
  what `locate` reads). The manual check is `PATH=/usr/bin:/bin ./run.sh` with an nvm-only
  node, then a `tsconfig.json` project reaching Ready.
- **The css server on three dialects live.** `language_id_for` is unit- and registry-tested;
  no real `vscode-css-language-server` has been sent an `scss` document and had its
  diagnostics inspected. Multi-languageId routing is exercised nowhere else in the
  marketplace.
- **`vscode-json-language-server`'s usefulness without `provideFormatter`.** It may format
  nothing under cide's empty initialization options; diagnostics and completion are the case
  for keeping it, and dropping it is a one-line manifest edit if live use shows it inert.
- **Vue is syntax-only by decision, not verification**: `vue-language-server` hard-requires
  `initializationOptions.typescript.tsdk`, and cide sends none to servers it did not ship. An
  `initializationOptions` field on `LanguageServerDef` is the follow-up that would change the
  decision.

**Corrected the same day, by the first live test.** Go-to-definition in a plain HTML+JS folder —
`index.html`, `app.js`, `styles.css` — did nothing, and the reason was the marker gate working
exactly as written: `typescript-language-server`'s `projectMarkers` named `tsconfig.json`,
`jsconfig.json` and `package.json`, and the first project anybody tests a web extension on has
none of them and never will. An empty marker list was the wrong repair — it would start a Node
process in every Rust workspace, which is the cost the gate exists to refuse — so
`toolchain::find_markers` learned a second marker form: `*.ext` matches any file with that
extension (depth-2 walk unchanged, `node_modules/` still skipped, `*.` matches nothing rather
than everything), and web 0.1.1 adds `*.js`/`*.ts` and friends to the TS server's markers. The
server now starts exactly where there is a file for it to read.

The second live failure was not cide's at all, and nothing anywhere named it: with the server
running, Go to Declaration answered *No declaration found* for everything. `npm install -g
typescript` now installs **TypeScript 7** — the native Go port, which ships no `tsserver.js` —
and `typescript-language-server` (6.0.0 included) can only drive the JS tsserver, so it falls
back to a stub reporting `$/typescriptVersion 1.0.0 (bundled)` and answers `null` to every
request. No error reaches any surface; the driver script that pinned this is the difference
between "cide's goto is broken" and "the toolchain under it is". Three things now name it:
web 0.1.2's installHint says `typescript@5`, the extension README carries the why, and —
because a hint is only read before installing — **`Session` now reads `$/typescriptVersion`**:
a major below 2 is the stub, and the server's Problems-panel row becomes the remedy sentence
instead of a `Ready` that is a lie, sticky against later progress
(`a_stub_typescript_version_turns_the_row_into_the_remedy_and_stays_there`). One server's
vocabulary hardcoded in the client, deliberately: the alternative was every future user of the
most popular language server on earth debugging a silent nothing. Two smaller facts from the
same session: typescript-language-server *corrects* a `.js` document labelled `typescript` to
`javascript` on its own (logged, harmless — cide's one `typescript` id spanning four dialects
costs nothing here), and with TypeScript 5.9 pinned the loose-file definition resolves
first try.

## Four more reports from a day's use: the picker, the PATH, the tab, the paste (M35), and what is not verified

Four things reported together and sharing nothing but the session. Two were controls that were
never wired up, one was a default that is only wrong off Linux, and one was a feature this
codebase had written down a reason not to build.

**A folder picker with no way to make a folder, and GTK is why.** The report was that the Open
project dialog does not let you create one. The first assumption — that cide had suppressed
something — was wrong in both directions: `create-folders` is `TRUE` by default and cide never
touches it, *and* turning it on would change nothing. `GtkFileChooserWidget`'s template does hold
a `browse_new_folder_button`; it is `visible = FALSE` after map for a `SELECT_FOLDER` chooser, and
the `browse_header_revealer` that contains it is never revealed. GTK3 shows that button for `SAVE`
and `CREATE_FOLDER` and for nothing else. Measured on the machine rather than read: a PyGObject
probe built the same dialog both ways round (`use-header-bar` true and false), mapped it, walked
the widget tree and printed the button's visibility and every ancestor's. Both `False`.

So the control has to come from cide, and it does — as an **extra widget**, a name field and a
*Create Folder* button under the file list, gated on `spec.folders` so the theme-import picker is
unchanged. `FileChooserNative` forwards `set_extra_widget` to its fallback dialog, which is the
other thing that was measured rather than assumed (the button came back parented, visible and
mapped). A row rather than a name prompt, and that is not a style preference: `FileChooserNative`
is a `GtkNativeDialog` and not a `GtkWindow`, so nothing can be made transient *for the picker* —
a prompt could only be parented to the app window underneath it, which on Wayland is a sibling
free to stack below. The entry is inside the chooser, so the question does not arise. Enter in the
field does what the button does, because a text field beside a button that ignores Enter is a
field people press Enter in and conclude is broken, and a failure (`EEXIST`, above all) shows as
an icon and a tooltip on the entry itself.

macOS needed none of it: `rfd`'s `build_pick_folders` sets `NSOpenPanel.canCreateDirectories` to
true by default, so the other `cfg` arm has had the button all along. One case where this silently
does nothing is named in the code: a launch with `GTK_USE_PORTAL=1`, or inside a Flatpak, sends
`show()` to the portal and the extra widget is not forwarded — survivable, because the portal
dialogs carry their own *New Folder*.

**Two PATH bugs wearing one report.** *"macos bash and claude problem — it doesn't see all PATH,
for example it doesnt see nvm/npm, claude doesn't see my local `~/bin` that is added to PATH."*
Two causes, and `docs/platforms.md` had both written down as known gaps since M17.

The first is `ui/src/panes/TerminalPane.tsx`, which named `/bin/bash` as the program for every
shell pane with a comment saying Settings would take it over "in M11". On macOS `/bin/bash` is
3.2.57 — the 2007 GPLv2 release Apple still ships — and the login shell since Catalina is
`/bin/zsh`, so `bash -l` reads `~/.bash_profile` and the user's entire `~/.zshrc` never runs,
including the `nvm.sh` source line and the `brew shellenv` eval that are *the* documented ways
those two tools get onto `PATH`. The pane now asks for **the login shell** rather than for a
program: it sends an empty `program`, and `session_spawn` substitutes `cide_core::shell` —
`$SHELL` if it is executable, then `getpwuid_r`, then `/bin/zsh` on macOS and `/bin/bash`
elsewhere, then `/bin/sh` — with `-l`. `cide-headless` reads the same function, so the three
near-misses this workspace held are one ladder. Deliberately **not** a setting: the OS knows the
answer, and a text field pre-filled with the right value has exactly two reachable states, the
right answer and a typo.

The second is deeper and is the one `claude` hits. Every cide child gets `process PATH +
toolchain::extra_dirs()`, and for a Finder or Dock launch the process `PATH` is launchd's
`/usr/bin:/bin:/usr/sbin:/sbin`. `extra_dirs` answers that with directories cide can name in
advance, and no list can name a directory that exists only inside a sentence in somebody's
`~/.zshrc` — which is exactly what `export PATH="$HOME/bin:$PATH"` and an nvm version directory
are. `platforms.md` had already named the fix and the two objections to it, and this milestone
takes it because the report is precisely the trigger that document specified.

`cide_core::login_path` runs the login shell once per process, on its own thread, behind a
four-second deadline, and appends what it reports. Each objection is answered by a piece of its
shape rather than by an assurance. **`toolchain` still spawns nothing** — the probe is its own
module, `extra_dirs` stays pure and static, and a new `toolchain::discovered_dirs` composes the
two; `search_paths` and `child_env::child_path` both read *that*, so find-it and give-it remain
one list and `the_path_a_child_searches_is_the_path_which_searched` now asserts over it. **The
hang is bounded twice** — the child's deadline, and a three-second wait for any caller that
arrives before the answer, which on timeout answers empty rather than waiting again, so a wedged
rc costs one pause and not one per spawn. **It is not silent** — one `info!` per launch naming the
shell, the elapsed time and every directory added, because an empty answer and a perfect one look
identical from a pane.

Two details are load-bearing and neither is obvious. The probe cannot go through `run_filter`:
that applies `prepare_command`, which composes `discovered_dirs`, which waits on the probe that is
running — so `child_env` grew `run_filter_bare`, the one spawn in the workspace that keeps the
scrub and skips the `PATH` pass, and the honest reason is that this child's whole purpose is to
disagree with cide about `PATH`. And the shell's output is read only from **between two markers**
the probe prints, because an interactive rc prints things: an nvm banner, a `command not found`
warning, a `motd`. Without the markers a banner becomes a directory name, silently, for ever
after. The negative case is in the tests, along with `set -x` echoing the command line — which is
why the *last* marker wins.

Enabled on Linux as well as macOS, and that is a testing decision as much as a correctness one: a
desktop-launched AppImage inherits a narrow `PATH` for the same reason a Finder-launched `.app`
does, and Linux is the only platform where the path can be exercised. Measured here with
`PATH=/usr/bin:/bin`: 430 ms, recovering nvm, pyenv, linuxbrew, sdkman and `~/go/bin`. `~/bin` was
also added to the static macOS list, because it is the directory the report names and it should
not depend on a probe that is allowed to fail.

**Double-clicking a project tab did nothing, and the reason is one attribute.** `WindowFrame`'s
delegated `dblclick` listener bails unless `target.closest('[data-window-drag="true"]')` finds
something, and the only element in the header carrying that attribute is the empty `.filler` —
a *sibling* of the tab strip, deliberately, because putting it on the header would turn every tab
and button into a drag handle. So the gesture worked on the empty space beside the tabs and not on
the tabs. An explicit `onDoubleClick` on the tab row now calls the same exported `toggleMaximize`.
Explicit rather than marking the row as a drag region: the delegated path would in fact have
reached the same function, but only because `tauriOwnsDoubleClick` answers `false` when a
`<button>` sits between the target and the region — a rule that exists to mirror Tauri's injected
`drag.js` and should not quietly become load-bearing for this. The close button is excluded, since
its first click closes the project and the second lands on whatever reflowed into that spot.

**Pasting a screenshot, in the tree and in a buffer.** *"if i do PASTE action in file tree and in
buffer image — we should be able to create image file in place where we pasting it."* There was no
route in the workspace that writes **binary** bytes to a file — `file_write`, `fs_write_file` and
`ops::write` all take a `String` — and no paste handler on the editor surface at all.

The constraint that shaped it is `crates/cide-ipc/src/image.rs`'s opening rule: pixels never cross
the IPC. A clipboard holds a *bitmap*, not a PNG — 33 MB of RGBA for a 4K screenshot — so
encoding it in the webview means marshalling that through Tauri's JSON IPC twice. Instead
`fs_paste_image` reads, encodes and writes on one blocking Rust thread and answers a path; the
frontend names a directory and receives a string. Nothing about the image is ever in JavaScript.

That also disposes of all three objections `ui/src/terminal/clipboard.ts` records against
`read_image`, and the distinction is worth stating because that note reads as a blanket refusal.
It is about the **plugin command**, the webview-facing one: it needs a capability this app does
not grant, and it answers with a `ResourceId` into the window's resource table that requires
`core:resources:allow-close` to free, so every call would leak a full bitmap for the life of the
window. `ClipboardExt::read_image` is the *host* API behind it — capabilities gate `invoke`, not
Rust — and hands back an owned value that drops at the end of the function. The Ctrl+V decision
that module documents for a Claude pane is untouched and is not on this road.

The buffer half needed one more decision: whether a paste is an image without decoding one. The
answer is that the question is free — a `paste` event carries `clipboardData.types`, a list the
browser has already assembled — so the DOM event is the detector and Rust is the reader. It also
has to be that way round for a reason `terminal/clipboard.ts` met from the other side: the handler
is synchronous, `preventDefault` cannot be deferred, and reading a clipboard cannot be done
without awaiting.

Four rules live in `editor/pasteImage.ts`, import-free and driven by `check:paste-image`, because
every one of them fails invisibly. A clipboard holding an image **and text** is text — the same
trade the terminal makes, and what keeps a copied file path pasting as a path. `relativeTo` takes
the **document** path, not its directory, and an off-by-one `../` renders as a broken-image glyph
with the file present on disk and nothing logged, which reads as a preview bug. A generated name
always contains spaces, so a markdown destination is `<`-wrapped — CommonMark otherwise parses
`![](Pasted image ….png)` as a link to `Pasted` with prose after it. And markdown gets `![]()`
while everything else gets the bare relative path, with the alt text left **empty**: a screen
reader reading out "Pasted image 2026-08-31 at 22.41.07" is worse than none.

In the tree, Ctrl+V with an empty in-app file clipboard used to mean nothing at all; it now means
this. `FsError::NoClipboardImage` is its own variant precisely because of that: the command is
reached every time somebody copies text elsewhere and presses Ctrl+V here, several times a
session, and that is not a failure. The bare chord swallows it; the context menu's *Paste Image*,
which the user pointed at, shows it. That item is never greyed, and the reason is the one above —
predicting whether the clipboard holds an image costs a full decode, so the refusal is cheap to
report and expensive to foresee. The name is invented from the clock rather than typed, so a
collision is cide's problem and not the user's: `ops::write_new_bytes` walks
`copy::candidate_name` with `create_new(true)`, which means a pasted image collides the way a
pasted *file* does, and there is no dialog because there is nothing to ask.

**What is not verified.** None of the four was seen on a Mac, and two of them are macOS fixes:
the shell ladder's rungs are unit-tested over injected inputs and its platform default is a
`cfg!`, so what a real `.app` gets from `getpwuid_r` and from launchd's `SHELL` is reasoned about
and not observed, and the `~/bin` and `path_helper` half of `extra_dirs` is still a list built
from installer documentation. The GTK dialog was probed on this machine but the finished picker
has not been driven by hand on a display, nor has the tab double-click, nor either paste — those
four are the ones to try first. The PATH probe *was* run end to end here, against a deliberately
narrowed `PATH`, and the numbers above are measured.

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

## Colour schemes, and importing a VS Code theme (M24)

The report was that highlighting reads flat, JSON worst of all. The diagnosis was not the
grammars.

`ui/src/editor/highlight.ts` was the only table saying what colour a token is: twelve roles over
ten colour tokens, and those tokens were the **chrome palette's** — the same six hues that
`ui/src/settings/theme.ts` maps to the terminal's ANSI slots, which `tokens.css` said outright
beside `--blue`: *"Read by the editor's highlight table AND by the terminal's ANSI palette."* So
there was no syntax palette to tune. Retuning a syntax colour repainted every terminal in the
app, and `check-theme.mjs` holds those six to a 3:1 floor and a grey-ramp ordering that are a
*terminal's* requirements and have nothing to do with a buffer.

**In a JSON file that resolved to five colours, and the important one was thrown away.** The key
is `propertyName` — `languages/data.ts` does real positional work to find it, matching a quoted
string with a colon after it — and the role table painted it `--text`. `.cm-editor` is also
`color: var(--text)`. A JSON key was pixel-identical to unhighlighted prose, in a file where the
key is the only structure there is. `true`/`false`/`null` were the second: `atom` is a subtag of
`keyword`, so a config file's booleans were the same purple as a Rust `fn`.

Three changes, in that order.

**The editor's colours are their own family.** Twenty-nine `--tk-*` custom properties, declared
in both palette blocks of `tokens.css` and read by `editor/highlight.css`. Nothing else changed
to make that work: `cideHighlightStyle` is built with `class:` rather than `color:` precisely so
a class resolves the property at paint time, and the minimap — which paints into a canvas and
cannot read a class — already resolved the same properties through `getComputedStyle`. Most of
them still hold the value the chrome token held, restated as literals rather than written
`var(--purple)`, on `--term-*`'s own argument: the equality is a coincidence of today's design.

**The role table is twenty-four entries instead of twelve.** `constant`, `doc`, `escape`,
`regexp`, `namespace`, `macro`, `label`, `bracket`, `punctuation`, `strong`, `link` and
`variable` were split out, and `property` was given a colour of its own. Several of the new ones
hold the value they used to share, and are split anyway — a role is the unit an *imported* scheme
can speak about, so a role that does not exist is a colour a theme has no way to give us. Order
in that table is precedence, because `HighlightStyle` resolves a tag through its parent chain and
takes the first match: `constant` has to precede `keyword` *and* `number`, `doc` has to precede
`comment` *and* `string`, `attribute` has to precede `property`, `bracket` has to precede
`punctuation`. Every such pair carries a comment where it appears, and `check:editor` asserts
each one, because the failure is silent — the wrong role wins and nothing anywhere fails.

Bare `variableName` is still not a role. That is deliberate and pinned: an ordinary identifier is
body text, and colouring every one of them is how a syntax theme turns into noise.

**A scheme is a setting, and everything except cide's own is imported.** Settings › Appearance
grows one row under Theme: a picker of `cide` plus whatever the user has imported, and an
*Import…* button.

**What that button takes is a `.vsix`.** It did not at first, and the report was the shape of the
mistake: *"I've downloaded a `.vsix` file — but I cannot import it. What should I try to
import?"* The answer was `unzip -j theme.vsix "extension/themes/*"`, which is not an answer. A
`.vsix` is what a marketplace hands you; the theme JSON is a file inside it, and a picker that
refuses the download with no explanation is the worst version of that.

So `import` dispatches on the file's **first four bytes** rather than its extension — `PK\x03\x04`
is a ZIP whatever it is called, and a theme is JSON whatever *it* is called, which also covers a
`.vsix` a browser renamed to `.zip` and a `-color-theme.json` a download manager left with no
extension at all. For an archive it reads `extension/package.json`, follows
`contributes.themes[].path`, and converts **every** theme it finds, falling back to scanning
`extension/themes/*.json` when the manifest declares none — refusing a file that visibly contains
themes because its manifest is shaped unusually is the same refusal in a new place.

Every theme, not one, because themes ship in light/dark pairs far more often than not and the
setting is keyed by polarity: importing one of a pair leaves the other theme on the builtin and
the user back at the file dialog. The row selects whichever variant matches the window and says
in a sentence what went to the other theme, so a scheme cannot appear months later that nobody
remembers importing.

Reading an archive the user downloaded brought two bounds with it — 4 MiB per entry and 32 themes
per package, both two orders of magnitude above anything real. `zip` being correct about the
format says nothing about the size of what it hands back, and neither line is worth omitting.

The converter is Rust — `cide_core::scheme` — for two reasons, of which the second decided it.
Only `cide-app` may link tauri, so a converter in the webview would be domain logic in the glue
crate; and a converter in TypeScript could not be reached by `cargo test`, which matters because
the interesting part is a scope matcher with precedence rules and it needs a table of cases
rather than a screenshot. `SCOPES` is that table: for each of cide's roles, the TextMate scopes
that stand for it, most representative first.

Two departures from TextMate's own rule are worth naming. A theme rule matches when its selector
is a dot-segment prefix of the scope being asked about — but a great many published themes only
ever name *language-qualified* scopes (`support.type.property-name.json`,
`entity.name.function.js`) and never the bare one, and under the strict rule those themes convert
to almost nothing. So a selector sitting one segment *under* the candidate is accepted as weak
evidence, scored an order of magnitude below every prefix match so it can never outrank one.
And resolution is **total**: every unfilled role walks a fallback chain (`macro → function → fg`,
`bracket → punctuation → operator → fg`) rather than being left empty, because the input is a
file somebody else wrote and refusing it means the feature does not work for real inputs. The
same bargain `SettingDef::coerce` already makes.

`zip` is the one new dependency, at `default-features = false, features = ["deflate-flate2"]`:
the default set drags in aes, bzip2, lzma, ppmd, zstd, xz and a time crate, and a `.vsix` uses
none of them. `flate2` is named beside it only to *choose* the inflate backend — `deflate-flate2`
enables the dependency and stops there, and the crate then refuses to compile with "you need to
choose a zlib backend", because feature unification is per build graph. Its default `rust_backend`
selects `miniz_oxide`, which the image decoders already build, so the whole road costs `zip`,
`arbitrary` and nothing else. A hand-written ZIP reader was the alternative and lost: a
central-directory parser is a hundred lines against a file from the internet, and malformed
offsets, truncated entries and decompression bombs are exactly what a well-exercised library has
already got right.

The file is read **once**. What is stored, in `$XDG_CONFIG_HOME/cide/schemes/<id>.json`, is the
converted scheme — re-converting at every launch would let a change to `SCOPES` silently repaint
a buffer somebody was happy with, months later. Re-importing is how a user opts into an improved
table.

**The setting is two fields, one per theme.** `EditorSettings::color_scheme_light` and
`color_scheme_dark`. A scheme paints the buffer's *background* — the user asked for that
explicitly — so a single field would let an imported dark scheme paint a dark rectangle inside a
white window, next to a white sidebar and a white terminal, and every contrast ratio
`check-theme.mjs` records against `--panel` would stop describing the editor. Keyed by polarity,
an imported dark scheme is only ever a *different dark*. `Theme` stays `dark | light` and every
existing consumer of it — the file icons, the native window fill, `theme-boot.js`, `otherTheme`,
`theme.toggle` — is untouched.

### The two things that were nearly wrong

**The caret's line has no role, and must not get one.** `EditorSurface.module.css` spends thirty
lines on why `.cm-activeLine` is 40% of the selection rather than an opaque colour: the active
line paints *above* CodeMirror's selection layer, so anything opaque there hides the selection on
the caret's row — a reported bug — and only a tint of the selection's own colour composites back
to the selection exactly. `editor.lineHighlightBackground` is a key every VS Code theme carries,
almost always with an alpha channel, and `ColorScheme` stores six hex digits. Honouring it would
have reintroduced that bug for every imported scheme. It is deliberately absent from
`SURFACE_KEYS`, and both files say so.

**The repaint guard cannot be keyed on the scheme's id.** An import does two things — patches the
setting that names the id, and broadcasts `cide://schemes-changed` with the scheme itself — and
they reach a window in whichever order the event loop hands them over. A guard keyed on
`theme:id` latches on the first: the id resolves to nothing, the builtin is painted, and the list
arriving a moment later compares equal and is never applied. The import then looks like it did
nothing at all, in a way no error reports. `schemeIsPainted` compares the resolved scheme by
**reference**, which also covers a re-import replacing a scheme's colours under the same id.

### What the gates cover

`check:scheme` is new and exists because the role set is spelled in four files that cannot import
one another — `cide_ipc::theme`, `editor/scheme.ts`, `editor/highlight.css` and `tokens.css` —
and every way they can disagree is silent on screen. A role declared and read nowhere is a colour
nobody sees; a role read and declared nowhere paints *nothing at all*, because an undefined
custom property is not a colour and the span simply inherits. It also asserts that a block
carrying part of the family carries all of it — the rule `check-theme.mjs` already applies to the
terminal palette, and the one this file's first draft did not have: a `--tk-doc` missing from the
dark block resolves to the light value through the cascade and every per-role assertion still
passes.

### Selecting code in the dark theme made it disappear — and the first two fixes were the wrong bug

**Read this section as a worked example of diagnosing from a model instead of from the artefact.**
Two rounds of palette arithmetic went into this report before anyone opened the screenshot in an
image library and sampled a pixel, and the value being tuned was not reaching the screen at all.

`@codemirror/view`'s base theme carries:

```
"&light.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground":
  { background: "#d7d4f0" }
```

`&` and `&light` are **generated classes** — `baseThemeID` and `baseLightID`, see `buildTheme`
and `lightDarkIDs` in that package — so that selector is six classes, specificity **(0,6,0)**.
`EditorSurface.module.css` said `.body :global(.cm-editor .cm-selectionBackground)`, which is
(0,3,0). It never applied, in any theme, since the file was written. Every focused editor painted
CodeMirror's light-mode lavender `#d7d4f0`; `--tk-fg` in the dark theme is `#d7d7dd`. That is
**1.02:1**. The text was not dim, it was gone — which is exactly what the report said, twice, and
exactly what a pixel sample showed in ten seconds: selected rows at `#d7d4f0`, the identifier
`serde` on them at `#d7d7dd`, and the active line at `#0e1436`, which *was* the new token and
proved the palette was live while the selection was not.

It also explains the detail that should have redirected the investigation a round earlier:
imported themes looked fine. Min Dark's foreground is a mid grey `#7d7d7d`, which is legible on
lavender. cide's near-white is not. The bug was never in the palette.

The root cause is one line that was never written: **nothing in this application ever set
`EditorView.darkTheme`**, so every editor in cide has been a *light-mode* CodeMirror, including
under the dark theme. That facet decides which of the two generated classes lands on the editor
element, and the base themes of view, autocomplete, lint, search and merge are written in terms
of them — around forty rules. Most were masked by our own stylesheets overriding the same
properties. The selection was the one with specificity to spare.

`editor/cmPolarity.ts` sets the facet, in a compartment so a theme switch reconfigures rather than
rebuilds — a rebuild would take scrollback, selection and undo history with it, the same argument
the language, lint and blame compartments each make. All three CodeMirror surfaces carry it, and
`check:scheme` asserts each one does, because a fourth surface added without it would reintroduce
the whole family silently.

That alone does not settle the colour: the `&dark` twin of the rule above is the same (0,6,0)
selector with `#233` in it. So the selection rule moved to `editor/highlight.css` — the one
stylesheet that is global rather than module-hashed — written as `html:root .cm-editor.cm-focused
> .cm-scroller > .cm-selectionLayer .cm-selectionBackground`, which is (0,6,1) and wins on the
element. Spelled out rather than reached with `!important`, because an `!important` here could
not be overridden by anything later, and a contributed theme is where this palette is heading.
The module files keep a comment where the rules were, saying not to put them back.

**What the two earlier rounds were actually worth.** The contrast analysis was correct about a
real defect and is now load-bearing rather than theoretical, because `--tk-sel` finally reaches
the screen. The rest of this section is that work, and it stands.


The gate above shipped measuring ink against **one** ground — `--tk-bg` — and the next report was
*"when I select the code in dark theme I don't see the text."* It was true, and it was worse than
it sounds.

A selection is a *ground*. Every dark ink in `tokens.css` was chosen against `--tk-bg`, which is
nearly black; `--sel` sat 1.60:1 above it, so selecting a region raised the floor under all of
them at once. Comments went from a marginal 2.76:1 to **1.73:1** — gone — and punctuation,
brackets, operators and emphasis to 3.09:1. Checking ink against one ground and then painting it
on another is not a weaker check; it is a check of the wrong thing.

**None of it was new.** `--faint: #5c5c66` and `--sel: #2b3a55` have held those values since M3,
and the pairing was never measured: `EditorSurface.module.css` carried a comment asserting the
worst case on a selection was "still the 4.45:1 `tokens.css` picked the token for", and that
number was the token's ratio against `--panel`. The colour-scheme work neither caused this nor
fixed it — it moved the values into `--tk-*` verbatim and carried the wrong comment along.

**The first fix was not enough, and the second attempt is the one worth reading.** `--tk-comment`
came up to `#72727e` (3.84:1, which it wanted anyway) and `--tk-sel` was darkened to `#132a4d`,
1.27:1 above the background. Ink cleared 3:1, the gate went green, and the report came back:
*"still have elements that is close to selection color."* Correct — 3:1 is the WCAG floor for
**large** text, and a 12.5px buffer is not large text.

Chasing a higher absolute floor is unbounded, and the arithmetic says so out loud. Contrast
against a selection is `(ink + 0.05) / (selection + 0.05)`, so the selection's luminance is a tax
on every ink at once. `#72727e` has luminance 0.171; against a **pure black** selection that is
4.42:1. No selection of any colour can carry this palette's comment to the 4.5:1 body-text bar.
Either the quiet half of the palette stops being quiet, or the selection stops charging the tax.

So the selection carries almost no luminance. `--tk-sel` is `#0d1560`: 1.12:1 against the
background, visible through *chroma* at a redmean distance of 124 — more separation than the old
`#2b3a55` managed at 131, for a third of the luminance. Blue because blue's luminance coefficient
is 0.0722 against green's 0.7152, so it is the only channel that can carry that much chroma at
that little brightness, which is also why every dark theme's selection is blue.

The consequence is the point, and it is a different kind of claim from a floor: every ink keeps
**89%** of the contrast it has unselected. Selecting a region stopped being a legibility question
rather than becoming a better-tuned one.

The chrome's `--sel` is untouched. It grounds a selected file-tree row against `--panel` and
answers a different question; the two being separable is what the `--tk-*` family is for.

The caret's line went from 40% of the selection to **55%**, and that number turning out not to be
load-bearing is the useful part. Its own comment argues at length that the *token* matters —
a fraction of the selection over the selection is the selection exactly — and that holds at any
fraction, but the note repeated "40%" as though it were a design value. With a darker selection
40% put the active line 39 redmean units off the background where it used to be 52; 55% puts it
at 55, and improves the light theme's from 29 to 40 on the way past.

### Refusing a file that is not a theme, and doing it legibly

*"I'm not able to import cpptools-linux-x64.vsix."* That is Microsoft's C/C++ extension: it
contributes languages, debuggers, semantic tokens and eighteen other things, and **no themes at
all**. Refusing it is correct. Two things about *how* it was refused were not.

**The message reached nobody.** `CoreError` is a tagged enum, so a rejected `invoke` arrives in
the webview as `{ kind: 'serde', detail: '…' }`, and the Settings screen rendered it with
`String(e)` — `"[object Object]"`. The sentence Rust had composed, naming the file and what was
looked for inside it, was thrown away at the last step, and the user saw a failure with no
explanation. `ipc/errorText.ts` is the fix and `check:scheme` pins it, because a failure path is
only ever seen by the person it is failing and nothing about the happy path can catch it
regressing. `KeymapSection` had the same bug on the same screen and is fixed with it.

**The file was read whole to be rejected.** `import` did `fs::read` before looking at the header —
137 MB into a `Vec` to inspect four bytes. It now reads the four, and hands the archive road the
`File` itself, so `zip` seeks to the central directory and inflates only the entries a theme lives
in. A bare `.json` is capped at 8 MB, so pointing the picker at something enormous that is *not*
an archive fails with a sentence rather than by exhausting memory.

And the refusal now says what the file turned out to be, read from the manifest's own
`displayName`:

> `cpptools-linux-x64.vsix` contains no colour themes — **C/C++ is an extension, but not a theme**.
> Looked for contributes.themes in extension/package.json, then for extension/themes/\*.json

"contains no colour themes" on its own reads as cide failing. Naming the extension makes it read
as the wrong file, which is what it is.

### The whole buffer washed out, and the value that settled it

*"Still something is wrong — check how the theme is rendered in cide and how it is represented in
the theme's own screenshots."* Two artefacts, and between them the answer took one measurement:
classify the ink in cide's render, classify the fills in the theme's showcase SVG, compare the two
palettes.

The showcase's code block uses six colours — `#b392f0`, `#f97583`, `#f8f8f8`, `#ffab70`, `#6b737c`
and, for everything else, **`#bbbbbb`**. cide's render had the first five and, in place of the
last, `#7d7d7d`: a full stop darker, across every identifier in the file, which is most of it.

Min Dark states no `editor.foreground` at all. It states the *workbench* `foreground: #7D7D7D` —
the colour of a sidebar, a status bar and an activity rail — and `SURFACE_KEYS` listed that as the
second choice for the editor's ink. VS Code does not read it for the editor; it falls back to its
built-in default theme, which is the `#BBBBBB` the showcase renders. So the key is gone from that
row, and a theme stating neither ground now gets **VS Code's** defaults rather than cide's, on the
principle the whole importer rests on: an imported scheme is a rendering of somebody else's theme,
so where the theme is silent the right answer is what its author saw.

It is also the third bug in this feature found by measuring an artefact rather than reasoning
about the model, after the lavender selection and the scope matcher — and the second where the
evidence was sitting in a file the whole time.

**Re-import is required for this one.** What is stored is the converted scheme, so a fix to the
conversion reaches a scheme only when it is imported again. The selection repairs itself on load;
the colours do not.

### An imported theme showing the wrong colours, and how the scope matcher was reading themes

*"Custom colour scheme is not fully working — I don't see all colours like before."* Under-specified
as a report, and this time the theme file was the ground truth: Min Dark's `tokenColors` say what
it wants, so what cide computed could be checked against it line by line rather than guessed at.
Two of the answers were provably wrong.

**A broad rule was beating a qualified one.** The scoring gave any prefix match a flat 100 and any
narrower sibling a flat 10, so a theme stating both `support` and
`support.type.property-name.json` answered the property question with `support`. Min Dark states
exactly that pair, and cide painted JSON keys `#79b8ff` where the theme says `#f8f8f8` — the rule
VS Code actually applies to a JSON key. The primary score is now **how many dot-segments the two
share**, with direction only breaking a tie at equal depth: exact, then a true TextMate prefix,
then a narrower sibling. Depth is the better signal in both directions, because a theme that
qualifies a scope by language is still describing that family and describing it more precisely
than a one-segment rule three levels above it. One test asserted the old ordering and is reversed,
with the reason written into it.

**Punctuation was inheriting a keyword colour.** Two things caused it and both had to go. The
narrower-sibling rule fired on `punctuation.separator.key-value` — a deliberate special case, the
`:` between a key and its value — and read it as a statement about every comma in the file; and
the fallback chain ran `bracket → punctuation → operator`, where `operator` is a *keyword* in
TextMate and is usually coloured like one. Min Dark's buffer came out with its punctuation,
brackets and operators all in the theme's keyword red. `NO_WEAK_EVIDENCE` now refuses the
narrower-sibling rule for those three roles, and the chain ends at `fg`, which is what VS Code
paints punctuation a theme is silent about.

The distinction the list encodes is *language qualification versus special case*, and it is not
something a scope string carries — hence a list rather than a rule.

Min Dark now converts to the colours its own file asks for: keys `#f8f8f8`, punctuation and
brackets at the foreground, operators the theme's keyword red (faithful — `keyword` is a prefix of
`keyword.operator`, so VS Code paints them that way too), strings `#ffab70`, comments `#6a737d`.

### An imported theme that names no selection at all

The next report was the mirror image, on an imported theme: *"no selection visible at all, like
I'm just moving the cursor."* Min Dark declares `editor.background` and **no
`editor.selectionBackground`** — a legal and not-rare thing for a theme to do, since VS Code falls
back to its own default.

The fallback that had been written for that case blended the background 18% towards the
foreground. On Min Dark's `#1f1f1f` and `#7d7d7d` that is `#303030`: 51 redmean units, below
anything the eye registers as a state change. It was also the wrong *direction* — blending towards
the ink is pure luminance, which is the tax the section above spends its length avoiding.

Three changes, and the first is the one that was simply a mistake:

**Alpha is composited now, not dropped.** `editor.selectionBackground` carries an alpha channel in
most themes, and the two readings are nothing alike: `#ffffff20` over a dark editor is a
barely-lifted grey, which is what the theme meant, while `#ffffff` with the alpha discarded is a
white block over the text. `composite` flattens onto the theme's own background, so what is stored
is still six hex digits — `ColorScheme` has no notion of translucency — but it is the *rendering*
the theme asked for rather than its notation.

**The selection is validated, not merely filled.** A theme can state a selection that is useless
in practice as easily as it can omit one, and both produce the same report, so both get the same
answer. If it does not read as a selection — under 72 redmean units, which is what the built-in
light scheme measures — it is replaced. Overriding a value a theme did state is a real intrusion
and is taken deliberately: the alternative is a feature that silently does not work, and a
`ColorScheme` is a rendering of a theme rather than a copy of one. A theme that got it right is
left alone, and there is a test that says so.

**The derivation tints towards chroma, not lightness.** A deep saturated blue sits at roughly the
luminance of a dark editor's background while being obviously a different colour, so the gentlest
tint towards it that clears 72 units buys visibility for almost nothing: measured across real
themes a dark scheme keeps **93–99%** of its ink contrast. *Gentlest* is the whole criterion —
every step away from the background costs some ink something, so the answer is the first candidate
that clears the bar rather than the most visible one available. A light background cannot play
that trick, since there is nothing above white, so its tint is a mid blue and the cost is the same
0.84-ish the built-in light scheme already pays.

Min Dark now imports with `#191f4b`, 75 units off its background; Min Light with `#dbe9ff`, 76.
Schemes already on disk repair themselves, because `load_all` normalises what it reads — no
re-import needed.

`check:scheme` gained the assertion that states exactly that: **selecting may not cost any role
more than 20% of its contrast.** That is the check that survives a retune of the palette, where an
absolute floor does not — and it is the one that would have caught the insufficient fix, which
passed every floor. The threshold is set where it fails every state known to be bad: the shipped
bug scores 0.63, the intermediate `#132a4d` 0.79, and VS Code Dark+'s own `#264f78` fails against
this palette too. The dark palette now scores 0.89 and the light theme's warm `#ffe7d9` 0.84 —
fine at it, because losing a sixth of a large ratio is not losing a sixth of a small one.

Beside it, the two absolute floors as a backstop, and the assertion that the selection is
distinguishable from the background at all — measured with `apart`, not `contrast`, because the
whole direction of the fix was to remove the selection's luminance and a ratio would have scored
that as a regression.

Its ink floor went **up** to 3:1 in the same change, which is `check-theme.mjs`'s
`TERMINAL_MIN_CONTRAST`. It was 2.5, argued as "a comment is deliberately quiet and `#5c5c66` is
2.76:1" — true, and the wrong conclusion: 2.76:1 was already marginal, and it was the reason the
selection could not be repaired by moving the selection. Both moved, the palette clears 3.0 on
both grounds in both themes, and the carve-out stopped being a decision. The gutter keeps a lower
floor of its own, named rather than exempted: a line number is never drawn on a selection, and
brighter numbers compete with the code they number.

One more thing fell out of it. `ColorScheme::normalise`'s fallback chain answered `bg` for a
missing `sel`, so a theme with no `editor.selectionBackground` — legal, and rare rather than
impossible — imported with an *invisible selection*: the same bug, arriving by a different road.
It is now derived, 18% of the way from the background towards the ink, by the same sRGB blend CSS
uses so the selection and the active line cannot disagree.

`check:editor` grew the role assertions, including a block that walks a JSON buffer tag by tag
and asserts six distinct roles, and `cargo test -p cide-core scheme` converts a theme shaped like
a real one and asserts the same six come out as six different colours with the key not equal to
prose. That pair is the acceptance criterion for the whole change.

**One bug was found on the way and fixed.** `languages.ts::tagIsUnknown` — the check that refuses
a grammar rule from an extension naming a tag cide cannot colour — used `tagsFor`, which answers
for any name `@lezer/highlight` exports. Several of those have no role at all (`inserted`,
`deleted`, `changed`, `list`, `content`, `strikethrough`, and bare `variableName`), so an
extension writing `"tag": "inserted"` passed validation, parsed, matched, and painted nothing:
precisely the failure the check's own comment says it exists to prevent, arriving through the
check itself. It is `tokenClassFor` now, which answers `null` unless a role actually claims the
tag.

**Not done.** None of it has been confirmed on screen — the same reason as every milestone since
M14, KDE will not raise a shell-launched window. What is checked is the tables, the converter,
the applier's decisions and the four copies of the closed set. Two things are deliberately out of
scope and named here so they are not read as oversights: `fontStyle` from an imported theme is
ignored, because decoration is a closed set in `highlight.css` and honouring per-scope
italic/bold means a second custom property per role; and the terminal's ANSI palette stays with
the chrome theme, because `check-theme.mjs` enforces a contrast floor and a grey-ramp ordering on
those slots that an arbitrary imported palette would violate. There is no `contributes.themes`
extension kind either — it is the natural follow-up, and it argues with ADR 0010's *"an extension
cannot ship CSS"* in a way that needs its own decision.

Semantic highlighting is the next real step up in *accuracy*, as distinct from this change, which
is about palette: `cide-lsp` requests no `semanticTokens` today, so a stream lexer's guess is
still what colours a Rust or TypeScript buffer.

## The whole file in a diff, and IDEA's split view (M25), and what is not verified

The git diff tab used to show the patch — hunks at git's default three context lines, nothing
between them — and its side-by-side view kept the two sides row-for-row aligned by drawing
hatched filler cells wherever one side had no line. Both were the complaint: the reader wants
the *document* with the changes in it, and IDEA's answer to an added block is not a column of
empty cells but a thin green line on the side that lacks it.

**The wire.** `FileDiff` and `RevisionDiff` carry `oldText`/`newText` now — both sides whole,
read in the same round trip (`crates/cide-git/src/diff.rs::side_texts` names which blob each
`DiffSide` pair reads; the index blob was the one not exposed before). The hunks stay at context
3 **byte for byte**, because `FileDiff::rev` hashes those exact patch bytes and staging
re-derives them: widening context would have turned every partial stage into `StaleSelection`.
`texts_omitted` marks a side over the 2 MiB cap (`revision::MAX_BLOB_BYTES`, shared on purpose);
binary, submodule and typechange skip the texts entirely. Texts are display enrichment — a read
failure degrades to `None`, never to an error.

**The reconstruction** is `ui/src/panes/diffRows.ts`, import-free so `check:diff-render`
compiles it standalone. Hunk rows are the wire rows, untouched — every `hunk:line` position,
tick box and selection survives by construction, and gap rows carry no position at all. Every
context and addition line is validated against the text; any disagreement (the ident filter, a
drift this module did not foresee) falls back to the hunks-only rendering, which is never wrong,
only shorter. Files over 5,000 lines fold long unchanged runs behind "⋯ N unchanged lines"
expander rows; under that, the file is simply all there.

**The split view** is two independently scrolling columns now — left old, right new, no fillers,
no `blameGap` spacers — with a thin 2px marker (`--green`/`--red`, both palettes) at each point
where the other column has a block this one lacks, end-of-file included. The columns are kept in
step by `ui/src/panes/diffSync.ts`: anchors measured at changed-run boundaries once per render
and after a *settled* resize, proportional interpolation between them, and MergePane's
Set-of-marks echo guard — its comment explains why a time-released flag cannot guard a loop
whose echo arrives asynchronously. Scrolling the right column through an inserted block holds
the left still at its marker, which is IDEA's behaviour. The left column hides its scrollbar
(two scrollbars at two fractions read as two documents); the fallback split (no texts) renders
through the same two columns from the hunks alone, with the `@@` header on the per-hunk bar as
the landmark between discontinuous line numbers.

**The merge pane** got the same gesture: a block a pane has no lines for — the other side
inserted where it has nothing, or it deleted the block — draws a `cm-mergeInsert` marker line
at the boundary instead of either lying a full tone band onto the neighbouring line (the result
pane's old behaviour) or drawing nothing at all (the side panes', which made a theirs-only
insertion invisible in the ours pane). The marker is a `::before` with no layout height, so
CodeMirror's height cache and the block-anchored scroll sync are undisturbed, and it follows the
pane's colour rule: a settled block draws nothing, anywhere.

**Not verified on screen.** The model, the markup, the run table, the mapping arithmetic and the
marker positions are all pinned by `check:diff-render` (including `diffRows`/`diffSync` compiled
standalone) and `check:merge`; the Rust side is differential against the real `git` in
`staging.rs` and `revision.rs`. What no check can hold is the *feel* of the scroll sync — wheel,
find-in-page landing in one column, the snap past a pure insertion when driving from the side
that lacks it — and that has not been confirmed on a display, same reason as every milestone
since M14. Deliberately out of scope, named so they are not read as oversights: deletions still
fall back to hunks-only (`newText` is `None`, and a deleted file's one hunk *is* the whole old
file, so the render is pixel-identical); the left column is never blamed (a second blame at the
other revision is a real feature, not an omission of this one); and the unified whole-file view
drops the sticky `@@` headers rather than making them float — the line numbers are on screen,
and the staging affordances moved to a slim non-sticky bar per hunk.

### Syntax colour, and a changes iterator (M25)

The diff was monochrome from the day it shipped: `panes/GitDiffPane.tsx` draws plain DOM rows and
emitted the line text raw. It is coloured now by **the buffer's own grammars** — one table, so a
file open in an editor tab and in a diff beside it cannot be two different colours. The road
already existed: `editor/markdown/fenceTokens.ts` was built for the markdown preview, which is the
same problem (colour a string in plain DOM with no `EditorView` anywhere), and it is pointed at
two documents here instead of one fence. Both sides are tokenized, because a deletion exists only
on the old one.

**The whole thing rests on one string compare.** The text and the patch reach the pane as two
claims — the same pair `wholeFileSegments` validates — and every way they can disagree (CRLF, an
ident-expanded keyword, a rev that moved under an open pane) lands as a row whose content is not
the text's line at that number. Colouring one of those paints a *plausible* lie: a keyword-red
`if` on a line with no `if` in it, no gap, no artefact, nothing in any log. So `diffTokens`
compares each row against the line its tokens spell and refuses on any difference, which makes
every misalignment degrade to plain text instead. `check:diff-render` drives both directions of
that failure — right number with wrong text, and right text at the wrong number.

The tokenizer is behind a **dynamic `import()`** and the pane holds only plain data. That is not
guarding a crash — `@codemirror/language` imports cleanly under node, and `check:markdown` already
runs `fenceTokens.ts` there — it is what lets `check:diff-render` assert on the emitted SSR bundle
that no CodeMirror package is linked into the surface it renders under node. Matched on the import
*statement*, because `diffBlame.ts` says the words "@codemirror/merge" in prose and that comment
survives into the bundle.

**Next / previous change** is `navigate.nextChange` / `navigate.prevChange`, on **F7** and
**Shift+F7** — IDEA's chords — plus a pair of chevrons in the pane header. The unit is one changed
**run**, not one hunk: `diffRows.columnRows` already groups a deletion run and the addition run
after it into one edit, and the iterator is simply a fourth consumer of that run table, so the two
columns, the scroll sync and the walk cannot disagree about where a change begins. A hunk bundles
several edits plus context and would skip past distinct changes.

Three things about it are decisions rather than defaults:

* **It clamps, it does not wrap**, and `MergePane`'s block stepper — which *did* wrap — was
  changed to match. One gesture must not have two rules, and both now go through one `stepIndex`,
  so they cannot drift apart again. A refusal is reportable (`unmet(…)` puts a line in the log)
  where a wrap on a one-change diff is indistinguishable from a chord that did nothing, and the
  ends being reachable is what lets the buttons draw themselves disabled.
* **Exactly one scroller is written.** The split view's `follow` treats a programmatic `scrollTop`
  as a person's, maps it through `mapScroll` and moves the other column itself, so the iterator
  touches neither the echo set nor `diffSync`. Writing both columns would be the bug: two writes
  are two events, each moving the other, and the reader lands where neither asked. The row is
  found by `data-at` and never by index — the columns interleave zero-height insertion markers, so
  a child index is not a row index.
* **The marker changes no row's height.** A background wash and an inset shadow, on the *right*
  edge because staging owns the left one. The scroll anchors are measured once per render and then
  only after a *settled* resize, so a border on the current run would silently drift the two
  columns apart by its own pixels with nothing throwing.

The chord is scoped `diffFocused && !terminalFocused`, the first compound clause in the shipped
table. F7 is `ESC [ 18 ~` and `mc` puts a menu on it; the gate is a window capture listener, so
`diffFocused` alone would take the key from a shell pane split beside a diff tab. A diff pane and a
merge pane are both `kind == "editor"`, so the second half costs the feature nothing. To give it
back:

```json
{"key": "f7", "command": "-navigate.nextChange", "when": "diffFocused && !terminalFocused"}
```

— with the clause, because a removal matches on all three and one that forgets it matches nothing.

`diffFocused` is a **host** flag, read from a claim stack in the webview
(`panes/changeNav.ts`), and no derived spelling works: a tab kind misses the log tool window's
revision diff, which is not a tab, and is *true* for a Claude `openDiff`, which is out of scope.

### IDEA's split gutter (M25)

Two changes to the side-by-side layout, both of them about the same thing: making it legible which
block on the left is which block on the right.

**The left column's line numbers moved to its right edge.** With a number at each outer margin the
two columns' numbers are the two things furthest apart on screen, and comparing "line 204 there"
with "line 206 here" is a saccade across the whole pane. They now meet either side of the gutter.
A grid places by order, so this is a real reordering of the row's children with a matching track
list on `.column[data-side='old']`, and `check:diff-render` reads the child order off the markup
rather than off the stylesheet — the two have to agree and only one of them is a fact about the
DOM.

**And the gutter draws a ribbon per changed block.** A third grid track between the columns, with
a filled shape joining each block's top and bottom on the left to the same block's top and bottom
on the right. A run that is one line on one side and six on the other reads as a *taper*; a pure
insertion has no lines on the left at all, so its shape collapses to a point there, which is what
makes an insertion legible as arriving *between* two lines rather than replacing one.

`panes/diffConnector.ts` is the geometry and it is import-free, because none of this is visible in
a markup snapshot: a wrong path is a shape in the wrong place, not a missing element. It is fed
`columnRows`' run table — the same table the two columns, the scroll sync and the changes iterator
read, so the gutter cannot come to disagree with any of them about what a change is — plus each
column's measured row tops. Scroll is subtracted **per column** rather than by transforming the
SVG, because the follower is written after the leader's event and for a frame the two are genuinely
at different offsets, which one transform cannot express.

Three things it does that are about cost rather than looks: coordinates are rounded to a tenth of a
pixel and the whole path set is rebuilt as one string and compared with the last before the DOM is
touched at all (this runs on a scroll frame, and most frames move no shape — the columns scroll in
step); runs far outside the viewport are never built; and a run whose boundary has not been measured
yet is skipped explicitly, because a `NaN` in a path renders as nothing *and* logs nothing.

**Only the right column selects, and its box follows the line number.** The split view drew a tick
box in both columns and a hunk bar in both as well — a tri-state box on the left facing a blank row
on the right, twice per hunk. Two boxes facing each other across the gutter read as two independent
selections of one file, which is the opposite of what the model does: a mark is one `hunk:line`
position and the two sides are two views of one set. The bar went entirely from the whole-file
rendering, because its landmark value is the `@@` header and a whole-file view has none to show —
the line numbers are continuous and on screen — so all that was left of it was the affordance. The
hunks-only fallback keeps its bars, where those headers are the only landmark between discontinuous
line numbers, with the controls moved to the new side alongside the per-line boxes.

**The cost is real and is written down rather than left to be found: a deletion cannot be ticked in
the split layout.** A deletion exists only on the old side, so its position rides the left column
and there is no longer a box on it. The row still *paints* as selected when it is, so split shows a
selection faithfully — it just cannot originate one there. Unified is the layout that stages, and
it is unchanged; split is the one that reads.

**The current change is a rail and not a wash**, and the first version got that wrong. It shipped
with a 7% `--accent` wash layered over the origin tint, the way `data-selected` layers its 14% one.
`--accent` is a terracotta, and 7% of it over `--diff-add-bg`'s green composites to an olive that is
neither — reported as *"line 206 is not marked as added, it should be green"*, which was exactly
right: the wash answered "this is the current change" by taking away "this is an addition". That is
the same failure `data-selected`'s own comment records, arriving from the other side. A tick is a
state of the row and can afford to tint it; being walked past is not.

**Still unmet.** `panes/DiffPane.tsx` — Claude's `openDiff`, a `@codemirror/merge` view — is still
uncoloured and has no chord for its chunks; it was left out deliberately rather than missed, and
it is the one surface where highlighting is a five-line change (`loadLanguage` into the shared
extension array, one compartment per side). Nothing about the scroll, the marker or the claim
stack is covered by a check script: none of them constructs a live DOM, so they were confirmed by
hand and belong in the `CIDE_AUDIT_PANES=1` harness. And a step during a live splitter drag lands
the driven column correctly while the follower may be off by the drag's delta until the next
scroll — the bounded `whenResizeSettles` imprecision `diffSync.ts` already admits, not a new one.

## Narrowing a search to a folder and a file pattern (M25), and what is not verified

The ⌕ panel searched every file under every project root, always. `SearchQuery` carried a
pattern and three toggles and nothing about *where*, and `cmd::search` mapped `project_fs.roots`
1:1 into `Vec<SearchRoot>` unconditionally. Two boxes and one gesture close that.

**The wire.** `SearchQuery` gains `scope` (one folder, `""` for every root) and `include`
(comma-separated file globs, `""` for every file). Fields on the query rather than arguments
beside it, because `SearchRegistry::job_for` keys the running job on `job.query == *query` and
every frame echoes the query back — so being a field buys job identity and the
stale-frame check for free, and anything outside the struct would have to rebuild both. The
failure that guards against is not hypothetical: it is one folder's results drawn under another
folder's heading, with nothing on screen to say which. `SearchModel.ts::sameQuery` is the
frontend half and `check:search` drives one field at a time — and then every key of
`EMPTY_QUERY` — so a field added in Rust and forgotten there fails the build rather than
painting a wrong answer.

`SearchFrame::error` is now a `SearchProblem` — `Pattern`, `Scope` or `Include`, each carrying
its sentence — rather than a bare string. One nullable field and a *kind*, not three fields and
not a heading: two fields that can disagree is what `PickerRow::source` argues against, and a
heading on the wire would be a Rust crate deciding what a 252px box says. The panel maps the
kind to the words and outlines the box the user can act on. It stays an answer *on the frame*
and never a rejected `invoke`, for the reason that was already written down for regexes and is
just as true of the other two: half of every folder path is unusable while it is being typed.

**Scoping is a filter, not a narrower root list.** `cide_search::content::within_scope` composes
into the `admits` closure `cmd::search` already builds, and `Visitor::visit` turns a rejected
directory into `WalkState::Skip` — so a search scoped to `crates/cide-git` skips `ui/` at its
top rather than walking it to reject every file one at a time, and the cost of the whole
narrowing is one directory listing per level of the scope's own ancestor chain
(`a_scope_prunes_rather_than_filtering_what_it_has_already_read` measures it through `scanned`).
A directory *above* the scope is deliberately admitted, or the walk never reaches the scope at
all. Containment is `Path::starts_with`, which is component-wise: a `str::starts_with` would
call `/p/crates/cide-github` a child of `/p/crates/cide-git` and search the wrong crate.

Replacing `roots` with the scope directory would have been a marginally cheaper walk and would
have re-based every `SearchHit::rel`, so one file would read `src/log.rs` in the panel and
`cide-git/src/log.rs` in the file picker. `a_scoped_hit_is_still_named_relative_to_its_root`
is what holds that.

**The globs are `ignore`'s own overrides**, one `Override` per root, which is what
`rg -g '*.ts'` uses and means the syntax is gitignore's: no `/` matches a basename at any depth,
a `/` anchors at the root, a leading `!` excludes. Two properties of `ignore` 0.4.33 are load
bearing and both look like bugs if you assume otherwise, so `compile_globs` names them:
`Override::matched` ends `if mat.is_none() && self.num_whitelists() > 0 && !is_dir`, and that
`!is_dir` is why a whitelist-only `*.ts` does not prune every directory in the tree; and a
whitelist *outranks* gitignore for files, so `*.ts` would on its own resurface a gitignored
`.ts` — it does not, because `cide_fs::Filter` carries its own per-directory `Gitignore` and is
consulted before a file is opened. That is the "belt and braces" `Job::walk`'s comment has
always claimed, and this is the first case where it holds the line rather than merely agreeing.

**What the box holds** is the spelling `SearchHit::rel` draws: relative to the root, prefixed
with the root's label when the project has more than one. So the box reads `cide-git/src` while
the headings under it read `cide-git/src/log.rs`. `resolve_scope` is the only reader and tries
three readings in a fixed order — a bare root label, then the literal `<root>/<text>` for every
root, then a `<label>/<rest>` prefix. The literal beating the prefix is the load-bearing one: a
repository with a directory named after itself would otherwise have `cide/src` resolve to
`<root>/src` and search a tree the user did not name. A scope naming a **file** resolves to its
parent directory, which is the rule behind "or if a file is selected, its parent" — stated once,
in Rust, where a test can run it and where the frontend does not have to guess from a tree row
that may have been scrolled out of the row cache.

**Ctrl+Shift+F from the file tree** seeds the folder box from the selected row. The focus test
is at runtime in `keys/dispatch.ts`'s `sidebar.search` arm and **not** a `when` clause, and
`ui/src/sidebar/treeFocus.ts` is the argument at length. The short version is that the tree's
other focus-scoped chords — Ctrl+C/X/V, Delete, the speed-search letters — live on its scroller
*because nothing in the default keymap binds them*; `ctrl+shift+f` is bound, the gate is
`window.addEventListener('keydown', gate, true)`, so it resolves and swallows the chord and the
scroller's `onKeyDown` is never called. A `CONTEXT_FLAGS` entry is the other reflex and is wrong
twice: a `Command::when` does not gate the keyboard at all, and gating is not what is wanted —
the command must keep working everywhere and merely do slightly more in one place. `sidebarFiles`,
the flag that exists, means the panel is *visible*, which it is while the user types in a
terminal beside it.

Pressed anywhere else the chord does exactly what it always did, and an existing scope is left
alone: a chord that silently widened a search back to the whole project would be worse than one
that does not narrow it. The tree's context menu carries the same act as *Search in Folder*
(*Search in Containing Folder* on a file row), disabled with a sentence for a row under no
project root — the search only ever walks roots, so scoping to a dependency source could not
produce a hit and an enabled item that reported it afterwards is the dead control this panel has
already shipped twice.

`applyScope` attaches the store to the project *before* writing the scope, and that ordering is
the whole of a bug that would only have bitten a user who had switched project since they last
opened the panel: `attach` clears both boxes when the project changes — right, because a path
means nothing in another tree — and the panel is usually not mounted when the gesture is made,
so its own `attach` runs a commit later and would have thrown the scope away.

**The two boxes are folded away** behind a ⋯ disclosure, because two more inputs are ~56px of a
252px panel spent on something most searches do not use. They are shown unconditionally whenever
either holds something, so the panel can never be quietly searching one folder while looking
exactly like it is searching all of them, and the disclosure is refused with its reason in that
state rather than becoming a button that changes nothing.

**Not verified on screen.** The model, the resolution, the pruning, the glob semantics and the
kind-to-heading mapping are pinned by `check:search` and by `cargo test -p cide-search`
(differential over real directory trees) and `-p cide-app`. What no check holds is the feel: the
140ms debounce over a half-typed folder path, whether the yellow outline on the offending box
reads as guidance rather than an error, and whether the disclosure is discoverable at all. Same
reason as every milestone since M14. Deliberately out of scope, named so they are not read as
oversights: there is no *exclude* box beside the include one (a leading `!` in the same box is
`rg -g`'s answer and costs no chrome); the scope is one folder rather than a set, because the
box and the wire both say one place and a search narrowed to five folders has no honest
spelling; and neither box completes paths or globs as you type.

## A file in its own window: detached tabs, and what a sole pane's buttons mean (M25), and what is not verified

The report: the corner cluster of a file pane draws a maximize button (over a pane that already
fills its tab), and its detach and close buttons "print `lastPane`" — both ran into
`layout::close`/`take_pane`'s refusal to empty a tree — where detach should move the file into a
window of its own and close should close the tab.

**Close and detach on a tab's only pane act on the tab, and maximize is withheld.** The rule is
one function, `windows/windowTabs.ts::clusterPlan`, consulted by the cluster's buttons
(`App.tsx`), the context-menu rows and the `pane.close` / `pane.detachToWindow` /
`pane.maximize` chords (`keys/dispatch.ts`) — the chord doing something different from the
button it shares a name with being the drift `check:detached` now pins against. Close routes
through `closeTab`, so the unsaved-buffer and live-session dialogs are the same ones the strip's
× raises. The pinned console is exempt: its sole pane keeps the pane-scoped buttons and the
role-based refusals that word them.

**Detaching a file means detaching the tab**, because a detached *pane* window refuses editors
by design — the buffer registry is per `TabId` (`editor/openBuffers.ts`), and a pane taken out
of its tab would need a second buffer over the same file. `WindowRole::DetachedTab` existed as
"a domain variant nothing can produce"; it is now produced by `window_detach_tab` and undone by
`window_redock_tab`, `window_close` and `intercept_close` (Alt+F4), each ending in
`workspace::redock_tab`. Unlike a pane, **the tab never leaves `project.tabs`** — quit guards,
restore plans and the awaiting arithmetic all walk that list — so the whole arrangement rests on
one invariant: *no shell draws a torn-out tab*. The domain holds it from its side (`detach_tab`
moves `active_tab` off and drops the tab from the focus order; `activate_tab` answers success
without moving; `close_tab` picks successors around it) and the webview from its
(`windowTabs.ts::shownTabs` filters both the strip and — load-bearingly — `TabContent`, which
mounts every tab it is handed). The window renders through the shell's own path — same
`TabContent`, same `PaneFrame`/`PaneBody`, so splits inside it are ordinary splits — under a
small header (`windows/DetachedTabHeader.tsx`) carrying the title, the drag region, Redock and
the `WindowLights` the shell's header draws. Opening a file whose tab is torn out raises that
window instead of appearing to do nothing, and `planReveal` learned the same fact from the other
end: the shell answers `here: false` for a pane whose tab is out, so a reveal raises the tab's
window rather than claiming success over a pane nothing shows.

**Not verified on screen.** The rules are pinned by `check:detached`, `check:editor`'s reveal
fixtures and `cargo test -p cide-core workspace`; restore of a `tab:` window across a relaunch
rides the same `restore_windows` path as `pane:` windows and has not been watched happening.
Ctrl+Tab's switcher can still reach a torn-out tab from the shell — deliberate: MRU switching is
"go to where I was", and choosing one raises its window — while `tab.next`/`tab.prev` step over
it, because a positional walk should match the strip on screen. Sizing: the new window opens at
the tab panel's measured size, or `DETACHED_WIDTH×HEIGHT` when nothing measured.

## One window per project draws one project, and an open now opens a window (M26)

The report: with *Settings → Appearance → One window per project* chosen, every window's header
still carried a tab for **every** open project instead of the one that window shows.

**The header now draws `shownProjects(role, …)`.** `App.tsx` passed
`Object.values(workspace.projects)` — every project in the *process* — to `AppHeader`, so three
projects in per-project mode gave three windows the same three-tab strip. That was not only
cosmetic: two of each window's three tabs were **inert**, because `workspace::activate_project`
sets `active` only on shells whose `projects` contains the id, so clicking another window's tab
moved *that* window's active project and left this one with a highlighted tab over content it
does not render. The strip was also the only place offering to close a project this window has no
view of. The rule lives in `windows/windowTabs.ts` beside `shownTabs`, which is the same
invariant one ring in — *a thing is drawn by exactly one window* — and `check:detached` drives
both. `keys/target.ts::windowProjectsOf` had already learned this for Ctrl+`; it reads the same
`role.projects` and stays a separate function only because `check-commands` compiles that module
alone, so it may not import a value. Neither may derive a strip from `workspace.projects`, and
that is what the two checks pin.

**And the bug the first fix uncovered: in per-project mode, opening a project named a window
nobody built.** `workspace::open_project` ends in `rebuild_windows`, which mints a
`shell:<uuid>` role for the new project, and **no command turned that role into an OS window** —
`reconcile` was called after a project or tab *close* and never after an open. The project
existed, held a console tab with a live `claude`, and had no window anywhere; the only reason it
was ever visible is the bug above, which drew it as a tab in some other window. `open_project_here`
now reconciles, which is a no-op in stacked mode. `cmd::window::focus_project` — "bring this
project to the front, whatever that means in this mode" — reconciles too when the role names a
window the desktop has lost, so the repair is not one call site's.

**Not verified on screen.** `check:detached` pins the filter and both source seams,
`cargo test -p cide-app` covers the dock model beside it, and the mode flip itself is
`CIDE_AUDIT_WINDOWS=1`, which is still unrun. `focus_project` has exactly one caller today and it
is on macOS (see *Platforms* — the dock menu), so on Linux it is compiled, type-checked and
never called: that is deliberate, and the `allow(dead_code)` on it says so rather than a `cfg`
that would stop it being compiled at all.

## cide's own rust-analyzer: the bundled fork (M25), and what is not verified

rust-analyzer keeps its entire index in memory — 2–8 GB per project on real workspaces — and
cide, until now, had no say: no `initializationOptions`, `workspace/configuration` answered
`{}`, the binary came from PATH alone, and `Server::args()` was declared in M22 and never
applied to a spawn. The decision (ADR 0011) is to fork rust-analyzer, give the fork a
disk-backed index, and ship it inside cide. The fork lives in `../forks/rust-analyzer`
(branch `cide`, cut from an upstream release tag) — never a workspace member — and its
cide-side infrastructure is what this milestone builds:

- **Discovery is a ladder** (`cide_lsp::discover::locate`): `CIDE_RA_PATH` (a developer
  override that wins alone, and refuses rather than falls through when broken) → the
  `cide-rust-analyzer` sidecar beside `current_exe()` → PATH. Two names on purpose: the
  registry keeps saying `rust-analyzer` (panel rows, source ids, the extension-conflict rule),
  the shipped file is `cide-rust-analyzer` so the tarball's "put `bin/` on PATH" can never
  shadow the user's own build.
- **A start failure falls down the ladder** (`server::next_candidate`): a bundled build that
  dies before its handshake is retried as the next rung, so a broken sidecar degrades to
  exactly the pre-M25 behaviour. A crash *after* the handshake never advances — it would dodge
  the crash budget, and an OOM would only repeat harder on the stock build.
- **Settings → Inspections → "rust-analyzer build"** chooses Built-in vs System per server
  (`InspectionSettings::server_binaries`, absent = Built-in); flipping it restarts the server
  in every open project. The map is keyed by binary name so a future bundled gopls is one more
  row and no DTO change.
- **Only the shipped build is configured** (`cide_lsp::config`): provenance-gated
  `initializationOptions` + real `workspace/configuration` answers, carrying the two-repo
  contract — today only `cide.diskIndex.dir`, the per-profile cache directory
  (`persist::cache_dir()/rust-analyzer`; cide names and creates it, the fork owns its
  contents). A stock PATH server gets byte-for-byte the old handshake.
- **Packaging**: `packaging/rust-analyzer.lock` pins URL+rev for xtask preflight (absent
  sibling = failure with the clone command; wrong rev = warning), the generated Flatpak
  manifest (a pinned git source), and release.yml (both build jobs fetch the pin, with an
  actions/cache on the fork's target dir). The AppImage/deb carry it as a second Tauri
  sidecar in `tauri.bundle.conf.json`; the tarball installs `bin/cide-rust-analyzer`.

**Phase 1 is implemented in the fork** — six commits on `../forks/rust-analyzer`'s `cide`
branch, plus a **second fork beside it, `../forks/salsa`** (three commits on 0.28.2, and the
manifest's `../salsa` path patch is why the pair stays siblings of each other), wired through the
`[patch.crates-io]` line upstream left commented for exactly this. What persists, in the
order it landed: every salsa input (per-type serde that re-validates invariants as
deserialize errors; `ProcMacros` through a placeholder codec whose restore-time re-set under
a new revision is a soundness requirement — live expander channels restore from nothing);
item trees plus the whole macro-expansion interned world they stand on (`SyntaxContext`'s
hand-expanded `PERSIST = false` stubs became real codecs, `MacroCallId` and `tt::TopSubtree`
round-trip — the latter as its flat token stream so the varint encoding stays an
implementation detail); and **def maps** — the sixteen `impl_intern_key!` ids, the tracked
structs `ModuleId`/`BlockId`, and `ItemScope`'s eighteen fields, with struct-keyed maps as
ordered pairs (JSON map keys must be strings and a derive fails at *serialize* time — the
worst place) and `Name` as its bare string. The salsa fork is what makes the tracked-struct
half sound: deleted slots and the free list (the only record of a dead slot's generation)
round-trip, and restored-memo validation no longer panics on ingredients whose lazy typed
init has not run. The **LSP lifecycle** works end to end: restore in `GlobalState::new` with
the VFS pre-seeded from the snapshot's id→path table (FileIds are interner insertion order,
so replay makes them line up), save at shutdown and on the idle-GC tick, validity keyed on a
hand-bumped format version + build stamp + linked-projects fingerprint, and every failure
path deleting the snapshot and starting cold.

Measured on a tiny crate + sysroot (1,428 modules, debug build): the warm def-map walk went
**11.0 s → 2.6 s → 4.7 ms** across the three slices; the LSP twice-through test logs
`disk index saved` then `disk index restored` with zero panics. Baseline to beat on this
workspace, stock release build: 5.45 GiB peak RSS, 66 s cold analysis. Phase 2's cide side
is in: the RSS watchdog (Settings → Inspections → memory limit, off by default, floored at
256 MiB), with upstream's dead runtime-LRU plumbing documented rather than configured at a
no-op. `cachePriming` was off in the Phase-2 contract (it only warmed caches a restart
would drop) and is **on again since the disk index landed**: primed memos flow into the
snapshot and the memo tier, and priming's `$/progress` is the one honest per-module
progress feed — `Indexing 12/21 (libc)` with a true fraction — which is what drives the
Problems panel's determinate bar and the rail's busy dot.

**Phase 3 — the memory is bounded, not just restorable.** The four heavy persisted queries
carry `lru` caps in the fork (item trees 512/256, def maps 128/256), and eviction stopped
meaning "recompute later": the salsa fork gained a process-global `MemoStore` — eviction
serializes a persistable memo into it before dropping the value, and a fetch that finds
nothing usable in RAM consults it before executing. The store is `memos.redb` beside the
snapshot (redb, `Durability::None` — a cache, not a database), and it rides the snapshot's
validity decision: deleted unless this process also restored the snapshot and the build
stamp matches, because its keys are positional ingredient indices and its values carry
revision numbers no other epoch can verify. Two hard-won rules (ADR 0011, Decision 5): a
stored memo is reinstalled **only when it shallow-verifies** — deep verification walks edges
that may name ids of non-persisted ingredients restored as untyped placeholder pages, which
is a panic on the full cide corpus, and its economics are re-parsing the substrate the
reload was meant to skip; and **the LSP's idle save runs before GC and the LRU sweep**, so
the snapshot keeps the whole resident set and the store serves only what pressure pushed out
between saves. Measured on this workspace (release, 514 crates): the harness's deliberate
worst case — evict, *then* save — walks warm in 9.0 s with 38k of 87k fetches served from
the store; the LSP's save-first order keeps the millisecond-class walk.

Four follow-up tracks ran in parallel worktrees and merged:

- **Inference does not persist, and now the code says why**: `InferenceResult` is saturated
  with `rustc_type_ir` types interned in a process-global GC arena — not salsa ids — ~25
  mutually recursive external generic types with no serde surface; persisting them is
  rustc's `TyEncoder` rewritten by hand, a project and not a pass (the same wall stops MIR
  bodies, layouts and `TraitImpls`). What landed instead is the bounded half: `lru` caps on
  the six heaviest hir-ty queries (infer 2048, MIR 512/512/256, layouts 1024), which the
  GC tick's existing `collect_ty_garbage` turns into real arena releases.
- **The store expires and compacts**: a save-generation epoch persisted in the store itself,
  `put`s stamped in-transaction, `get` hits batched in memory and flushed at the sweep;
  entries untouched for 12 generations (~1 h at the save cadence) age out, and
  `Database::compact()` shrinks the file — which required learning that redb pins every
  `Durability::None` commit until a durable one lands, so the sweep's commit is the one
  fsync per save.
- **The snapshot and store values moved from JSON to named MessagePack** — the two salsa
  blockers (an unknown-length `serialize_seq`, a borrowed map key) were two lines fixed at
  the source, not worked around. Named mode over compact struct-as-array deliberately: array
  mode silently breaks any type whose Serialize/Deserialize halves disagree in authorship,
  and this codebase is full of hand-written halves. 458 → 384 MB, save 884 → 826 ms, load
  2.2 → 1.9 s, oracle byte-identical; `snapshot.db.json` became `snapshot.db` and format 2's
  orphan is deleted on upgrade.
- **The def-map/item-tree caps are runtime-configurable**: Settings → Inspections → "index
  working set" — one percentage (25–400, 0 = fork defaults) scaling the four compiled caps,
  carried as `cide.diskIndex.lru.{fileItemTree,blockItemTree,crateDefMap,blockDefMap}` in
  the same one-producer-one-consumer contract as the cache dir, applied at host
  construction on both the fresh and restored paths, restarting the server on change. A
  percentage rather than four absolutes so a stored "half the default" keeps meaning that
  across a fork retune.

**Not done / not verified, named:** the **symbol index** deliberately does not persist —
cide never sends `workspace/symbol`, so in cide the index has no consumer; it is also the
hardest layer (`'db` lifetime, `SyntaxNodePtr`s whose byte offsets silently go stale against
changed text), and a payoff-free hard problem is a cut, not a debt. The snapshot is JSON
(salsa's format needs a self-describing serializer with borrowed map keys; a compact format
is a small upstream-shaped salsa patch away). Neither fork has a remote yet:
`packaging/rust-analyzer.lock` still pins the upstream tag, so a package built today ships a
stock-behaving binary under the cide name — the lock moves when the forks get remotes, and
preflight now also demands the salsa fork when the manifest patches to it. hir-def's 500
tests and the roundtrip oracles are green, but no full editing session has run against a
restored index yet, and inference/`hir-ty` memos (the largest remaining recompute on warm
start) are unexplored territory — explored now: it cannot persist (the `rustc_type_ir`
wall above), so its ceiling is caps + GC, not disk. The macOS release job has never built
either fork. The hir-ty caps (unlike the def-map four) are still compile-time constants,
and the 2048 infer cap has not been tested against interactive latency on a large
single-file body — the live soak should watch for infer thrash. MessagePack's −16% says
most snapshot bytes are field names and per-entry structure; a stricter format is reachable
now that the salsa preconditions are fixed, but needs the mixed Serialize/Deserialize pairs
audited first.

## The second bundled server: gopls (M26), and what is not verified

The rust-analyzer road (M25) shaped everything "per-server" on the claim that a bundled
gopls would later cost a row, not a mechanism. M26 is that claim tested: the ladder, the
pre-handshake fallback, the Built-in/System settings map, the restart reaction and the RSS
watchdog all took the second server unchanged. What M26 adds:

- **The row**: `("gopls", "cide-gopls", "CIDE_GOPLS_PATH")` in `discover::BUNDLED`, a
  second Segmented row in Settings → Inspections → "Language server builds", and packaging —
  `packaging/gopls.lock` pins golang/tools at a gopls release tag (v0.23.0, two releases
  ahead of a typical distribution's), `../forks/tools` is the checkout (branch `cide`, a
  fork with no patches *yet*), preflight gains the go-toolchain verdict (absent go fails;
  old go only warns, because `GOTOOLCHAIN=auto` downloads what the module demands), and the
  AppImage/deb/tarball/flatpak all carry `cide-gopls` (built `CGO_ENABLED=0`, static).
  release.yml's copy-pasted fork blocks became a `checkout-fork` composite action — four
  uses, one shape.
- **The env lane**: gopls is configured through its environment, not the handshake —
  `cide_lsp::config::extra_env` is the second half of the provenance-gated contract, and the
  shipped gopls gets `GOPLSCACHE = <profile cache>/gopls`: per-profile, cide names the
  directory, gopls owns the contents and the eviction budget (its file cache self-collects).
  A System gopls keeps its machine-global cache — shared with the user's other editors, not
  cide's to move.

**Phase 1 — the metadata graph persists.** Upstream gopls has kept its *derived*
per-package indexes on disk since v0.12, but the **metadata graph** — the whole-workspace
`go list` result — was recomputed on every start. The measurement gate ran at three scales
and decided the patch: tools (659 packages) and any-sync (867) warm-start fine on upstream's
caches alone, but kubernetes (5,771 packages) still paid **~5.7 s of blocking `go list` on
every warm start** — the filecache halves the heap and does nothing for the wall. So the
fork now carries its one patch: the graph is saved into gopls's own filecache (a
`"metagraph"` kind, inside `GOPLSCACHE`, under gopls's own budget and GC) keyed by the view
definition, the go env, and the contents of every go.mod/go.sum/go.work; a key match
**serves the seeded graph immediately and still runs the real load in the background**,
reconciling changed, deleted and added packages through the ordinary invalidation machinery
— so a `.go` file that moved while gopls was down is caught one `go list` later, off the
critical path, and a touched go.mod is a different key and an honest cold start. Every
failure path converges on the cold load. Measured: kubernetes warm initial load
**5.2 s → 0.495 s** (10.5×, identical package counts, stats-run peak RSS 4.8 GB → 654 MB);
any-sync **850 ms → 86 ms**. One found-the-hard-way rule is documented in the fork:
`TypesSizes` cannot ride through gob (go/types' unexported `gcSizes`), so it is stripped at
encode, reconstructed at decode, and a graph with any *other* sizes is refused outright.

**What is not done:** the in-memory cache caps (the second Phase-1 target) were deliberately
not attempted — the metadata graph was the measured wall. `packaging/gopls.lock` still pins
the upstream tag while the fork now diverges from it, so preflight correctly warns and a
package built today ships *stock* gopls under the cide name — the lock moves when the fork
gets a remote, same as rust-analyzer's. The end-to-end run (`CIDE_GOPLS_PATH=… ./run.sh` on a Go project:
Problems populating, the per-profile cache filling) has not been watched on screen. No
packaged bundle carrying both sidecars has been built with `--run`. The flatpak golang SDK
extension is stated in the manifest but a flatpak build has not been attempted.

## Reformat code — Shift+Alt+F (M26), and what is not verified

cide had no formatting at all. The only appearances of `cargo fmt` in the tree were as *the
antagonist*: it is the worked example in `cide_core::document`'s
`a_stale_precondition_refuses_and_leaves_the_file_alone` and in `EditorPane.tsx` for "a write
nobody typed landing under a dirty buffer". So the user's only route was to leave the editor,
run the formatter in a shell pane and click back — which is exactly the sequence those guards
exist to survive.

**Three roads behind one command id** (two in M26, the third in M32), resolved in
`cmd::format::run`:

- **The language server.** `textDocument/formatting`, which *is* rustfmt against rust-analyzer
  and *is* gofmt against gopls. Both are bundled since M25/M26, so **Rust and Go format out of
  the box with nothing installed**. It also comes free for any server an extension contributes.
- **A user-configured filter**, for the builtin languages with no server — TypeScript, YAML,
  TOML, Markdown, Python, shell, SQL, clike. Settings → Editor → Formatters, keyed
  by language id, and a row **wins** over the server: it is the only lever over a formatter cide
  chose, short of switching the whole server to System, which is a far larger hammer.
- **cide's own JSON reprinter** (M32, `cide_core::format::json`), the floor beneath both:
  reached only when no row exists *and* no registered server claims the language — gated on
  the claim, never on the server's answer, so a future JSON server extension takes over by
  being installed and a server failure never silently swaps formatters. It is a token-level
  reprinter, not a serde round-trip, under one invariant asserted over the whole test corpus:
  `lex(output) == lex(input)` — only whitespace moves. That is what lets it format the files
  the `json` id actually claims (`.jsonc` included): comments stay where they were said,
  duplicate keys, `9007199254740993` and trailing commas survive byte for byte, and a
  JSON-Lines file is refused rather than reflowed into one broken document. Broken JSON is
  refused with a line/column sentence; indentation follows the editor's own
  `tab_size`/`insert_spaces`.

A formatter is a **`Vec<String>`, one box per argument, never a command line** — the rule
`ClaudeCli::args` already states and for its reason: a single string needs a quoting parser cide
would have to invent and would get wrong at the first path with a space in it. The argv reaches
`execvp` directly, so `$HOME`, `*`, `|` and `&&` are argument bytes rather than syntax; only
`${file}` and `${dir}` are substituted. It is run as a **filter** — buffer on stdin, result on
stdout — which is the only shape that can format an *unsaved* buffer, and a tool that rewrites
in place and prints nothing (`cargo fmt` itself, the first thing anyone tries) is refused by
name rather than allowed to empty the buffer.

**Nothing is ever written to disk.** Every road returns text, the buffer takes a transaction, the
tab goes dirty like any other edit, and Ctrl+S is unchanged.

**The four rules that make it not corrupt anything**, each silent if dropped:

1. **`flushDoc` is awaited** before the request. `didChange` is a 300 ms trailing throttle, so
   without it the server formats text up to 300 ms old — and the answer is applied to the buffer,
   so stale input produces a *corrupt* result rather than a stale one. Same argument `savedDoc`
   already makes for `didSave`, one step further.
2. **The buffer is re-checked on reply** (`formatModel.isStillCurrent`). The flush narrows the
   window; a keystroke landing inside what remains would otherwise be discarded silently.
3. **A buffer past 1 MiB is refused**, because that is `cmd::diagnostics::MAX_SYNC_BYTES` — above
   it `didChange` stops being sent and the server holds *the file as last saved*, so formatting
   would throw away every unsaved edit in a large file.
4. **The change is trimmed to what differs** (`formatModel.minimalChange`). A
   `{from: 0, to: doc.length}` replacement is one line and remaps every position in the document:
   the caret jumps, the scroll jumps, every fold and diagnostic is rebuilt, and the undo step
   becomes the whole file.

**The chord, and the bug that changed it.** `alt+shift+f`, `editorFocused` — VS Code's own
chord for Format Document on Windows and macOS. IDEA's Ctrl+Alt+L is not available: it is KDE's
Lock Screen on a stock install and never reaches the app, which `keymap.rs` had already recorded
once at `alt+l`.

It shipped as `ctrl+alt+f`, and **the first person to press it got nothing.** Their
`~/.config/kxkbrc` carries `altwin:swap_lalt_lwin` — an Apple keyboard on Linux — so the key
*labelled* Alt emits `meta`, the stroke arriving at the gate was `ctrl+meta+f`, and nothing was
bound to it.

**Nothing about that is diagnosable from inside the app**, and that is the part worth
remembering rather than the chord: the gate correctly matches no binding, passes the event
through, and says nothing — which is indistinguishable from the feature being broken, and was
reported as exactly that. Separating "not wired" from "not typable" took `cide-headless keymap`
(the binding is there), a harness driving the real gate against the real table (it resolves and
dispatches), zero `cide::ui` lines in the log (the handler was never reached), and finally
reading `kxkbrc`. The palette row was what proved the feature itself had worked all along.
**A "no binding matched this chord" trace would have made it a one-minute answer, and there
isn't one.**

One thing the audit turned up and left standing: `Alt+Shift+F` appears in this machine's
`kglobalshortcutsrc` as `1-capture-full-page=none,Alt+Shift+F,Capture > Full Page`. That format
is `active,default,description` and the **active field is `none`**, so the chord is free — it is
a *latent* grab, not a present one. Enabling that capture would take the key globally and cide
would never see it. `alt+shift+s` (`scratch.new`) has sat beside the same entry for Capture >
Selected Area since M15 on exactly this reasoning.

The `editorFocused` clause is not optional: the gate is a window capture listener, so an
unscoped chord is swallowed in every terminal pane in every window. On macOS the chord carries
no Ctrl, so `platform_layer` rewrites nothing and it stays ⇧⌥F — which is what VS Code binds
Format Document to there.

**What the servers actually answer, measured rather than assumed.**
`rust_analyzer_formats_a_misformatted_file` and `gopls_formats_a_misformatted_file` print the
capability pair and assert the formatted bytes. Against the stock `PATH` builds:

```text
rust-analyzer: documentFormattingProvider=Some(true) documentRangeFormattingProvider=Some(false)
gopls:         documentFormattingProvider=Some(true) documentRangeFormattingProvider=Some(false)
```

So **neither shipped server offers range formatting, and the whole-document fallback is what runs
today, always** — Shift+Alt+F with a selection formats the file. The range branch is still there
and is still probed, because rust-analyzer gates its own behind an `initializationOptions` key
that only a bundled build receives, and a contributed server may answer `true` outright; the
probe is what lets either light it up with no code change. gopls' reply is also the strongest
check `convert::apply_text_edits` has: it sends several small edits rather than one replacement,
so the test reproducing gofmt's output byte for byte is what proves they are not shifted against
each other.

**What is not verified.** None of it has been watched on screen — `./run.sh` was not driven, so
the caret-and-scroll preservation, the settings editor, the context-menu row and the notice text
are all read rather than seen. Only the **stock** provenance was exercised: no bundled sidecar
was built, so whether the forks answer the same capability pair is untested, and the
`CIDE_RA_PATH`/`CIDE_GOPLS_PATH` road was not run. No configured filter has been driven through
the real UI — `prettier` end to end is only covered by unit tests standing in for it (`tr`,
`cat`, `true`, `sh`). CRLF is reasoned about (the formatter never sees `\r`; `restoreLineEndings`
runs only on save) and not tested. `check:css-prefix` fails on `EditorSurface.module.css:954` at
this commit and did so before this work — it is not from this feature.

## Four reports from a day's use: the rail, the diff, the ticks (M31), and what is not verified

Four things reported together from using cide on itself, which turned out to share nothing except
the session they were noticed in. Three were bugs with a wrong line in them; one was a default
that had been argued for at length and was wrong anyway.

**The activity rail clips, and it is not an extensions problem.** `.rail` is a flex column of
`flex: none` items — a shrunk button breaks the measurements `chrome/layoutAudit.ts` asserts, and
the stylesheet says so — inside an `overflow: hidden` shell, so an overfull rail clips and a
clipped rail button is *unreachable*. That is worse here than on the tab strip, because the button
is the only route into a contributed panel: there is no command for an `ext:` view. The
arithmetic is the part worth recording, because it says the report was mis-framed. Ten builtin
buttons want `10×32 + 9×4 + 16 = 372px`; the smallest window cide will open is `MIN_HEIGHT` 430,
less a 38px header and a 26px status bar, leaving **366**. So stock cide is already six pixels
overfull at its own minimum window size with nothing installed, and `--ui-scale` above 1 makes it
worse rather than better — the header and status bar scale, the 32px button and the 4px gap do
not. Capping extensions would have "fixed" the report and left the bug. `chrome/railOverflow.ts`
is the rule, an import-free module in `tabOverflow.ts`'s shape with `check:rail-overflow`
compiling and running it standalone, and the one thing it does that reads like a redundancy is
count the `···` control's own slot *inside* the fit rather than adding it afterwards — the latter
computes room for exactly the buttons that fit and then draws one more thing, which is the
original bug with an extra step.

The rail takes counts and two CSS lengths, where `clippedTabs` takes measured boxes. That is not
inconsistency: a tab's width depends on its filename and its dirty dot, so measurement is the only
honest input there, while a rail button is 32px whatever is in it — and taking boxes here would
invite a feedback loop, because hiding a button changes the boxes that decide what to hide. The
lengths are still read off the DOM rather than written as `32` and `4`, so retuning the stylesheet
cannot leave the rail miscounting by a slot on short windows only.

**Hiding an extension's icon is a setting, and it deliberately is not `enabled`.** `"i doesn't
like to stuck there all extensions (especially language like YAML, Proto, etc)"` — and those two
really do declare `location: "sidebar"` panels with icons, so cide cannot classify them: there is
no signal separating a language's outline from any other contributed panel, and the user has to be
able to say. `Installed::rail_icon` is the stored flag and `ext_set_rail_icon` the command, kept
apart from `enabled` because disabling stops the worker, drops the languages and silences the
diagnostics — somebody who wants YAML off the rail still wants YAML highlighted, and
`marketplace.rs` asserts exactly that (the panel is still resolved, the assets are still served).
The second decision is the one that took a rewrite: a hidden panel is **still listed under `···`**.
Filtering it out of both the strip and the menu was the obvious reading of "hide" and it makes the
setting a one-way door — the only route into the panel is gone, and the way back is a Settings
page nobody would connect with a panel that vanished. So `···` means *panels not in the strip*,
for both reasons, and the check pins the complement so no panel can fall into neither list.

**A diff opened at line 1**, by omission rather than decision: `currentChange` starts at `-1` and
the view's scroll effect bails on a null anchor, so the machinery to centre the first change was
all present and nothing ever asked it to. `changeAnchors` skips `shared` runs, so `anchors[0]` is
the first real difference. The seed is one line in each of the two wiring components and the
interesting part is what it must *not* touch: the refetch path resets to `-1` on purpose, and its
comment — *a scroll nobody asked for is worse than a stepper that starts again* — is right.
Opening and being-interrupted are opposite cases and now get opposite answers, discriminated by
`revRef.current === null`, which the pane already kept. `check:diff-render` pins the pair against
source, because collapsing them either way is silent: one direction puts every diff back at line
1, the other drags the reader off the hunk they were reading every time an agent touches the file.

**"git tree commits everything when commiting."** The commit path was never the problem and it is
worth saying so before somebody fixes it: `commitUnits` splits ticked ids per repo,
`commitSelections` turns each into a `PathSelection`, and `cide_git::commit` rebuilds the index
from exactly those, with `tests/staging.rs` pinning it. Three separate faults upstream of it, in
the order they bite. The ticks were a plain `useState` re-seeded on every mount while the tree
beside them came from a module-level cache — and the panel unmounts on **every rail switch** — so
glancing at Files and coming back rebuilt the selection from `defaultSelection`. Second, and this
is the one that matches the report's wording exactly: `adopt` pruned the ticks against *every*
payload, including the `EMPTY` one `refresh` adopts with `authoritative: false` when the status
walk fails. An `index.lock` held by a bash pane is enough; `pruneSelection([], prev)` reads it as
*every file is gone* and empties the set, and the next good payload re-ticked the whole active
changelist through `arrivals`. The guard already existed and was applied to `pruneTo` and to the
row selection and not to the third thing pruned beside them — three pieces of user state, one
payload, two protected. Third, the policy: `defaultSelection` ticked the active changelist, so
every commit was an act of *unticking*, and the cost of missing one is a commit carrying work
somebody had not finished.

The default is gone rather than switched off, and `arrivals` no longer says anything about ticks —
a file arriving mid-commit adding itself to that commit is the same failure one file at a time, and
harder to notice. The old comment worried that an empty panel is "nothing to commit and no clue
why"; the footer already renders `nothing selected` and Commit is already disabled, so the answer
was on screen the whole time. `check:git` asserts the export stays **gone**, because reinstating
it silently restores the reported behaviour and nothing else in the suite would see it. The
stories that needed ticked rows to exercise the tri-state kept them — the rule moved into
`fixture.ts`, where "the active changelist is ticked" is a fair description of a fixture and a bad
description of a product default.

**Committing with a diff open showed `[object Object]`.** A `GitError` is
`#[serde(tag, content)]`, so a rejection is an object with no `message`, and the pane's
`String(e)` printed nothing at all under a heading that said *"Nothing to show on this side."* —
on the commonest path there is, since committing a file whose diff is open is exactly what
produces `NoSuchChange`. `client.ts` names this hazard in prose and `branchModel::explain` is the
sanctioned unwrap; the pane had simply never used it, at four catch sites, one of which *knew*
about the tagged shape and stringified `detail`, which is itself an object for both refusals it
can actually get. `explain`'s own wording for `noSuchChange` is written for the staging caller —
a race, and a failure — so the diff pane special-cases the one tag it has better context for. The
digest now exposes the rendered `reason`, which nothing drove before, and every story is scanned
for `[object Object]`.

**The lag has one confirmed cause and one measured-but-unfixed one.** Every open `EditorPane`
subscribed to `cide://git-status` and did a full `fileApi.read` on each — and `TabContent` keeps
every tab of the project mounted, which is what makes tab switching free. The old comment said "a
read is cheap", which is true of one read and was the wrong unit: the cost is *(open editors) ×
(mutations per commit)* round trips, each carrying a whole file body. Both bounds are borrowed
rather than invented — a **throttle** and not a restarting debounce, on `gitStatusStore.ts`'s
argument that a debounce can be starved by a steady stream of writes, and a hidden tab defers to
its next reveal, which is `GitDiffPane`'s rule and the reason `onScreen` exists on the component
at all. Correctness is unchanged: still an unconditional read-and-compare on every git mutation,
just once per burst and only where somebody can see it. Separately, `setCollapsed(new Set(...))`
ran on every fetch including the many that return byte-identical diffs, which both re-collapsed
hunks the reader had opened and churned `model` identity into a full re-measure — three geometry
effects each walking every row for `offsetTop`, a forced synchronous layout over thousands of
nodes. It is now gated on the diff having actually moved.

**What is not verified.** None of it has been seen on a display. `./run.sh` was not driven, so
the `···` control has never been clicked, the overflow menu has never been opened, no window was
resized to the point of clipping, no extension icon was hidden through the real Settings page, no
diff was watched scrolling to its first change, and no commit was made from the panel. Every
claim here rests on the check suite, the Rust tests and reading. The lag work in particular is
**unmeasured**: the fan-out was counted by reading the subscribers, not by instrumenting a real
commit, so how much of *"cide starts to lag"* the editor throttle actually removes is unknown —
the plan called for measuring first and that step was not performed. The multi-repo half of the
broadcast is untouched: a commit still emits one whole-tree `cide://git-status` per repository, and
coalescing those was scoped out rather than done. `RevisionDiffPane` keeps the ungated
`setCollapsed`, because it has no rev to compare and already treats every refetch as a new diff.
And `use_staging_area`, or a repo mid-merge, still makes `cide_git::commit` commit the whole index
and ignore the ticks entirely with nothing in the panel saying so — a real trap, found while
reading this, and left alone as a different report.

## The look and feel (M23), and what is not verified

The report was "look and feel is bad — font style, font size, icons, across all components", with
UX explicitly out of scope. The diagnosis was not an absent design system: `tokens.css` is 649
lines of argued custom properties with documented AA ratios per token. It was that the system had
been transcribed from a mock that was small, sharp-cornered and motionless — plus one area that
was genuinely broken.

**Icons were the broken one.** There were three unrelated systems: 190 vendored Material file
icons drawn as `<img>`; **three** hand-written inline SVGs on three different grids, whose strokes
rendered at 1.67px, 1.08px and 1.0px; and **about a hundred and thirty Unicode characters** across
forty files. `⑂` (U+2442) shipped in the branch indicator despite a note two files away recording
that no UI font carries it; `↻` meant two different actions in one git toolbar; `×` and `✕` both
meant close inside `PaneTitleBar.tsx`. `chrome/ActivityRail.tsx` had already measured why this can
never be tuned away — a glyph's ink is a property of whichever face fontconfig picked, and seven
rail glyphs spanned 0.53em to 0.82em — and had fixed it for the rail alone, stating "this
application bundles no UI icon set" as policy. That policy is reversed here, in the three comments
that carried it.

The set is **Lucide, ISC, vendored as data** by `ui/scripts/vendor-ui-icons.mjs` at a pinned
revision, mirroring `vendor-icons.mjs`. No runtime dependency: it emits `src/icons/iconPaths.ts`,
one 24×24 path `d` per mark. Lucide's envelope is already what `RailIcon` drew, attribute for
attribute, so the rail did not move.

**Every icon is flattened to a single `d`, and that is the load-bearing decision.** An extension
contributes a rail icon as one path string validated by `cide_ext::manifest::is_svg_path`; if the
built-in set were element lists, `<Icon>` would need two render paths and the one exercised only
by untrusted input would be the least-tested. The vendor script asserts upstream's envelope on
every file and refuses any icon with a filled child. The conversion was verified rather than
assumed: **all 71 flattened marks were rasterised at 192×192 and compared pixel-for-pixel against
upstream's own multi-element SVG.** One bug was caught that way — Lucide paths often open with a
relative `m`, which SVG treats as absolute only while it is the first command in its own `d`;
concatenated after another subpath it silently displaces the mark, and `arrow-down` came out with
its head 12px off.

The rest of the sweep, in one commit:

- **The type ladder collapsed from fourteen rungs to six** (see the section below).
- **Radius**: 186 declarations across twelve ad-hoc values → five rungs. The 3/4/5px trio that
  produced the sharp, unresolved look became 4/6/8, with 12px for modals and menus. `50%` stayed
  literal: eight real circles, and "half of whatever this box is" is not a length.
- **Spacing**: an eleven-value set with no rhythm → a seven-rung scale, unscaled, because
  `--ui-scale` moves text and the boxes around text and not gutters.
- **Elevation**: one `--shadow` read by fifteen rules — a drag ghost and a modal lifting off the
  page by the same amount — became three heights.
- **Motion**: the app had **six** transitions in sixty-two stylesheets. It has eighty-odd, all
  reading `--dur-1`/`--dur-2` so one media query answers `prefers-reduced-motion` for all of them.
- **Focus**: 64 rules spelling `outline: 1px solid var(--accent)` became two tokens — an outward
  2px ring, and an inset `box-shadow` for the strips that `overflow: hidden` would clip.
- **The six identical sidebar panel headers** — 10.5px UPPERCASE at 0.09em — became 12px sentence
  case, along with nineteen more sites of the same idiom.
- **`CommitBox`'s 104px textarea** stopped being permanently accent-outlined; the accent moved to
  `:focus`.

**Two new gates**, because nothing in the repository watched either family. `check:ui-icons`
asserts the set is exactly what the app draws, that every mark is a legal extension icon, that
each size preset's rendered stroke lands in 1.4–1.8px, and that no Unicode symbol is still being
drawn outside a per-file allowlist with a stated reason — implemented over the TypeScript AST so
that the hundreds of comments quoting the old glyphs are invisible to it. `check:motion` bans
`transition: all`, bans every non-paint property, fences off the four files where a transition
would break the splitter drag or `repaintHost`'s 1px nudge, and requires exactly one
`prefers-reduced-motion` block.

**What is not verified.** Every gate passes, including the full `check:*` suite, `tsc`, the Rust
workspace and a production build. **`./run.sh --audit-chrome` and `--audit-panes` have not been
re-run** — both need a display this environment does not have. The audit's 48 expectations were
re-derived from the stylesheets they measure rather than from a run, so the first person with a
screen should run both and record the result. Colour and mark fidelity still want a human eye, and
so does the one deliberate density change nothing can check: the tab strip, git toolbar, git log
rows and git segmented control all grew, and whether that reads as comfortable or as loose is not
a thing a script can answer.

## The chrome font size (M19)

The app had two font-size settings and both were for code — the editor's buffer and the
terminal's cell. Everything else was a pixel literal: **405 `font-size` declarations across 51
stylesheets**, with no `em`, no `rem`, no `%` and no `inherit` anywhere in `ui/src`. The only
base was `html, body { font-size: 13px }` in `tokens.css`, and almost nothing inherited from
it. So there was no token to redirect — a setting for the file tree, the git panel, Problems,
the log, the tabs and the menus could only be built by rewriting all 405 onto one.

`Settings → Appearance → UI font size` is that setting. It stores a point size (default **13**,
band **9–20**, half-pixel steps) and everything else is derived from one unitless multiplier,
`--ui-scale`, written on `<html>`.

**Why a multiplier and not a size.** There is no single chrome size in the design. The *ratios*
between the sizes are the design — a tab's label over its path is a hierarchy, and the chrome
audit below measures several of those numbers directly. One multiplier moves them all and keeps
every ratio. `tokens.css` carries the ladder, each rung `calc(<design>px * var(--ui-scale))`, and
the number in the token's name is the design size rather than the painted one.

**What scales and what does not.** Text, and the boxes drawn around text: row heights, the
header, tab strip, status bar, pane title and find bar tokens, and pixel line-heights. Not
icons — `--h-rail` and the `--icon-*` boxes stay put, because an SVG does not grow with a type
scale. Not borders. Not the sidebar widths or the tool window height, which are dragged by the
user and persisted, and would fight the saved value.

**The three things that CSS could not reach**, each of which would have been a silent half-fix:

- **Seven virtualized lists** — the file tree, the search panel and five pickers — take their
  row height as a number handed to `estimateSize`, which places rows with an absolute transform.
  CSS never sees it. Left alone they would have kept 24px rows under grown text, clipping the
  names and making the scrollbar lie about the list's length. `settings/useUiScale.ts` is the
  hook they read.
- **The tool window's ceiling** subtracts the header, tab strip and status bar to decide how
  much room the panes keep. Those three now scale, so a flat sum would let the panel take 22px
  it does not have at 17px — out of `MIN_PANES`, on tall windows only.
- **`--h-findbar`** is derived from the bar's own parts, and three of the four scale while the
  border does not. It is `calc(32px * var(--ui-scale) + 1px)` for that reason; written as
  `33px * var(--ui-scale)` it was right at the default and *short of its own parts* below it,
  which clips the bar — the one failure that derivation exists to prevent.

**The first frame.** The size rides in on the URL as `&ui=`, baked in by `windows.rs` and read
by `public/theme-boot.js` in `<head>`, exactly as `&theme=` is. Not polish: `installThemeSync`
reads the size out of the bootstrap snapshot, and that is a round trip — the same round trip
that is too slow for the theme. A late theme is a flash of the wrong palette; a late size is a
reflow of every row, tab and panel in the window, on every launch at any size but the default.

**`check:ui-scale` is the fence around the sweep.** A rewrite of 405 declarations is not the
risky part; the next stylesheet is — one `font-size: 12px` written in good faith, a label that
silently stops following the setting, and nothing looking wrong at the default, which is where
every author works. The gate asserts that no bare pixel font size survives outside the code
surfaces, that every `--ui-scale` `calc()` is written literal-first (five other check scripts
read design numbers straight out of the stylesheets), that the ladder and its readers are the
same set in both directions, and that the four copies of the base size — Rust, `fontScale.ts`,
`tokens.css`, `theme-boot.js` — agree. They are four because none can import another.

**The ladder was fourteen rungs and is now six (M23).** It ran from 8px to 20px in half-pixel
steps, transcribed from the mock, and this section used to argue that all fourteen were the
design. Two things retired that argument. The mock is gone — it was never in this repository, and
`chrome/layoutAudit.ts` held the only surviving copy — so "the mock draws fourteen sizes" stopped
being a reason for anything. And fourteen sizes is not a hierarchy: 76 declarations sat at 10.5px
and 135 at or below it, adjacent surfaces differed by half a pixel, and no reader perceives that
as rank. The six are 11 (micro), 12 (secondary), 13 (body), 14 (emphasis), 16 (section) and 20
(display).

One mapping in that collapse was **forced rather than chosen** and must not be "simplified" back:
the old 11.5 went to 12 and the old 12.5 to 13, keeping them one rung apart, because
`check-theme.mjs` asserts a usage-file heading is strictly larger than the source lines under it.
Sending both to 12 fails that gate, and its failure message is the user's own words from when it
last shipped that way — *"it doesn't see where is filename and where is code"*.

**The editor and the terminal are untouched.** `--fs-code`, `--lh-code`, `--fs-term` and
`--term-line-height` ship as flat literals with no `--ui-scale` term, and `.cm-scroller` reads
only `--fs-code`. Two declarations moved the other way while the sweep was in the file: the
gutter's line numbers and its lint marker were literal `11px`, so they had never grown with
`editor.fontSize` either. They are `calc(var(--fs-code) * 0.88)` now — the same ratio, applied
at every editor size.

**What is not verified.** The chrome side was checked in the running app at 13 and at 18 against
an identical layout. The editor and terminal staying put was checked in the shipped CSS bundle
and by the gates, not on screen: switching tabs needs input injection this desktop has no tool
for. The chrome audit below has not been re-run to a clean result since the sweep.

## The chrome audit

M3's stated acceptance criterion was a screenshot diff against the design mock at 1440x900 in
both themes. Neither side of that diff was ever obtainable here — the mock is a template needing
a runtime this repo does not have, and KDE Wayland will not raise a shell-launched window for a
capture tool. What survives is the geometry, so the check became data:

```
CIDE_AUDIT=1 ./target/debug/cide
```

It resizes to a viewport of exactly 1440x900 (correcting for the compositor's invisible
window border, which is 52px on this desktop), renders a fixture covering every chrome state
a live app would not show on its own, and measures 48 dimensions in each theme. An element that
is missing reports as a **failure**, not a skip — a check that silently disappears is
indistinguishable from one that passes.

**Those 48 numbers are no longer the mock's, and as of M23 they are not a transcription of
anything.** The mock was never in this repository; `chrome/layoutAudit.ts` was the only
surviving copy of it, and the redesign redrew about twenty of its rows deliberately — the chrome
it specified was small, sharp-cornered and motionless. Every changed row was edited even where
the ±2px tolerance would have hidden the change, which is roughly half of them: a stale
`expected` turns the file from a specification into a rubber stamp, and it is the only tool in
the repository that can see these numbers at all.

Current result at the time of writing: **not re-run since M23.** The gates below all pass, the
values in the table were derived from the stylesheets they measure, and the audit needs a
display this environment does not have. Run `./run.sh --audit-chrome` and record what it says.

The half that is not automated is colour and mark fidelity, which still wants a human eye.

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
                   Log, graph lanes, history, blame, and the commit actions.     (M19)
  cide-fs/         Gitignore-aware indexing and watching.                        (M8)
  cide-search/     Fuzzy pickers behind a Matcher trait.                         (M8)
  cide-lang/       tree-sitter: what a Rust or Go file declares.                 (M12)
  cide-lsp/        An LSP *client*: rust-analyzer and gopls.                     (M12)
  cide-deps/       What a project depends on, and where its source is unpacked.   (M13)
  cide-hook/       Second binary: bridges a Claude hook to the running IDE.      (M7)
  cide-headless/   Third binary: proves the core links without tauri.
                   `tree|commands|keymap|tasks|agents`.
ui/                React 19 + Vite 8 frontend. One document per window.
docs/adr/          Decisions that would otherwise be refactored away.
```

`cide-headless` is load-bearing architecture, not a demo: it links `cide-core`, `cide-ipc`,
`cide-pty`, `cide-tasks` and `cide-agents`, and must never be able to link `tauri`. If domain
logic leaks into the app crate, it stops building — cheaper than a code-review convention. Every
crate it links is a crate the rule is enforced on, which is why M18's two were added to it rather
than left to a reviewer's memory.

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
