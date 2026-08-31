/**
 * The HEAD text each open buffer is measured against. (M35)
 *
 * `editor/blameStore.ts` is the template and most of its reasoning is reproduced here rather than
 * referred to, because the two stores will be read side by side and the *divergences* are what a
 * reader needs. What is copied wholesale: the key shape, the `generation` counter that drops a
 * superseded reply, the single lazily-started `cide://fs-changed` subscription, and the 120 ms
 * **throttle** (never a debounce — see `gitCountStore.ts` on starvation).
 *
 * # HEAD, not the index
 *
 * IDEA's "base revision". Staging a hunk does not make its marker disappear, which is what a user
 * who stages half a file expects — and the practical half is that HEAD moves only on a commit,
 * checkout, pull or rebase, which is exactly what `gitRefsMoved` detects. Staging costs no
 * refetch at all.
 *
 * # Three divergences from blame, each of which *removes* machinery
 *
 * **No `registerDirtyBuffer`.** Blame needs the dirty buffer because Rust does the attribution.
 * Here the only thing fetched is the immutable HEAD blob; the working side of the comparison is
 * the live CodeMirror document, which the extension already holds. So no copy of the buffer ever
 * crosses the IPC wire, on any path, however fast the user types.
 *
 * **No `loading` state, and therefore nothing to strobe.** Blame needed a `keepVisible` flag to
 * stop the column emptying on every git write. Here a refetch that returns the same blob oid
 * writes nothing and — as importantly — **emits nothing**: notifying with identical data still
 * costs every open editor a render and a re-diff. The previous baseline simply stays until a
 * genuinely different one lands.
 *
 * **Every failure is silent.** Blame reports its reason through a notice because turning the
 * column on is a *gesture*, and a gesture that does nothing is this project's named worst
 * failure. Change markers are ambient — nobody asked for them — so a file outside every
 * repository, a file absent from HEAD, a binary blob, a truncated one and an empty repository
 * with an unborn HEAD all resolve to the same `none`. **No error variant is ever inspected**,
 * which is deliberate: `file_at_revision` rejects an unborn HEAD with `NoSuchCommit` and a
 * missing path with `NotTracked`, and a taxonomy that knew about one of those would fall through
 * on the other.
 */
import { events, gitLog as gitLogApi, history as historyApi } from '@/ipc/client'
import type { ProjectId, RepoId } from '@/ipc/client'
import { gitRefsMoved } from '@/gitlog/logModel'
import { splitBaseline } from './changeModel'

/** What a buffer's markers are measured against. */
export type BaselineState =
  | { readonly kind: 'none' }
  /** HEAD's lines, already normalised by `splitBaseline`. */
  | { readonly kind: 'ready'; readonly lines: readonly string[]; readonly oid: string }

const NONE: BaselineState = { kind: 'none' }

/**
 * The key separator — `\u0000`, the one byte a Linux path cannot contain.
 *
 * `blameStore` says the rest: anything printable could be produced by a real filename, and a
 * colliding key is two buffers sharing one answer.
 */
const SEP = '\u0000'

const keyOf = (project: ProjectId, path: string): string => `${project}${SEP}${path}`

const states = new Map<string, BaselineState>()
/** How many panes have each key open, so the last one closing drops the entry. */
const refs = new Map<string, number>()
const listeners = new Set<() => void>()

/**
 * Bumped on every write, and returned by [`baselineRevision`].
 *
 * `useSyncExternalStore` compares snapshots with `Object.is`, so the snapshot has to be one
 * stable value — a **number**. Returning a state object here is the `check:selectors` bug that
 * re-renders for ever and unmounts the whole root.
 */
let rev = 0

function emit(): void {
  rev += 1
  for (const listener of listeners) listener()
}

export function subscribeBaselines(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function baselineRevision(): number {
  return rev
}

/**
 * The lines to measure this buffer against, or `null` when there is no answer.
 *
 * **Identity-stable**: the same array reference comes back until a genuinely different baseline
 * lands, so the effect in `EditorSurface` that pushes it into CodeMirror does not refire — and
 * therefore does not rebuild a `RangeSet` — every time some *other* file's baseline arrives.
 */
export function baselineFor(project: ProjectId, path: string): readonly string[] | null {
  const state = states.get(keyOf(project, path))
  return state !== undefined && state.kind === 'ready' ? state.lines : null
}

/**
 * A pane opened this file. Fetches on the first one; the rest join.
 *
 * Refcounted rather than fetched per pane, because a split showing one file twice is one
 * question about one file's history — the same keying `docSync.ts` uses, for the same reason.
 */
export function openBaseline(project: ProjectId, path: string): void {
  const key = keyOf(project, path)
  const held = refs.get(key) ?? 0
  refs.set(key, held + 1)
  if (held > 0) return
  watchGit()
  fetch(project, path, generation)
}

/** A pane closed it. The last one out drops the answer. */
export function releaseBaseline(project: ProjectId, path: string): void {
  const key = keyOf(project, path)
  const held = refs.get(key) ?? 0
  if (held <= 1) {
    refs.delete(key)
    // Dropping the state without emitting: nobody is drawing this buffer any more, and a
    // notification here would re-render every *other* editor in the window for a fact none of
    // them can see.
    states.delete(key)
    return
  }
  refs.set(key, held - 1)
}

/**
 * Read HEAD's copy of one file.
 *
 * Two hops, both of which can answer "there is nothing here" without that being an error:
 * `git_locate` returns `null` for a file in no repository — a scratch file, a dependency source,
 * a tab from another project — and the blob read rejects for a path HEAD does not have.
 */
function fetch(project: ProjectId, path: string, tag: number): void {
  void historyApi
    .locate(project, path)
    .then((found) => {
      // Not an error: the file is simply outside every repository this project has open. No
      // request is made, and nothing is reported — see the header on why this is silent.
      if (found === null) {
        set(project, path, NONE, tag)
        return undefined
      }
      return gitLogApi.fileAt(project, found.repo as RepoId, found.path, 'HEAD').then((blob) => {
        /*
         * Two refusals, and each of them would otherwise paint a confident lie.
         *
         * `binary` comes back with an empty `text`, so every line of the buffer would read as
         * added. `truncated` is the 2 MiB cap in `cide_git::revision::MAX_BLOB_BYTES`: the cut is
         * whole-line, so nothing looks wrong — everything below it simply reads as added, which
         * is a screenful of plausible green.
         */
        if (blob.binary || blob.truncated) {
          set(project, path, NONE, tag)
          return
        }
        set(project, path, { kind: 'ready', lines: splitBaseline(blob.text), oid: blob.oid }, tag)
      })
    })
    .catch(() => {
      /*
       * Variant-blind, deliberately. `NotTracked` is a file HEAD does not have — a new file, or
       * one that has just been renamed, since `git_diff_file`'s pathspec makes rename detection
       * inert. `NoSuchCommit` is an empty repository whose HEAD is unborn. Both mean the same
       * thing here, and so does anything else this call can fail with.
       */
      set(project, path, NONE, tag)
    })
}

/**
 * Install an answer, unless it has been superseded or the file has been closed.
 *
 * The oid comparison is the anti-strobe rule and the anti-work rule at once: a ref moving that
 * did not touch *this* file returns the same blob, and re-installing it would cost every open
 * editor a render and a full re-diff for a baseline that had not changed.
 */
function set(project: ProjectId, path: string, state: BaselineState, tag: number): void {
  const key = keyOf(project, path)
  if (!refs.has(key)) return
  if (tag !== generation) return
  const held = states.get(key)
  if (held !== undefined) {
    if (held.kind === 'none' && state.kind === 'none') return
    if (held.kind === 'ready' && state.kind === 'ready' && held.oid === state.oid) return
  } else if (state.kind === 'none') {
    // The absent state and `none` are the same thing to every reader, so recording it is only
    // worth a write when something was there before.
    states.set(key, state)
    return
  }
  states.set(key, state)
  emit()
}

/**
 * Bumped once per sweep, captured by every fetch.
 *
 * Without it a sweep that starts while a fetch is in flight lets the *older* answer land
 * afterwards — `blameStore`'s counter, for exactly the same race.
 */
let generation = 0

/** Re-read every open buffer's baseline, because a ref moved. */
export function refreshBaselines(projects: ReadonlySet<ProjectId> | null): void {
  // Once for the whole sweep, before any request goes out. Bumping per file would have each
  // call invalidate the previous one's in-flight request.
  generation += 1
  const mine = generation
  for (const key of [...refs.keys()]) {
    const cut = key.indexOf(SEP)
    if (cut === -1) continue
    const owner = key.slice(0, cut) as ProjectId
    if (projects !== null && !projects.has(owner)) continue
    fetch(owner, key.slice(cut + SEP.length), mine)
  }
}

/** The window's `cide://fs-changed` subscription, started once. */
let watching = false

const REFRESH_MS = 120
let tick: ReturnType<typeof setTimeout> | null = null
/** The projects whose refs moved inside the current throttle window. */
let dirtyProjects = new Set<ProjectId>()

/**
 * Follow the filesystem watcher, from the first open buffer onwards.
 *
 * Started here and not at module scope, for `blameStore`'s reason: a `void events.onFsChanged(…)`
 * beside the imports runs the moment anything pulls this module in, including a check script that
 * only wanted a type, and `listen` reaches for Tauri. Not in a component either — `EditorPane`
 * mounts once per pane, so a split would subscribe twice and sweep twice per ref move. Never torn
 * down: one listener for the life of the window, against a map whose contents come and go.
 *
 * `gitRefsMoved` and not `FsChange.git`, because the flag also covers `.git/index` and a `git add`
 * — or a `git status` in a loop — must not re-read every open file's HEAD blob. A working-tree
 * write needs no refresh either: the baseline is HEAD, and HEAD does not move when a file is
 * saved. The *buffer* side of the comparison is the live document, which the extension re-reads
 * on its own timer.
 */
function watchGit(): void {
  if (watching) return
  watching = true
  // A failed subscription leaves the markers exactly as stale as they would have been before this
  // existed, which is a degradation and not a reason for a toast on a gesture nobody made.
  void events
    .onFsChanged((project: ProjectId, change) => {
      if (!gitRefsMoved(change)) return
      dirtyProjects.add(project)
      if (tick !== null) return
      tick = setTimeout(() => {
        tick = null
        const projects = dirtyProjects
        dirtyProjects = new Set()
        refreshBaselines(projects)
      }, REFRESH_MS)
    })
    .catch(() => {})
}

/** Drop everything. For checks and for a project teardown; never called on a normal path. */
export function resetBaselinesForTest(): void {
  states.clear()
  refs.clear()
  generation += 1
  emit()
}
