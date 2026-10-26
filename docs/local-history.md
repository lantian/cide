# Local History

Local History retains observed disk versions of eligible project files, including files
changed by agents and external tools without an editor tab. It works in projects without
Git. Git file history remains a separate action and reads repository commits as before.

Use **Local History → Show History** in a file-tree or editor-tab menu, or run
**Local History: Show history for this file** from the command palette. Run
**Local History: Browse project history** to search captured paths, including deleted files.
Reopening the same root/path activates its existing view. The project browser selects a
root, filters paths and offers **Deleted files only**. Lists load bounded pages; revisions
are newest first and show observation time, known source, identity and lifecycle/rename
information.

## Storage and identity

History is machine-local application state at:

```text
persist::state_dir()/local-history/<canonical-root-BLAKE3>/
  root.json
  owner.lock
  records/<sequence>-<revision>.json
  blobs/<content-BLAKE3>
```

`state_dir()` is `$XDG_STATE_HOME/cide`, otherwise `~/.local/state/cide`. A named application
profile uses `cide-<profile>` instead. History lives outside workspace layout JSON, browser
storage and project files. Its files and directories use owner-only access on Unix.
Original file bytes, including CRLF, are stored unchanged. Identical contents can share a
blob; a watcher echo of the latest contents does not create a duplicate revision or change
that revision's source.

The canonical physical root identifies the store. Closing tabs or projects, changing
transient project IDs, switching branches in one worktree, and restarting cide preserve
retained history. Another worktree directory has a different store. Nested open roots use
the deepest matching root. In-root symlink aliases resolve to a single physical file
identity; out-of-root targets are excluded. Readable history already retained by a parent
root remains browseable after a nested root is opened, but restore must use the current
owning root.

There is one ordered authority per root shared by cide windows, plus an exclusive store
ownership lock. A second process unable to acquire that lock reports history unavailable
and continues ordinary editing. Rapid close/reopen waits for the retiring authority and
admitted writes before the new background loader acquires ownership.

## Defaults, eligibility and controls

| Setting | Default / rule |
| --- | --- |
| Capture | Enabled for open roots |
| Retention | Seven elapsed days, subject to budget |
| Root budget | 256 MiB of committed content and metadata |
| File limit | 2 MiB, fixed for this release |
| File contents | Regular UTF-8 text without NUL bytes |
| Root policy | Machine-local; shared by that root's windows |

Eligibility is independent of hidden-file visibility in the tree. Git ignore rules are
shared with the filesystem policy, including worktree common `info/exclude`. Default
exclusions include `.git`, `node_modules`, `vendor`, `.venv`, `venv`, `__pycache__`,
`.pnpm-store`, `.env`, `.env.*`, and Local History's storage. User exclusions are
root-relative globs, for example `private/**` or `**/*.secret`. Direct cide writes follow
the same policy. Scratch and out-of-project files are outside root history.

Open **Settings** in the history toolbar to change enablement, retention, budget and
exclusions, inspect used space, or clear this root. Retention must be a positive whole
number of days; the budget must resolve to a positive integer number of bytes within the
supported 32-bit limit. Invalid or absolute/traversing exclusion globs are refused.

Disabling stops new capture and invalidates queued work while retaining existing revisions.
Re-enabling starts a new baseline. Adding an exclusion stops future capture; already
retained content remains until expiry or explicit clear. Cleanup evicts the oldest unpinned
revisions and reclaims unreferenced blobs. Even an unchanged file's last baseline may
expire; no version is promised indefinitely. A budget too small for required metadata or
pinned data produces a visible capture gap.

**Clear history** requires confirmation, preserves root policy and does not change project
files. It durably advances the generation before removal, invalidating open comparisons and
old queued jobs across windows. With capture enabled, a fresh baseline immediately starts
recording current files. Disable capture first if the intended result is an empty store.

## Capture boundaries

The initial baseline runs asynchronously and establishes each file's earliest observed
contents. A file changed or deleted before its first read has no guaranteed earlier version.
The observer subscribes independently of tree visibility and discovers eligible directories
as the baseline walks them. Progress and degraded coverage appear in the history view.

cide text saves and applicable tree writes capture the prior state, including absence on
creation, and successful post-state. Refused autosaves and failed writes do not record the
requested contents as successfully written. Capture failures report degraded coverage while
preserving ordinary save and file-operation outcomes. Explicit restore has stricter safety
requirements.

Known cide saves, file operations and restores have their own source labels. Baseline,
reopen and external observations are identified separately. External agent writes are
labelled **External observation**; filesystem evidence alone does not establish which agent
or person wrote them.

External writes can coalesce between observations. Watcher overflow, queue saturation,
truncated events and failed reads can leave gaps; detected gaps schedule bounded
reconciliation and show a notice. Reopening observes changed contents or absence and
reports the intervening closed period. It cannot reconstruct intermediate offline edits.
Unsaved editor keystrokes are not captured.

cide moves within a root explicitly preserve lineage and prior paths, including eligible
descendants of directories and overwrite destinations. A cross-root move is deletion and
creation in separate stores. An external rename without reliable identity evidence is also
deletion and creation. Reusing a deleted path starts a new lifecycle. An observed absence
retains earlier captured contents subject to retention.

## Comparing and restoring

Select a revision to compare it with the current disk file or another retained revision.
The comparison is read-only, labels both sides and their observation times, and shows
absence or unavailable content explicitly. Large eligible text uses bounded diff work and
virtualized documents. Selecting revisions never writes a file or dirties a buffer.

Restoring replaces the whole file and requires explicit confirmation naming the path and
revision. To restore, capture must be enabled and the path must remain eligible in its open
owning root. The parent must exist for recreation. The backend checks permissions, root
containment, symlink destination, lifecycle, store generation and current disk digest or
absence. A stale preview requires a fresh comparison and confirmation.

Every mounted editor for the canonical target, including detached windows and drawings,
must acknowledge preparation with its live dirty state. Preparation freezes edits and saves.
Dirty editors, missing acknowledgements, unready editors or window/mount/layout changes
abort; resolve or close the relevant editor and retry. No force-restore path bypasses these
guards, and unsaved text is not copied into history.

Before mutation, the backend verifies and pins a durable revision of the current contents
or absence. Failed safety capture leaves the file untouched. It rechecks disk state under
the cide write/editor gates immediately before atomic write or recreation. Restoring an
absence revision deletes the file under the same guards. A completed restore refreshes
clean editors and invalidates lists in other windows; the pre-restore state can be selected
to reverse the operation, while it remains retained. A drawing reload replaces its engine
so undo cannot resurrect the discarded scene.

If the file mutation succeeds but post-state recording fails, the result says the file was
restored and history is degraded. The safety revision remains pinned for reconciliation.
All exits release preparation gates. Ordinary filesystems provide no universal
compare-and-swap against arbitrary external programs: an external writer can still race the
last check. Local History is observational local recovery storage, not a backup.

## Recovery limits

Records are committed individually after their blobs are durable. Startup rebuilds indexes
in the background, removes interrupted temporary/orphaned data, skips corrupt records and
marks missing or corrupt contents unavailable without invalidating unrelated readable
revisions. Restore refuses unavailable targets and safety records. Unsupported schemas,
invalid root metadata/identity, unwritable storage and ownership conflicts make that root's
history unavailable while the project and ordinary editing remain usable.

There is no reconstruction of lost blobs, automatic import of older pre-capture contents,
directory-wide rollback, partial-hunk restore or cloud synchronization. Preserve damaged
store data for diagnosis rather than treating a clear as a recovery operation. After fixing
a storage or competing-owner problem, reopen the root to retry loading. Retained source
contents may be sensitive despite the default exclusions and access modes; use root
exclusions, disablement and clear according to the intended retention policy.

## Reviewer acceptance

Use disposable roots and files. The automated results and remaining native-delivery check
are recorded in [local-history-verification.md](local-history-verification.md).

1. Open a non-Git root with a text file and a hidden source file, with hidden tree rows off.
   Wait for baseline completion. Use both context menus and palette commands; reopening a
   view should activate the existing view. Check keyboard selection and paginated lists.
   Confirm existing Git history still dispatches separately in a Git project.
2. Change an unopened captured file from a terminal or agent, allowing separate observation
   intervals. Find both versions and **External observation** provenance. Repeat for the
   hidden file, then delete it externally and find it through **Deleted files only**.
   Test rapid writes without expecting every intermediate version.
3. Check ignored output, `.env.local`, binary/NUL content, text larger than 2 MiB and an
   out-of-root symlink. They must be excluded with reasons. Add an exclusion for a previously
   captured file: retained versions should remain visible and future capture should stop.
4. In cide, save, overwrite, rename, move and delete files and directories containing eligible
   descendants. Compare recoverable pre-states and inspect rename lineage. Recreate a deleted
   path and distinguish its lifecycle. Move between roots and inspect independent stores.
5. Close/reopen the project and restart cide. Compare exact CRLF bytes after restore. Change
   a file while the root is closed; reopening should show a coverage notice and the next
   observed version. Open another worktree with the same relative filename and confirm
   isolation. Rapidly close/reopen a root while a save completes.
6. Compare retained/current and retained/retained versions, including absence. Switch files
   and revisions quickly and clear from another window while a comparison is pending; stale
   responses must not revive selections or enable restore for a different preview.
7. Restore over a clean file open in the shell and a detached window. Both should refresh.
   Make either editor dirty, including a parked drawing; restore must refuse without losing
   the buffer. Close or replace a participating window during preparation and retry. Check
   release of edit/save gates after refusal and success, including drawing undo behavior.
8. Change disk contents after preview, including a same-length change, and confirm restore
   refuses until refreshed. Restore a deleted file with an existing parent, reverse it using
   its absence revision, then remove its parent and check recreation is refused. Retarget a
   symlink after preview and check refusal.
9. Validate positive settings and malformed globs. Disable and re-enable; confirm retention
   and baseline behavior. Clear from one window and observe invalidation in another. Check
   the confirmation explains fresh capture if enabled. In an isolated profile, constrain
   the budget and check visible degradation with ordinary editing still available.

### Display-review handoff

Implementation and automated verification are complete; native-window acceptance and visual
sign-off are pending reviewer execution. Do these on a normal desktop with a disposable
project/profile. This sandbox has no display/audio and refuses Unix sockets. No GUI app,
virtual display, screenshot tool or renderer was launched here.

- Repeat the native linked-folder watcher test listed in the verification record outside
  the sandbox, then confirm actual external edits to unopened hidden files reach Local
  History. The existing failure also occurs on the pre-change commit; record the desktop
  result separately from the socket-free observer fixtures.
- Execute the acceptance sequence above in shell and detached windows, especially a live
  dirty buffer, close-during-prepare, stale disk refusal and clean reload after restoration.
  Verify keyboard access to the root selector, filters, revisions, comparison selector,
  settings and confirmation dialogs; Cancel must remain the safe initial confirmation focus.
- Review dark and light themes at UI sizes 11, 13 and 17, plus a narrow window/bottom panel.
  Check source/time labels, long paths, lifecycle text, absent/unavailable sides, large-file
  scrolling, focus indicators, resizing and dialog overflow without obscured actions.
- Capture the non-Git context menu, populated project/deleted-path browser, read-only diff
  and absence comparison, settings/clear confirmation, and dirty/stale restore errors in
  both themes. Include baseline/degraded/empty states where useful. Inspect captures and any
  documentation renders for clipping, contrast and stale contents; attach the resulting
  artifacts and observed outcomes to the review using the reviewer's normal workflow.

This handoff records the remaining acceptance work, not a claim that screenshots, renders
or native multi-window behavior have been verified.
