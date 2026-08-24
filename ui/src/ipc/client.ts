/**
 * The single seam between the frontend and the Rust core.
 *
 * This is the ONLY file permitted to import from `@tauri-apps/api`. Everything else goes
 * through the namespaced objects below, which is what keeps the command surface greppable
 * and lets `cargo xtask contract-check` reason about it. An eslint `no-restricted-imports`
 * rule enforces this once linting lands.
 *
 * Rust handlers are snake_case (`session_attach`) because Tauri derives the command name
 * from the function name; the dotted names here are the vocabulary the rest of the app
 * uses.
 */
import { Channel, convertFileSrc, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  Axis,
  Bootstrap,
  ColorScheme,
  CompletionAnswer,
  CompletionResolveAnswer,
  DefinitionAnswer,
  DiagnosticSourceId,
  DiagnosticsSnapshot,
  DiffAnswer,
  Direction,
  FileOutline,
  FormatAnswer,
  FormatRange,
  FsChange,
  FsStatus,
  GraphicsStatus,
  HeadlessRequest,
  HeadlessResult,
  KeymapEdit,
  KeymapEditResult,
  KeymapReport,
  ResolvedBinding,
  FileDoc,
  ImageDoc,
  FileStamp,
  PaneId,
  PaneRestore,
  PickerFrame,
  PickerItem,
  ProbeAnswer,
  SymbolFrame,
  SymbolIndexStatus,
  ProjectId,
  QuitDecision,
  ReopenedFile,
  SearchFrame,
  SearchQuery,
  SessionExit,
  SessionState,
  Settings as SettingsDto,
  SettingsPatch,
  SettingsSection,
  Side,
  SplitId,
  SplitIntent,
  SplitOutcome,
  TabId,
  TreeMatches,
  TreeRow,
  TreeRowKind,
  UnsavedTab,
  UsagesAnswer,
  ViewPosition,
  WindowMode,
  Workspace,
} from './generated'

import type {
  BranchInfo,
  ChangesTree,
  CommitOutcome,
  CommitRequest,
  DiffSide,
  FileDiff,
  LineRef,
  PathSelection,
  PushOutcome,
  RepoId,
  RepoInfo,
  ShelfEntry,
  StashEntry,
  TreeStatusMap,
} from './generated'

export type * from './generated'

export type SessionId = string

export interface IpcHealth {
  customProtocol: boolean
  mibPerSec: number
  webkitVersion: string
}

export interface Geometry {
  cols: number
  rows: number
  cellWidth: number
  cellHeight: number
}

export const app = {
  /** Report first paint. Also logs the window's real geometry on the Rust side. */
  ready: () => invoke<void>('app_ready'),

  /**
   * Everything this window needs to paint, in one round trip.
   *
   * One call rather than four: asking separately for the role, the workspace, the keymap
   * and the command list would render three intermediate states on the way, which on a
   * degraded IPC path is visible as flicker.
   */
  getBootstrap: () => invoke<Bootstrap>('app_get_bootstrap'),

  /**
   * What each pane should become on launch: resume its conversation, or start clean.
   *
   * Advice, not instruction — spawning stays with the frontend because only it knows a
   * pane's size. Only entries marked `eager` spawn without being asked; the rest wait behind
   * a resume affordance, so reopening a six-pane project does not start six agents at once.
   */
  restorePlan: () => invoke<PaneRestore[]>('app_restore_plan'),

  /**
   * Resize until the webview viewport is exactly `width` x `height`.
   *
   * The window's size and its viewport differ by the compositor's invisible border, so the
   * current viewport is reported and Rust corrects by the difference.
   */
  setViewport: (width: number, height: number) =>
    invoke<void>('window_set_viewport', {
      width,
      height,
      currentWidth: window.innerWidth,
      currentHeight: window.innerHeight,
    }),

  /**
   * What closing would cost: unsaved file tabs, and Claude turns it would interrupt.
   *
   * Ask before closing anything wider than a tab — a project, a window, the app — and show
   * `CloseConfirm` only when the answer is not `clear`. `project` narrows it to one project;
   * omit it for the whole application.
   *
   * The unsaved half is always reported. The session half is governed by the
   * `confirmCloseWithLiveSession` setting, because a lost turn is recoverable
   * (`claude --resume`) and a lost buffer is not.
   */
  quitRequested: (projectId?: ProjectId) =>
    invoke<QuitDecision>('app_quit_requested', { project: projectId ?? null }),
}

export const project = {
  /** Open a project over one or more roots. Returns the new project's id. */
  open: (paths: string[], name?: string) =>
    invoke<ProjectId>('project_open', { paths, name: name ?? null }),
  /**
   * Close a project, its tabs and its windows.
   *
   * Rejected with `unsavedChanges` when any of its file tabs is dirty, unless `force` —
   * see `unsavedChanges()` below for reading the refusal.
   */
  close: (id: ProjectId, force = false) =>
    invoke<{ rev: number }>('project_close', { project: id, force }),
  /** Bring a project to the front. What the header's project tabs do. */
  activate: (id: ProjectId) => invoke<{ rev: number }>('project_activate', { project: id }),
  reorder: (from: number, to: number) => invoke<{ rev: number }>('project_reorder', { from, to }),
}

export const tab = {
  /** Open a closable full-screen Claude tab. Splitting it creates new sessions. */
  newClaude: (projectId: ProjectId, title?: string) =>
    invoke<TabId>('tab_new_claude', { project: projectId, title: title ?? null }),

  activate: (projectId: ProjectId, id: TabId) =>
    invoke<{ rev: number }>('tab_activate', { project: projectId, tab: id }),

  /**
   * Close a tab. Both refusals live in Rust, and both are enforcement rather than courtesy:
   *
   * * `tabPinned` for the project console.
   * * `unsavedChanges` for a file tab with unsaved edits, unless `force` is passed.
   *
   * `force` defaults to false so that no call site loses a buffer by omission. Pass true
   * only after the user has seen `CloseConfirm` naming the file and chosen to discard.
   */
  close: (projectId: ProjectId, id: TabId, force = false) =>
    invoke<{ rev: number }>('tab_close', { project: projectId, tab: id, force }),

  /**
   * Move a tab within its strip — what a tab drag commits on `pointerup`.
   *
   * `before` is the tab the dragged one lands **in front of**; `null` drops it at the end. A
   * boundary rather than a destination index, and **ids rather than indices**, both deliberate:
   * the drop position is computed in the webview at pointer-move time against a snapshot, and
   * between that move and the release an agent's `openDiff` or another window can insert or
   * remove a tab. An index would then move whichever tab now sits there — silently, and
   * somewhere nobody aimed. Rust resolves both ids to indices under the workspace lock.
   *
   * Refuses with `tabPinned` for the console — moving it, or dropping anything ahead of it. The
   * frontend does not let a user reach either (`chrome/tabDrag.ts` refuses the grab and clamps
   * the caret to boundary 1), but that is the courtesy; this is the enforcement.
   */
  reorder: (projectId: ProjectId, id: TabId, before: TabId | null) =>
    invoke<{ rev: number }>('tab_reorder', { project: projectId, tab: id, before }),

  /**
   * Put back the last tab this project closed. Ctrl+Shift+T.
   *
   * `null` — not a rejection — when there is nothing to reopen, or when every record on the
   * stack turned out to be stale (the file was deleted, the tab is already the active one).
   * "Nothing to reopen" is a precondition that failed, not a failure, so it reports through
   * `unmet` rather than the failure toast; see the Rust handler for the full rule.
   */
  reopenClosed: (projectId: ProjectId) =>
    invoke<TabId | null>('tab_reopen_closed', { project: projectId }),
}

/**
 * Was this rejection the write precondition refusing — "something else changed this file"?
 *
 * A **tag**, never the message. `CoreError::FileChanged` exists as its own variant precisely so
 * this question can be answered without matching on prose: the caller has to tell "the file
 * moved under you, here is the conflict bar" from "the disk is full, here is a failure toast",
 * and a reworded sentence turning one into the other is a silent regression in the direction
 * that loses work.
 *
 * Structural rather than `as`: the payload comes from another process. Same shape and same
 * argument as `unsavedChanges` below.
 */
export function fileChanged(error: unknown): boolean {
  if (typeof error !== 'object' || error === null) return false
  return (error as { kind?: unknown }).kind === 'fileChanged'
}

/**
 * Read a rejected command as the domain's unsaved-work refusal.
 *
 * Returns the tabs that would have been lost, or `null` for every other failure. This is the
 * one place that knows the shape `CoreError` serialises to — `{ kind, detail }` — because
 * `CoreError` lives in `cide-core` and `generated.ts` carries only the `cide-ipc` DTOs, so
 * there is no generated type to narrow against. Structural, not `as`: the payload comes
 * from another process and a cast would assert rather than check.
 */
export function unsavedChanges(error: unknown): UnsavedTab[] | null {
  if (typeof error !== 'object' || error === null) return null
  const { kind, detail } = error as { kind?: unknown; detail?: unknown }
  if (kind !== 'unsavedChanges') return null
  if (typeof detail !== 'object' || detail === null) return null
  const { tabs } = detail as { tabs?: unknown }
  return Array.isArray(tabs) ? (tabs as UnsavedTab[]) : null
}

/**
 * The split tree.
 *
 * `split` creates the pane and leaves it session-less on purpose: only the frontend knows
 * how big the pane is, and a child spawned before its slot has been laid out gets the
 * fallback 80x24. So the pane appears, the terminal measures it, spawns, and calls
 * `bindSession`.
 */
export const pane = {
  /** `intent: null` asks for the tab's default — shell sideways, new session downwards. */
  split: (
    projectId: ProjectId,
    tabId: TabId,
    paneId: PaneId,
    axis: Axis,
    side: Side,
    intent: SplitIntent | null = null,
  ) =>
    invoke<SplitOutcome>('pane_split', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      axis,
      side,
      intent,
    }),

  /**
   * Rejected for the console's primary pane, for a tab's last pane, and — without `force` —
   * for an editor pane holding unsaved edits. That last one is the same refusal `tab.close`
   * makes: closing the pane inside a file tab discards exactly as much as closing the tab.
   */
  close: (projectId: ProjectId, tabId: TabId, paneId: PaneId, force = false) =>
    invoke<{ rev: number }>('pane_close', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      force,
    }),

  focus: (projectId: ProjectId, tabId: TabId, paneId: PaneId) =>
    invoke<{ rev: number }>('pane_focus', { project: projectId, tab: tabId, pane: paneId }),

  /** `null` clears the flag. Maximizing also focuses — the renderer hides the rest. */
  maximize: (projectId: ProjectId, tabId: TabId, paneId: PaneId | null) =>
    invoke<{ rev: number }>('pane_maximize', { project: projectId, tab: tabId, pane: paneId }),

  /**
   * Give every tile of a pane's row the same width — the row it is a tile of, and no other
   * chain: the rows above and below keep their heights exactly.
   *
   * `axis: 'row'` is the tiles beside the pane, `'col'` the rows of the tab. Only the row
   * form has a gesture today (the pane menu's *Even out this row*, and `pane.evenRow`).
   * A pane with no chain of that axis above it is a row of one and this does nothing.
   */
  distribute: (projectId: ProjectId, tabId: TabId, paneId: PaneId, axis: Axis) =>
    invoke<{ rev: number }>('pane_distribute', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      axis,
    }),

  /**
   * Move a divider. `ratio` is the **pair share** — the first of the two tiles this divider
   * separates within its row, not the split node's `a`-share. The two are the same number
   * for every two-pane tab and for the shipped console, which is why this signature did not
   * move. Returns the value actually stored, which may be clamped to [0.1, 0.9].
   */
  setRatio: (projectId: ProjectId, tabId: TabId, split: SplitId, ratio: number) =>
    invoke<number>('pane_set_ratio', { project: projectId, tab: tabId, split, ratio }),

  /** Where focus would go. Read-only; follow with `focus`. */
  navigate: (projectId: ProjectId, tabId: TabId, paneId: PaneId, direction: Direction) =>
    invoke<PaneId | null>('pane_navigate', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      direction,
    }),

  swap: (projectId: ProjectId, tabId: TabId, a: PaneId, b: PaneId) =>
    invoke<{ rev: number }>('pane_swap', { project: projectId, tab: tabId, a, b }),

  /** Record which session a pane is showing, once it has been spawned at the right size. */
  bindSession: (projectId: ProjectId, tabId: TabId, paneId: PaneId, session: SessionId) =>
    invoke<{ rev: number }>('pane_bind_session', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      session,
    }),

  /**
   * A new full-width row holding one pane.
   *
   * `after: null` appends at the bottom; otherwise the row lands on `side` of the row that
   * currently holds `after`. `intent: null` asks for the tab's default — the same one
   * splitting downwards has always used, a new Claude session in the console.
   */
  addRow: (
    projectId: ProjectId,
    tabId: TabId,
    after: PaneId | null = null,
    side: Side = 'after',
    intent: SplitIntent | null = null,
  ) =>
    invoke<SplitOutcome>('pane_add_row', {
      project: projectId,
      tab: tabId,
      after,
      side,
      intent,
    }),
}

/**
 * Windows: detach, re-dock, and the stacked-vs-per-project mode.
 *
 * All three are the same underlying operation — rearranging which panes are shown where —
 * and none of them touches a session. That is the point of the registry owning sessions
 * rather than the tree owning them.
 */
// Named `windows`, not `window`: the singular would shadow the global inside this module,
// and `setViewport` above genuinely needs `window.innerWidth`.
export const windows = {
  /**
   * `rect` is the pane's current pixel size, so the new window opens at the size the pane
   * already had rather than a fixed default — a wide terminal that reopens narrow reflows
   * its whole transcript on arrival.
   */
  detachPane: (
    projectId: ProjectId,
    tabId: TabId,
    paneId: PaneId,
    rect?: { width: number; height: number },
  ) =>
    invoke<string>('window_detach_pane', {
      project: projectId,
      tab: tabId,
      pane: paneId,
      rect: rect ?? null,
    }),

  /** Puts the pane back in its home tab and closes the window it was in. */
  redockPane: (label: string) => invoke<{ rev: number }>('window_redock_pane', { label }),

  /**
   * Tear a whole tab out into a window of its own — the road a *file* takes, because an
   * editor's buffer is registered per tab and a lone editor pane cannot leave its tab.
   * `rect` follows `detachPane`'s reasoning: the tab reopens at the size it already was.
   */
  detachTab: (
    projectId: ProjectId,
    tabId: TabId,
    rect?: { width: number; height: number },
  ) =>
    invoke<string>('window_detach_tab', {
      project: projectId,
      tab: tabId,
      rect: rect ?? null,
    }),

  /** Puts the tab back in its shell's strip and closes the window it was in. */
  redockTab: (label: string) => invoke<{ rev: number }>('window_redock_tab', { label }),

  setMode: (mode: WindowMode) => invoke<{ rev: number }>('window_set_mode', { mode }),

  /**
   * Close a window, which means whatever the window is showing.
   *
   * In per-project mode the window *is* the project, so this can discard unsaved edits and
   * is rejected with `unsavedChanges` without `force`. Re-docking a detached pane loses
   * nothing and never consults it.
   */
  close: (label: string, force = false) =>
    invoke<{ rev: number }>('window_close', { label, force }),
}

/**
 * Claude Code's IDE integration.
 *
 * `openDiff` blocks the agent's turn, so a tab opened by `diffContent` is one the model is
 * sitting still waiting on. Every route out of that tab has to end in `answer` or in the
 * Rust-side cancellation the close paths perform — an unanswered diff is a conversation
 * that never resumes, with nothing on screen to say why.
 */
export const claude = {
  /**
   * The two documents for a pending diff.
   *
   * Fetched rather than carried on the tab: `DiffSpec` is persisted to `workspace.json`, and
   * proposed file contents have no business in a saved layout.
   */
  diffContent: (projectId: ProjectId, requestId: string) =>
    invoke<{ original: string; proposed: string }>('claude_diff_content', {
      project: projectId,
      requestId,
    }),

  /**
   * Tell every connected `claude` in the project where the editor selection is.
   *
   * Broadcast rather than addressed, unlike `at_mentioned`: a mention is a message aimed at
   * one conversation, a selection is a fact about the editor, and when an editor is focused
   * there is no "current" Claude pane to aim at.
   *
   * Lines are 1-based here and converted to the protocol's 0-based in Rust, at the boundary,
   * exactly once.
   */
  selectionChanged: (
    projectId: ProjectId,
    path: string,
    text: string,
    startLine: number,
    endLine: number,
  ) =>
    invoke<void>('claude_selection_changed', {
      project: projectId,
      path,
      text,
      startLine,
      endLine,
    }).catch(() => {}),

  /*
   * `mentionFile` used to live here, wrapping a `claude_mention_file` command that no longer
   * exists. Both were dead: this wrapper ended in `.catch(() => {})` and had **no call site**,
   * because Ctrl+P's ⌥⏎ deliberately went to `claudeSend.lines` at the bottom of this file
   * instead — a call that swallows its own failure is indistinguishable from a control wired
   * to nothing, which is what the whole `ClaudeSendError` type exists to fix. Removed with the
   * Rust command rather than left as a quieter second route into the same server.
   */

  /** Answer a diff. `acceptedEdited` carries the buffer the user actually has on screen. */
  answer: (projectId: ProjectId, requestId: string, outcome: DiffAnswer) =>
    invoke<void>('claude_diff_result', { project: projectId, requestId, outcome }),
}

/**
 * The file tree, the watcher and file operations. (M8)
 *
 * The tree is a **windowed row list**, not a tree: `count()` sizes the scroller and
 * `rows(offset, len)` fills the viewport. Expansion state lives in Rust, because it is what
 * decides the flattening — a frontend that owned it would have to send it back on every
 * window request.
 *
 * `index` is deliberately not part of `project.open`. A large repository takes seconds to
 * walk, and a project that appears instantly with a tree that fills in beats one that hangs
 * before it appears. Call it after opening, and `close` when the project closes.
 */
export const fs = {
  /** Walk the project's roots and start watching. Safe to call twice; the second is a no-op. */
  index: (projectId: ProjectId) => invoke<FsStatus>('fs_index', { project: projectId }),

  /** Drop the index and stop watching. Call on project close. */
  close: (projectId: ProjectId) => invoke<boolean>('fs_close', { project: projectId }),

  status: (projectId: ProjectId) => invoke<FsStatus>('fs_status', { project: projectId }),

  /** Total visible rows — the virtual scroller's range. */
  treeCount: (projectId: ProjectId) => invoke<number>('fs_tree_count', { project: projectId }),

  /** The rows in `[offset, offset + len)`. Clamped in Rust; past the end is `[]`, not an error. */
  treeRows: (projectId: ProjectId, offset: number, len: number) =>
    invoke<TreeRow[]>('fs_tree_rows', { project: projectId, offset, len }),

  /**
   * Which visible rows a speed-search query matches, and where in each row's name.
   *
   * One round trip per keystroke, like `picker.query`, and for the same reason: these rows are
   * not in the webview. The explorer holds 200-row chunks of a flattening Rust owns, so
   * matching here would silently miss everything outside the cache — and *which* matches exist
   * would depend on where the user last scrolled.
   *
   * The frame echoes the query and carries the row count it was computed against; see
   * `sidebar/useSpeedSearch.ts` for what is done with both.
   */
  treeMatch: (projectId: ProjectId, query: string) =>
    invoke<TreeMatches>('fs_tree_match', { project: projectId, query }),

  /** Returns the new row count, so the scroller can resize without a second call. */
  expand: (projectId: ProjectId, path: string) =>
    invoke<number>('fs_expand', { project: projectId, path }),

  collapse: (projectId: ProjectId, path: string) =>
    invoke<number>('fs_collapse', { project: projectId, path }),

  /**
   * Expand everything above a path and return the row it now sits on.
   *
   * `null` when the path has no row — gitignored, or deleted. Scroll to the number; show
   * nothing on `null` rather than guessing a position.
   */
  reveal: (projectId: ProjectId, path: string) =>
    invoke<number | null>('fs_reveal', { project: projectId, path }),

  /**
   * What the index holds at each path: `'file'`, `'dir'`, or `null` for nothing.
   *
   * The oracle behind terminal file links, and a batch because it is asked from a mouse hover:
   * one call answers every candidate on the line the pointer just entered. It touches no disk —
   * the answer is a hash lookup against the walked tree — so a hover costs one round trip and
   * no `stat`s.
   *
   * `null` means "not in the index", which is a stronger statement than "not on disk": the
   * index is gitignore-filtered and does not descend through symlinked directories, so
   * `target/…`, `node_modules/…` and everything outside the project's roots answer `null` and
   * never become links. Rejects while the project has no index yet; the caller treats that as
   * "nothing lights up", not as an error worth showing.
   */
  pathsExist: (projectId: ProjectId, paths: string[]) =>
    invoke<Array<TreeRowKind | null>>('fs_paths_exist', { project: projectId, paths }),

  /**
   * The same question, answered from **disk** instead of from the index.
   *
   * The oracle for the one case `pathsExist` structurally cannot answer: an absolute path
   * outside every root, which the index will never hold however long it walks. One `stat` per
   * path, clamped to the same 128, so it is asked *only* for the candidates
   * `terminal/pathMatch.ts`'s `outsidePaths` produced — in-project hovering keeps its
   * zero-syscall property, which is a design property of `pathsExist` and not an accident.
   *
   * Not project-scoped, because the answer is not: it stats what it is given. A FIFO, a socket
   * and a device node all answer `null` rather than `'file'`, so the shape that would park a
   * blocking-pool worker in `read_to_end` for ever never gets an underline in the first place.
   */
  statPaths: (paths: string[]) => invoke<Array<TreeRowKind | null>>('fs_stat_paths', { paths }),

  readFile: (projectId: ProjectId, path: string) =>
    invoke<string>('fs_read_file', { project: projectId, path }),

  /** Atomic: written to a temporary file in the same directory and renamed over. */
  writeFile: (projectId: ProjectId, path: string, contents: string) =>
    invoke<void>('fs_write_file', { project: projectId, path, contents }),

  create: (projectId: ProjectId, path: string, directory = false) =>
    invoke<void>('fs_create', { project: projectId, path, directory }),

  rename: (projectId: ProjectId, from: string, to: string) =>
    invoke<void>('fs_rename', { project: projectId, from, to }),

  /** Moves to the desktop trash — never `unlink`. Returns where each path landed. */
  delete: (projectId: ProjectId, paths: string[]) =>
    invoke<string[]>('fs_delete', { project: projectId, paths }),

  /**
   * Create a scratch file of this type and answer where it landed.
   *
   * `ext` is a bare extension without the dot — `'rs'`, `'json'` — taken from `SCRATCH_TYPES`
   * in `editor/languages.ts`, which is the same table that decides which grammar the resulting
   * buffer loads. Rust validates the *shape* and not the membership, deliberately: shipping the
   * offered list across the wire would make it a DTO that has to stay in step with a TypeScript
   * record, and `check:editor` pins the agreement on this side instead.
   *
   * The file is created empty and immediately, and the tree's *Scratches* group is re-listed
   * before this resolves — so the caller may open the tab and reveal the row with no wait and
   * no watcher.
   */
  scratchNew: (projectId: ProjectId, ext: string) =>
    invoke<string>('fs_scratch_new', { project: projectId, ext }),

  /**
   * Make sure this project's notes file exists, and answer its absolute path.
   *
   * The whole wire surface of *Project Notes*. The caller hands the answer straight to
   * `file.open`, so what appears is an **ordinary** file tab — save, the dirty marker, undo,
   * find-in-file and the markdown grammar all work with no special case, which is the reason
   * the feature is one command and not a new tab kind.
   *
   * Idempotent and **never truncating**: the first call creates the file and its directory, and
   * every call after that answers the same path with the contents untouched. The path is a
   * blake3 of the project's canonicalised primary root under `$XDG_STATE_HOME/cide/notes`, which
   * is why it comes from Rust rather than being assembled here — a second copy of the keying
   * rule is a second copy that can drift, and the one it would drift from decides where the
   * user's writing is.
   *
   * A multi-root project has one notes file, keyed by `roots[0]`.
   */
  notesEnsure: (projectId: ProjectId) => invoke<string>('fs_notes_ensure', { project: projectId }),

  /**
   * Every directory the file tree's disk-changing verbs may act inside.
   *
   * The project's roots **plus** the scratch drawer, which is outside all of them. Asked once
   * per attach, because the answer only moves when a project's roots do. It is what
   * `sidebar/rowPaths.ts::mutationRefusal` greys *Rename…*, *Cut* and *Move to Trash* from, and
   * the reason it comes from Rust rather than being derived here is that the drawer's path is a
   * blake3 of a canonicalised root under `$XDG_STATE_HOME` — a fact only Rust can compute, and
   * the same list `cide_fs::ops::check_within` is given.
   */
  writableRoots: (projectId: ProjectId) =>
    invoke<string[]>('fs_writable_roots', { project: projectId }),

  /**
   * Every path the file tree can hang a row from: the project's roots **plus** each synthetic
   * group's top-level children — the package directories under *External Libraries*, and each
   * file in the *Scratches* drawer. (M16)
   *
   * The containment question, not the writability one above, and the two lists disagree in both
   * directions: a dependency source is revealable and never writable, the scratch *drawer* is
   * writable and draws no row. What asks is `sidebar/rowPaths.ts::crumbTargets`, which decides
   * before the click whether a segment of the status bar's path trail leads anywhere — an
   * affordance that looks the same and does nothing being the defect this project keeps
   * producing.
   *
   * Asked once per attach and re-asked whenever the tree self-heals, because the answer moves
   * twice: when a project's roots move, and when *External Libraries* finishes resolving. It is
   * deliberately I/O-free on the Rust side and never triggers a resolution, so an unresolved
   * group answers with the roots alone and the library crumbs light up on the `cide://fs-status`
   * the resolver emits.
   */
  revealRoots: (projectId: ProjectId) =>
    invoke<string[]>('fs_reveal_roots', { project: projectId }),

  /**
   * The same speed-search rule over a list the caller supplies — the git panel's changes tree.
   *
   * The `picker.rank` shape, and the same argument: two sidebar trees that answer one query
   * differently reads as a bug even when both answers are defensible on their own. The changes
   * tree holds its rows in the webview and could match them in a loop; that loop would be a
   * second implementation of `cide_fs::speed`, agreeing with the first only until somebody
   * edited either.
   */
  matchLabels: (query: string, labels: readonly string[]) =>
    invoke<TreeMatches>('tree_match_labels', { query, labels }),
}

/**
 * The fuzzy pickers.
 *
 * `query` is a poll, not a subscription: the index is still filling while the user types, so
 * the frame carries `running` and the overlay asks again while it is true. `matched` and
 * `total` are the two numbers in the `6 of 2,418` counter, and they climb during a walk.
 */
export const picker = {
  /**
   * Query a project's file index. Usable before `fs.index` has finished.
   *
   * `libraries` widens the answer to the project's resolved dependency packages — never to a
   * dependency *cache*. See `ProjectFs::index_libraries`. Off by default and off costs nothing:
   * with it false Rust takes one frame from one matcher, exactly as it did before the parameter
   * existed.
   *
   * Passed explicitly rather than defaulted in Rust: `picker_query`'s parameter is
   * `Option<bool>` because Tauri rejects a missing key for a plain `bool`, and a default living
   * only here is one the next caller forgets. Both sides answer `false`.
   */
  query: (projectId: ProjectId, query: string, limit?: number, libraries = false) =>
    invoke<PickerFrame>('picker_query', {
      project: projectId,
      query,
      limit: limit ?? null,
      libraries,
    }),

  /**
   * Start the dependency walk behind `query`'s `libraries` scope, if one has not run.
   *
   * Returns as soon as the walk is claimed; the overlay watches it fill through `query`'s
   * `running` flag and its climbing counter, the same way it watches the project walk. Calling
   * it twice is free — the second finds the walk running or finished and starts nothing — which
   * is what lets the caller be "the toggle went on" rather than a piece of bookkeeping.
   */
  indexLibraries: (projectId: ProjectId) =>
    invoke<void>('picker_index_libraries', { project: projectId }),

  /**
   * Rank a list supplied here — the command palette.
   *
   * Same scoring as the file picker on purpose: two overlays that rank one query differently
   * reads as a bug even when both answers are defensible.
   */
  rank: (query: string, items: PickerItem[], limit?: number) =>
    invoke<PickerFrame>('picker_rank', { query, items, limit: limit ?? null }),
}

/**
 * Code structure: one file's outline, and the project-wide symbol picker. (M12)
 *
 * A namespace of its own rather than more of `picker`, because its rows are not `PickerRow`s. A
 * `PickerRow` carries one `text` that is both matched and drawn; a symbol row draws four things —
 * kind glyph, name, dimmed container, dimmed file — of which exactly one is matched. Sharing the
 * type would put the file path into the fuzzy score and make the highlight offsets meaningless.
 */
export const symbols = {
  /**
   * One file's structure.
   *
   * `text` is the **live buffer**, and passing it is not optional in spirit: the breadcrumb and
   * the member walk are positional, so an outline of on-disk content names the wrong function
   * for as long as the buffer is dirty.
   */
  outline: (projectId: ProjectId, path: string, text?: string) =>
    invoke<FileOutline>('symbols_outline', { project: projectId, path, text: text ?? null }),

  /** Build the project's symbol index. Idempotent — safe to call whenever the overlay opens. */
  index: (projectId: ProjectId) =>
    invoke<SymbolIndexStatus>('symbols_index', { project: projectId }),

  /** Rank the project's symbols. Rejects with `{kind:'notIndexed'}` until `index` has run. */
  query: (projectId: ProjectId, query: string, limit?: number) =>
    invoke<SymbolFrame>('symbol_query', { project: projectId, query, limit: limit ?? null }),
}

/**
 * Problems: the merged view over the language servers, tree-sitter and Claude. (M12)
 *
 * There is no `onChanged` here — see `events.onDiagnostics`. The event carries only a project id
 * and the frontend answers with `get`, because the snapshot depends on settings the emitter would
 * have to read and the receiver already has.
 */
export const diagnostics = {
  /**
   * The snapshot as it stands, projected through the current inspection settings.
   *
   * Never rejects. A project with no language server is `{kind:'unavailable'}` carrying a
   * sentence, because "nothing is analysing this" is an answer and a rejected promise is not.
   */
  get: (projectId: ProjectId) =>
    invoke<DiagnosticsSnapshot>('diagnostics_get', { project: projectId }),

  didOpen: (projectId: ProjectId, path: string, version: number, text: string) =>
    invoke<void>('diagnostics_did_open', { project: projectId, path, version, text }),

  /**
   * The buffer changed.
   *
   * **Debounce this.** A command per keystroke is an IPC round trip per keystroke, and the whole
   * text goes with it — the same rule `claude.selectionChanged` states for a selection drag.
   */
  didChange: (projectId: ProjectId, path: string, version: number, text: string) =>
    invoke<void>('diagnostics_did_change', { project: projectId, path, version, text }),

  didSave: (projectId: ProjectId, path: string) =>
    invoke<void>('diagnostics_did_save', { project: projectId, path }),

  didClose: (projectId: ProjectId, path: string) =>
    invoke<void>('diagnostics_did_close', { project: projectId, path }),

  /**
   * Where the thing at `line`/`column` is declared.
   *
   * Never rejects: "still indexing", "no server for this language" and "no declaration here" are
   * all *answers*, and each is a different `DefinitionAnswer` variant carrying its own sentence.
   * Collapsing them into a rejection — or into `null` — is what would make a busy analyser look
   * like an empty one.
   *
   * `line` and `column` are 1-based and `column` is UTF-16, the same units as `RevealTarget`.
   * It may block for up to five seconds server-side; the command runs on the blocking pool.
   */
  definition: (projectId: ProjectId, path: string, line: number, column: number) =>
    invoke<DefinitionAnswer>('diagnostics_definition', { project: projectId, path, line, column }),

  /**
   * Which action a Ctrl+click at this position would take: jump, or list usages.
   *
   * The discriminator behind the Ctrl gesture, and the *only* thing the Ctrl-hover underline has
   * to ask. It runs `textDocument/definition` and compares the answer against the position it was
   * asked about — so on a reference it costs exactly what Go to definition already cost, and on a
   * declaration it says so **without** searching for any references. A hover that could start a
   * whole-workspace search would be a hover that can wedge the language server.
   *
   * `timeoutMs` is clamped server-side to `[100 ms, 5 s]`. The hover passes 600 and means it: an
   * underline that arrives five seconds after the pointer stopped has been wrong for four of them.
   * Absent, it waits as long as Go to definition does.
   *
   * Never rejects, for the reason `definition` above states.
   */
  probe: (
    projectId: ProjectId,
    path: string,
    line: number,
    column: number,
    timeoutMs?: number,
  ) =>
    invoke<ProbeAnswer>('diagnostics_probe', {
      project: projectId,
      path,
      line,
      column,
      timeoutMs: timeoutMs ?? null,
    }),

  /**
   * Every place the symbol at this position is used.
   *
   * Waits up to **twenty** seconds, which is four times what the definition path allows and is
   * about the gesture rather than the plumbing: a references search on a widely-used trait method
   * against a cold rust-analyzer legitimately takes tens of seconds, and the user has a popup in
   * front of them saying so. Cancel it with [`usagesCancel`] rather than letting it run.
   *
   * `Found { rows: [] }` and `notFound` are different answers and the caller must keep them apart:
   * the first is "used nowhere", the second is "there is no symbol here".
   */
  usages: (projectId: ProjectId, path: string, line: number, column: number) =>
    invoke<UsagesAnswer>('diagnostics_usages', { project: projectId, path, line, column }),

  /**
   * Withdraw the outstanding Find usages for this project.
   *
   * Genuinely cancels: it releases the blocked thread *and* sends `$/cancelRequest`, so the server
   * stops searching. Without the second half, "Escape cancelled it" and "Escape stopped showing
   * it" would be indistinguishable on screen and only the second would be true.
   *
   * A no-op when nothing is outstanding, which is most calls — the popup cancels on unmount
   * without checking whether the answer already landed.
   */
  usagesCancel: (projectId: ProjectId) =>
    invoke<void>('diagnostics_usages_cancel', { project: projectId }),

  /**
   * The completion list at a position. (M25)
   *
   * Waits **two** seconds server-side, the shortest deadline of any of these, and the reason is
   * the gesture: this popup opens by itself while the user is still typing, so a list computed
   * for a prefix from two seconds ago is not late — it is wrong, and drawing it would move the
   * selection under somebody mid-keystroke.
   *
   * `before` is the character immediately left of the caret, or `null` at the start of a line.
   * Rust compares it against what the server declared as a trigger character and shapes the LSP
   * `context` accordingly; the trigger list never crosses this seam.
   *
   * **Sync the document first.** The server's copy is up to 300 ms behind the buffer
   * (`editor/docSync.ts`), and completing against text that does not contain the caret is
   * meaningless. `editor/completion.ts` awaits `syncNow` before calling this and is the only
   * caller.
   *
   * Never rejects. An `unavailable` answer is usually shown to nobody at all — see
   * `CompletionAnswer`.
   */
  completion: (
    projectId: ProjectId,
    path: string,
    line: number,
    column: number,
    before: string | null,
  ) =>
    invoke<CompletionAnswer>('diagnostics_completion', {
      project: projectId,
      path,
      line,
      column,
      before,
    }),

  /**
   * The deferred edits of one completion item — the `use` line, the `import` block. (M25)
   *
   * Only for a row that arrived with a non-null `resolve`, which is the minority: rust-analyzer
   * offers **no auto-import candidates at all** unless the client can fetch their edits lazily,
   * so for those rows the edit genuinely does not exist until this is called. Every other accept
   * skips it and is instant.
   *
   * `token` names the reply the row came from and `index` the row within it. An `unavailable`
   * answer is **not** "no edits": accepting a symbol without its import leaves the file not
   * compiling, so the caller must refuse the accept rather than do half of it.
   */
  completionResolve: (projectId: ProjectId, token: number, index: number) =>
    invoke<CompletionResolveAnswer>('diagnostics_completion_resolve', {
      project: projectId,
      token,
      index,
    }),

  /**
   * Withdraw the outstanding completion for this project.
   *
   * Releases the blocked thread *and* sends `$/cancelRequest`, for [`usagesCancel`]'s reason with
   * the volume argument on top: a user typing at speed supersedes a request per keystroke, and
   * without the protocol half each one leaves the server computing a list nobody will read.
   *
   * A no-op when nothing is outstanding, which is most calls.
   */
  completionCancel: (projectId: ProjectId) =>
    invoke<void>('diagnostics_completion_cancel', { project: projectId }),

  /**
   * Every place the symbol at this position is *implemented*. (M18)
   *
   * The answer to "goto in Go lands on the interface". `textDocument/definition` resolving a call
   * through an interface to the interface's method is gopls being correct; the concrete one is a
   * different protocol request, and this is it.
   *
   * Same shape, same twenty-second budget and the same [`usagesCancel`] as [`usages`], because it
   * shares the popup with it — one list on screen means one outstanding request, and the webview
   * never has to name which.
   *
   * `notFound` here means "no symbol at this position", and the caller falls back to
   * [`definition`] on it rather than saying nothing.
   */
  implementations: (projectId: ProjectId, path: string, line: number, column: number) =>
    invoke<UsagesAnswer>('diagnostics_implementations', {
      project: projectId,
      path,
      line,
      column,
    }),

  /**
   * Re-run every analyser for this project. (M18)
   *
   * The Problems panel's *Re-run analysis* button and the `problems.refresh` command. Resolves
   * with the sentence to show — a control whose whole effect happens in another process over the
   * next few seconds is otherwise indistinguishable from one that is wired to nothing.
   *
   * Cheap on purpose, and distinct from [`restart`]: this re-runs the checks (`cargo check`
   * again, gopls re-reads the files it has findings for) and takes seconds, where a restart
   * replaces the process and costs minutes of re-indexing on a large workspace.
   */
  refresh: (projectId: ProjectId) => invoke<string>('diagnostics_refresh', { project: projectId }),

  /** Restart one analyser after it gave up. The panel's only recovery gesture. */
  restart: (projectId: ProjectId, source: DiagnosticSourceId) =>
    invoke<void>('diagnostics_restart', { project: projectId, source }),

  /**
   * Stop every analyser, delete every server's on-disk cache, start them all again — the
   * palette's "Invalidate index caches". Global, not per project: the caches are per
   * server, and a survivor in another project would re-save what was just deleted.
   */
  invalidateCaches: () => invoke<void>('diagnostics_invalidate_caches', {}),

  /**
   * Reformat a buffer — Ctrl+Alt+F. (M26)
   *
   * On this object because one of its two roads is `textDocument/formatting` against the same
   * language server everything else here asks; the other spawns a filter the user configured,
   * and Rust decides which, so the webview never learns the difference.
   *
   * `text` is sent rather than read from disk, and the caller must have **awaited**
   * `flushDoc(path)` first, or the server is holding text up to 300 ms old. `range` is the
   * selection when there is one; it is honoured only by a server advertising
   * `documentRangeFormattingProvider` and ignored on the filter road.
   *
   * Never rejects for a user-facing outcome: every answer is a `FormatAnswer` arm carrying a
   * sentence, including "no formatter for this language".
   */
  format: (
    projectId: ProjectId,
    path: string,
    languageId: string,
    text: string,
    range: FormatRange | null,
  ) =>
    invoke<FormatAnswer>('format_document', {
      project: projectId,
      path,
      languageId,
      text,
      range,
    }),
}

/**
 * The Settings tab's surface.
 *
 * `set` takes a patch rather than the whole struct: two windows can have Settings open, and
 * "the user did not touch this" has to be distinguishable from "the user set this to false"
 * or whichever saved last would overwrite the other's fields.
 *
 * There is no `settings.onChanged` listener. Settings live inside the workspace, so a write
 * arrives in every window through `cide://workspace-changed` like any other mutation — a
 * second event would give two windows two orderings of one change.
 *
 * Note what is *not* here: the window mode. Changing it opens and closes real OS windows, so
 * it stays `windows.setMode`; see `SettingsPatch` in the Rust DTOs.
 */
export const settings = {
  get: () => invoke<SettingsDto>('settings_get'),

  /** Apply a partial update and get the settings as they now stand. */
  set: (patch: SettingsPatch) => invoke<SettingsDto>('settings_set', { patch }),

  /** Open the project's Settings tab, or activate the one it already has. */
  openTab: (projectId: ProjectId, section: SettingsSection | null = null) =>
    invoke<TabId>('tab_open_settings', { project: projectId, section }),

  /** The resolved keymap, its conflicts, and anything wrong in the user's overrides file. */
  keymap: () => invoke<KeymapReport>('keymap_report'),

  /**
   * Change `keymap.json` and get the keymap as it now stands.
   *
   * A **list** of edits because one gesture on the screen is often two of them — bind this
   * chord, and take it off the command that had it — and those must be one file write, or a
   * failure between them leaves a keymap with the chord bound twice. See `KeymapEdit` for why
   * the wire carries the change rather than the file.
   *
   * Every window is told through `cide://keymap-changed`, this one included, so the answer is
   * for drawing the screen and not for installing the new bindings.
   */
  keymapEdit: (edits: readonly KeymapEdit[]) =>
    invoke<KeymapEditResult>('keymap_edit', { edits }),

  /**
   * The Linux graphics ladder: what is stored, and what this process actually got.
   *
   * The two can differ, and saying so is the point. These variables are read while the
   * webview is being created, so a change made here takes effect on the next launch and
   * never on this one.
   */
  graphics: () => invoke<GraphicsStatus>('graphics_status'),

  /**
   * Pick a `.vsix` or a theme `.json` and import every colour theme in it.
   *
   * **A list, because a `.vsix` normally carries a light/dark pair** — and the setting is keyed
   * by polarity, so importing one of a pair would leave the other theme on the builtin.
   *
   * Empty means the user cancelled, which is not an error and must not be reported as one;
   * `project.pick` answers the same gesture the same way. A file that is not a theme rejects.
   *
   * The Rust side opens the dialog, because a picker with no transient parent stacks behind the
   * window on KDE Wayland; see `cmd::project::show_picker`.
   *
   * It does **not** select what it imported. Selecting is a settings patch, and the polarity of
   * an imported theme may not be the one this window is showing — which is a sentence the screen
   * can say and a command cannot.
   */
  importScheme: () => invoke<ColorScheme[]>('scheme_import'),

  /** Forget an imported colour scheme. Removing one that is already gone is not an error. */
  removeScheme: (id: string) => invoke<void>('scheme_remove', { id }),
}

/**
 * The non-interactive Claude lane: one prompt in, one answer out.
 *
 * Not a session — no pane, no PTY, no hooks, and no entry in the user's `/resume` picker.
 * Used for commit-message generation, "explain this selection" and palette one-shots.
 */
export const headless = {
  run: (projectId: ProjectId, request: HeadlessRequest) =>
    invoke<HeadlessResult>('claude_headless', { project: projectId, request }),
}

export const diag = {
  /** Pull `len` bytes as raw octets — the fast custom-protocol path. */
  echoBytes: async (len: number): Promise<ArrayBuffer> => {
    const bytes = await invoke<ArrayBuffer>('diag_echo_bytes', { len })
    return bytes
  },

  /** Push `count` frames of `len` bytes down a channel, mirroring how PTY output arrives. */
  pushBytes: (len: number, count: number, onFrame: (data: ArrayBuffer) => void) => {
    const sink = new Channel<ArrayBuffer>()
    sink.onmessage = onFrame
    return invoke<void>('diag_push_bytes', { len, count, sink })
  },

  reportIpc: (health: IpcHealth) => invoke<void>('diag_report_ipc', { health }),

  /** Print a finished benchmark report to the app's stdout. */
  benchReport: (report: string) => invoke<void>('diag_bench_report', { report }),

  /** Forward a diagnostic line to the app's stderr, next to the Rust log. */
  log: (message: string) => invoke<void>('diag_log', { message }),
}

/** True when the app was started with `CIDE_BENCH=1`, i.e. run the gate and exit. */
export function benchMode(): boolean {
  return new URLSearchParams(location.search).get('bench') === '1'
}

export const session = {
  /**
   * Spawn a child.
   *
   * `project` is what puts `CLAUDE_CODE_SSE_PORT` in the child's environment. Omitting it
   * does not merely skip a nicety: without that variable a `claude` started here falls back
   * to picking among every lockfile in the shared `~/.claude/ide` directory and can bind to
   * another editor entirely.
   */
  spawn: (opts: {
    program: string
    args: string[]
    cwd: string
    geometry: Geometry
    project?: string | undefined
    /** Continue an existing conversation. With `fork`, the parent to branch from. */
    resume?: SessionId | undefined
    /**
     * Branch instead of continuing: the new session shares history up to this point and
     * then diverges, leaving the parent's transcript intact. Verified against the real CLI
     * — the id we pass is honoured and both sessions stay independently resumable.
     */
    fork?: boolean | undefined
  }) => invoke<SessionId>('session_spawn', opts),

  /**
   * Attach a sink to a session.
   *
   * Sinks are a list on the Rust side, so attaching a second one before detaching the
   * first is how detach-into-a-window stays gapless.
   */
  attach: (id: SessionId, geo: Geometry, onData: (data: ArrayBuffer) => void) => {
    const sink = new Channel<ArrayBuffer>()
    sink.onmessage = onData
    return invoke<ArrayBuffer>('session_attach', { session: id, sink, geometry: geo })
  },

  detach: (id: SessionId) => invoke<void>('session_detach', { session: id }),

  /**
   * Report that this window has finished processing `bytes` of output.
   *
   * Must be called from `term.write`'s completion callback and not from the channel
   * handler. The channel handler only proves the bytes *arrived*; the completion callback
   * is the point at which xterm has parsed them, which is the thing worth reporting. Acking
   * on arrival would report a rate the renderer cannot sustain and defeat the mechanism.
   *
   * Deliberately fire-and-forget: an ack that fails because the pane closed underneath it
   * is ordinary, and awaiting one on every frame would put an IPC round-trip in the render
   * path.
   */
  ack: (id: SessionId, bytes: number) => {
    void invoke<void>('session_ack', { session: id, bytes }).catch(ackFailed)
  },

  /** The byte sequence that reconstructs the current screen. Send this before live bytes. */
  scrollback: (id: SessionId) => invoke<ArrayBuffer>('session_scrollback', { session: id }),

  /**
   * Where this pane's child is **now**, or `null`.
   *
   * `null` covers every uninteresting case at once — no such session, no pid, no `/proc`, a
   * deleted directory, and a cwd outside the project's roots. The last of those is a refusal
   * and not an absence: the child chooses its own cwd and is not trusted, so one that has
   * `chdir`-ed out of the project must not become a base for resolving that same child's
   * output. See `cmd/session.rs`.
   *
   * Asked while hovering a path in a terminal, so the caller caches it briefly rather than
   * asking per line; it is a `read_link`, not a walk, but a hover is not a place for a round
   * trip per row.
   */
  cwd: (project: ProjectId, id: SessionId) =>
    invoke<string | null>('session_cwd', { project, session: id }),

  inAlternateScreen: (id: SessionId) =>
    invoke<boolean>('session_in_alternate_screen', { session: id }),

  /**
   * Send bytes to the child.
   *
   * `seq` is not bookkeeping — it is what makes a keystroke arrive **once**. Tauri's
   * `ipc-protocol.js` attaches its rejection handler as the second argument of the second
   * `.then`, so it also catches a failure of `response.json()`/`arrayBuffer()` — a rejection
   * that happens *after* the Rust command has already run. It then flips
   * `customProtocolIpcFailed` permanently and re-sends the identical message over
   * `postMessage`. At-least-once delivery for a command whose whole effect is a side effect.
   * The retry carries the same `seq`, and `session_write` drops anything it has already
   * applied, which is the only way to be sure that path cannot double a character.
   */
  write: (id: SessionId, data: string) =>
    invoke<void>('session_write', { session: id, data, seq: nextWriteSeq(id) }),

  resize: (id: SessionId, geo: Geometry) =>
    invoke<void>('session_resize', { session: id, geometry: geo }),

  hasExited: (id: SessionId) => invoke<boolean>('session_has_exited', { session: id }),

  /**
   * The same question as `hasExited`, answered with the exit code instead of a boolean.
   *
   * Used by the rehydration path in `TerminalPane` — a pane host evicted and re-created after
   * its child had already gone, so the `cide://session-state` event carrying the code fired
   * before anything was listening. `hasExited` stays for the window audit, which genuinely
   * wants a predicate; this one is for the caller that is about to print the number.
   *
   * Never rejects for an unknown session: that is `{ kind: 'unknown' }`, the one case where no
   * code exists anywhere, and it is a routine answer after a workspace restore rather than a
   * failure. See `SessionExit` in the generated bindings.
   */
  exit: (id: SessionId) => invoke<SessionExit>('session_exit', { session: id }),

  kill: (id: SessionId) => invoke<void>('session_kill', { session: id }),

  /**
   * Every session the Rust registry holds a child for.
   *
   * The registry rather than the tree: a session outlives the pane's position, its tab and
   * its window, so this is the only honest answer to "what is still running".
   */
  list: () => invoke<SessionId[]>('session_list'),
}

/**
 * Events pushed from Rust.
 *
 * Every payload carries `rev`. Two windows can now mutate one workspace, so a snapshot can
 * arrive out of order and the receiver drops anything older than what it already holds.
 */
export const events = {
  /** Fires in every window after any accepted mutation, whichever window caused it. */
  onWorkspaceChanged: (handler: (workspace: Workspace) => void) =>
    listen<{ rev: number; workspace: Workspace }>('cide://workspace-changed', (e) =>
      handler(e.payload.workspace),
    ),

  /**
   * The user's keybindings changed, in this window or in another one.
   *
   * Needed *because* `onWorkspaceChanged` exists and does not carry it: the keymap is not part
   * of the workspace, and `applySnapshot` rebuilds `boot` around a new `workspace` while
   * keeping the old `keymap` array. So the one thing already broadcast to every window is
   * exactly the path that cannot refresh a binding.
   */
  onKeymapChanged: (handler: (keymap: ResolvedBinding[]) => void) =>
    listen<{ keymap: ResolvedBinding[] }>('cide://keymap-changed', (e) =>
      handler(e.payload.keymap),
    ),

  /**
   * The set of imported colour schemes changed, in this window or in another one. (M24)
   *
   * `onKeymapChanged`'s reason, one field along: the schemes ride `Bootstrap`, and
   * `applySnapshot` rebuilds `boot` around a new `workspace` while keeping the old `schemes`
   * array — so the event that carries the *setting* naming a scheme is exactly the one that
   * cannot carry the scheme.
   */
  onSchemesChanged: (handler: (schemes: ColorScheme[]) => void) =>
    listen<{ schemes: ColorScheme[] }>('cide://schemes-changed', (e) =>
      handler(e.payload.schemes),
    ),

  /**
   * A session moved between idle, busy, awaiting permission or exited.
   *
   * Carries no `rev`, deliberately: a session going busy does not move a pane, and
   * re-hydrating the whole workspace on every tool call would repaint the tree several
   * times a second during a turn.
   */
  onSessionState: (handler: (session: string, state: SessionState) => void) =>
    listen<{ session: string; state: SessionState }>('cide://session-state', (e) =>
      handler(e.payload.session, e.payload.state),
    ),

  /** Live model, token and cost figures from the statusline. */
  onSessionStatus: (handler: (session: string, status: unknown) => void) =>
    listen<{ session: string; status: unknown }>('cide://session-status', (e) =>
      handler(e.payload.session, e.payload.status),
    ),

  /**
   * A window-manager close was refused because it would discard unsaved work.
   *
   * The only refusal that arrives as an event rather than an `Err`: Alt+F4 and the
   * compositor's own close button never reach a command, so there is no call whose
   * rejection the caller could catch.
   */
  onCloseBlocked: (handler: (window: string, unsaved: UnsavedTab[]) => void) =>
    listen<{ window: string; unsaved: UnsavedTab[] }>('cide://close-blocked', (e) =>
      handler(e.payload.window, e.payload.unsaved),
    ),

  /**
   * A thumb button (mouse back / forward) was pressed in **this** window. (M12)
   *
   * From Rust rather than from a DOM listener, and that is not a preference: WebKitGTK maps only
   * GDK buttons 1–3 and turns everything else into `WebMouseEventButton::None`, which
   * `MouseEvent`'s constructor then reports as `button === 0`. Both thumb buttons arrive in the
   * DOM as an ordinary left click, so they cannot be told apart here at all. A GTK handler in
   * `windows.rs` reads the raw button, swallows the press so it never becomes a phantom click,
   * and sends this.
   *
   * `button` is a **key token**, not a command: `mouseback` / `mouseforward` are bound in
   * `cide_core::keymap::defaults` and resolved by the key gate's third entry point, so the user
   * can rebind or unbind them from `keymap.json` like any other chord. `emit_to` one window, not
   * a broadcast — two windows walking their own history from one press is a Back that sometimes
   * goes back twice.
   */
  onMouseNav: (
    handler: (
      button: string,
      modifiers: { ctrl: boolean; alt: boolean; shift: boolean; meta: boolean },
    ) => void,
  ) =>
    listen<{
      window: string
      button: string
      ctrl: boolean
      alt: boolean
      shift: boolean
      meta: boolean
    }>('cide://mouse-nav', (e) => {
      /*
       * The window filter, and it is load-bearing rather than defensive.
       *
       * Rust sends this with `emit_to(label, ..)`, which reads as "only that window" and is
       * not: Tauri filters by `EventTarget`, `listen()` above registers `EventTarget::Any`,
       * and the `Any` arm short-circuits past every filter. So the event reaches every
       * webview. With two shell windows open, one press of the thumb button ran Back in
       * both — each walking its own history, opening its own file and taking focus — and the
       * window the user was not pointing at moved for no visible reason.
       *
       * `windowLabel()` is what this webview is; the payload says where the press happened.
       */
      if (e.payload.window !== windowLabel()) return
      handler(e.payload.button, {
        ctrl: e.payload.ctrl,
        alt: e.payload.alt,
        shift: e.payload.shift,
        meta: e.payload.meta,
      })
    }),

  /**
   * A tool touched files.
   *
   * The fast path for reloading an open buffer — it arrives sooner than the watcher and
   * names the file exactly. It does *not* replace the watcher: a `sed -i` in a shell pane
   * or a `cargo fmt` never goes through a Claude Code tool.
   */
  onSessionTool: (handler: (session: string, paths: string[]) => void) =>
    listen<{ session: string; paths: string[] }>('cide://session-tool', (e) =>
      handler(e.payload.session, e.payload.paths),
    ),

  /**
   * One coalesced burst of filesystem change, already ignore-filtered. (M8)
   *
   * A whole `cargo build` is one of these. `truncated` means the burst was larger than the
   * watcher holds, so the paths are a prefix and anything you care about should be re-read.
   * `git` means `HEAD`, the index or a ref moved — the branch readout and the git panel.
   */
  onFsChanged: (handler: (project: ProjectId, change: FsChange) => void) =>
    listen<{ project: ProjectId; change: FsChange }>('cide://fs-changed', (e) =>
      handler(e.payload.project, e.payload.change),
    ),

  /**
   * Indexing started or finished, or the watcher degraded.
   *
   * `watch.backend === 'polling'` is the degraded mode: `watch.reason` is a sentence written
   * for a banner, and it names the sysctl that fixes it.
   */
  onFsStatus: (handler: (project: ProjectId, status: FsStatus) => void) =>
    listen<{ project: ProjectId; status: FsStatus }>('cide://fs-status', (e) =>
      handler(e.payload.project, e.payload.status),
    ),

  /**
   * A project's changes tree was recomputed.
   *
   * Fires after any git mutation cide made, in every window, and carries the new tree — so a
   * subscriber pays no round trip. `cmd/git.rs::refreshed` is its only emitter.
   *
   * Changes made *outside* cide — a `git add` in a bash pane, a `git pull`, a formatter — do not
   * fire it. Those arrive as `cide://fs-changed`, and **a surface that wants both has to
   * subscribe to both.** The final clause of this comment used to say the git panel's refresh
   * was wired to the watcher; it was not, and had not been for several milestones, so the panel
   * went stale on every change cide did not make while the badge in its own header — which
   * *was* wired to it — stayed correct and disagreed. Both subscribe now, as do the file tree,
   * its status tags, the log, blame, the diff pane and the branch readout.
   */
  onGitStatus: (handler: (project: ProjectId, tree: ChangesTree) => void) =>
    listen<{ project: ProjectId; tree: ChangesTree }>('cide://git-status', (e) =>
      handler(e.payload.project, e.payload.tree),
    ),

  /**
   * A project's merged diagnostics changed. (M12)
   *
   * Carries **only** the project id: the snapshot depends on the user's inspection settings, and
   * the receiver already has those where the emitter would have to read them. So this is a
   * "something moved" ping and the handler answers with `diagnostics.get`.
   *
   * Already coalesced in Rust — rust-analyzer publishes per file, and a workspace check is
   * hundreds of publishes in a burst. Do not add a second debounce here; a throttle on the
   * *refresh* is a different thing and is what `diagnosticsStore` does.
   */
  onDiagnostics: (handler: (project: ProjectId) => void) =>
    listen<{ project: ProjectId }>('cide://diagnostics', (e) => handler(e.payload.project)),
}

/**
 * The commit tool window's whole surface.
 *
 * Every mutation answers with the new `ChangesTree` rather than an acknowledgement. The panel
 * is a tri-state tree in which moving one file changes the state of every group above it, so
 * a call that returned `{ok: true}` would be followed at once by a second asking what
 * happened — and the frame in between shows a tree that is visibly wrong.
 *
 * `repo` is always required, even for a single-root project: a project can hold several roots
 * and each root its submodules, and every one of them is a repository with its own index.
 */
export const git = {
  /** `includeIgnored` walks ignored files, which on a repo with a big `target/` is slow. */
  status: (project: ProjectId, includeIgnored = false) =>
    invoke<ChangesTree>('git_status', { project, includeIgnored }),

  /**
   * Per-path status for the **file tree**, keyed by the absolute path a `TreeRow` carries.
   *
   * Not `status()` in a different shape: that returns the commit panel's tri-state tree, both
   * sides of the index per path, repo-relative. This returns one letter per path, already
   * rebased onto the project's roots and already rolled up into the directories above each
   * change.
   *
   * There is no `paths` argument and it is not called per scroll. The map is bounded by the
   * number of *changed* paths rather than by the size of the repository, so it is read once
   * per invalidation — `git.onGitStatus` and `events.onFsChanged` — and looked up locally.
   * Every project it is asked about is answered: a root with no repository above it simply
   * has an empty map.
   */
  treeStatus: (project: ProjectId) =>
    invoke<TreeStatusMap>('git_tree_status', { project }),

  /**
   * Which repositories the project actually contains, roots before their submodules.
   *
   * The cheapest git question there is — `Repository::discover` per root plus a `submodules()`
   * walk, no working-tree walk anywhere — which is what makes it callable from a command
   * handler on the keystroke that runs the command.
   *
   * Asked over the wire rather than read off the workspace mirror, and that is the point.
   * `ProjectRoot` used to carry a `repo` field; Rust never filled it, `keys/target.ts` believed
   * it, and the resulting `repoOpen` context flag was false for every project ever opened —
   * which hid the whole Git group from the command palette. Whether a project has a repository
   * is a fact about the disk that a `git init` in a bash pane changes, so it is asked at the
   * moment it is needed and never cached.
   */
  repos: (project: ProjectId) => invoke<RepoInfo[]>('git_repos', { project }),

  branchInfo: (project: ProjectId, repo: RepoId) =>
    invoke<BranchInfo>('git_branch_info', { project, repo }),

  /**
   * One file's diff. `side` picks the pair: `staged` is HEAD→index, `unstaged` is
   * index→working tree, `combined` is HEAD→working tree and is what the changelist panel
   * shows.
   *
   * Carry the returned `rev` back on any `PathSelection` built from this diff. Staging
   * refuses a mismatch, which is what stops a selection made while Claude was editing from
   * being applied to lines that have since moved.
   */
  diffFile: (project: ProjectId, repo: RepoId, path: string, side: DiffSide) =>
    invoke<FileDiff>('git_diff_file', { project, repo, path, side }),

  /** Which lines a selection resolves to, without applying it — for the tri-state boxes. */
  resolveSelection: (
    project: ProjectId,
    repo: RepoId,
    side: DiffSide,
    selection: PathSelection,
  ) => invoke<LineRef[]>('git_resolve_selection', { project, repo, side, selection }),

  stage: (project: ProjectId, repo: RepoId, selections: PathSelection[]) =>
    invoke<ChangesTree>('git_stage', { project, repo, selections }),

  unstage: (project: ProjectId, repo: RepoId, selections: PathSelection[]) =>
    invoke<ChangesTree>('git_unstage', { project, repo, selections }),

  /** **Destroys uncommitted work.** Confirm before calling; Rust will not ask. */
  rollback: (project: ProjectId, repo: RepoId, selections: PathSelection[]) =>
    invoke<ChangesTree>('git_rollback', { project, repo, selections }),

  /**
   * Rejected with `indexChangedExternally` when someone ran `git add` outside cide. That is
   * the guard bar, not a failure: offer `adoptIndex` (reload) or re-send with
   * `request.force` (overwrite).
   */
  commit: (project: ProjectId, repo: RepoId, request: CommitRequest) =>
    invoke<CommitOutcome>('git_commit', { project, repo, request }),

  /**
   * Shells out to `git push` when a credential helper is configured or the remote is HTTPS.
   *
   * Answers with the counts as well as the transport's text since M20 — `pushNote` is what
   * turns them into a sentence, and they are read from the object database before the push
   * rather than parsed out of git's output, which on the binary route is the remote server's.
   */
  push: (
    project: ProjectId,
    repo: RepoId,
    remote: string | null = null,
    refspec: string | null = null,
    setUpstream = false,
  ) => invoke<PushOutcome>('git_push', { project, repo, remote, refspec, setUpstream }),

  /** The "reload" half of the guard bar: accept the index as it now stands. */
  adoptIndex: (project: ProjectId, repo: RepoId) =>
    invoke<ChangesTree>('git_adopt_index', { project, repo }),

  /** IDEA's "use Git staging area instead". In this mode cide never rebuilds the index. */
  setUseStagingArea: (project: ProjectId, repo: RepoId, enabled: boolean) =>
    invoke<ChangesTree>('git_set_use_staging_area', { project, repo, enabled }),

  changelist: {
    create: (project: ProjectId, repo: RepoId, name: string, comment = '') =>
      invoke<ChangesTree>('git_changelist_create', { project, repo, name, comment }),
    rename: (project: ProjectId, repo: RepoId, id: string, name: string, comment = '') =>
      invoke<ChangesTree>('git_changelist_rename', { project, repo, id, name, comment }),
    /** Its paths fall back to the default list; the default list itself cannot be deleted. */
    delete: (project: ProjectId, repo: RepoId, id: string) =>
      invoke<ChangesTree>('git_changelist_delete', { project, repo, id }),
    movePaths: (project: ProjectId, repo: RepoId, id: string, paths: string[]) =>
      invoke<ChangesTree>('git_changelist_move_paths', { project, repo, id, paths }),
    /** Only *new* changes join the newly active list; existing ones stay where they are. */
    setActive: (project: ProjectId, repo: RepoId, id: string) =>
      invoke<ChangesTree>('git_changelist_set_active', { project, repo, id }),
  },

  /** cide's own patch files. Separate from the stash, which is git's — see `git.stash`. */
  shelf: {
    list: (project: ProjectId, repo: RepoId) =>
      invoke<ShelfEntry[]>('git_shelf_list', { project, repo }),
    shelve: (project: ProjectId, repo: RepoId, name: string, selections: PathSelection[]) =>
      invoke<ShelfEntry>('git_shelve', { project, repo, name, selections }),
    /** `keep` leaves the patch on the shelf — IDEA's "unshelve and keep". */
    unshelve: (project: ProjectId, repo: RepoId, id: string, keep = false) =>
      invoke<ChangesTree>('git_unshelve', { project, repo, id, keep }),
    patch: (project: ProjectId, repo: RepoId, id: string) =>
      invoke<string>('git_shelf_patch', { project, repo, id }),
    drop: (project: ProjectId, repo: RepoId, id: string) =>
      invoke<ShelfEntry[]>('git_shelf_drop', { project, repo, id }),
  },

  /** Real `git stash` entries, which a terminal can also see. */
  stash: {
    list: (project: ProjectId, repo: RepoId) =>
      invoke<StashEntry[]>('git_stash_list', { project, repo }),
    save: (project: ProjectId, repo: RepoId, message: string, includeUntracked = false) =>
      invoke<ChangesTree>('git_stash_save', { project, repo, message, includeUntracked }),
    pop: (project: ProjectId, repo: RepoId, index: number) =>
      invoke<ChangesTree>('git_stash_pop', { project, repo, index }),
    apply: (project: ProjectId, repo: RepoId, index: number) =>
      invoke<ChangesTree>('git_stash_apply', { project, repo, index }),
    drop: (project: ProjectId, repo: RepoId, index: number) =>
      invoke<StashEntry[]>('git_stash_drop', { project, repo, index }),
  },
}

/** The window label this webview was opened with, e.g. `shell:<uuid>`. */
export function windowLabel(): string {
  return new URLSearchParams(location.search).get('window') ?? 'shell:unknown'
}

/** `shell` | `pane` | `tab` — which role this window plays. */
export function windowRole(): 'shell' | 'pane' | 'tab' {
  const prefix = windowLabel().split(':')[0]
  return prefix === 'pane' || prefix === 'tab' ? prefix : 'shell'
}

/**
 * Files and file tabs. (M9)
 *
 * `read` and `write` are the only commands in the app that do arbitrary blocking IO on a
 * path the user chose, and they are `async` on the Rust side for exactly that reason — a
 * project on a stalled network mount would otherwise take the event loop, and with it every
 * terminal in the window.
 *
 * The text crosses in both directions exactly as it sits on disk, line endings included.
 * Normalising at this boundary would be convenient and would also destroy the only record
 * of what a CRLF file's endings were; see `editor/lineEndings.ts`.
 */
export const file = {
  /** Open a file tab, or activate the one already showing this path. */
  open: (projectId: ProjectId, path: string) =>
    invoke<TabId>('tab_open_file', { project: projectId, path }),

  /**
   * Reopen a file the **navigation history** remembers. What Back and Forward walk into.
   *
   * Not `open`, and the difference is two things `open` cannot do. It consults the closed-tab
   * stack, so a Back into a file that was closed while split brings the *split* back — pane ids
   * and live Claude sessions included — instead of minting a fresh single-pane editor and
   * leaving the record behind to poison the next Ctrl+Shift+T. And it stats first, so a Back
   * into a deleted file or a discarded scratch answers `gone` instead of creating a permanent
   * tab reading "This file could not be opened".
   *
   * Answers rather than rejects, because none of the three outcomes is a failure: `restored`
   * and `opened` both just need a hydrate, and `gone` is an informational notice — the same
   * division `tab.reopenClosed` makes for the same reason.
   */
  reopen: (projectId: ProjectId, path: string) =>
    invoke<ReopenedFile>('tab_reopen_file', { project: projectId, path }),

  /**
   * Open a file a **terminal pane** named. Same tab list, entirely different trust.
   *
   * A separate command rather than an argument to `open`, because the difference is not a flag
   * — it is who chose the path. Every caller of `open` hands back a path the backend itself
   * produced (the tree, the picker, the git panel), and `tab_open_file` enforces nothing at
   * all. This one's argument was parsed out of a pane's bytes, which are a repository's build
   * output, a file some tool printed, a tool result — attacker-influenced by definition.
   *
   * So Rust checks, on every call: absolute and free of `..`, canonicalised, a regular file (not
   * a directory, not a device, and above all not a FIFO), under the editor's size limit *before*
   * a tab exists, and inside a project root. It rejects with a typed
   * `{kind, message, path, real}` the notice stack shows verbatim — a refusal the user cannot
   * see is indistinguishable from a link wired to nothing.
   *
   * `approvedTarget` is the answer to the one refusal a user may overrule. Absent — every first
   * click, and every click on a path the project contains — nothing has changed. Present, it is
   * the **canonical path the confirmation named**, and Rust re-canonicalises and compares, so an
   * approval is an approval of a file rather than of a string. The rules for when that dialog
   * may be offered at all are in `terminal/outsideOpen.ts`; `OutsideOpenGate` draws it.
   *
   * There is no line argument: the caret is `editor/revealRequest.ts`'s job and is requested
   * on this side, before this call, for the reason written there.
   */
  openFromTerminal: (projectId: ProjectId, path: string, approvedTarget?: string | undefined) =>
    invoke<TabId>('terminal_open_path', {
      project: projectId,
      path,
      approvedTarget: approvedTarget ?? null,
    }),

  /** Record whether a file tab has unsaved edits. This is what draws the tab's dirty dot. */
  setDirty: (projectId: ProjectId, id: TabId, dirty: boolean) =>
    invoke<{ rev: number }>('tab_set_dirty', { project: projectId, tab: id, dirty }),

  /**
   * Open the three-pane conflict resolver for one path, or activate the tab already on it. (M20)
   *
   * On `file` rather than on `branch` because it opens a **tab**, which is what every other
   * `tab_open_*` here does; the conflict *verbs* live on `branch` beside `pull`, which is what
   * lands the conflict in the first place.
   */
  openMergeTab: (projectId: ProjectId, repo: string, path: string) =>
    invoke<TabId>('tab_open_merge', { project: projectId, repo, path }),

  read: (path: string) => invoke<FileDoc>('file_read', { path }),

  /**
   * Write a buffer back, optionally only if the file on disk has not moved.
   *
   * `ifUnchanged: null` writes unconditionally — what Ctrl+S passes, because that is the user
   * deciding. **Autosave** passes the stamp `FileDoc` came back with, and a mismatch rejects
   * with `CoreError::FileChanged`, which `fileChanged()` above recognises and `EditorPane` turns
   * into the conflict bar. See `cide_ipc::FileStamp` for why this is a precondition rather than
   * a watcher subscription.
   *
   * Returns the file's new stamp, so the caller's token moves with the write.
   */
  write: (path: string, text: string, ifUnchanged: FileStamp | null = null) =>
    invoke<FileStamp | null>('file_write', { path, text, ifUnchanged }),

  /**
   * Remember where the user is in a file. (M12)
   *
   * **Not a workspace mutation, and that is the whole design.** `setDirty` above goes through
   * `WorkspaceState::update`, which bumps `rev` and broadcasts the entire tree to every window;
   * that is right for a dirty dot and ruinous for something the editor reports as the user
   * scrolls. This one lands in a separate store with its own two-second debounce and no event
   * at all — `crates/cide-app/src/positions_state.rs` has the four-stage ladder that keeps a
   * scroll gesture from becoming a disk write.
   *
   * `touchedAt` is filled in by Rust whatever is sent: it is the eviction key of a 256-entry
   * LRU, and a renderer's clock is not what should order it. Zero is the honest thing to send.
   *
   * Fire-and-forget, like `setDirty`: the authoritative answer to "where am I in this file" is
   * the buffer on screen, so nothing waits for the round trip and a failure costs a position
   * rather than interrupting a scroll.
   */
  notePosition: (at: Omit<ViewPosition, 'touchedAt'>) =>
    invoke<void>('file_note_position', { at: { ...at, touchedAt: 0 } }),

  /** Where the user last was in `path`, or `null`. */
  position: (path: string) => invoke<ViewPosition | null>('file_position', { path }),
}

/**
 * Images. Two calls, and the second one is not an `invoke` at all. (M18)
 *
 * The split matters, so it is stated here rather than left to be discovered:
 *
 * * `read` is an ordinary command. It answers **identity** — format, byte size, mtime, and the
 *   canonical path — and it grants this webview permission to fetch that one file over the
 *   asset protocol. It rejects with a `CoreError` whose message is a sentence: too large, not
 *   a regular file, empty, or the headline case, a name that promised a PNG over bytes that
 *   are not one.
 * * `srcUrl` builds the URL the `<img>` actually loads. **No pixels ever cross the IPC.**
 *   See `crates/cide-app/src/cmd/file.rs::image_read` for why — in one line, Tauri's IPC is
 *   JSON, and a 40 MB PNG base64'd into a JSON string is over 100 MB of peak footprint on the
 *   thread that draws every terminal in the window.
 */
export const image = {
  read: (path: string) => invoke<ImageDoc>('image_read', { path }),

  /**
   * The URL for an image `read` has already vouched for.
   *
   * `convertFileSrc` is the fourth import from `@tauri-apps/api` in this file and it is the
   * one that is neither `invoke` nor `listen`. It belongs here anyway, for the reason the
   * header gives about the whole module: it is a seam onto Tauri, and a seam that lives in a
   * component is a seam nobody can grep for. It is also not a call — it reads
   * `window.__TAURI_INTERNALS__` and returns a string — so it costs nothing to route through
   * a function.
   *
   * Built from `doc.path`, which is **canonical**, because that is what Tauri's asset scope
   * matches against; see `ImageDoc::path`.
   *
   * The query string is a cache-buster and not decoration. The asset protocol serves a
   * cacheable HTTP-shaped response keyed by URL, so an image an agent has just rewritten would
   * otherwise keep showing yesterday's pixels for the life of the tab. Tauri's handler reads
   * `request.uri().path()`, which excludes the query, so this is invisible to the Rust side.
   * A doc with no stamp — a filesystem that would not answer — simply gets no buster, which is
   * the behaviour cide had before and not worth a failure.
   */
  srcUrl: (doc: ImageDoc): string => {
    const base = convertFileSrc(doc.path)
    return doc.stamp === null ? base : `${base}?v=${doc.stamp.mtimeNanos}-${doc.stamp.len}`
  },
}

/* --------------------------------------------------------------------------------------
 * Removed at integration: the provisional `git`, `fs` and `picker` namespaces and their
 * hand-written `TreeRow` / `PickerFrame` / `TreeStatus` types.
 *
 * Two agents built each of these in parallel — one the Rust half with real DTOs in
 * `cide-ipc`, one the frontend half against guessed command names and `invoke<unknown>`.
 * Both landed. The generated-DTO versions above are the real ones; these were scaffolding
 * that existed only so the panels could be built and reviewed before their backend did.
 * Keeping both would have left two `export const git` in one module and a `TreeRow` that
 * shadows the generated type it was standing in for.
 * ------------------------------------------------------------------------------------ */
export const fsEvents = {
  /*
   * `onChanged` used to live here and is deliberately gone. It listened to `cide://fs-changed`
   * and read `e.payload.paths`, but `emit::FsChanged` puts them at `payload.change.paths`, so
   * its `paths` argument was `undefined` for every burst. `events.onFsChanged` above is the
   * one that matches the wire.
   *
   * It survived a long time because nothing called it. The moment something did — the terminal
   * link cache — the destructuring threw inside the listener, and because `listen()`'s promise
   * had already resolved, the `.catch` guarding that call site never ran and the failure was
   * silent for the life of the window. `Explorer.tsx` had found the trap earlier and chose to
   * document it rather than remove it; a comment on the *correct* helper cannot stop someone
   * reaching for the wrong one by name, and one caller later that is exactly what happened.
   */

  /** Walk progress, for the picker's `Indexing…` state and the explorer's meta counter. */
  onIndexProgress: (handler: (project: ProjectId, indexed: number, total: number) => void) =>
    listen<{ project: ProjectId; indexed: number; total: number }>(
      'cide://fs-index-progress',
      (e) => handler(e.payload.project, e.payload.indexed, e.payload.total),
    ),
}

/**
 * Run a command that may not exist in this build yet.
 *
 * Tauri answers an unregistered command with a rejected promise naming it, and an unhandled
 * rejection inside a `useEffect` is a blank sidebar plus a console line nobody reads. This
 * turns that into a value: `fallback` on failure, and one `diag.log` line the Rust log
 * carries — at most once per command name, so a virtualizer scrolling over a missing
 * `fs_tree_rows` does not write a thousand of them.
 *
 * Deliberately swallows *every* rejection, not only "command not found": Tauri does not
 * distinguish the two on the wire, and a picker that throws on an unreadable path is no more
 * useful to the user than one that returns nothing.
 */
export async function pendingCommand<T>(
  name: string,
  call: () => Promise<T>,
  fallback: T,
): Promise<T> {
  try {
    return await call()
  } catch (e) {
    if (!degraded.has(name)) {
      degraded.add(name)
      void diag.log(`[cide] ${name} unavailable, degrading: ${String(e)}`)
    }
    return fallback
  }
}

const degraded = new Set<string>()

/** True once `pendingCommand` has seen this command fail. Drives the sidebar's dim notice. */
export function isDegraded(name: string): boolean {
  return degraded.has(name)
}

/**
 * The three session calls that have to name a pane. (M7)
 *
 * A sink used to be keyed `(session, window)`, so two panes mirroring one session in one
 * window were one attachment and the second silently detached the first — the one case
 * mirroring exists for. The pane id is part of that key now, which means `attach`, `ack` and
 * `detach` all have to carry it: an ack that names no pane credits a different sink, and a
 * detach that names no pane drops a different pane's view.
 *
 * These supersede `session.attach` / `session.ack` / `session.detach` for anything that
 * lives in a pane, which today is `TerminalPane` and nothing else. The originals still work
 * — `pane` is optional on the Rust side, and omitting it means the old window-wide slot — so
 * a caller outside a pane is not broken by this. New pane callers use these.
 */
export const paneSession = {
  /**
   * Attach this pane's sink, and receive the screen it should start from.
   *
   * The returned bytes are the screen **as of the moment this sink was registered**, taken on
   * the coalescer thread so the two cannot disagree. Asking `session.scrollback` first and
   * attaching second is the shape this replaces: the mirror runs ahead of the sinks by up to
   * a flush interval, so anything that arrived in between was painted from the snapshot and
   * then delivered again as live output.
   *
   * The caller must write these bytes before any bytes the channel delivers. See
   * `TerminalPane`, which queues channel frames until it has.
   */
  attach: (pane: PaneId, id: SessionId, geo: Geometry, onData: (data: ArrayBuffer) => void) => {
    const sink = new Channel<ArrayBuffer>()
    sink.onmessage = onData
    return invoke<ArrayBuffer>('session_attach', { session: id, pane, sink, geometry: geo })
  },

  /**
   * Report that this pane has parsed `bytes`. Fire-and-forget for the same reason
   * `session.ack` is: it runs in `term.write`'s completion callback, once per frame.
   */
  ack: (pane: PaneId, id: SessionId, bytes: number) => {
    void invoke<void>('session_ack', { session: id, pane, bytes }).catch(ackFailed)
  },

  /** Drop this pane's sink. The child keeps running; only this view of it ends. */
  detach: (pane: PaneId, id: SessionId) => invoke<void>('session_detach', { session: id, pane }),
}

/* --------------------------------------------------------------------------------------
 * M11: the two named uses of the headless lane, the CLI version verdict, and the log
 * directory. Appended as one block, per the house rule about this file.
 * ------------------------------------------------------------------------------------ */

/**
 * Whether the installed `claude` is one this build's IDE protocol was checked against.
 *
 * The verdict is computed in Rust and the range is never mirrored here: it lives in
 * `cide_ide_mcp::protocol::SUPPORTED_CLI`, and a copy in TypeScript would be a second place
 * to update on the next CLI release — which is precisely the drift this exists to notice.
 */
export interface ClaudeCliSupport {
  /** The parsed version, or null when nothing answered `--version`. */
  version: string | null
  /** The versions the protocol was verified against, already formatted for a human. */
  verifiedRange: string
  /** One sentence, or null when there is nothing worth saying. */
  warning: string | null
  /**
   * The last `claude` to complete the IDE handshake **on this machine**, or null if none has.
   *
   * A different question from the three fields above, which are all a property of the source
   * tree measured against a probe of `PATH`. This is a per-machine observation: a real CLI
   * read our lockfile, chose the WebSocket transport, presented the auth header and had its
   * `initialize` reply accepted, here. It is the only thing on the Settings screen that is
   * evidence rather than a record of what somebody typed into a constant.
   *
   * Mirrors `cide_core::handshake::Handshake`. Hand-mirrored like the rest of this interface,
   * because `ClaudeCliSupport` is a plain `serde::Serialize` local to `cmd/settings.rs` rather
   * than a `cide-ipc` ts-rs DTO — so nothing generates it and it moves with the Rust by hand.
   */
  handshake: {
    /** What the CLI called itself in `initialize`; null when it named no version. */
    version: string | null
    /** Unix milliseconds. */
    atUnixMs: number
    /** The verified range as it stood when this was recorded, so a stale record reads stale. */
    verifiedRange: string
  } | null
  /**
   * The configured binary this whole verdict is about, echoed back. (M16)
   *
   * Echoed rather than assumed, because `version` and `warning` describe *this* value: a screen
   * that captioned them with whatever is currently in the input would label one binary's
   * version with another's name for as long as a probe was in flight.
   */
  binary: string
  /** The absolute path it resolves to, or null when it cannot be run. Shown, never spawned. */
  resolved: string | null
  /**
   * Why it cannot be run at all — the only hard verdict on this screen. Null when it can.
   *
   * A wrapper script, a `--version` that prints nothing parseable, a version past the verified
   * range: all warnings, none of them this. `mise`, `asdf` and `direnv` are legitimate ways to
   * name a `claude` and none of them answers `--version` in a shape worth refusing over.
   */
  problem: string | null
  /**
   * Every refusal and warning sentence, keyed by the flag or variable it belongs to.
   *
   * The names are matched in `settings/claudeCli.ts` for instant feedback with no round trip;
   * only the PROSE comes from Rust, because a sentence written twice is a sentence that will say
   * two things. Before this existed the screen struck a token out and never said why —
   * `Verdict::note()` had one caller and it was a `tracing` line.
   */
  argReasons: { name: string; reason: string }[]
  envReasons: { name: string; reason: string }[]
  /**
   * Why a rename typed into *What cide adds to the command line* was thrown away, keyed by the
   * injection's wire name (`sessionId`, `resume`, `forkSession`, `settings`).
   *
   * Keyed by the injection and not by the text, because the text is exactly what was discarded
   * — and the same string may appear as a user argument elsewhere on the screen, where it means
   * something else and has a different sentence. Empty in the common case: a rename that
   * survived says nothing.
   */
  injectReasons: { name: string; reason: string }[]
}

/**
 * One-shot Claude tasks that build their own prompt in Rust.
 *
 * Deliberately not `headless.run` with a prompt assembled here. The prompts have rules in
 * them — no preamble, a bounded diff, "say when you cannot see the rest of the file" — and
 * every one of those failures is silent and lands in the user's commit box. They are tested
 * functions in `cide_claude::prompt`; this is the seam that reaches them.
 */
export const claudeTasks = {
  /**
   * Draft a commit message from what a commit would record.
   *
   * Rejects with `{kind: 'nothingToDescribe'}` when there is no diff, which is a disabled
   * button rather than an error toast.
   */
  commitMessage: (projectId: ProjectId, repo: RepoId) =>
    invoke<HeadlessResult>('claude_commit_message', { project: projectId, repo }),

  /**
   * Explain a selection. The *text* travels, not a path and a range: the buffer on screen may
   * be dirty, and explaining what is on disk when the user asked about what they can see is
   * the kind of wrong answer nobody catches.
   */
  explainSelection: (
    projectId: ProjectId,
    path: string,
    startLine: number,
    endLine: number,
    text: string,
    language: string | null = null,
  ) =>
    invoke<HeadlessResult>('claude_explain_selection', {
      project: projectId,
      path,
      startLine,
      endLine,
      text,
      language,
    }),

  /**
   * The CLI-version verdict. Degrades to "nothing to report" rather than throwing, because
   * it renders inside a settings section that must still draw without it.
   */
  cliSupport: (): Promise<ClaudeCliSupport> =>
    pendingCommand<ClaudeCliSupport>(
      'claude_cli_support',
      () => invoke<ClaudeCliSupport>('claude_cli_support'),
      {
        version: null,
        verifiedRange: 'unknown',
        warning: null,
        handshake: null,
        // Empty rather than absent: the screen looks a sentence up by name and an empty table
        // simply finds none, which is the same degraded-but-drawable shape every other field
        // here takes. A missing key would be a crash in the one path that exists so the section
        // still renders when the command is unavailable.
        argReasons: [],
        envReasons: [],
        injectReasons: [],
        // Not `null` and not the default `'claude'`: an empty string is what the screen reads
        // as "no answer yet", and inventing a binary name here would caption the fallback with
        // a claim about a program nothing probed.
        binary: '',
        resolved: null,
        problem: null,
      },
    ),
}

/**
 * Open the directory `tauri-plugin-log` writes to, and return its path.
 *
 * Rust does the opening: the `opener` plugin's JS command is capability-gated per window and
 * a detached-pane window deliberately has none, so going through it would make this work in
 * some windows and not others for reasons no user could guess.
 */
export function openLogDir(): Promise<string> {
  return invoke<string>('app_open_log_dir')
}

/* --------------------------------------------------------------------------------------
 * Content search — the sidebar's ⌕ panel. (M11)
 *
 * Not the picker. `picker.query` ranks *paths* by a fuzzy score; this greps file *contents*
 * and returns every matching line in walk order. They share the word and nothing else.
 * ------------------------------------------------------------------------------------ */

/**
 * Ask for a page of a content search, starting one if this query is not the one running.
 *
 * A poll, like `picker.query`, and for the same reason: the walk produces results for
 * seconds and the panel has to draw the first ones immediately. Call it again while
 * `frame.running` is true, with `offset` advanced past the hits already appended — a poll
 * during a search that has found 4 000 hits then carries the few found since the last one,
 * not 4 000 rows a second time.
 *
 * The whole `query` is echoed back on the frame. Compare it before painting: a frame for the
 * query the user has already typed past must be dropped, and the toggles are part of the
 * identity (`case sensitive` off and on are two different searches over one pattern).
 *
 * Rejects with `NoIndex` for a project whose file walk has not started; that is the
 * "opening…" state, not an error. See `store/fileIndex.ts`'s `isNoIndex`.
 */
export const search = {
  query: (projectId: ProjectId, query: SearchQuery, offset = 0, limit?: number) =>
    invoke<SearchFrame>('search_query', {
      project: projectId,
      query,
      offset,
      limit: limit ?? null,
    }),

  /**
   * Stop the project's search and forget its results.
   *
   * Idempotent, and safe on a project that never searched — `false` simply means there was
   * nothing to stop. The panel calls it when it unmounts or its input is emptied, so that a
   * walk of a large repository does not keep running for a panel nobody can see.
   */
  cancel: (projectId: ProjectId) => invoke<boolean>('search_cancel', { project: projectId }),
}

/* ------------------------------------------------------------------------------------
 * The git diff tab. (M11)
 *
 * Appended as one block per the house rule about this file. It is separate from the `git`
 * namespace above because it is not a git command at all: `tab_open_diff` mutates the
 * workspace, the way `file.open` does, and the diff itself still comes from `git.diffFile`.
 * ------------------------------------------------------------------------------------ */
export const gitDiff = {
  /**
   * Open a diff tab for one file, or activate the one already showing it.
   *
   * Takes a *key*, not a diff. The caller usually has a `FileDiff` in hand and it is
   * deliberately not accepted: `DiffSpec` is persisted to `workspace.json`, so a diff passed
   * here would be a diff written to disk and stale by the time anything read it back. The
   * pane re-reads through `git.diffFile` with this key — the same arrangement as a Claude
   * diff and `claude.diffContent`.
   *
   * `side` is the side the tab *opens* on; the pane can switch afterwards. It matters
   * because a selection is only valid for the operation whose side it was made against —
   * `git.stage` re-derives with `unstaged`, `git.unstage` with `staged`, `git.commit` with
   * `combined`.
   *
   * `oldPath` is git's pre-image path, for a rename. Display only.
   *
   * No `hydrate()` afterwards, unlike `file.open`'s call sites. `tab_open_diff` goes through
   * `WorkspaceState::update`, which broadcasts `cide://workspace-changed` with the new
   * snapshot to every window, and `store/workspace` applies it — so the tab appears on its
   * own. Calling `hydrate()` as well would only add a round trip that answers with what has
   * already arrived.
   */
  openTab: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    side: DiffSide,
    oldPath: string | null = null,
  ) => invoke<TabId>('tab_open_diff', { project, repo, path, side, oldPath }),

  /**
   * Point the diff the user is reading at another file — a *single* click on a changelist
   * row while a diff is already open.
   *
   * > *"Only when diff is already opened one click should change current diff to selected
   * > file."*
   *
   * Same arguments as {@link openTab} and a different operation, which is the whole point:
   * `openTab` reuses a tab only when the repo **and** the path match, so routing a single
   * click there gave a new tab per file and clicking down a 30-file changelist produced 30
   * tabs. This retargets one tab instead.
   *
   * Which tab, and what happens to one the user meant to keep, is decided in Rust —
   * `cmd::file::tab_retarget_diff` — because it is a workspace question and a second window
   * asks it too. In short: the tab already showing this file wins, then the preview tab, then
   * a new preview tab. Double-click (`openTab`) marks a tab kept, and a kept tab is never
   * retargeted.
   *
   * Answers with the tab that ended up showing the file, and needs no `hydrate()` for the
   * same reason `openTab` does not: the mutation broadcasts `cide://workspace-changed`.
   */
  retargetTab: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    side: DiffSide,
    oldPath: string | null = null,
  ) => invoke<TabId>('tab_retarget_diff', { project, repo, path, side, oldPath }),
}

/* --------------------------------------------------------------------------------------
 * Input-path integrity. Appended as one block, per the house rule about this file.
 *
 * Two things that were previously invisible: a write that may be delivered twice, and an ack
 * that failed and told nobody.
 * ------------------------------------------------------------------------------------ */

/**
 * The IPC transport has degraded to `postMessage`. (M11)
 *
 * Not on the `events` object above only because of the house rule about this file; it is an
 * ordinary event listener and behaves like the ones there.
 *
 * Fires whenever `diag.reportIpc` is told the custom protocol is not in use — at boot, and
 * again from `watchTransport`'s re-probe if it fails later. It is the one thing that turns "the
 * app feels slow" into a fact, so it belongs in front of the user and not only in the log.
 */
export const onIpcDegraded = (handler: (health: IpcHealth) => void) =>
  listen<{ health: IpcHealth }>('cide://ipc-degraded', (e) => handler(e.payload.health))

/** The last sequence number handed to `session.write`, per session. */
const writeSeq = new Map<SessionId, number>()

/**
 * The next write sequence number for this session.
 *
 * Monotonic per session and never reset while the window lives. Keyed on the *session*
 * because that is what the Rust side dedupes against: two mirrored panes write into one
 * child, and a per-pane counter would give them colliding numbers, so the second pane's
 * first keystroke would be dropped as an already-applied retry.
 *
 * A finished session leaves its entry behind. That is one number per session ever typed
 * into, which is not a size worth managing, and clearing it would be worse: a late retry of
 * the last write from a session being torn down would find the counter gone and start again
 * from 1, which is the duplicate this exists to prevent.
 */
function nextWriteSeq(id: SessionId): number {
  const next = (writeSeq.get(id) ?? 0) + 1
  writeSeq.set(id, next)
  return next
}

/** Whether a failed ack has already been reported, so the render path cannot flood the log. */
let ackFailureReported = false

/**
 * Report an ack that did not land.
 *
 * This used to be `.catch(() => {})` on the one call that keeps credit flowing. An ack that
 * never arrives leaves its bytes outstanding for ever; once they pass `CreditPolicy.high` the
 * sink is choked and the pane stops receiving raw output until the watchdog forgives it,
 * over and over. The symptom is a terminal that feels slow with nothing anywhere saying why —
 * which is exactly the kind of report this block was written for.
 *
 * Still fire-and-forget, and still tolerant: an ack racing a pane close is ordinary. Once per
 * window, because it runs in `term.write`'s completion callback and a per-frame log line
 * would be its own slowdown.
 */
function ackFailed(error: unknown): void {
  if (ackFailureReported) return
  ackFailureReported = true
  const line = `session ack failed — this pane's flow control is now one-sided: ${String(error)}`
  console.error(`[cide] ${line}`)
  void diag.log(line).catch(() => {})
}

/**
 * The system clipboard, for the code pane's Cut / Copy / Paste.
 *
 * These are **not cide commands** — they are `tauri-plugin-clipboard-manager`'s, invoked
 * directly rather than through `@tauri-apps/plugin-clipboard-manager`. The plugin is already
 * a dependency of `cide-app` and registered in `lib.rs`; its JS package is not a dependency
 * of `ui`, and its whole content is the two `invoke` lines below. Adding a package to fetch
 * a wrapper this thin, in a repo that pins every version exactly, lost to writing it out —
 * and writing it out is also what keeps the rule at the top of this file true, that this is
 * the only file reaching for `@tauri-apps`.
 *
 * They do not appear in `contract/commands.json` for the same reason: `contract-check`
 * reflects over the commands *this workspace* registers, and a plugin's are not ours to pin.
 * What does have to be granted is the permission — `clipboard-manager:default` deliberately
 * enables nothing at all ("we believe the clipboard can be inherently dangerous"), so
 * `clipboard-manager:allow-read-text` and `allow-write-text` are named explicitly in both
 * files under `crates/cide-app/capabilities/`. Without them these two reject, which is why
 * `editor/clipboard.ts` probes rather than assuming.
 *
 * `read_text` **rejects** on an empty or non-text clipboard rather than answering `null`, so
 * every caller has to handle a rejection anyway; none of this is wrapped in a swallowing
 * `.catch` here, because a Paste that silently does nothing is precisely the dead control
 * this feature exists to avoid.
 */
export const clipboard = {
  /** The clipboard's text. Rejects when it holds none, and when the permission is missing. */
  readText: () => invoke<string>('plugin:clipboard-manager|read_text'),
  /** Replace the clipboard's text. `label` is a Linux-only clipboard selector; we want the default. */
  writeText: (text: string) =>
    invoke<void>('plugin:clipboard-manager|write_text', { text, label: null }),
}

/* --------------------------------------------------------------------------------------
 * The sidebar's context menus. (M12)
 *
 * Appended as one contiguous block, per the house rule about this file. One entry, and it is
 * down here rather than on the `fs` namespace above only because of that rule — it is an
 * ordinary `fs_*` handler and belongs beside `fs.reveal` in every other sense.
 * ------------------------------------------------------------------------------------ */

/**
 * Show a path in the desktop file manager, and answer with the directory that opened.
 *
 * Not `fs.reveal`, which is the other kind of reveal entirely: that one expands the file
 * tree's own ancestors and scrolls to a row, inside this window. This one leaves the app.
 *
 * The **containing** directory is what opens for a file row, because no "open this folder and
 * select this entry" flag is portable across Linux file managers. See
 * `cmd::fs::fs_show_in_manager`.
 */
export const fsReveal = {
  showInManager: (projectId: ProjectId, path: string) =>
    invoke<string>('fs_show_in_manager', { project: projectId, path }),
}

// -------------------------------------------------------------------------------------------
// Choosing a project: the parented folder picker, and the projects opened before this one.
//
// One contiguous block, appended rather than folded into `project` above, per the house rule.
// The five commands are grouped by the surfaces that call them — the header's `+`/`▾` menu and
// the project tab's context menu — which is also why `reveal` sits beside them rather than in
// `fs`: it names a *project*, not a path a webview chose.
// -------------------------------------------------------------------------------------------

/**
 * Imported here, at the bottom, so this block stays contiguous. ES module imports are hoisted,
 * so its position is a matter of where the diff lands and nothing else.
 */
import type { RecentEntry } from './generated'
import type {
  BranchList,
  CheckoutMode,
  CheckoutOutcome,
  ConflictFile,
  ConflictSide,
  ContinueOutcome,
  FetchOutcome,
  MergeOutcome,
  MergeState,
  PullRequest,
  RepoId as BranchRepoId,
} from './generated'


export const projectMenu = {
  /**
   * Ask the user for project folders. `[]` means cancelled.
   *
   * **Use this, not `@tauri-apps/plugin-dialog`'s `open()`.** The plugin's JS command sets a
   * parent window only on Windows and macOS — on Linux it never does, and no option it accepts
   * can — which is why the picker opened *behind* the app on KDE Wayland. `project_pick` builds
   * the dialog against the window's real GTK toplevel instead. See `cmd::project::project_pick`.
   */
  browse: () => invoke<string[]>('project_pick'),

  /** Projects opened before, most recent first, each tagged with whether it is still there. */
  recent: () => invoke<RecentEntry[]>('project_recent'),

  /**
   * Reopen a remembered project. Rejects when the directory has gone.
   *
   * Not `project.open`: that one never touches the filesystem, so a stale entry would open a
   * project over a path that is not there — empty tree, empty picker, a `claude` spawned into
   * nothing, and no error anywhere near the cause.
   */
  reopen: (path: string) => invoke<ProjectId>('project_open_recent', { path }),

  /** Forget one remembered project, or all of them with `null`. Answers with what is left. */
  forget: (path: string | null) => invoke<RecentEntry[]>('project_forget_recent', { path }),

  /** Show a project's primary root in the desktop's file manager. Answers with the path. */
  reveal: (projectId: ProjectId) => invoke<string>('project_reveal', { project: projectId }),
}

/* --------------------------------------------------------------------------------------
 * M12 — "Awaiting: X", and the clipboard the terminal's own menu needs.
 *
 * One contiguous block, appended rather than folded into the namespaces above, per the
 * house rule for this file.
 * ------------------------------------------------------------------------------------ */

/**
 * Sessions that have finished processing and are waiting for the user.
 *
 * The *decision* is `panes/awaitingRule.ts` — it needs one bit of history that `SessionState`
 * does not carry, so it cannot be made from a single event on either side. The *aggregation*
 * is Rust's, because only Rust knows which pane is shown in which OS window and only Rust can
 * rename an OS window: a webview has no access to its own title bar.
 *
 * Reports are per session and idempotent. Every window observes the same
 * `cide://session-state` broadcasts and will report the same answer, which costs a command
 * and changes nothing; a window that has only just opened has observed nothing and therefore
 * reports nothing, which is exactly right — a whole-set report from a fresh window would
 * clear every marker in the app.
 */
export const awaiting = {
  /**
   * Record whether `session` is waiting on the user. Rust retitles every affected window and
   * broadcasts the new set on `cide://session-awaiting`.
   */
  set: (session: SessionId, isAwaiting: boolean) =>
    invoke<void>('window_set_awaiting', { session, awaiting: isAwaiting }),

  /**
   * The set as Rust currently holds it, asked once per window.
   *
   * `cide://session-awaiting` below is a *change* notification, so a window that opens
   * between two changes hears nothing — and a window built by a detach is exactly that. Rust
   * has already put `Awaiting: 1` in its OS title by the time its pane renders, so without
   * this ask the title bar and the pane's own marker disagree in the window the user tore out
   * *because* it was waiting.
   */
  current: () => invoke<SessionId[]>('window_awaiting_sessions'),
}

/**
 * The authoritative waiting set, after any window's report.
 *
 * Broadcast rather than answered to the caller so a window that opened *after* a session
 * started waiting still paints its marker — it has no history of its own to derive that from.
 *
 * A standalone listener rather than a member of `events`, matching `onIpcDegraded` above: the
 * append-only rule for this file means a new event cannot reach inside an existing object
 * literal.
 */
export const onSessionAwaiting = (handler: (sessions: SessionId[]) => void) =>
  listen<{ sessions: SessionId[] }>('cide://session-awaiting', (e) => handler(e.payload.sessions))

/* --------------------------------------------------------------------------------------
 * Creating a file or a folder from the file tree's context menu.
 *
 * One contiguous block, appended rather than folded into `fs` above, per the house rule for
 * this file.
 * ------------------------------------------------------------------------------------ */

/**
 * *New File…* and *New Folder…*, as the tree's context menu means them.
 *
 * Deliberately **not** `fs.create` with a joined path, and the difference is three behaviours
 * the tree depends on:
 *
 * * `fs.create` reaches `ops::create`, which calls `create_dir_all` on the way to the path it
 *   was given. The row this gesture starts from was drawn some time ago, so the folder it
 *   names may have been deleted since — and a *New File* that silently puts a deleted folder
 *   back is worse than one that refuses. This command requires the parent to already be a
 *   directory.
 * * The name is checked *as a name*. `src/main.rs` typed into the box is refused with a
 *   reason rather than obeyed as two components or mangled into `src_main.rs`. The same rules
 *   run in `sidebar/newEntry.ts` so the user is told while typing; this is the copy that is
 *   load-bearing.
 * * The path is folded into the file index before the command returns, so the tree can show
 *   the row **now** instead of waiting out the watcher's debounce.
 *
 * Answers with the absolute path it created, so the caller selects that row rather than
 * re-deriving the same string a second time in a second language.
 *
 * Note what this does **not** do: open the file. Creating is not opening — that is the
 * single-click rule the tree already follows, applied to creation.
 */
export const fsCreate = {
  entry: (projectId: ProjectId, parent: string, name: string, directory: boolean) =>
    invoke<string>('fs_create_in', { project: projectId, parent, name, directory }),
}

/**
 * *Send lines to Claude* — the one gesture in this app that reports when it cannot send.
 *
 * # Why this is a second namespace rather than a member of `claude` above
 *
 * Two reasons, and the second is the load-bearing one.
 *
 * This file is **append-only** by house rule: it has conflicted in three consecutive rounds, so
 * a new capability arrives as a block at the end rather than as a field reaching inside an
 * existing object literal. `onSessionAwaiting` directly above says the same thing about events.
 *
 * And the `claude.mentionFile` this replaced **swallowed its result** — `.catch(() => {})` —
 * which is fatal for what this is. The point of this call is the rejection:
 * `claude_send_lines` answers `{kind: 'noServer'}` or `{kind: 'notConnected'}` when nothing is
 * listening, and the call sites deliberately do **not** catch it, so `chrome/Failures.tsx`'s
 * `unhandledrejection` listener puts the reason on screen. A silent no-op is indistinguishable
 * from a control wired to nothing, which is precisely how this feature was reported. That
 * command and its wrapper are now deleted; this is the only route.
 */
export const claudeSend = {
  /**
   * Broadcast the selection to the project, then `@`-mention the range into one pane's prompt.
   *
   * Both notifications leave Rust in that order and from the same numbers, so the range Claude
   * highlights and the range it is told about cannot disagree. See `cmd::file::claude_send_lines`
   * for why this mentions a range rather than pasting the text.
   *
   * `lineStart`/`lineEnd` are **1-based**, like everything a user reads, and are converted to
   * the protocol's 0-based numbering once at the Rust boundary. Omit both to send the whole
   * file, which is what a caret with no selection means.
   *
   * # `paneId` is a preference, and the answer says what was actually used
   *
   * Rust tries `paneId` first and, when that pane's `claude` is not on the IDE server, walks
   * the project's other Claude panes in `cmd::file::mention_candidates` order. That is not a
   * detail a caller may ignore: **reveal `result.pane`, never the pane you asked for**, and say
   * something when `result.fallback` is set. Taking the user to an empty prompt while their
   * lines sit in a different conversation is worse than the plain error this replaced.
   *
   * Rejects on purpose when nothing at all could receive. Call it as `void claudeSend.lines(…)`
   * and let `Failures` explain.
   *
   * # `exact` turns the preference into an address
   *
   * Pass it when the *user* named the pane — the code menu's `2: git-details` rows — rather than
   * when the app guessed one. Rust then tries that pane and only that pane, and a pane with no
   * connected `claude` is a rejection instead of a reroute. The fallback exists because ⌥⏎ has
   * to guess; a stated choice is not a guess, and quietly delivering it elsewhere would answer a
   * question nobody asked. `result.fallback` can never be true for an `exact` send.
   */
  lines: (
    projectId: ProjectId,
    paneId: PaneId,
    path: string,
    text: string,
    lineStart?: number,
    lineEnd?: number,
    exact = false,
  ) =>
    invoke<ClaudeSendTarget>('claude_send_lines', {
      project: projectId,
      pane: paneId,
      path,
      text,
      lineStart: lineStart ?? null,
      lineEnd: lineEnd ?? null,
      exact,
    }),

  /**
   * What the user has called each running conversation — `/rename`'s name, by session id.
   *
   * Keyed by the id cide addresses a session under, so a pane is looked up as
   * `names[pane.conversation ?? pane.session]`. A conversation nobody has named is **absent**
   * rather than empty, which is what lets the caller fall back to the pane's own title without
   * having to guess whether `''` was meant.
   *
   * Resolves to `{}` rather than rejecting when there is nothing to read — no `~/.claude`, a
   * relocated `CLAUDE_CONFIG_DIR`, a machine where `claude` has never run. Every one of those
   * costs a menu row its label and nothing else, so none of them is a failure worth a toast.
   * See `cmd::file::claude_session_names`, and `editor/claudeNames.ts` for when it is called.
   */
  names: () => invoke<Record<string, string>>('claude_session_names', {}),
}

// Appended here rather than reached into the import list at the top of the file, for the reason
// the `PasteCollision` import below states at length: this file is append-only by house rule.
// Not re-exported: `export type * from './generated'` near the top already gives every caller
// `ClaudeSendTarget`, and a second export of the same name is a redeclaration waiting to happen.
import type { ClaudeSendTarget } from './generated'

// The two generated types this block needs, imported here rather than added to the list at the
// top of the file. This file is **append-only** by house rule — it has conflicted in five
// consecutive rounds, once as a redeclaration that would not compile — and reaching up into an
// existing import specifier is exactly the edit that conflicts. An `import` is legal anywhere at
// a module's top level and is hoisted either way.
import type { PasteCollision, PasteDecision, PasteMode, PastedEntry } from './generated'

/**
 * The file tree's own clipboard: copying, cutting and pasting **the files**, not their paths.
 *
 * A separate namespace from `fs` above for the same reason `fsCreate` is one — this file is
 * appended to, never reached into — and the split is not arbitrary: `fs` is the tree's *read*
 * surface (count, rows, expand, reveal) plus the single-path edits, and this is the one gesture
 * that moves bytes between two places on disk.
 *
 * Every rule lives in `cide_fs::copy`, and the ones a user meets are worth naming at the call
 * site because they are not what the words "copy" and "paste" promise on their own:
 *
 * * **A collision is asked about first.** `plan` answers which names are taken *without
 *   writing anything*; the dialog collects an answer for each and `paste` carries them. A
 *   source with no decision is renamed to `main copy.rs`, exactly as before — so `paste` can
 *   only ever overwrite what it was explicitly told to, and a caller that drops its answers on
 *   the floor renames rather than destroys.
 * * **`cut` moves nothing until this call.** The mode travels with the paste, so a cut the user
 *   changed their mind about costs exactly nothing.
 * * **A directory is copied recursively**, and refused into itself or into anything inside it.
 * * **A symlink is copied as a symlink.** Its target is not followed and not duplicated.
 *
 * Rejects, and the rejection matters: `OutsideProject`, `IsRoot`, `IntoItself`, `CannotReplace`
 * and `PartialPaste` are all things the user needs to read. The file tree catches it and puts
 * the reason in its own strip; a call site with no such surface should let it reach `Failures`.
 */
export const fsClipboard = {
  /**
   * Which names this paste would land on that are already taken. Writes nothing.
   *
   * Called before `paste`, so a confirmation can be a decision rather than an apology: an empty
   * answer means the paste goes straight through, and anything in it is a question. Cancelling
   * after this call has cost the user nothing, because nothing has happened yet.
   *
   * Rejects with the same refusals `paste` does — a folder pasted into itself is refused here
   * *instead of* being asked about, which is the point of running the same checks twice.
   */
  plan: (projectId: ProjectId, sources: readonly string[], destDir: string, mode: PasteMode) =>
    invoke<PasteCollision[]>('fs_paste_plan', {
      project: projectId,
      sources: [...sources],
      destDir,
      mode,
    }),

  /**
   * Paste `sources` into `destDir`, answering collisions with `decisions`.
   *
   * `destDir` is a *directory*, always — the tree resolves a file row to its parent before
   * calling, exactly as *New File…* does (`sidebar/clipboardModel.ts`). Answers one entry per
   * source, in the order they were given, saying where each one actually landed.
   *
   * `decisions` is the dialog's answers and the only way anything here overwrites a file. An
   * empty list is the safe call and is what a paste with no collisions sends.
   */
  paste: (
    projectId: ProjectId,
    sources: readonly string[],
    destDir: string,
    mode: PasteMode,
    decisions: readonly PasteDecision[],
  ) =>
    invoke<PastedEntry[]>('fs_paste', {
      project: projectId,
      // A fresh array: `invoke` serialises what it is handed, and a `readonly string[]` from a
      // zustand store is the store's own array.
      sources: [...sources],
      destDir,
      mode,
      decisions: [...decisions],
    }),
}

/**
 * Branches: the status bar's selector, and the git half of the command palette.
 *
 * # Why a namespace of its own rather than fields on `git`
 *
 * The house rule for this file is append-only — it has conflicted in five consecutive rounds
 * — so a new capability arrives as a block at the end rather than as fields reaching inside
 * an existing object literal. `claudeSend` above says the same thing at length.
 *
 * It also happens to be the right seam. `git` is the commit tool window's surface: everything
 * on it takes a `RepoId` and answers with a `ChangesTree`, because every one of those calls
 * moves a tri-state checkbox. Nothing here does. `list` answers for the whole project at once
 * (a superproject has several repositories and the selector has one slot), and the mutations
 * answer with the new branch lists, or with an outcome carrying something a list cannot
 * express — which files a refusal named, whether a stash was made and survived.
 *
 * # None of these swallow their rejection
 *
 * Every call rejects with a `GitError` and every call site is expected to let it. A checkout
 * that cannot be made rejects with `{kind: 'checkoutWouldOverwrite', detail: {branch, paths}}`
 * — and `paths` is the whole feature: it is what the popup turns into *stash* or *stash and
 * bring them along*. A `.catch(() => {})` here would reduce that to a menu item that appears
 * to do nothing, which is the class of bug this round exists to remove.
 */
export const branch = {
  /**
   * Every repository in the project, each with its local and remote branches.
   *
   * The whole project in one round trip, because the popup has to be able to say which
   * repository a branch belongs to when there is more than one — and because the status bar
   * widget wants the same answer for its one line. Repositories that cannot be read are
   * skipped, so an unmounted root does not empty the list.
   */
  list: (project: ProjectId) => invoke<BranchList[]>('git_branch_list', { project }),

  /**
   * Create a branch, optionally switching to it.
   *
   * Never stashes and never overwrites: when `checkout` is asked for and `startPoint` is not
   * `HEAD`, Rust computes the blockers *before* creating anything, so a refusal leaves no
   * stray branch behind. To switch with a stash, `checkout` afterwards with a mode.
   */
  create: (
    project: ProjectId,
    repo: BranchRepoId,
    name: string,
    startPoint: string | null = null,
    checkout = false,
  ) => invoke<BranchList[]>('git_branch_create', { project, repo, name, startPoint, checkout }),

  /**
   * Switch branches. `name` may be a remote-tracking branch (`origin/feature`), which creates
   * and tracks a local one.
   *
   * `mode` is `'refuse'` on the first attempt, always. The rejection carries the paths that
   * stand in the way; `'stash'` and `'stashAndRestore'` are what the user's answer to that
   * sends back. There is no force — see `cide_git::branch` for why.
   */
  checkout: (project: ProjectId, repo: BranchRepoId, name: string, mode: CheckoutMode = 'refuse') =>
    invoke<CheckoutOutcome>('git_branch_checkout', { project, repo, name, mode }),

  /** Which paths a switch would overwrite, without switching. Empty means it is safe. */
  blockers: (project: ProjectId, repo: BranchRepoId, name: string) =>
    invoke<string[]>('git_branch_blockers', { project, repo, name }),

  rename: (project: ProjectId, repo: BranchRepoId, from: string, to: string) =>
    invoke<BranchList[]>('git_branch_rename', { project, repo, from, to }),

  /** `force` is the answer to `branchNotMerged`, and the UI must have said what would be lost. */
  delete: (project: ProjectId, repo: BranchRepoId, name: string, force = false) =>
    invoke<BranchList[]>('git_branch_delete', { project, repo, name, force }),

  /** `git fetch`. Shells out when a credential helper is configured, like `git.push`. */
  fetch: (project: ProjectId, repo: BranchRepoId, remote: string | null = null) =>
    invoke<FetchOutcome>('git_fetch', { project, repo, remote }),

  /**
   * Fetch, then integrate — fast-forward, merge or rebase. (M20)
   *
   * `request` carries the whole decision: the remote, the strategy the user chose (or `null`
   * to let git config and the cide default decide), whether to skip the fetch because the refs
   * are already the ones a `pullNeedsStrategy` described, and whether to remember the answer in
   * the repository's own `pull.rebase`.
   *
   * Three answers rather than two. It resolves with a `FetchOutcome` whose **`conflicts` may be
   * non-empty** — the merge or rebase landed and is waiting to be resolved, which is a success
   * and not a failure — and it rejects with `pullNeedsStrategy` when nothing has said how to
   * reconcile, which is a question rather than an error. `chrome/pullStrategyModel.ts` unpacks
   * that one; everything else goes through `explain`.
   *
   * Defaulted so the existing one-line call sites (`branchApi.pull(p, r)`) still mean *pull,
   * decide everything from configuration*.
   */
  pull: (
    project: ProjectId,
    repo: BranchRepoId,
    // Spelled out rather than `{}`: `skipFetch` and `remember` are plain booleans on the wire,
    // and these two defaults are the ones that must never be assumed — a first-click Pull
    // always fetches, and never rewrites the repository's config behind the user.
    request: PullRequest = { skipFetch: false, remember: false },
  ) =>
    invoke<FetchOutcome>('git_pull', { project, repo, request }),

  /**
   * The merge or rebase in progress and its conflicted paths, or `null` when the tree is clean.
   *
   * Read on demand rather than mirrored into the workspace store, because it is *git's* state:
   * a `git merge` typed into a terminal pane changes it, and a mirror of something git owns is
   * stale with no upper bound on how long it stays that way. `cide://git-status` is what says
   * to ask again.
   */
  conflicts: (project: ProjectId, repo: BranchRepoId) =>
    invoke<MergeState | null>('git_conflicts', { project, repo }),

  /** The three sides of one conflicted file, for the resolver. */
  conflictRead: (project: ProjectId, repo: BranchRepoId, path: string) =>
    invoke<ConflictFile>('git_conflict_read', { project, repo, path }),

  /** Write the resolved text and collapse the path's conflict stages. */
  conflictResolve: (project: ProjectId, repo: BranchRepoId, path: string, content: string) =>
    invoke<MergeState | null>('git_conflict_resolve', { project, repo, path, content }),

  /**
   * Resolve a path by taking one side whole.
   *
   * **`ours` and `theirs` are the index's stages, not the panel's words.** During a *rebase*
   * git swaps them — stage 2 is the branch being rebased onto and stage 3 is your own commit —
   * so a caller must take the side the labels in `ConflictFile` name, never the one the button
   * would be called in a merge. `cide_git::conflict::side_labels` is where that is decided.
   */
  conflictTake: (project: ProjectId, repo: BranchRepoId, path: string, side: ConflictSide) =>
    invoke<MergeState | null>('git_conflict_take', { project, repo, path, side }),

  /** Put a resolved path back into conflict. Only possible while its stages can be rebuilt. */
  conflictUnresolve: (project: ProjectId, repo: BranchRepoId, path: string) =>
    invoke<MergeState | null>('git_conflict_unresolve', { project, repo, path }),

  /**
   * Conclude the merge, or advance the rebase to its next stop.
   *
   * Resolves with `state: null` when the operation is over and with a fresh `MergeState` when a
   * rebase stopped again — which is why the button says *Continue* and not *Finish*.
   */
  mergeContinue: (project: ProjectId, repo: BranchRepoId, message: string | null = null) =>
    invoke<ContinueOutcome>('git_merge_continue', { project, repo, message }),

  /** `git merge --abort` / `git rebase --abort`. Puts the tree back where it was. */
  mergeAbort: (project: ProjectId, repo: BranchRepoId) =>
    invoke<void>('git_merge_abort', { project, repo }),

  /**
   * Merge a branch — local or remote-tracking, exactly as named — into the checked-out one.
   *
   * Fast-forwards when it can, writes a real merge commit otherwise. Resolves with a
   * `MergeOutcome` whose **`conflicts` may be non-empty**: real `MERGE_HEAD` state is on disk
   * and the resolver takes over — a success with work attached, `git.pull`'s contract. See
   * `cide_git::merge` for the refusal ladder.
   */
  merge: (project: ProjectId, repo: BranchRepoId, name: string) =>
    invoke<MergeOutcome>('git_merge', { project, repo, name }),
}

/**
 * The one step of *take me to the pane my mention landed in* that a webview cannot take.
 *
 * Everything else about that reveal is a workspace mutation — activate the tab, drop a
 * maximize, move the focused pane — and those already have commands. What is left is raising
 * an **OS window**, which no amount of JavaScript can do: the target pane may be torn out into
 * a window of its own, or may be the shell's console while the gesture was made from a
 * detached editor. See `editor/revealPane.ts` for the order the two halves run in.
 *
 * Appended as its own namespace rather than added to `windows` above, per this file's
 * append-only rule — it has conflicted in five consecutive rounds, once as a redeclaration
 * that would not compile.
 */
export const sendFocus = {
  /**
   * Raise the window showing a pane, and answer which window that was.
   *
   * `null` — not a rejection — when nothing is showing the pane, which happens when it closed
   * between the send and the reveal. The mention still landed: it went to a *session*, and a
   * session outlives the pane that was drawing it. Rust reserves an error for something the
   * caller could act on, and there is nothing to act on here; the caller words the `null`.
   *
   * Rejects only if the command is genuinely missing (a frontend hot-reloaded past its
   * binary). `revealPane` catches that and folds it into its own sentence, so the user is told
   * the send worked and the switch did not, rather than being shown a bare failure for a
   * gesture that half succeeded.
   */
  // `string`, not `WindowLabel`, and that is this file's append-only rule rather than a
  // preference: importing the alias means editing the import block at the top, which is the
  // exact hunk that has conflicted every round. `windows.detachPane` above returns a label as
  // `string` for the same reason, and `WindowLabel` *is* `string` in `generated.ts`.
  revealPane: (projectId: ProjectId, paneId: PaneId) =>
    invoke<string | null>('window_reveal_pane', { project: projectId, pane: paneId }),
}

/**
 * Whether a pane could resume the conversation it was showing.
 *
 * Appended as its own namespace rather than added to `session` above, per this file's
 * append-only rule — that block has conflicted in five consecutive rounds.
 *
 * The question is asked by a pane whose child has just died, once, so the bar it puts over the
 * dead terminal can offer *Resume this conversation* beside *Start a new session*. It is not a
 * registry lookup: a transcript is a file under `~/.claude/projects` and outlives the process
 * that wrote it, which is exactly why the answer cannot be derived on this side of the wire.
 *
 * `cwd` is the directory the session was spawned in — the project's primary root, the same one
 * `session.spawn` was given. Never rejects for an unknown session; the answer is simply `false`.
 */
export const claudeSession = {
  resumable: (cwd: string, id: SessionId) =>
    invoke<boolean>('session_resumable', { cwd, session: id }),
}

/* -----------------------------------------------------------------------------------------
 * The git tool window's own state — open, height, which tabs. (M18)
 *
 * Appended as its own block, with its own `import type` inside it, per this file's append-only
 * rule: the import header at the top is the exact hunk that has conflicted every round.
 *
 * A namespace of its own rather than fields on `git` above, and the seam is real rather than
 * bureaucratic. Everything on `git` takes a `RepoId` and answers with a `ChangesTree`, because
 * every one of those calls moves a tri-state checkbox in the commit panel. Nothing here touches a
 * repository at all: these four move a strip of chrome, and they answer with nothing, because the
 * result arrives the way every other workspace mutation does — as a `cide://workspace-changed`
 * snapshot that `store/workspace.ts` mirrors. The only thing the two share is the word "git" in
 * the feature's name.
 *
 * **Fire-and-forget from a drag, deliberately.** `setLayout` is called once on `pointerup`, never
 * during the gesture: the splitter writes `--h-toolwindow` on `<html>` and commits when the
 * pointer is released, because routing pointer moves through the store would re-render `App` —
 * the tab strip, the pane tree, every `PaneSlot` — on every move, and each `PaneSlot` resize calls
 * `fit()`, which reflows xterm and sends a SIGWINCH to a live `claude`. That argument is
 * `SidebarSplitter.tsx`'s, and it is stronger here: this splitter changes every pane's *rows*.
 * --------------------------------------------------------------------------------------- */
import type { HistoryTabId } from './generated'

export const toolWindow = {
  /**
   * Geometry and visibility. Every argument is optional so one gesture writes one thing.
   *
   * `height` is CSS pixels and `logSplit` is per mille (0–1000); Rust clamps both where the patch
   * lands, so a value out of band is corrected once in the stored document rather than being
   * re-corrected on every read. A call that changes nothing does not bump `rev` and broadcasts
   * nothing, which is what lets a splitter commit unconditionally on `pointerup` without
   * flooding every window for a drag that ended where it started.
   */
  setLayout: (
    project: ProjectId,
    layout: { open?: boolean; height?: number; logSplit?: number; filesAsTree?: boolean },
  ) =>
    invoke<void>('tool_window_set_layout', {
      project,
      open: layout.open ?? null,
      height: layout.height ?? null,
      logSplit: layout.logSplit ?? null,
      // `?? null` and not `?? true`: omitting the key means *do not touch it*, and the field's
      // own default is `true`. Defaulting here would make every height commit also assert a
      // grouping, so dragging the divider would silently undo the toggle.
      filesAsTree: layout.filesAsTree ?? null,
    }),

  /** Bring a tab to the front, revealing the panel. `null` is the Log tab, which always exists. */
  activate: (project: ProjectId, tab: HistoryTabId | null) =>
    invoke<void>('tool_window_activate', { project, tab }),

  /**
   * Show a file's history, keyed on the repository and the **repo-relative** path.
   *
   * Open-or-activate: a file whose history is already up re-activates that tab and answers with
   * the id it already had. Four menus reach this for the same file often — the tab strip, the file
   * tree, the git changes tree and the editor's own menu — and three right-clicks must not give
   * three identical tabs. The id is minted in Rust, so a webview cannot hand in one that collides
   * with a tab it cannot see.
   */
  openHistory: (project: ProjectId, repo: RepoId, path: string) =>
    invoke<HistoryTabId>('tool_window_open_history', { project, repo, path }),

  /** Close one history tab. The Log tab has no id and cannot be closed. */
  closeHistory: (project: ProjectId, tab: HistoryTabId) =>
    invoke<void>('tool_window_close_history', { project, tab }),
}

/* -----------------------------------------------------------------------------------------
 * Reading history: where a path lives, and who wrote each line. (M18)
 *
 * Its own block at the end of the file, with its own `import type`, per the append-only rule.
 * Separate from `git` above because that namespace is the *commit tool window*'s surface — every
 * one of its calls takes a `RepoId` and answers with a `ChangesTree`, because every one of them
 * moves a tri-state checkbox. Nothing here mutates anything: this is the read-only half of the
 * repository, and the only part of the git surface that answers about commits rather than about
 * the working tree.
 *
 * None of these swallows its rejection. A `GitError` is `{kind, detail}`, so a caller that
 * catches and prints gets `[object Object]`; `chrome/branchModel.ts::explain` is the sentence and
 * `chrome/Failures.tsx` is the surface. That is the bug `check:branches` exists for.
 * --------------------------------------------------------------------------------------- */
import type { BlameFile, BlameParent, BlameRequest, RepoPath } from './generated'

/** The default blame: libgit2's own, which follows whole-file renames and cannot be told not to. */
export const BLAME_DEFAULT: BlameRequest = {
  follow: 'renames',
  ignoreWhitespace: false,
  firstParent: false,
  newest: null,
}

export const history = {
  /**
   * Which repository an absolute path belongs to, and its path inside it.
   *
   * `null` is an answer, not a failure: a scratch file, a dependency source opened by
   * go-to-definition, or a tab from another project is in none of this project's repositories,
   * and the caller's job is then to say so rather than to report an error.
   *
   * Asked over IPC rather than derived from `RepoInfo.root` in TypeScript, because `canonical`
   * resolves symlinks and the webview cannot: a symlinked root compares unequal to the work tree
   * it is actually inside, so a prefix test here is correct until the first symlinked checkout
   * and silently wrong after it. The innermost repository wins, so a file in a submodule
   * resolves to the submodule.
   */
  locate: (project: ProjectId, path: string) =>
    invoke<RepoPath | null>('git_locate', { project, path }),

  /**
   * Annotate one file.
   *
   * `contents` is the editor's buffer and is sent **only when the tab is dirty** — passing it
   * costs a copy of the file over the wire, and omitting it on a dirty tab attributes every line
   * after an unsaved insertion to the wrong commit, which in a gutter beside live text is a
   * per-line falsehood rather than a stale view. `null` lets Rust read the file it can already
   * see.
   */
  blame: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    contents: string | null = null,
    request: BlameRequest = BLAME_DEFAULT,
  ) => invoke<BlameFile>('git_blame', { project, repo, path, contents, request }),

  /** The parent of `rev` as it touched `path`. `null` is the end of the walk, not a failure. */
  blameParent: (project: ProjectId, repo: RepoId, path: string, rev: string) =>
    invoke<BlameParent | null>('git_blame_parent', { project, repo, path, rev }),
}

/* -----------------------------------------------------------------------------------------
 * The task tracker: `.cide/tasks.json`, and the event that says it moved. (M18)
 *
 * Its own contiguous block at the foot of the file with its own `import type`, per the
 * append-only house rule — and the listener is its own exported object rather than a member of
 * `events` for the reason stated on `onSessionAwaiting`: appending cannot reach inside an
 * object literal that was closed hundreds of lines ago, so a new event either sits beside
 * `events` or the rule breaks. `fsEvents` is the precedent.
 *
 * `tasks` and not `task`, and deliberately not folded into `claudeTasks` above — that
 * namespace is the *headless one-shot* surface (`claude -p`), which shares nothing with this
 * but four letters.
 *
 * **Every mutation answers with the whole board**, which is `git`'s argument in this file
 * restated: a call answering `{ok: true}` would be followed at once by a second asking what
 * happened, and the frame in between shows a list that is visibly wrong. It also means a
 * mutation needs no `hydrate()` follow-up and no wait for the broadcast to come back round.
 * --------------------------------------------------------------------------------------- */
import type { TaskBoard, TaskEdit, TaskId, TaskNew } from './generated'

export const tasks = {
  /**
   * The board as it stands, or `null` in a build whose backend has no such handler.
   *
   * **Through `pendingCommand`, and that is not defensive habit.** This is called from a render
   * effect (`tasksStore.attach`, which `App.tsx` runs on every project switch). Tauri answers an
   * unregistered command with a *rejected promise*, an unhandled rejection out of an effect
   * unmounts the whole tree under React 19, and the user's window goes blank — not the sidebar,
   * the window. `useDiagnostics.refresh` is wired exactly this way and for exactly this reason.
   *
   * `null` is therefore "this build cannot answer", which the store turns into `BOARD_UNKNOWN`
   * — *nobody has looked* — and never into `absent`, which is a claim about the user's project
   * that nothing checked.
   */
  board: (project: ProjectId) =>
    pendingCommand('tasks_board', () => invoke<TaskBoard>('tasks_board', { project }), null),

  /*
   * The three mutations are **not** wrapped, and the asymmetry is the design.
   *
   * They are user gestures — a click on New task, a status segment, a comment — and a gesture
   * that silently does nothing is the failure this project has paid for repeatedly. A rejection
   * here reaches the caller, which is where it can be shown; swallowing it would leave the panel
   * painting the pre-click board with no way for anyone to find out why.
   */

  /** Create a task, and with it `.cide/tasks.json` if the project has none. */
  create: (req: TaskNew) => invoke<TaskBoard>('task_new', { req }),

  /** One mutation of one task. `TaskEdit` is an enum, not a patch — see its Rust doc. */
  edit: (project: ProjectId, task: TaskId, edit: TaskEdit) =>
    invoke<TaskBoard>('task_edit', { project, task, edit }),

  remove: (project: ProjectId, task: TaskId) => invoke<TaskBoard>('task_delete', { project, task }),
}

/**
 * The tracker changed — for anyone: this window, another window, or an agent through the
 * orchestration MCP server.
 *
 * Carries the **whole board plus a `rev`**, which is `workspace_changed`'s arrangement and it
 * is here for `workspace_changed`'s reason: `.cide/tasks.json` genuinely has several writers, so
 * two snapshots can arrive out of order and a receiver that painted the last one to land would
 * show a board behind the click that changed it. `TasksPanel/model.ts`'s `newerBoard` is the
 * drop rule; the `rev` on the envelope duplicates the one inside a `ready` board, and the store
 * reads the board's own.
 *
 * A standalone object rather than a member of `events`, per the block header.
 */
export const taskEvents = {
  onChanged: (handler: (project: ProjectId, board: TaskBoard, rev: bigint) => void) =>
    listen<{ project: ProjectId; rev: bigint; board: TaskBoard }>('cide://tasks-changed', (e) =>
      handler(e.payload.project, e.payload.board, e.payload.rev),
    ),
}

/* -----------------------------------------------------------------------------------------
 * The commit log, one commit's contents, and diffs between revisions. (M18)
 *
 * Appended as its own block with its own `import type`, per the append-only rule at the top of
 * this file.
 *
 * `page` is one command for both the Log tab and a per-file History tab, because the two differ
 * by exactly one field of the query — `path`. Keeping them one call is what stops the two lists
 * drifting into two answers for "which commits touched this".
 * --------------------------------------------------------------------------------------- */
import type {
  CommitDetail,
  CommitLineCounts,
  CommitPage,
  LogQuery,
  LogScope,
  ResolvedRev,
  RevSide,
  RevisionBlob,
  RevisionDiff,
  RevisionRange,
} from './generated'

/**
 * A query over one repository's HEAD, newest first, with no filters.
 *
 * Spread and overridden rather than built field by field at each call site: `LogQuery` has
 * thirteen fields and `deny_unknown_fields` on the Rust side, so a caller that forgets one gets a
 * deserialisation failure rather than a default. One place to add the fourteenth.
 */
export function logQuery(scope: LogScope, over: Partial<LogQuery> = {}): LogQuery {
  return {
    scope,
    refs: { kind: 'head' },
    cursor: { kind: 'newest' },
    path: null,
    follow: false,
    simplify: 'default',
    firstParent: false,
    author: null,
    text: null,
    // Zero asks Rust for its own default rather than pinning one here: `log::LIMIT_DEFAULT` and
    // `SCAN_DEFAULT` are the authority, and a second copy on this side would be the one that
    // goes stale.
    limit: 0,
    scanLimit: 0,
    graph: true,
    graphLanes: 16,
    ...over,
  }
}

export const gitLog = {
  /* `page` lived here until the walk became cancellable and needed the tool tab it belongs to.
   * It is `logWalk.page` now, at the foot of this file — one call, not two wrapping one command,
   * because `git_log` grew a required argument and a second entry point without it could only
   * ever send `undefined`. Its doc comment moved with it. */

  /** One commit's message and changed files. `parent` picks which parent a merge diffs against. */
  detail: (project: ProjectId, repo: RepoId, rev: string, parent: number | null = null) =>
    invoke<CommitDetail>('git_commit_detail', { project, repo, rev, parent }),

  /** The `+`/`-` counts a capped detail left uncounted. See `show::MAX_COUNT_FILES`. */
  lineCounts: (
    project: ProjectId,
    repo: RepoId,
    rev: string,
    paths: string[],
    parent: number | null = null,
  ) => invoke<CommitLineCounts>('git_commit_line_counts', { project, repo, rev, parent, paths }),

  /** One file's diff between two revisions. */
  diff: (project: ProjectId, repo: RepoId, path: string, next: RevSide, prev: RevSide) =>
    invoke<RevisionDiff>('git_diff_revision', { project, repo, path, new: next, old: prev }),

  /** The changed-file list for an arbitrary pair. Not a `ChangesTree` — see the command. */
  diffFiles: (project: ProjectId, repo: RepoId, next: RevSide, prev: RevSide) =>
    invoke<RevisionRange>('git_diff_revision_files', { project, repo, new: next, old: prev }),

  /** One file's bytes as one commit left them. */
  fileAt: (project: ProjectId, repo: RepoId, path: string, rev: string) =>
    invoke<RevisionBlob>('git_file_at_revision', { project, repo, path, rev }),

  /**
   * Resolve anything `git rev-parse` accepts.
   *
   * Called before a name is persisted into a tab, so the tab stores the oid it resolved to: a
   * tab holding `main` would name a different tree tomorrow, which is exactly the staleness
   * `DiffSpec` refuses to carry.
   */
  resolve: (project: ProjectId, repo: RepoId, spec: string) =>
    invoke<ResolvedRev>('git_resolve_rev', { project, repo, spec }),
}

/* -----------------------------------------------------------------------------------------
 * Subagents: the roster, the per-project switch, and the event that says either moved. (M18)
 *
 * Its own contiguous block at the foot of the file with its own `import type`, per the
 * append-only house rule — and the listener is its own exported object rather than a member of
 * `events`, for the reason stated on `onSessionAwaiting`: appending cannot reach inside an
 * object literal that was closed a thousand lines ago, so a new event either sits beside
 * `events` or the rule breaks. `taskEvents` directly above is the precedent this mirrors.
 *
 * `agents` and not `agent`, and deliberately not folded into `session` above: a run *is* a
 * session once it has a child, but this namespace is about the roles a project defines and the
 * switch that lets any of them start, which is a per-project file rather than a process.
 * --------------------------------------------------------------------------------------- */
import type { AgentRoster, OrchestrationConfig, OrchestrationPatch } from './generated'

export const agents = {
  /**
   * What cide knows about this project's subagents, or `null` in a build with no such handler.
   *
   * **Through `pendingCommand`, and that is not defensive habit** — it is the same reason
   * `tasks.board` gives one block up. This is called from a render effect (`agentsStore.attach`,
   * which `App.tsx` runs on every project switch). Tauri answers an unregistered command with a
   * *rejected* promise naming it, an unhandled rejection out of an effect unmounts the whole
   * tree under React 19, and what the user gets is a blank window rather than a blank sidebar.
   *
   * `null` is therefore "this build cannot answer", which the store turns into `ROSTER_UNKNOWN`
   * — *nobody has looked* — and never into `disabled`, which is a claim about the user's project
   * that nothing checked, printed above a button that commits a file.
   */
  roster: (project: ProjectId) =>
    pendingCommand('agents_roster', () => invoke<AgentRoster>('agents_roster', { project }), null),

  /** The project's `.cide/config.json`, defaults and all. `null` for `roster`'s reason. */
  config: (project: ProjectId) =>
    pendingCommand(
      'agents_config_get',
      () => invoke<OrchestrationConfig>('agents_config_get', { project }),
      null,
    ),

  /**
   * Write a patch into `.cide/config.json`, creating it. Answers the config as it now stands.
   *
   * **Awaited by the caller and deliberately not wrapped**, which is the exact opposite of how
   * `settings.set` a few hundred lines up is called: every caller of that one spells it
   * `void settingsApi.set(patch).catch(() => {})`, and it can afford to, because a
   * `cide://workspace-changed` snapshot follows every settings write — a switch whose write
   * failed flicks back on its own, one round trip later, with no code needed to make it. There
   * is no such snapshot for a per-project file, so nothing in the window would ever contradict
   * a write that did not happen.
   *
   * It also writes a **tracked file into the user's repository** — the one gesture in this
   * feature that changes what their next commit contains — so a silent failure here is a user
   * pressing *Enable subagents* and being told nothing at all. The rejection reaches the store,
   * which reaches `notifyFailure`.
   */
  setConfig: (project: ProjectId, patch: OrchestrationPatch) =>
    invoke<OrchestrationConfig>('agents_config_set', { project, patch }),
}

/**
 * The roster moved — a role file was edited, a run started, a phase changed, the switch flipped.
 *
 * Carries the **whole roster** and, unlike `cide://tasks-changed`, **no `rev`** — and it needs
 * none. The registry is one in-process writer behind one lock, so emits leave in the order the
 * state took, and the last one to arrive is by construction the newest. `.cide/tasks.json` has
 * several writers (two windows, and every agent through the MCP server), which is why that
 * board carries a counter and `newerBoard` drops what is not strictly newer; there is nothing
 * here for such a rule to compare.
 *
 * Because the payload is whole, the store `adopt`s it rather than scheduling a re-ask:
 * answering an event that just told you the answer with a round trip asking for it is a
 * question with a known answer. `cide://diagnostics` is a *hint* and coalesces for that reason;
 * this is not one.
 *
 * A standalone object rather than a member of `events`, per the block header.
 */
export const agentEvents = {
  onChanged: (handler: (project: ProjectId, roster: AgentRoster) => void) =>
    listen<{ project: ProjectId; roster: AgentRoster }>('cide://agents-changed', (e) =>
      handler(e.payload.project, e.payload.roster),
    ),
}

/* -----------------------------------------------------------------------------------------
 * Opening a historical diff as a tab. (M18)
 *
 * Its own block at the end, per the append-only rule.
 *
 * Separate from `gitDiff` above — which opens a *working-tree* diff — because the two live in
 * different preview slots and must not share one. The git changes tree and the log's changed-file
 * list are on screen at the same time, so a single slot would mean clicking a file in the log
 * throwing away the working diff the user was staging from: the mirror image of the thirty-tab
 * report `tab_retarget_diff` was written to fix. `cide_core::workspace::PreviewSlot` derives which
 * slot a tab is in from its origin, and `retarget_diff` refuses to move a tab across families —
 * re-pointing a working diff at a revision would put a Stage button over a diff of two commits.
 * --------------------------------------------------------------------------------------- */
import type { RevSide as RevSideArg } from './generated'

export const revisionDiff = {
  /**
   * Double-click in a commit's changed-file list: a **kept** tab, titled `main.rs @ a1b2c3d`.
   *
   * Keyed on all four of `(repo, path, new, old)`, unlike `gitDiff.openTab`, which deliberately
   * omits the side. A working-tree tab *switches sides in place*, so the side is a mode of one
   * tab; a revision pair **is** the tab's identity, and two comparisons of one file are two
   * different documents.
   */
  openTab: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    next: RevSideArg,
    prev: RevSideArg,
    oldPath: string | null = null,
  ) =>
    invoke<TabId>('tab_open_revision_diff', {
      project,
      repo,
      path,
      new: next,
      old: prev,
      oldPath,
    }),

  /** Single click while a revision diff is already up: reuse the revision preview slot. */
  retargetTab: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    next: RevSideArg,
    prev: RevSideArg,
    oldPath: string | null = null,
  ) =>
    invoke<TabId>('tab_retarget_revision_diff', {
      project,
      repo,
      path,
      new: next,
      old: prev,
      oldPath,
    }),
}

/* -----------------------------------------------------------------------------------------
 * The commit actions: revert, cherry-pick, reset, tag, and checking out a commit. (M18)
 *
 * One appended block at the very end with its own `import type`, per the append-only rule
 * stated four times at the top of this file — it has conflicted in five consecutive rounds.
 *
 * # Why a namespace of its own and not fields on `git`
 *
 * Everything in `git` answers with a `ChangesTree`, because every one of those calls moves a
 * tri-state checkbox in the commit panel and a caller that got `{ok: true}` would have to ask
 * a second time — with a visibly wrong tree on screen in between. **These answer with an
 * outcome instead**: the interesting result of a reset is *what it dropped*, of a replay
 * *which commit it made*, of a tag *whether it moved one that already existed*, and none of
 * that survives being flattened into a status walk. The panel is not left stale — Rust
 * broadcasts `cide://git-status` after each of the five mutating ones, which is the channel
 * the panel, the file tree and the activity rail's badge already listen on. So the two
 * namespaces are two different questions, and mixing them would make the return type of a
 * `git.*` call something a caller has to look up.
 *
 * `resetPreview` is the exception that proves it: a read, no broadcast, and its own call
 * rather than a field of the request, because the dialog's Hard row says *"DISCARD 3 changed
 * files"* and has to **name all three** before the user commits to anything.
 *
 * # Nothing here may swallow a rejection
 *
 * A `GitError` is `{kind, detail}` with no `message`, so `String(error)` is `[object Object]`
 * and `error.message` is `undefined` — which is how an action that refused for a good, stated
 * reason ends up looking like a button wired to nothing. That is the bug `check:branches`
 * exists for. `chrome/branchModel.ts::explain` turns the tagged object into the sentence
 * (`replayWouldConflict` names the paths, `tagExists` names where the tag points *now*,
 * `mergeNeedsMainline` is the question the dialog has to ask), and `chrome/Failures.tsx` is
 * the surface. Every caller either catches and `explain`s, or rethrows an `Error` carrying
 * that sentence so the one `unhandledrejection` listener reports it.
 *
 * `ProjectId`, `RepoId` and `CheckoutMode` are already in scope from the blocks above — ES
 * imports are module-wide — so only the types new to this block are imported here.
 * --------------------------------------------------------------------------------------- */
import type {
  DetachOutcome,
  ReplayOutcome,
  ReplayRequest,
  ResetOutcome,
  ResetPreview,
  ResetRequest,
  TagOutcome,
  TagRequest,
} from './generated'

export const commitActions = {
  /**
   * Apply a commit's patch **inverted** onto HEAD — the log's *Revert*.
   *
   * Not the Git panel's *Rollback*, which throws uncommitted work away. `request.mode` chooses
   * between committing straight away and leaving the inverse in the working tree for the user
   * to read first, which is IDEA's default and the safer one for a revert.
   *
   * `request.mainline` is required for a merge and refused for anything else, and it is
   * deliberately not defaulted: *revert this merge* without saying which side to keep reverts
   * either an entire feature branch or none of it, with no way to tell which you got until you
   * read the diff. The refusal (`mergeNeedsMainline`) carries the parents, so the dialog can ask.
   */
  revert: (project: ProjectId, repo: RepoId, request: ReplayRequest) =>
    invoke<ReplayOutcome>('git_revert', { project, repo, request }),

  /**
   * Apply a commit's patch **as it stands** onto HEAD — the log's *Cherry-pick*.
   *
   * Its own call rather than an op argument shared with `revert`, even though Rust runs both
   * through one implementation: a caller that passed the wrong enum would revert where it meant
   * to cherry-pick, and there is no undo for that.
   */
  cherryPick: (project: ProjectId, repo: RepoId, request: ReplayRequest) =>
    invoke<ReplayOutcome>('git_cherry_pick', { project, repo, request }),

  /**
   * What each kind of reset would discard. **Touches nothing, and broadcasts nothing.**
   *
   * Call this before opening the confirmation, not while closing it. It carries the dropped
   * commits (capped in Rust at ten, with `moreDropped` for the rest), the `staged` list a Mixed
   * reset costs, the `dirty` list a Hard one destroys, `commitsGained` so a reset *forward* is
   * described as what it is, and `useStagingArea` — which is what makes a mixed reset cost a
   * hand-built `git add -p` selection rather than a derived index.
   *
   * `target` is a full oid from a log row. It is the only member here that takes a bare string
   * rather than a request object, because it is the only one with nothing else to decide.
   */
  resetPreview: (project: ProjectId, repo: RepoId, target: string) =>
    invoke<ResetPreview>('git_reset_preview', { project, repo, target }),

  /**
   * Move HEAD — and, per `request.kind`, the index and the working tree.
   *
   * **`hard` destroys uncommitted work and Rust will not ask.** `request.shelveFirst` is the
   * offer that belongs beside the red button: name a shelf and the working tree is captured
   * into it *before* the reset, with the whole `ShelfEntry` coming back on the outcome so the
   * toast can offer *Unshelve* without a second round trip.
   *
   * `force` is the second click, after a dialog has listed what is at stake: it proceeds past
   * the external-staging guard (which fires for `mixed` and `hard` and not for `soft`) and past
   * a dirty tree. It does **not** proceed past a rebase or merge in progress, and no flag does.
   */
  reset: (project: ProjectId, repo: RepoId, request: ResetRequest) =>
    invoke<ResetOutcome>('git_reset', { project, repo, request }),

  /**
   * Create — or, with `request.force`, move — a tag.
   *
   * `request.message` is `null` for a lightweight tag and a string for an annotated one. Which
   * field is present *is* the choice, so the two cannot disagree; it is not cosmetic, because
   * `git describe` and most release tooling ignore lightweight tags.
   *
   * Rejects with `tagExists`, which carries **where that tag points now** and not merely that
   * the name is taken — that oid is the whole of the question the user is being asked, and
   * `force` is what they send back after reading it. Pushing the tag is not part of this, on
   * purpose: a `git push refs/tags/<name>` is not undoable by the person who ran it.
   */
  tag: (project: ProjectId, repo: RepoId, request: TagRequest) =>
    invoke<TagOutcome>('git_tag_create', { project, repo, request }),

  /**
   * Check out a commit, which necessarily detaches HEAD.
   *
   * Separate from `branch.checkout` and answering a `DetachOutcome`, because `CheckoutOutcome`'s
   * `branch` field is *the branch you are now on* and `branchModel.checkoutNote` puts it
   * straight into "Switched to {branch}" — a short oid there reads as a branch name, and a
   * detached HEAD mistaken for a branch is how a day's commits end up on no ref. What comes
   * back instead carries `previous`: the way back, which is the thing a detached user least
   * often works out for themselves and the one the toast should offer.
   *
   * `mode` is `'refuse'` on the first attempt, always — the same three-state answer a branch
   * switch takes. The rejection carries the paths in the way; `'stash'` and `'stashAndRestore'`
   * are what the user's answer to that sends back.
   */
  checkoutDetached: (
    project: ProjectId,
    repo: RepoId,
    revision: string,
    mode: CheckoutMode = 'refuse',
  ) => invoke<DetachOutcome>('git_checkout_detached', { project, repo, revision, mode }),
}

/* -----------------------------------------------------------------------------------------
 * Running a subagent: dispatch, stop, pause, resume, and open one into a pane. (M18)
 *
 * Its own contiguous block at the foot of the file with its own `import type`, per the
 * append-only house rule stated at the top — appending cannot reach inside the `agents` object
 * literal several hundred lines above, and that literal is the exact hunk that conflicts when
 * two rounds land in the same week.
 *
 * # Why a second namespace rather than three more fields on `agents`
 *
 * `agents` answers questions about **files in the project**: the roster derived from
 * `.cide/agents/*.md`, and the switch in `.cide/config.json`. These act on the **run
 * registry** — a process-global thing that outlives the panel, the window and the project tab
 * — and `cmd/agents.rs` splits its own ten commands along exactly that line. Keeping the two
 * apart on this side means the answer to "does this call touch the user's repository" is
 * visible at the call site.
 *
 * # None of them goes through `pendingCommand`, and that is the design
 *
 * `agents.roster` and `agents.config` are wrapped because they are called from a render effect,
 * where a rejected promise unmounts the tree under React 19. These are **user gestures** — a
 * click on Dispatch, on ⏹, on ⏸, on ▶, on Open, on *Retry turn* — and a gesture that silently
 * does nothing is the failure this project has paid for most often. `tasks.create`/`edit`/
 * `remove` state the same asymmetry one block up. The rejection reaches `agentsStore`, which
 * reaches `notifyFailure`.
 * --------------------------------------------------------------------------------------- */
import type { DispatchRequest, RunId } from './generated'

export const agentRuns = {
  /**
   * Put a run on its role's queue, and answer the id it was given.
   *
   * **It returns as soon as the run is enqueued and does not wait for it** — not even for the
   * spawn, let alone the turn. That is `cmd::agents::agents_dispatch`'s stated rule and it is
   * this application's `openDiff` invariant restated: a dispatch is reachable from inside the
   * orchestrator's own turn, so a call that waited would let one subagent stuck in a permission
   * prompt freeze the session that dispatched it.
   *
   * The consequence for the panel is that **there is nothing here to hang a spinner on**. The
   * resolved promise means "the queue has it", which is a fact already on screen one
   * `cide://agents-changed` later as a Queued row; a progress indicator tied to this call would
   * finish while the run had not started, which is a worse lie than no indicator at all.
   *
   * `request.task` is optional on the wire and the panel nonetheless always fills it — see
   * `DispatchRequest::task`'s Rust doc: a run with no task is a run the Tasks panel cannot
   * account for.
   */
  dispatch: (request: DispatchRequest) => invoke<RunId>('agents_dispatch', { request }),

  /**
   * End a run: cancel it if it is still queued, kill its child if it has one.
   *
   * Answers nothing, unlike every mutation in `tasks` — there is no roster to hand back,
   * because a stop is not the only writer of one: the registry broadcasts
   * `cide://agents-changed` when the state actually takes, which is after the child has died
   * rather than when the request was accepted. A roster returned from here would be the one
   * from *before* the kill landed.
   */
  stop: (project: ProjectId, run: RunId) => invoke<void>('agents_stop', { project, run }),

  /**
   * Close this project's dispatch queue and freeze its children — or freeze one run.
   *
   * # One command with a nullable `run`, not two named ones
   *
   * `run` omitted (or `null`) is the **project scope**: the queue is shut *and* every live run
   * is frozen, along with the project's own console session. A run id freezes that one child.
   * They are one command because the two scopes share a queue, a signal ladder and a refusal
   * path, and splitting them would put two implementations of one refusal on the wire —
   * `cmd::agents::agents_pause` makes the argument in full, and `agents_stop` already makes the
   * same one about covering a queued run and a live one.
   *
   * The consequence the caller has to know about: **the project scope freezes the pane the user
   * is typing in.** Resume therefore has to be reachable from somewhere that is not that pane —
   * the Agents panel, and the `agents.resume` palette row — or the user has frozen their own
   * console with no way back. See `AgentRegistry::pause`.
   *
   * `null` is sent explicitly rather than omitted, because serde's `Option<RunId>` reads a
   * missing key and a `null` the same way and an explicit one keeps the two scopes visible at
   * the call site.
   */
  pause: (project: ProjectId, run: RunId | null = null) =>
    invoke<void>('agents_pause', { project, run }),

  /**
   * `SIGCONT`, reopen the queue, drain it. The mirror of `pause`, including its scope rule.
   *
   * A run frozen long enough that its turn may have died is watched for a few seconds afterwards
   * and, if nothing stirs, offered a retry through `AgentRun.staleTurn` — a field on the run and
   * not an event, so a window opened after the resume shows the same warning as the one that
   * asked for it. That offer is a *suspicion*, never a re-dispatch; `retryTurn` is how it is
   * taken.
   */
  resume: (project: ProjectId, run: RunId | null = null) =>
    invoke<void>('agents_resume', { project, run }),

  /**
   * Take the stale-turn offer and re-send the run's last dispatched prompt.
   *
   * **This spends a turn.** That is the whole reason it is a wrapper of its own rather than a
   * boolean on `ackStaleTurn`: a boolean between the user and a call that costs them money is
   * the wrong shape, and it makes the two answers to one question — *retry* and *leave it* —
   * indistinguishable at every call site, in the palette and in a log. `cmd/agents.rs` carries
   * the same paragraph, and the two are deliberately two commands on the wire as well.
   *
   * Refused with a sentence when the run carries no offer, so a double press sends one prompt
   * and is told about the second.
   */
  retryTurn: (project: ProjectId, run: RunId) =>
    invoke<void>('agents_retry_turn', { project, run }),

  /** Take the stale-turn offer off a run and do nothing else. The *leave it* answer. */
  ackStaleTurn: (project: ProjectId, run: RunId) =>
    invoke<void>('agents_ack_stale_turn', { project, run }),

  /*
   * There is deliberately **no `openPane` wrapper**. Opening a run into a pane is a *gesture*,
   * not a command: it lives in `ui/src/sidebar/AgentsPanel/openRun.ts` and goes through
   * `addRow` with `SplitIntent::Mirror`, because `addRow` calls `rememberSpawnPlan` before
   * `hydrate()` and that is what makes `TerminalPane` set `PaneHost.mirrored`. A pane built by
   * a command instead arrives over `cide://workspace-changed` with no spawn plan, the flag
   * stays unset, and `closePane` kills the agent's child on the way out. See `cmd/agents.rs`,
   * which carries the long version.
   */
}

/* -----------------------------------------------------------------------------------------
 * A file as one commit left it: `TabKind::Revision`. (M18)
 *
 * One appended block at the very end with no `import type` of its own — `ProjectId`, `RepoId`
 * and `TabId` are already in scope from the header and a second import of them would be a
 * duplicate identifier — per the append-only house rule stated four times at the top of this
 * file. It has conflicted in five consecutive rounds.
 *
 * # Why this is not a member of `revisionDiff`
 *
 * That namespace is *comparisons*: `(repo, path, new, old)`, a pair of trees, a tab whose whole
 * subject is what changed. This one names a single point — `(repo, path, rev)` — and produces a
 * read-only **buffer**, not a patch. Folding it in would put two different tab kinds behind one
 * object and invite the first reader to assume `revisionDiff.*` all answer with diffs.
 *
 * There is deliberately **no `retargetTab`**. A revision tab has no preview slot and no
 * single-click gesture that mints one: the only ways to open one are the blame popup's
 * *Annotate previous revision* and a crumb in a revision pane's own trail, and both are
 * deliberate. `cide_core::workspace::PreviewSlot` is derived from a `DiffOrigin`, which this
 * kind does not have — so a scratch revision tab is not merely unimplemented, it is
 * unrepresentable, and that is the right answer rather than a gap.
 * --------------------------------------------------------------------------------------- */

export const revisionFile = {
  /**
   * Open a read-only tab for `path` as `rev` left it, or activate the one already showing it.
   *
   * `rev` must already be a **full oid**. `HEAD~3` resolves to a different commit tomorrow and
   * this value is persisted into the tab; `gitLog.resolveRev` is what goes in front of anything
   * a user typed. Every caller today hands over an oid Rust produced (`BlameParent.rev`, or a
   * crumb read back off the tab), so nothing resolves at this call site.
   *
   * `from` is the walk that led here — newest **last**, `rev` itself excluded, which is the
   * order `TabKind::Revision::from` documents and the order the pane's crumb strip draws left to
   * right. Rust normalises it (`cide_core::workspace::revision_chain`) rather than trusting it:
   * a walk that goes back through a merge and down the other parent rejoins, and an un-deduped
   * chain grows by one entry per lap in a file that is written to disk.
   *
   * Open-or-activate is keyed on `(repo, path, rev)` and **not** on `from` — two routes to one
   * commit are one document — and the chain of the tab that already exists wins. So passing a
   * longer walk here never lengthens an open tab's trail, and calling this with the tab's own
   * `(rev, from)` is exactly "activate me", which is what the current crumb does.
   */
  openTab: (
    project: ProjectId,
    repo: RepoId,
    path: string,
    rev: string,
    from: readonly string[] = [],
  ) => invoke<TabId>('tab_open_revision', { project, repo, path, rev, from }),
}

/* -----------------------------------------------------------------------------------------
 * Taking a role's finished work into your own branch. (M18)
 *
 * Its own contiguous block at the foot of the file with its own `import type`, per the
 * append-only house rule stated at the top — the two agent namespaces above were both closed
 * hundreds of lines ago and appending cannot reach inside either literal.
 *
 * # Why it is neither `agents` nor `agentRuns`
 *
 * `agents` answers questions about **files in the project**; `agentRuns` acts on the **run
 * registry**. This does neither: it merges a git branch into the branch the user has checked
 * out, which is the one gesture in this feature that rewrites their working tree and adds a
 * commit to their history. Putting it beside `dispatch` and `stop` would hide, at the call
 * site, that this is the call which touches the repository — and that distinction is the
 * reason `agents` and `agentRuns` were split in the first place.
 *
 * # Why the outcome type is written out here by hand
 *
 * `cide_git::worktree::Integration` is not a wire type and `cide-ipc` does not carry a copy, so
 * `cmd/agents.rs` declares a plain `serde::Serialize` local to itself — the pattern
 * `ClaudeCliSupport` established and documents further up this file. Nothing generates this
 * TypeScript; it moves with the Rust by hand, and `agents_integrate`'s own
 * `serialising_keeps_the_three_answers_apart` test pins the JSON these three arms mirror.
 *
 * The three arms must stay three arms. "Nothing to do", "merged, here is the commit" and
 * "refused, and here are the conflicting paths" are three different sentences with three
 * different next actions, and any collapse of two of them — a boolean, an empty `paths` meaning
 * success — reaches the user as a refusal that looks like a no-op.
 * --------------------------------------------------------------------------------------- */
import type { AgentId } from './generated'

/** What `agentWorktree.integrate` did, or refused to do. Mirrors `cmd::agents::AgentIntegration`. */
export type AgentIntegration =
  /** The role's branch holds nothing this checkout lacks. Nothing was merged, and nothing needed to be. */
  | { kind: 'upToDate' }
  /** It landed. `commit` is the full oid — a merge commit, or the tip a fast-forward moved to. */
  | { kind: 'merged'; commit: string; files: number }
  /**
   * Refused, **having changed nothing at all**.
   *
   * `cide_git::worktree::integrate` computes the merge in memory and asks `has_conflicts` before
   * a byte is written, so the user's checkout is exactly as it was. `paths` is the entire value
   * of that refusal: a conflict reported without them is a dead end, which is why every caller
   * of this is expected to put them in front of the user rather than count them.
   */
  | { kind: 'conflicts'; paths: string[] }

export const agentWorktree = {
  /**
   * Merge `cide/<agent>` into the branch this project's root has checked out.
   *
   * **Not through `pendingCommand`**, like every other user gesture in this feature and unlike
   * `agents.roster`: this is a click, and a click that silently does nothing is the failure this
   * project has paid for most often. Rust refuses with a sentence — the project is not a
   * repository, `HEAD` is detached, a rebase is in progress, the role has never run and so has
   * no branch — and the rejection is meant to reach `notifyFailure`.
   *
   * It resolves only when the merge is **finished**, unlike `agentRuns.dispatch`: there is
   * nothing asynchronous behind it, the work happens on Rust's blocking pool, and the answer is
   * the whole point of the call.
   */
  integrate: (project: ProjectId, agent: AgentId) =>
    invoke<AgentIntegration>('agents_integrate', { project, agent }),
}

/* ---------------------------------------------------------------------------------------
 * The cancellable log walk. (M20)
 *
 * Its own block at the end of the file, with its own `import type`, per the append-only house
 * rule — `gitLog` was closed hundreds of lines ago and appending cannot reach inside that
 * literal.
 *
 * # This supersedes `gitLog.page`
 *
 * `git_log` now takes a `tab` as well as a project, so **`gitLog.page` above no longer matches
 * the command** and every call to it rejects with a deserialization error from Rust. Use
 * `logWalk.page`. `gitLog`'s other members — `detail`, `lineCounts`, `diff`, `diffFiles`,
 * `fileAt`, `resolve` — are untouched and stay where they are; only the paging call moved.
 *
 * # Why a page needs to name its tab
 *
 * So that it can be **stopped**. Rust keys a cancellation flag by `(project, tab)` and
 * `cide_git::log` polls it once per commit, so a walk that a newer query has superseded stops
 * within one commit instead of scanning to its budget for a page nobody will draw. Per *tab* and
 * not per project because the Log tab and N History tabs are live at once in the same panel:
 * keyed by project, opening a History tab would cancel the Log's walk mid-page. See
 * `crates/cide-app/src/cmd/log.rs`, which argues the whole shape.
 *
 * The tab id is the tool tab's, not the `HistoryTabId` of a row inside it — two levels of the
 * same panel, and `cide_ipc::ids` says at length why they are two id spaces.
 *
 * # A cancelled page is not an error
 *
 * The promise **resolves**, with `CommitPage.cancelled` true and whatever rows the walk had
 * reached. That is what typing in the filter box produces, several times per word. Drop the page
 * — the better answer is already in flight — and do not paint a failure: the rows in it are
 * correct as far as they go, and there is nothing for the user to act on.
 *
 * The generation counter a caller already keeps is still the authority on *which* answer to
 * paint. `cancelled` is the backend saying it agrees, and it is what makes the abandoned walk
 * stop costing anything; it is not a replacement for the counter, because a page can also be
 * superseded in the moments after it returns and before it is awaited.
 * --------------------------------------------------------------------------------------- */
import type { ToolTabId } from './generated'

export const logWalk = {
  /**
   * One page for one tool tab. Feed [`CommitPage.resume`] back as `{ kind: 'resume', token }` for
   * the next.
   *
   * A cursor and not an offset: a `skip` re-walks the prefix on every page, and a page that
   * stopped on the scan budget with three rows would ask for `skip: 3` and re-scan the same
   * commits for ever. It is also why `resume` being `null` is the only honest way to read "there
   * is no more" — `stop` can say `budget` or `page` and still leave a frontier.
   *
   * Asking again with an *identical* query — two windows showing the same tab — joins the walk
   * already running rather than restarting it. Asking with any other query cancels the standing
   * walk first, which is the whole point: the superseded page comes back `cancelled` and the
   * repository stops being read for it.
   */
  page: (project: ProjectId, tab: ToolTabId, query: LogQuery) =>
    invoke<CommitPage>('git_log', { project, tab, query }),

  /**
   * Stop the walk this tab has running, if it has one.
   *
   * Call it when the tab closes and when the panel unmounts. Idempotent, and `false` — meaning
   * there was nothing to stop — is the ordinary answer rather than a problem: a caller that had
   * to know whether a walk was in flight before it could stop one would need the state this call
   * exists to save it keeping.
   *
   * Not needed merely to supersede a query. Issuing the next `page` already cancels the standing
   * walk, and doing both is one extra round trip for no change in behaviour.
   */
  cancel: (project: ProjectId, tab: ToolTabId) =>
    invoke<boolean>('git_log_cancel', { project, tab }),
}

/* ---------------------------------------------------------------------------------------
 * Editing a role definition: the three commands behind Settings → Agents. (M18)
 *
 * `agents` a few hundred lines up *reads* a project's subagents and flips the per-project
 * switch. These three **write `.cide/agents/<name>.md`** — and its global twin beside
 * `keymap.json` — which until now could only be done with a text editor and a correct guess at
 * the front-matter grammar. `cmd::agents`' own section header calls them "the answer to the only
 * way there used to be of defining a role".
 *
 * # Why they are here at all, which is not a rhetorical question
 *
 * All three were registered in `generate_handler!`, listed in `contract/commands.json`,
 * unit-tested and clippy-clean, and **invocable from nothing**: no wrapper here meant no caller
 * anywhere, because this file is the frontend's only seam to `invoke`. That is this repository's
 * most-repeated defect — the M18 tally is four commands in one milestone — and
 * `ui/scripts/check-agents.mjs` fails on exactly that shape, by requiring every `agents_*` and
 * `tasks_*` command in the contract to be spelled somewhere in this file. These three lines are
 * what turn that gate green, and the gate is what stops them from rotting back out.
 *
 * # Raw `invoke`, awaited, not `pendingCommand`
 *
 * The opposite of `agents.roster`, and for the reason `agents.setConfig` states one block up:
 * `pendingCommand` exists for calls made from a *render effect*, where an unhandled rejection
 * unmounts the tree and a `null` fallback is an honest "this build cannot answer". Nothing here
 * is called from an effect — every one of them follows a deliberate click — and each writes a
 * file in the user's repository. A swallowed rejection would be a user pressing Save and being
 * told nothing at all.
 *
 * # A refusal is not a rejection
 *
 * `agents_save` answers `AgentSaveOutcome`, whose `rejected` arm carries one problem per field
 * in the **`Ok`** channel. `AgentSaveOutcome`'s doc argues the shape in full: `CoreError` has one
 * string per variant and a form needs eleven, so a rejection routed through the error channel
 * would arrive as one sentence with the field names inlined into prose — "says invalid without
 * saying where", which is the state the whole `AgentField` enum exists to make unrepresentable.
 * So `.catch` here is for a disk that would not take the write, and the `rejected` arm is for the
 * form.
 * --------------------------------------------------------------------------------------- */
import type { AgentDraft, AgentSaveOutcome, AgentScope } from './generated'

export const agentDefs = {
  /**
   * One definition file, as the form that edits it is populated — read **fresh**, at the moment
   * the form opens.
   *
   * `scope` is required rather than searched for, and that is `AgentScope`'s own argument: one
   * roster row can be backed by two files (a project role shadows a global one of the same name,
   * whole-file), so a call that guessed would populate the form from the file the user is not
   * looking at and save over the one they are.
   *
   * Rejects when there is no such file, which is how this screen discovers *where* a role lives:
   * nothing on the wire carries a definition's scope — `AgentDef` is the roster row and
   * deliberately does not — so the only honest answer to "is there a global `qa` as well" is to
   * ask for it. See `AgentsSection`'s `listRoles`.
   */
  draft: (project: ProjectId, agent: AgentId, scope: AgentScope) =>
    invoke<AgentDraft>('agents_draft', { project, agent, scope }),

  /**
   * Write one role's definition file. Creates, edits, renames and moves between scopes — all
   * four are this one call, because all four are "here is what this role should be, and here is
   * the file it is today", and `AgentDraft.original` is what distinguishes them.
   *
   * Answers `{ kind: 'saved', path, roster }` — `path` because a save may have *moved* the
   * definition, so the file the user edits next is not necessarily the one they opened — or
   * `{ kind: 'rejected', problems }`, in which case **nothing was written**.
   */
  save: (project: ProjectId, draft: AgentDraft) =>
    invoke<AgentSaveOutcome>('agents_save', { project, draft }),

  /**
   * Remove one role's definition file, and answer with the roster that is left.
   *
   * Scoped, and it matters more here than on `draft`: a project definition shadowing a global
   * one draws one row, and a delete that guessed would take the **global** file — which is in no
   * repository's history, is shared by every project the user opens, and has nothing to be
   * restored from.
   *
   * There is no confirmation in Rust, deliberately (`agents_delete`'s doc: a modal in the domain
   * would also have to be a modal in the MCP vocabulary and in every other door onto the same
   * act). The confirmation belongs to the button, and this screen's is the confirm-on-second-
   * click the agents and tasks panels already use.
   */
  delete: (project: ProjectId, agent: AgentId, scope: AgentScope) =>
    invoke<AgentRoster>('agents_delete', { project, agent, scope }),
}

// ---------------------------------------------------------------------------------------
// M22: extensions and their marketplaces.
//
// Appended in its own block with its own imports, on this file's house rule: appending cannot
// reach inside an object literal that was closed a thousand lines ago, and a feature whose seam
// is contiguous is a feature whose seam is greppable.
// ---------------------------------------------------------------------------------------

import type {
  Capability,
  Diagnostic,
  ExtensionPage,
  ExtensionRef,
  ExtensionSnapshot,
  MarketplaceId,
} from './generated'

/**
 * The extension registry.
 *
 * Every mutation answers with the **whole snapshot**, so nothing here is followed by a read. That
 * is `tasks`' rule and the argument is stronger here: installing an extension changes the language
 * registry, and a panel that re-asked would draw "not installed" for a frame beside a `.sql` file
 * that has already changed colour.
 */
export const ext = {
  /** Everything the Extensions panel draws. */
  snapshot: () => invoke<ExtensionSnapshot>('ext_snapshot'),

  /**
   * Re-read `extensions.json`, every clone and every installed tree.
   *
   * Cheap and called freely — the store reads the disk on every ask for `cide-agents`' reason: a
   * clone directory is one the user can `git pull` by hand, so a cached answer is wrong for as
   * long as nobody happens to invalidate it.
   */
  reload: () => invoke<ExtensionSnapshot>('ext_reload'),

  /**
   * Connect a marketplace by git URL or local path, and clone it.
   *
   * Slow — a clone over a network — and deliberately not given a progress channel: the marketplace
   * row carries a `Working` state for exactly this, and a second mechanism for saying "something
   * is happening" would be a second thing that can be left switched on.
   */
  connect: (source: string) => invoke<ExtensionSnapshot>('ext_connect', { req: { source } }),

  /** Forget a marketplace, and uninstall everything that came from it. */
  disconnect: (marketplace: MarketplaceId) =>
    invoke<ExtensionSnapshot>('ext_disconnect', { marketplace }),

  /** Fetch a marketplace and re-read its index. Never rejects; a failure is a row in the list. */
  refresh: (marketplace: MarketplaceId) =>
    invoke<ExtensionSnapshot>('ext_refresh', { marketplace }),

  /**
   * Install or update one extension.
   *
   * `granted` is what the user was **shown**, echoed back. Rust refuses the install if the
   * manifest has since changed, which is the whole of the consent mechanism: without it a
   * marketplace could add `process:spawn` between the sheet being drawn and the button being
   * pressed, and the user would have approved a different extension from the one that installed.
   */
  install: (id: ExtensionRef, granted: readonly Capability[]) =>
    invoke<ExtensionSnapshot>('ext_install', { req: { ...id, granted } }),

  /** Remove an installed extension. Its marketplace stays connected. */
  uninstall: (extension: ExtensionRef) =>
    invoke<ExtensionSnapshot>('ext_uninstall', { extension }),

  /** Switch one on or off. A disabled extension keeps its row, its files and its reason. */
  setEnabled: (extension: ExtensionRef, enabled: boolean) =>
    invoke<ExtensionSnapshot>('ext_set_enabled', { extension, enabled }),

  /**
   * Fold an extension's findings for one file into the project's diagnostics.
   *
   * The same store the language servers publish into — `DiagnosticStore` is where the source
   * filter, the staleness marks and the cap are applied, and a second path around it would be a
   * second answer to "how many errors are there", which is the number on the activity rail.
   *
   * The source id is derived in Rust from `extension`, never taken from the payload: an extension
   * publishing under `rustAnalyzer` would have its findings filtered, grouped and restarted as
   * somebody else's.
   */
  /**
   * Change one of an extension's settings.
   *
   * The value is coerced against the declared kind **in Rust** — clamped to a number's band,
   * refused if it names a choice that does not exist — so the answer this resolves with is the
   * value that was actually stored, not the one that was sent. The Settings page re-renders from
   * it for exactly that reason.
   */
  setSetting: (extension: ExtensionRef, key: string, value: boolean | string | number) =>
    invoke<ExtensionSnapshot>('ext_set_setting', { extension, key, value }),

  /** One extension's page: its catalog row, its installed row, and its README. */
  page: (extension: ExtensionRef) => invoke<ExtensionPage>('ext_page', { extension }),

  /**
   * Open this extension's page as a workspace tab, or activate the one it already has.
   *
   * `name` is the caption, stored on the tab kind so a restored tab has one before any snapshot
   * arrives — `TabKind::Extension` says why at length.
   */
  openTab: (project: ProjectId, extension: ExtensionRef, name: string) =>
    invoke<TabId>('tab_open_extension', { project, extension, name }),

  /**
   * Open a link from an extension's README in the user's browser.
   *
   * `http` and `https` only, and **Rust decides that**, not this file: a README is markdown
   * somebody else wrote, and a scheme check that lived in the component rendering the link would
   * be one `invoke` away from being bypassed.
   */
  openLink: (url: string) => invoke<void>('ext_open_link', { url }),

  publishDiagnostics: (
    project: ProjectId,
    extension: ExtensionRef,
    absPath: string,
    items: readonly Diagnostic[],
  ) =>
    invoke<void>('ext_publish_diagnostics', { project, extension, absPath, items }),
}

/**
 * The registry changed, in this window or another.
 *
 * Its own exported object rather than a member of `events`, on this file's append-only rule.
 */
export const extEvents = {
  onChanged: (handler: (snapshot: ExtensionSnapshot) => void) =>
    listen<{ snapshot: ExtensionSnapshot }>('cide://ext-changed', (event) =>
      handler(event.payload.snapshot),
    ),
}

/**
 * The URL an installed extension's file is served from.
 *
 * A scheme registered in `cide-app`, jailed to the installed directory and refused outright for a
 * disabled extension. Built here rather than in the worker host, so the one place a path out of a
 * manifest becomes a URL is the file that owns the seam.
 *
 * # Two spellings, one on each side of a platform split
 *
 * Tauri serves a custom scheme as `<scheme>://localhost/…` on Linux and Android and as
 * `http://<scheme>.localhost/…` on macOS and Windows. The *host* is therefore not ours to use, and
 * the identity pair lives in the path — see `cide_ext::assets::split_path`, which says the same
 * from the other end.
 *
 * Detected the way `@tauri-apps/api`'s own `convertFileSrc` does, off the user agent, because the
 * platform plugin's answer is asynchronous and this is called while a worker is being constructed.
 */
export function extAssetUrl(id: ExtensionRef, path: string): string {
  const clean = path.replace(/^\/+/, '')
  const tail = `${id.marketplace}/${id.extension}/${clean}`
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent
  const windowsOrMac = /Windows|Macintosh|Mac OS X/.test(ua)
  return windowsOrMac ? `http://cide-ext.localhost/${tail}` : `cide-ext://localhost/${tail}`
}
