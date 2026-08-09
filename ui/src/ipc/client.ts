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
}

export const project = {
  /** Open a project over one or more roots. Returns the new project's id. */
  open: (paths: string[], name?: string) =>
    invoke<ProjectId>('project_open', { paths, name: name ?? null }),
  close: (id: ProjectId) => invoke<{ rev: number }>('project_close', { project: id }),
  reorder: (from: number, to: number) => invoke<{ rev: number }>('project_reorder', { from, to }),
}

export const tab = {
  /** Open a closable full-screen Claude tab. Splitting it creates new sessions. */
  newClaude: (projectId: ProjectId, title?: string) =>
    invoke<TabId>('tab_new_claude', { project: projectId, title: title ?? null }),

  activate: (projectId: ProjectId, id: TabId) =>
    invoke<{ rev: number }>('tab_activate', { project: projectId, tab: id }),
  /** Rejected with `TabPinned` for the project console — the rule lives in Rust. */
  close: (projectId: ProjectId, id: TabId) =>
    invoke<{ rev: number }>('tab_close', { project: projectId, tab: id }),
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

  /** Rejected for the console's primary pane and for a tab's last pane. */
  close: (projectId: ProjectId, tabId: TabId, paneId: PaneId) =>
    invoke<{ rev: number }>('pane_close', { project: projectId, tab: tabId, pane: paneId }),

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

  close: (label: string) => invoke<{ rev: number }>('window_close', { label }),
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
 * Git — the M10 commit tool window (`ui/src/sidebar/GitPanel`).
 *
 * **None of these handlers is registered yet.** `cide-git` is a stub and the Rust side is
 * being written in parallel, so every call here rejects with Tauri's "command not found"
 * until it lands. They are absent from `contract/commands.json` for exactly that reason:
 * `contract-check` diffs the contract against the commands `cide-app` actually registers,
 * so listing them early would fail the gate rather than document intent. Whoever registers
 * `git_status` adds all eight names in the same change.
 *
 * The panel therefore treats a rejection as "no git data" and paints an empty tree — see
 * `useGitPanel`, which routes every call through one guard. That is not defensive
 * decoration: it is what lets the panel ship, be looked at, and be reviewed before the
 * backend exists.
 *
 * Returns are `unknown` rather than a DTO on purpose. The `ChangesTree` DTO is not in
 * `cide-ipc` yet, so `generated.ts` has nothing to import; `normalizeStatus` validates the
 * payload at the boundary instead. When the DTOs land, these signatures tighten to the
 * generated types and the normaliser becomes a thin cast.
 */
export const git = {
  /** The whole panel: every repo, changelist, submodule group and file. */
  status: (projectId: ProjectId) => invoke<unknown>('git_status', { project: projectId }),

  /** One file's diff text. `side` picks the staged or the working-tree comparison. */
  diffFile: (projectId: ProjectId, repo: string, path: string, side: 'staged' | 'worktree') =>
    invoke<unknown>('git_diff_file', { project: projectId, repo, path, side }),

  /** Whole-file staging. Hunk and line staging goes through `applyHunks`. */
  stagePaths: (projectId: ProjectId, repo: string, paths: string[], staged: boolean) =>
    invoke<void>('git_stage_paths', { project: projectId, repo, paths, staged }),

  /** A synthesized unified diff applied to the index only (`ApplyLocation::Index`). */
  applyHunks: (projectId: ProjectId, repo: string, patch: string) =>
    invoke<void>('git_apply_hunks', { project: projectId, repo, patch }),

  /**
   * Commit one changelist. Returns the new commit's id.
   *
   * `expectIndex` is the index state the panel last saw. Rust refuses the commit if the
   * real index has moved since — that refusal is what raises the "staging changed outside
   * cide" bar instead of silently clobbering a `git add` someone ran in a bash pane.
   */
  commit: (
    projectId: ProjectId,
    repo: string,
    message: string,
    amend: boolean,
    changelist: string | null,
    paths: string[],
    expectIndex: string | null,
  ) =>
    invoke<string>('git_commit', {
      project: projectId,
      repo,
      message,
      amend,
      changelist,
      paths,
      expectIndex,
    }),

  push: (projectId: ProjectId, repo: string, remote: string | null, refspec: string | null) =>
    invoke<void>('git_push', { project: projectId, repo, remote, refspec }),

  changelistCreate: (projectId: ProjectId, repo: string, name: string) =>
    invoke<void>('git_changelist_create', { project: projectId, repo, name }),

  /** Shelve paths as our own patch. With no paths, shelves the whole selection's repo. */
  shelve: (projectId: ProjectId, repo: string, paths: string[], name: string) =>
    invoke<unknown>('git_shelve', { project: projectId, repo, paths, name }),
}
