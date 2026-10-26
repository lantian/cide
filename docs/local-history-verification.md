# Local History verification

Recorded for `add-local-file-history` on 2026-10-26 in the restricted macOS checkout.
These checks exercise filesystem fixtures, Rust logic, frontend behavior, jsdom and SSR.
They do not establish native-window behavior or visual acceptance.

## Passed

| Command | Result |
| --- | --- |
| `cargo test --locked --offline -p cide-core --lib local_history` | 35 passed |
| `cargo test --locked --offline -p cide-ipc --lib local_history` | 22 passed |
| `cargo test --locked --offline -p cide-fs --lib filter` | 13 passed |
| `cargo test --locked --offline -p cide-fs --lib watch::tests` | 9 passed, including subscription admission, replayed events, polling aliases and coalescing |
| `cargo test --locked --offline -p cide-app --lib local_history_restore` | 1 passed |
| `cargo test --locked --offline -p cide-app --lib cmd::file` | 52 passed |
| `cargo test --locked --offline -p cide-core --lib commands` | 40 passed |
| `cargo test --locked --offline -p cide-core --lib toolwindow` | 6 passed |
| `cargo test --locked --offline -p cide-core --lib document` | 27 passed |
| `cargo test --locked --offline -p cide-core --lib persist` | 77 passed |
| `cargo test --locked --offline -p cide-core --lib buffers` | 2 passed |
| `cargo --locked xtask codegen --check` | Passed; generated DTOs match |
| `cargo --locked xtask contract-check` | Passed; 413 commands, 46 events in sync |
| `pnpm --dir ui run typecheck` | Passed |
| `pnpm --dir ui run build` | Passed; existing workspace dynamic-import and chunk-size warnings |
| `pnpm --dir ui run check:local-history` | Behavior and SSR passed |

All 33 additional affected UI checks passed (the two drawing checks passed after updating
fixtures and fixing discarded-pane callback cleanup):

```text
check:commands check:menu-model check:menus check:editor check:diff check:diff-render
check:restore check:theme check:scheme check:merge check:unlisten check:toolwindow
check:toolwindow-render check:detached check:excalidraw check:excalidraw-session
check:ui-scale check:kit check:selectors check:keys check:switcher check:sidebar
check:boundary check:markdown check:motion check:ui-icons check:casing
check:fs-clipboard check:tree-drag check:tree-status check:symlinks check:close check:restart
```

History fixtures cover exact CRLF restart recovery, independent corrupt records and missing
blobs, interrupted commits/clear, exclusive ownership, separate linked worktrees, nested
roots, symlink escapes and changed targets, source-preserving deduplication, rename lineage,
descendant deletion and overwrite, cross-root moves, retention/shared blobs/pins, failed
ordinary saves, unavailable storage, watch admission with hidden files, bounded queue
reconciliation, generation invalidation and rapid close/reopen with an admitted save.

Restore fixtures cover digest/absence tokens, parent and permission checks, dirty detached
editors, missing acknowledgements, mount/window lifecycle changes, write exclusion, durable
safety capture, final disk races, exact restore/recreation/deletion and successful mutation
followed by failed history recording. The drawing jsdom check exercises live dirty replies,
blocked saves while gated, clean restore reload, undo isolation and disposal of listeners,
registrations and autosave timers. It substitutes IPC and the canvas engine; it does not
validate a real canvas or native multi-window event delivery.

## Environment and remaining native acceptance

Rust ran offline with the locked dependency graph. Existing matching frontend dependencies
were copied into this checkout. The bundled pnpm was invoked through a temporary wrapper
with `pnpm_config_verify_deps_before_run=false` to avoid a network reinstall. No package or
lockfile changes were needed for that workaround.

The extra `cargo test --locked --offline -p cide-app --lib cmd::fs::tests` run initially
attempted desktop trash outside the sandbox. With `XDG_DATA_HOME` pointing to a private
temporary test-data directory, its deletion fixture passed: 18 tests passed, and the native
`linked_folders_are_searchable_editable_and_updated_by_the_project_watcher` test failed to
observe the newly created linked target within its deadline.

The identical native test was also run against a disposable archive of the pre-change
commit `b0e0452`, with the same locked/offline command and private test-data environment.
It failed at the same linked-target assertion after five seconds. This failure predates
Local History in this environment; native delivery still needs confirmation by the reviewer.
The socket-free observer admission, replay, alias and coalescing checks all passed.

Real-window acceptance, dark/light visual review, captures and renders remain for the reviewer. The
sandbox has no display/audio and refuses Unix sockets; no GUI application, virtual display
or socket-dependent capture tool was launched. No full-repository CI or live agent session
was run.
