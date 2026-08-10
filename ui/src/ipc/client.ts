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
import { Channel, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  Axis,
  Bootstrap,
  DiffAnswer,
  Direction,
  FsChange,
  FsStatus,
  GraphicsStatus,
  HeadlessRequest,
  HeadlessResult,
  KeymapReport,
  FileDoc,
  PaneId,
  PaneRestore,
  PickerFrame,
  PickerItem,
  ProjectId,
  QuitDecision,
  SessionState,
  Settings as SettingsDto,
  SettingsPatch,
  SettingsSection,
  Side,
  SplitId,
  SplitIntent,
  SplitOutcome,
  TabId,
  TreeRow,
  UnsavedTab,
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

  /** Returns the value actually stored, which may be clamped to [0.1, 0.9]. */
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

  /**
   * Put `@path` into one Claude pane's prompt — Ctrl+P's ⌥⏎.
   *
   * Addressed, not broadcast: a mention is text the user is about to send in one
   * conversation, so typing it into every Claude in the project would be wrong. The caller
   * picks the pane because only it knows which Claude the user was last looking at.
   *
   * Lines are 1-based here and 0-based on the wire; omit both to mention the whole file.
   */
  mentionFile: (
    projectId: ProjectId,
    paneId: PaneId,
    path: string,
    lineStart?: number,
    lineEnd?: number,
  ) =>
    invoke<void>('claude_mention_file', {
      project: projectId,
      pane: paneId,
      path,
      lineStart: lineStart ?? null,
      lineEnd: lineEnd ?? null,
    }).catch(() => {}),

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
}

/**
 * The fuzzy pickers.
 *
 * `query` is a poll, not a subscription: the index is still filling while the user types, so
 * the frame carries `running` and the overlay asks again while it is true. `matched` and
 * `total` are the two numbers in the `6 of 2,418` counter, and they climb during a walk.
 */
export const picker = {
  /** Query a project's file index. Usable before `fs.index` has finished. */
  query: (projectId: ProjectId, query: string, limit?: number) =>
    invoke<PickerFrame>('picker_query', { project: projectId, query, limit: limit ?? null }),

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
   * The Linux graphics ladder: what is stored, and what this process actually got.
   *
   * The two can differ, and saying so is the point. These variables are read while the
   * webview is being created, so a change made here takes effect on the next launch and
   * never on this one.
   */
  graphics: () => invoke<GraphicsStatus>('graphics_status'),
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
    return invoke<void>('session_attach', { session: id, sink, geometry: geo })
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
    void invoke<void>('session_ack', { session: id, bytes }).catch(() => {})
  },

  /** The byte sequence that reconstructs the current screen. Send this before live bytes. */
  scrollback: (id: SessionId) => invoke<ArrayBuffer>('session_scrollback', { session: id }),

  inAlternateScreen: (id: SessionId) =>
    invoke<boolean>('session_in_alternate_screen', { session: id }),

  write: (id: SessionId, data: string) => invoke<void>('session_write', { session: id, data }),

  resize: (id: SessionId, geo: Geometry) =>
    invoke<void>('session_resize', { session: id, geometry: geo }),

  hasExited: (id: SessionId) => invoke<boolean>('session_has_exited', { session: id }),

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
   * Fires after any git mutation cide made, in every window. Changes made *outside* cide —
   * a `git add` in a bash pane — do not fire it; those arrive through the filesystem
   * watcher, which is what the panel's own refresh is wired to.
   */
  onGitStatus: (handler: (project: ProjectId, tree: ChangesTree) => void) =>
    listen<{ project: ProjectId; tree: ChangesTree }>('cide://git-status', (e) =>
      handler(e.payload.project, e.payload.tree),
    ),
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

  /** Shells out to `git push` when a credential helper is configured or the remote is HTTPS. */
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

  /** Record whether a file tab has unsaved edits. This is what draws the tab's dirty dot. */
  setDirty: (projectId: ProjectId, id: TabId, dirty: boolean) =>
    invoke<{ rev: number }>('tab_set_dirty', { project: projectId, tab: id, dirty }),

  read: (path: string) => invoke<FileDoc>('file_read', { path }),

  write: (path: string, text: string) => invoke<void>('file_write', { path, text }),
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
  /** Debounced 300 ms in Rust. 5,000 touched files arrive as one event, not 5,000. */
  onChanged: (handler: (project: ProjectId, paths: string[]) => void) =>
    listen<{ project: ProjectId; paths: string[] }>('cide://fs-changed', (e) =>
      handler(e.payload.project, e.payload.paths),
    ),

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
  /** Attach this pane's sink. See `session.attach`; the pane is what keeps mirrors apart. */
  attach: (pane: PaneId, id: SessionId, geo: Geometry, onData: (data: ArrayBuffer) => void) => {
    const sink = new Channel<ArrayBuffer>()
    sink.onmessage = onData
    return invoke<void>('session_attach', { session: id, pane, sink, geometry: geo })
  },

  /**
   * Report that this pane has parsed `bytes`. Fire-and-forget for the same reason
   * `session.ack` is: it runs in `term.write`'s completion callback, once per frame.
   */
  ack: (pane: PaneId, id: SessionId, bytes: number) => {
    void invoke<void>('session_ack', { session: id, pane, bytes }).catch(() => {})
  },

  /** Drop this pane's sink. The child keeps running; only this view of it ends. */
  detach: (pane: PaneId, id: SessionId) => invoke<void>('session_detach', { session: id, pane }),
}
