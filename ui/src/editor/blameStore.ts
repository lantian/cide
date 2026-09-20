/**
 * Which buffers are annotated, and the blame each one is showing. (M19)
 *
 * A module-level store rather than React state, for the reason `overlays/store.ts` is one: the
 * writers are outside React. `keys/dispatch.ts` toggles it from the command palette and the key
 * gate, `editor/codeMenu.tsx` toggles it from a context menu, and the gutter's own click handler
 * reads it from inside a CodeMirror extension — none of which has a component to call `setState`
 * on.
 *
 * # Keyed per path, not per pane
 *
 * The same file split across two panes shows the same annotation, and toggling it in one is meant
 * to toggle it in both: the answer is a property of the *file's history*, not of the view.
 * `editor/highlightLevel.ts` keys the same way for the same reason.
 *
 * # The blame is against HEAD, and the buffer is only sent when it is dirty
 *
 * A dirty tab hands its text over so the answer is right from the first frame — see
 * `cmd::git::git_blame` for why omitting it is a per-line falsehood rather than a stale view. A
 * clean tab sends nothing and lets Rust read the file it can already see. Live edits after that
 * are **not** re-fetched: the gutter maps its markers through CodeMirror's own `tr.changes`,
 * which is exact and costs no round trip per keystroke.
 */
import { events, history as historyApi } from '@/ipc/client'
import type { BlameFile, ProjectId, RepoId } from '@/ipc/client'
import { gitRefsMoved } from '@/gitlog/logModel'

/** What a buffer's gutter is showing. */
export type BlameState =
  | { readonly kind: 'off' }
  | { readonly kind: 'loading' }
  | { readonly kind: 'ready'; readonly blame: BlameFile }
  /** The file is in no repository, is untracked, or is too large. `reason` is user-facing. */
  | { readonly kind: 'failed'; readonly reason: string }

const OFF: BlameState = { kind: 'off' }

/**
 * The key separator.
 *
 * `\u0000` because it is the one byte a path cannot contain: on Linux a filename may hold any
 * other character, a colon and a newline included, so anything printable could be produced by a
 * real path and collide two buffers onto one entry.
 */
const SEP = '\u0000'

const keyOf = (project: ProjectId, path: string) => `${project}${SEP}${path}`

/**
 * Live buffers that are **dirty**, by path, as functions returning their text.
 *
 * # Why a registry rather than an argument
 *
 * Blame has to be right on the first frame for a tab the user has been editing — see
 * `cmd::git::git_blame` for why annotating the committed file under unsaved edits is a per-line
 * falsehood rather than a stale view. But the two callers that turn the gutter on are in very
 * different places: the editor's own context menu has the `EditorView` in hand, while the command
 * palette and the key gate reach `keys/dispatch.ts`, which has no view and no way to find one —
 * there is no live-`EditorView` registry, and `editor/codeMenu.tsx` writes at length about that
 * being exactly why the menu can do things the palette cannot.
 *
 * Threading the text down through every caller would mean the palette route silently getting the
 * *wrong* answer while the menu route got the right one — the worst shape, because it works when
 * you test it and fails by the route you did not. So the store asks, and neither caller has to
 * know.
 *
 * # Only while dirty
 *
 * A clean tab registers nothing and Rust reads the file it can already see, which saves a copy of
 * the buffer over the IPC wire on the common path. The provider is a **function**, not a string,
 * so nothing here holds a stale copy of a document that is still being typed into.
 */
const dirtyText = new Map<string, () => string>()

/**
 * Offer this buffer's text to a blame started from anywhere. Call with `null` when the tab goes
 * clean or its pane unmounts — a provider left behind would hand a dead view's text to the next
 * blame of that path.
 */
export function registerDirtyBuffer(path: string, read: (() => string) | null): void {
  if (read === null) dirtyText.delete(path)
  else dirtyText.set(path, read)
}

/**
 * Whether `path` has an open buffer with unsaved edits. (M70)
 *
 * Reads the same registry, for the fact it already holds: a provider is registered **only while
 * a tab is dirty** and removed when it goes clean, so membership is the answer with nothing else
 * to keep in sync.
 *
 * The properties card asks. Its Size and Lines rows are read from the disk — every other row on
 * it unambiguously is — and while a buffer is dirty the disk and the editor legitimately differ.
 * One word (`on disk`) fixes that, and this is how the card knows to say it; without it the card
 * silently contradicts the status bar six inches away and neither says which is stale.
 *
 * Deliberately not reactive: the card is a snapshot taken when it opens, and a size that changed
 * under the reader between one glance and the next would be a worse answer than a labelled one.
 */
export function hasDirtyBuffer(path: string): boolean {
  return dirtyText.has(path)
}

const states = new Map<string, BlameState>()
const listeners = new Set<() => void>()

/**
 * Bumped on every write, and returned by [`revision`].
 *
 * `useSyncExternalStore` compares snapshots by identity, so a consumer that wants "did anything
 * change" needs one stable value to compare rather than an object per key. The counter is that,
 * and it is what lets [`blameState`] keep returning the same frozen `OFF` for an unchanged key.
 */
let rev = 0

function emit(): void {
  rev += 1
  for (const listener of listeners) listener()
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function revision(): number {
  return rev
}

export function blameState(project: ProjectId, path: string): BlameState {
  return states.get(keyOf(project, path)) ?? OFF
}

export function isAnnotated(project: ProjectId, path: string): boolean {
  return blameState(project, path).kind !== 'off'
}

/**
 * Turn the gutter on or off for one buffer.
 *
 * Off is immediate and drops the answer: keeping a megabyte of runs for a column nobody is
 * looking at buys one fast re-open and costs memory for the rest of the session.
 *
 * On fetches. `contents` is passed by the caller and must be the buffer text **only when the tab
 * is dirty** — see the module header.
 */
export function toggleBlame(project: ProjectId, path: string, contents?: string | null): void {
  const key = keyOf(project, path)
  if (states.has(key)) {
    states.delete(key)
    emit()
    return
  }
  startBlame(project, path, contents, false)
}

/**
 * Read a buffer's blame, either fresh or as a background re-read.
 *
 * # Why `keepVisible` exists
 *
 * > *"git blame panel in opened file also flickering and tooltip is hidden on flickering"*
 *
 * A refresh used to go through `toggleBlame`, which sets `{ kind: 'loading' }` before asking. So
 * every git write took the gutter **ready → loading → ready**: the column emptied, every row's
 * width changed, the layout moved and the hover card — which is anchored to a line that had just
 * been re-rendered — was dismissed. On a tree several agents are writing to, `FsChange.git`
 * arrives about once a second, so the column strobed and the popup could not be read at all.
 *
 * With `keepVisible` the previous answer stays on screen while the new one is fetched and the
 * swap happens in one paint. That is the same repair the log's soft refresh makes, and it is the
 * same argument: a re-read of a question already answered must not first un-answer it.
 *
 * The failure path is deliberately different. A *fresh* blame that fails shows its reason; a
 * background re-read that fails keeps the last good answer, because a ref moving is not evidence
 * that the file stopped being blameable and replacing a working column with a sentence would be
 * a worse answer than a slightly stale one.
 */
function startBlame(
  project: ProjectId,
  path: string,
  contents: string | null | undefined,
  keepVisible: boolean,
): void {
  const key = keyOf(project, path)
  // The caller's text wins when it passed one; otherwise ask the registry, so a blame started
  // from the palette is as correct as one started from the buffer's own menu. `undefined` from
  // both means the tab is clean and Rust should read the file itself.
  const text = contents ?? dirtyText.get(path)?.() ?? null
  /*
   * A background re-read of a buffer that already has an answer changes **nothing** here — no
   * state write and no notification — and that second half matters as much as the first.
   *
   * `emit` bumps a revision counter that every annotated editor reads, so notifying with the
   * same data still costs a render and a gutter reconfiguration. Leaving the state alone but
   * announcing it anyway would have fixed the empty column and kept the strobe.
   */
  const showing = keepVisible && states.get(key)?.kind === 'ready'
  if (!showing) {
    states.set(key, { kind: 'loading' })
  }
  // The first annotated buffer is what starts the watcher: there is nothing to refresh before it
  // and nothing to tear down after, so the subscription's whole lifetime is bracketed by "the
  // user has asked for blame at least once". See [`watchGit`].
  watchGit()
  if (!showing) emit()
  const mine = generation
  void historyApi
    .locate(project, path)
    .then((found) => {
      if (found === null) {
        // Not an error anywhere below this line — the file is simply outside every repository
        // this project has open, which is what a scratch file or a dependency source is.
        set(
          key,
          {
            kind: 'failed',
            reason: 'This file is not inside any repository in this project.',
          },
          mine,
        )
        return undefined
      }
      return historyApi
        .blame(project, found.repo as RepoId, found.path, text)
        .then((blame) => {
          set(key, { kind: 'ready', blame }, mine)
        })
    })
    .catch((error: unknown) => {
      // A background re-read that fails leaves the last good answer up. See the header: a ref
      // moving is not evidence the file stopped being blameable.
      if (keepVisible && states.get(key)?.kind === 'ready') return
      // A `GitError` is `{kind, detail}`, so `String(error)` is `[object Object]` — the bug
      // `check:branches` exists for. The gutter has no room for a sentence, so this reaches the
      // popup and the notice rather than the column.
      const detail =
        typeof error === 'object' && error !== null && 'detail' in error
          ? String((error as { detail: unknown }).detail)
          : String(error)
      set(key, { kind: 'failed', reason: detail }, mine)
    })
}

/** Drop a buffer's annotation — the file was reloaded, or its pane went away. */
export function forgetBlame(project: ProjectId, path: string): void {
  if (states.delete(keyOf(project, path))) emit()
}

/**
 * Re-read every annotated buffer, because a ref moved.
 *
 * Driven by `FsChange.git` through [`watchGit`] — so a `git commit` in a bash pane refreshes the
 * column with no new event and no new watcher. The buffer text is deliberately not re-sent: this
 * is the background refresh, and a caller that holds the text passes it to [`toggleBlame`]
 * instead.
 *
 * `project` narrows it to one project's buffers. A shell window holds several projects at once
 * and a ref moving in one says nothing about the others; re-blaming all of them would be one
 * `git blame` of every annotated file in the window per commit anywhere in it. Omitting it
 * re-reads everything, which is what a caller with no project in hand should ask for.
 */
export function refreshBlame(project?: ProjectId): void {
  sweep(project === undefined ? null : new Set([project]))
}

/**
 * One sweep over the annotated buffers, optionally narrowed to a set of projects.
 *
 * A *set* and not a project, because one coalesced burst can name several: a shell window holds
 * more than one project and a `git pull` in each within 120 ms of the other is one timer. The
 * important part is that it is **one** sweep — see the generation bump — and calling
 * [`refreshBlame`] once per project would have the second call's bump invalidate the first
 * call's in-flight requests, leaving those buffers stuck on `loading` for ever.
 */
function sweep(projects: ReadonlySet<ProjectId> | null): void {
  // The generation is bumped **once for the whole sweep**, before any request goes out, so that
  // every answer already in flight — from an earlier sweep or from a user's own toggle — is
  // dropped rather than installed over the fresher one. `chrome/gitCountStore.ts` has the same
  // counter for the same two races, and states its reasoning at length.
  generation += 1
  for (const [key, state] of [...states]) {
    if (state.kind === 'off') continue
    const cut = key.indexOf(SEP)
    if (cut === -1) continue
    const owner = key.slice(0, cut) as ProjectId
    if (projects !== null && !projects.has(owner)) continue
    const path = key.slice(cut + SEP.length)
    // **Not** `states.delete` + `toggleBlame`. That pair is what made the column strobe: the
    // delete emptied the gutter and the toggle put it back through `loading`, once per git
    // write. `startBlame(…, true)` leaves the answer on screen and swaps it when the new one
    // lands. The generation bump above is still what drops a superseded reply.
    startBlame(owner, path, undefined, true)
  }
}

/**
 * Bumped by every [`refreshBlame`], and captured by every fetch.
 *
 * Without it a sweep that starts while a fetch is in flight would let the *older* answer land
 * afterwards: [`refreshBlame`] deletes the key and [`toggleBlame`] puts it straight back, so the
 * `states.has(key)` guard in [`set`] — which is there for a toggle-*off*, a different race — sees
 * a key that exists and installs the stale blame over the fresh request. That is a gutter showing
 * the previous HEAD with no way to notice, which is the whole class of bug this column has.
 */
let generation = 0

/** The window's `cide://fs-changed` subscription, started once. See [`watchGit`]. */
let watching = false

/**
 * Follow the filesystem watcher, from the first annotated buffer onwards.
 *
 * # Why it starts here and not at module scope
 *
 * A `void events.onFsChanged(…)` beside the imports would run the moment anything pulled this
 * module in — including a check script or an SSR bundle that only wanted a type — and `listen`
 * reaches for Tauri. Starting on the first `toggleBlame` is both later and *exactly* right: there
 * is nothing to refresh until a buffer is annotated, and the listener costs nothing for the
 * majority of sessions where nobody turns the column on.
 *
 * # Why it is here and not in a component
 *
 * `panes/EditorPane.tsx` is the obvious host and it is the wrong one: it mounts once per editor
 * pane, so a split view would subscribe twice and every ref move would sweep every annotated
 * buffer once per open editor. The subscription is a window-level fact and belongs with the
 * window-level store.
 *
 * # Never torn down
 *
 * The same argument as `sidebar/GitPanel/openDiffTabs.ts`: one listener for the lifetime of the
 * window, against a store whose contents come and go with every tab. Refcounting it would mean
 * unsubscribing and re-subscribing every time the last annotated buffer closed, to answer a
 * question whose answer had not changed.
 *
 * # A throttle, and only for a ref
 *
 * `logModel::gitRefsMoved` is the predicate rather than `FsChange.git` itself, because the flag
 * also covers `.git/index` — and a `git add`, or a `git status` in a loop, must not re-blame every
 * open file. The 120 ms timer is a *throttle*, not a debounce: `cide-fs` has already coalesced the
 * burst, and a debounce that restarts on each trigger is the starvation `chrome/gitCountStore.ts`
 * and `editor/docSync.ts` have both written down.
 */
const REFRESH_MS = 120
let tick: ReturnType<typeof setTimeout> | null = null
/** The projects whose refs moved inside the current throttle window. */
let dirtyProjects = new Set<ProjectId>()

function watchGit(): void {
  if (watching) return
  watching = true
  // A failure to subscribe leaves the column exactly as it was before this existed — stale until
  // the user toggles it — which is a degradation and not a reason for a toast on a gesture nobody
  // made.
  void events
    .onFsChanged((project: ProjectId, change) => {
      if (!gitRefsMoved(change)) return
      dirtyProjects.add(project)
      if (tick !== null) return
      tick = setTimeout(() => {
        tick = null
        const projects = dirtyProjects
        dirtyProjects = new Set()
        sweep(projects)
      }, REFRESH_MS)
    })
    .catch(() => {})
}

function set(key: string, state: BlameState, tag: number): void {
  // Only if the buffer is still annotated: a toggle-off that lands while the fetch is in flight
  // must win, or the column comes back on its own a moment after the user turned it off.
  if (!states.has(key)) return
  // …and only if nothing has asked the same question again since. See [`generation`].
  if (tag !== generation) return
  states.set(key, state)
  emit()
}
