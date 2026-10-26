## 1. History types and policy

- [x] 1.1 Add local-history DTOs for root/revision identity, sources, absence/lifecycle records, pages, comparisons, coverage, validated policy and restore results; give added workspace/view fields backward-compatible defaults.
- [x] 1.2 Implement canonical-root/path identity, deepest-root selection, symlink containment, text/size checks, gitignore and default/user exclusions independently of tree visibility; cover non-Git, hidden files, nested roots and separate worktrees with fixtures.

## 2. Durable store

- [x] 2.1 Implement profile-local store paths, schema metadata, root ownership locking, immutable BLAKE3 blobs/revision records and background index loading with cursor pagination; verify restart persistence and byte-exact CRLF recovery.
- [x] 2.2 Implement consecutive-content deduplication, absence markers, lifecycle/rename records and independent-record recovery; cover interrupted blob/record commits, missing blobs, corrupt records and unsupported schema handling.
- [x] 2.3 Implement age/budget accounting, oldest-unpinned eviction and blob GC, restore pins, policy persistence, disable and generation-safe clear; test shared blobs, small budgets, queued work after clear and capture resumption.

## 3. Observation and capture integration

- [x] 3.1 Add the per-root capture coordinator, bounded queue, baseline/reopen scans, close cancellation, generation checks and degraded-status reporting; test baseline races, overflow/truncation reconciliation and closed-period coverage.
- [x] 3.2 Extend the existing filesystem observer with a history-policy subscription and watched-directory union independent of tree visibility; preserve existing tree filtering and avoid ignored/dependency watch expansion. Test external edits to unopened hidden files and external deletion.
- [x] 3.3 Wire editor text saves, autosave, tree text writes/create/overwrite and applicable text writes through the bytes/IDE-diff routes to pre/post capture. Verify refused/failed writes, watcher echo deduplication and successful ordinary saves when history storage fails.
- [x] 3.4 Integrate tree rename/move/delete, including eligible descendants of directories and overwrite destinations; preserve within-root lineage, represent cross-root moves separately and distinguish deletion/recreation lifecycles.

## 4. IPC and restore safety

- [x] 4.1 Add backend history status/path/list/compare/policy/clear commands and the shared frontend IPC seam, scoped validation, stale-generation errors and versioned history invalidation events through `emit.rs`.
- [x] 4.2 Implement digest/absence comparison tokens and restore preparation with canonical-path editor prepare/acknowledge gates across windows; abort for dirty editors, missing acknowledgements, lifecycle changes, stale disk state and invalid target permissions/containment.
- [x] 4.3 Implement durable pre-restore safety capture, pinned revisions, atomic restore/recreation/absence restore, clean-editor refresh and all-exit gate cleanup. Cover changed symlink targets, absent parents, failed safety capture and successful mutation followed by post-record failure.
- [x] 4.4 Regenerate TypeScript DTOs and command/event manifests using `cargo xtask codegen` and `cargo xtask contract-check --write`; verify generated outputs rather than editing them manually.

## 5. Local History UI

- [x] 5.1 Register active-file/project Local History commands and file-tree/editor-tab menu actions, plus backend-owned deduplicated view identity/layout; preserve existing Git history dispatch and support projects without Git.
- [x] 5.2 Build the project path/deleted-file browser and paginated per-file revision list with timestamps, known source, rename/lifecycle details and explicit loading/empty/excluded/degraded/error states.
- [x] 5.3 Add read-only selected-versus-current and revision-versus-revision diffs, absence rendering, stale-request cancellation and restore confirmation with actionable dirty/stale/unavailable errors; invalidate selections across windows on clear/cleanup.
- [x] 5.4 Add root-scoped enablement, exclusion, retention/budget, used-space and confirmed-clear controls; explain default limits, existing retained excluded content, baseline progress and observational coverage. Add behavior/SSR checks for menus, selection races, restore gating and settings validation.

## 6. Verification and handoff

- [ ] 6.1 Run focused socket-free history/store/observer/restore tests in affected Rust crates and frontend checks, including restart, unreadable storage, symlink escapes, worktree isolation, dirty detached-editor handshake and clear-generation races.
- [ ] 6.2 Run `cargo xtask codegen --check`, `cargo xtask contract-check`, `pnpm --dir ui run typecheck`, `pnpm --dir ui run build`, and affected command/menu/editor/diff/restore/theme checks; record which checks passed and any environmental blockers.
- [ ] 6.3 Document storage/defaults/exclusions, capture gaps, restore behavior and recovery limits in project docs; provide reviewer acceptance steps for non-Git edits, external agent edits, rename/delete/recreate, restart and multi-window restore.
- [ ] 6.4 Hand off real-window acceptance, dark/light visual review, captures and renders to the reviewer. This sandbox has no display/audio and refuses Unix sockets; do not launch GUI apps, virtual displays or socket-dependent capture tools here.
