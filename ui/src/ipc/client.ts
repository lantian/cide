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
  PaneId,
  PaneRestore,
  ProjectId,
  SessionState,
  Side,
  SplitId,
  SplitIntent,
  SplitOutcome,
  TabId,
  WindowMode,
  Workspace,
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

/* --------------------------------------------------------------------------------------
 * M8 — file tree and pickers.
 *
 * PENDING: none of the eight commands below exist in Rust yet; `cide-fs` and `cide-search`
 * are being built in parallel. They are declared here so the frontend can be written and
 * reviewed against the real names, and every caller in `ui/src/sidebar` and
 * `ui/src/overlays` goes through `pendingCommand()` so a missing handler degrades to an
 * empty tree and an empty picker rather than an unhandled rejection.
 *
 * The payload types below are likewise provisional. When `cide-ipc` gains `TreeRow`,
 * `TreeStatus`, `PickerKind`, `PickerHit` and `PickerFrame`, `cargo xtask codegen` will put
 * them in `generated.ts` and these local declarations should be DELETED in favour of the
 * generated ones — they are deliberately structural so that swap is a re-export, not a
 * rewrite. `contract/commands.json` is likewise not touched here: adding names with no Rust
 * handler behind them fails `cargo xtask contract-check` in this tree, and the entries
 * belong with whoever writes the handlers.
 * ------------------------------------------------------------------------------------ */

/** Git status of a tree row. `clean` renders in `--text` with no tag letter. */
export type TreeStatus = 'clean' | 'modified' | 'added' | 'deleted' | 'untracked' | 'ignored'

/**
 * One row of the flattened file tree.
 *
 * Flattened and windowed on the Rust side: a 100k-file repository is one array of rows there
 * and never crosses the IPC boundary whole. `depth` is what the renderer indents by; the
 * tree structure itself is never sent.
 */
export interface TreeRow {
  /** Absolute path. Unique, and the identity the renderer keys on. */
  path: string
  /** Display name. For a root row this is the root's label, not necessarily its basename. */
  name: string
  /** 0 for a root row. A multi-root project contributes one depth-0 row per root. */
  depth: number
  isDir: boolean
  /** Directories only; `false` for a collapsed directory and for every file. */
  expanded: boolean
  /** Whether a twisty should be drawn at all. */
  hasChildren: boolean
  status: TreeStatus
}

export type PickerKind = 'files' | 'commands' | 'symbols'

/** One candidate. `indices` are match positions in `name`, for highlighting. */
export interface PickerHit {
  /** Absolute path — what `tab.open_file` takes. */
  path: string
  /** Basename, shown first and in `--text`. */
  name: string
  /** Path relative to the project root, shown after the name in `--dim`. */
  relative: string
  indices?: number[]
}

/**
 * A streamed frame of picker results.
 *
 * Frames rather than a return value because the index is built while the user is already
 * typing: the picker has to be usable before indexing finishes, so results arrive as they
 * are found. `matched` and `total` are what the mock's `6 of 2,418` counter reads.
 */
export interface PickerFrame {
  picker: string
  /** Which query this frame answers. Frames for a stale query are dropped by the caller. */
  query: string
  hits: PickerHit[]
  matched: number
  total: number
  /** True while the walker is still adding to the corpus. */
  indexing: boolean
}

export const fs = {
  /** Windowed: `[offset, offset + len)` of the flattened tree. Never ships the whole tree. */
  treeRows: (project: ProjectId, offset: number, len: number) =>
    invoke<TreeRow[]>('fs_tree_rows', { project, offset, len }),
  treeCount: (project: ProjectId) => invoke<number>('fs_tree_count', { project }),
  /** Returns the new total row count, so the caller can resize without a second call. */
  expand: (project: ProjectId, path: string) => invoke<number>('fs_expand', { project, path }),
  collapse: (project: ProjectId, path: string) => invoke<number>('fs_collapse', { project, path }),
  /** Expands whatever is needed to make `path` visible and returns its row index. */
  reveal: (project: ProjectId, path: string) => invoke<number>('fs_reveal', { project, path }),
}

export const picker = {
  /**
   * Open a picker session and start streaming frames into `onFrame`.
   *
   * A session rather than a query-response call: the walker keeps finding files after the
   * first frame, and a `Vec` return would make the picker wait for a 100k-file repository
   * to finish indexing before it could show anything.
   */
  open: (projectId: ProjectId, kind: PickerKind, onFrame: (frame: PickerFrame) => void) => {
    const sink = new Channel<PickerFrame>()
    sink.onmessage = onFrame
    return invoke<string>('picker_open', { project: projectId, kind, sink })
  },
  query: (id: string, query: string) => invoke<void>('picker_query', { picker: id, query }),
  close: (id: string) => invoke<void>('picker_close', { picker: id }),
}

/**
 * File-system events. PENDING, like the commands above.
 *
 * Subscribing to an event Rust never emits is inert — `listen` resolves and the handler is
 * simply never called — so these are wired now rather than left as a TODO, and the tree
 * starts refreshing itself the moment `cide-fs` begins emitting. `contract/events.json` is
 * not touched for the same reason `contract/commands.json` is not: `contract-check` reflects
 * over `emit.rs`, and the entries belong with the code that emits them.
 */
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
