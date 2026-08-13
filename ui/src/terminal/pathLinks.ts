/**
 * File links in terminal panes: the DOM and xterm half of `pathMatch.ts`.
 *
 * Every *rule* is next door in `pathMatch.ts`, which is pure and import-free so
 * `ui/scripts/check-paths.mjs` can compile it alone and drive it under node. What is here is
 * everything that needs a terminal, a mouse or an IPC call, and it owns no decisions of its
 * own beyond the two written out below.
 *
 * # How xterm offers links, and why the cost of this feature is close to zero
 *
 * `Terminal.registerLinkProvider` (typings/xterm.d.ts:1102) takes one method:
 * `provideLinks(bufferLineNumber, callback)`. It is called **only when the pointer crosses
 * into a buffer line it has not asked about** (`Linkifier._handleHover`,
 * `src/browser/Linkifier.ts:89`) — never on render, never on output, never for the thousands
 * of lines a build log scrolls past. So there is no per-byte cost at all, and the callback may
 * be invoked asynchronously (`:135`), which is what lets the existence probe below be a round
 * trip rather than a guess.
 *
 * The line number is 1-based and absolute over the whole buffer including scrollback, while
 * `buffer.active.getLine()` is 0-based — hence the `y - 1` in [`windowedLineStrings`]. Ranges
 * handed back are 1-based with `end.x` inclusive.
 *
 * `OscLinkProvider` is registered in the terminal's constructor, so it is index 0 and outranks
 * this one; `Linkifier._removeIntersectingLinks` drops ours wherever an OSC 8 hyperlink
 * overlaps. That is the right precedence: a program that went to the trouble of emitting a
 * real hyperlink has said more than our matcher can infer.
 *
 * # Decision 1: the click gate is ours, in capture, on the host element
 *
 * xterm's activation has no modifier check and fires on `mouseup` for *any* button
 * (`Linkifier._handleMouseUp`, `:220`). Left alone, a plain left click — the gesture a user
 * makes to focus a pane — would open files, and so would the mouseup of a right click. So the
 * modifier policy has to be ours.
 *
 * It cannot be a listener that merely runs first, either. `CoreBrowserTerminal` installs an
 * always-on `mousedown` (`:779`) that calls `preventDefault()`, focuses, and then — when the
 * child has mouse tracking on and no force-selection modifier is held — **writes a mouse
 * report to the pty**. On Linux `shouldForceSelection` is `event.shiftKey`
 * (`SelectionService.ts:437`); Ctrl is not a bypass. So under `vim`, `htop`, `tmux` or any
 * other mouse-tracking TUI, an unguarded ctrl+click is bytes at the child.
 *
 * The gate is therefore a **capture-phase** `mousedown` on `host.el` — the element `term.open()`
 * was given, hence an ancestor of both `term.element` and the screen element, so it runs before
 * every listener xterm has. Exactly the idiom and exactly the element `inputHost.ts` already
 * relies on for the keyboard.
 *
 * Consequence, and it decides the architecture: stopping the event in capture also hides it
 * from xterm's own `Linkifier`, so `ILink.activate` will not fire for the click we swallowed.
 * That is why **the provider records what is under the pointer and the capture listener
 * performs the open**, using `ILink.hover`/`leave` as the channel — which is what they are for,
 * and which needs no coordinate mapping of our own. `activate` is still implemented and still
 * guarded on the modifier, as the belt-and-braces path for a click we did not swallow.
 *
 * Nothing else is touched: middle-click paste, the pane's own `contextmenu`, shift-drag
 * force-selection and ordinary selection all see their events exactly as before.
 *
 * # Decision 2: hover underlines, the modifier opens, and a plain click says so
 *
 * The link is decorated on hover (xterm's default: pointer cursor and underline), because an
 * affordance nobody can see is not an affordance. But a plain click must not open anything, and
 * an underline that does nothing when clicked is the dead control this project keeps finding.
 * So a plain left click on a link, when it made no selection, answers with one sentence saying
 * which gesture opens it. Fired from `click` rather than `mousedown` and gated on an empty
 * selection specifically so that *dragging* across a path to copy it — the other thing users do
 * to paths — stays silent.
 */
import type { IBufferLine, ILink, IDisposable } from '@xterm/xterm'
import { notify } from '@/chrome/notices'
import { events, fs as fsApi, session as sessionApi } from '@/ipc/client'
import { candidatePaths, matchPaths, resolveCandidate, type Candidate, type Resolution } from './pathMatch'
import type { TerminalHandle } from './xterm'

/**
 * What a pane needs to know before a path in it means anything.
 *
 * Supplied by `TerminalPane` through [`setPathLinkEnv`] rather than passed to
 * [`attachPathLinks`], because the two have different lifetimes: the attachment belongs to the
 * host (which outlives every mount) and these values belong to the React props (which change
 * without the host changing). The same split `paneHosts.ts` makes everywhere else.
 */
export interface PathLinkEnv {
  readonly project: string
  /** The project's roots, in `Project.roots` order. Containment's first lock. */
  readonly roots: readonly string[]
  /** The cwd this pane's child was spawned with — the project's primary root. */
  readonly cwd: string
  /**
   * Open a file tab. `at` is 1-based, and absent when the producer named no line.
   *
   * Nullable, and read late, because "no handler" has to be distinguishable from "a handler
   * that does nothing": a pane with nowhere to send an open offers **no links at all** rather
   * than underlining text that takes a click and swallows it. That is the failure this project
   * has now found twelve times, and it is one `?? null` away here.
   */
  readonly open: ((path: string, at: { line: number; column: number } | null) => void) | null
}

const envs = new Map<string, PathLinkEnv>()

/** Called from `TerminalPane`'s effect. Passing `null` on cleanup makes the pane's links inert. */
export function setPathLinkEnv(paneId: string, env: PathLinkEnv | null): void {
  if (env === null) envs.delete(paneId)
  else envs.set(paneId, env)
}

/** What the index says is at a path. `absent` is an answer, and it is cached like the others. */
type Existence = 'file' | 'dir' | 'absent'

/**
 * Answers from `fs_paths_exist`, keyed by absolute path.
 *
 * Bounded and invalidated rather than given a TTL. The invalidation channel already exists —
 * `cide://fs-changed`, debounced 300 ms in Rust — and it is exact: a path that changed is
 * dropped, and everything else stays. A TTL would be a number invented to stand in for a fact
 * the app is already being told.
 */
const existence = new Map<string, Existence>()

/** Entries kept. A build log hovered end to end is a few thousand distinct paths at most. */
const CACHE_MAX = 4096

/** Paths per `fs_paths_exist` call. Mirrors the clamp in `cmd/fs.rs`. */
const PROBE_MAX = 128

/** How long a pane's `/proc` cwd is believed. See [`sessionCwd`]. */
const CWD_TTL_MS = 1_000

function remember(path: string, kind: Existence): void {
  // Delete before set: `Map` iterates in insertion order and the eviction below takes the
  // oldest, so an in-place update would leave a freshly answered path first in line to go.
  existence.delete(path)
  existence.set(path, kind)
  while (existence.size > CACHE_MAX) {
    const oldest = existence.keys().next().value
    if (oldest === undefined) break
    existence.delete(oldest)
  }
}

let watching = false

/**
 * Start dropping cache entries for files the watcher reports.
 *
 * Once per window, on the first attachment, and never unsubscribed: this module is a
 * singleton for the life of the webview, exactly like `paneHosts`' registry, and a listener
 * that came and went with panes would leave the cache stale for whichever pane was last.
 */
function watchInvalidation(): void {
  if (watching) return
  watching = true
  void events
    /*
     * `events.onFsChanged`, not `fsEvents.onChanged`. Both listened to `cide://fs-changed`, but
     * the latter read `payload.paths` while the wire carries `payload.change.paths`, so its
     * `paths` was `undefined` on every burst — `for (const path of paths)` threw inside the
     * listener, the `.catch` below never saw it (the `listen()` promise had already resolved),
     * and this cache was never invalidated for the life of the window. `Explorer.tsx` had
     * already found that trap and written it down; the stale helper is now deleted rather than
     * documented, because a wrong-shaped duplicate that only hurts its second caller is not
     * something a comment can be relied on to prevent.
     */
    .onFsChanged((_project, change) => {
      for (const path of change.paths) existence.delete(path)
    })
    .catch(() => {
      // No watcher in this build or this window. The cache then only grows staler than it
      // would otherwise be, which shows up as a link offered for a file that has since been
      // deleted — and that click is refused in Rust with a sentence.
      watching = false
    })
}

/** In-flight and recent `/proc` answers, per session. */
const cwdCache = new Map<string, { at: number; cwd: string | null }>()
const cwdPending = new Map<string, Promise<string | null>>()

/**
 * The cwd this pane's child is in **now**, or `null`.
 *
 * This is what makes a shell pane that has run `cd ui` resolve `src/App.tsx` — which is not an
 * exotic case here, it is this repo's own documented gate (`cd ui && pnpm run typecheck`), and
 * without it every tsc, vite and esbuild diagnostic in a shell pane is unlinkable.
 *
 * Three things it is not, all of them accepted rather than hidden:
 *
 * * It is the **direct child's** cwd, so a `cd` inside a subshell or a `make -C` is invisible.
 * * It is the cwd **now**, not the cwd the line was printed from. That is precisely why
 *   `resolveCandidate` refuses to let a cwd out-rank a root: a user who built in `ui/`, then
 *   `cd ..`, then ctrl+clicks an old line must not be sent confidently to the wrong file.
 * * It is Linux-only (`/proc`), which this app is.
 *
 * Cached for a second so a fast vertical drag across a build log costs one IPC call rather
 * than one per line, and de-duplicated while in flight so a burst of hovers costs one too.
 */
async function sessionCwd(project: string, session: string | null): Promise<string | null> {
  if (session === null) return null
  const now = Date.now()
  const cached = cwdCache.get(session)
  if (cached !== undefined && now - cached.at < CWD_TTL_MS) return cached.cwd
  const inFlight = cwdPending.get(session)
  if (inFlight !== undefined) return inFlight

  const request = sessionApi
    .cwd(project, session)
    .catch(() => null)
    .then((cwd) => {
      cwdCache.set(session, { at: Date.now(), cwd })
      cwdPending.delete(session)
      return cwd
    })
  cwdPending.set(session, request)
  return request
}

/** Ask about every path not already answered for, in bounded batches. */
async function probe(project: string, paths: string[]): Promise<void> {
  for (let i = 0; i < paths.length; i += PROBE_MAX) {
    const batch = paths.slice(i, i + PROBE_MAX)
    let answers: Array<'dir' | 'file' | null>
    try {
      answers = await fsApi.pathsExist(project, batch)
    } catch {
      /*
       * The project has no index yet, or the command is older than this frontend. Neither is
       * worth a toast on a mouse *hover* — the honest result is that nothing lights up until
       * the walk finishes. Deliberately not cached as `absent`: caching a failure would make
       * the pane stay dark after the index arrives.
       */
      return
    }
    for (const [j, path] of batch.entries()) {
      const answer = answers[j]
      remember(path, answer === null || answer === undefined ? 'absent' : answer)
    }
  }
}

/**
 * Attach this pane's link provider and its click gate.
 *
 * Returns the removal function; `paneHosts.openTerminal` puts it in `host.cleanup` so
 * `teardown` takes it down with the host, exactly like `attachInputRouting`.
 */
export function attachPathLinks(
  handle: TerminalHandle,
  el: HTMLElement,
  ctx: {
    readonly paneId: string
    /** The live session on this pane, read fresh — it is `null` until the child spawns. */
    readonly session: () => string | null
  },
): () => void {
  watchInvalidation()
  const term = handle.term

  /**
   * What the pointer is over, as recorded by `ILink.hover`.
   *
   * The single piece of state this module keeps, and the reason it exists is written in the
   * header: the capture listener swallows the click before xterm's `Linkifier` can see it, so
   * `ILink.activate` cannot be the route.
   */
  let hovered: { readonly act: () => void } | null = null

  const provider = {
    provideLinks(y: number, callback: (links: ILink[] | undefined) => void): void {
      void linksFor(y).then(
        (links) => callback(links),
        () => callback(undefined),
      )
    },
  }

  async function linksFor(y: number): Promise<ILink[] | undefined> {
    const env = envs.get(ctx.paneId)
    // No project, no roots, or nowhere to send an open. The first two mean there is nothing to
    // resolve against — `candidatePaths` would answer `[]` for every candidate anyway — and the
    // third means a link would be drawn that could not do anything. All three return before the
    // line is even scanned, which is what makes "no link" the visible state rather than "a link
    // that swallows clicks".
    if (env === undefined || env.project === '' || env.roots.length === 0) return undefined
    if (env.open === null) return undefined

    const [lines, topIdx] = windowedLineStrings(y - 1, term)
    if (lines.length === 0) return undefined
    const logical = lines.join('')

    const candidates = matchPaths(logical)
    if (candidates.length === 0) return undefined

    // The pane's own cwd first, the spawn cwd second. `candidatePaths` de-duplicates, so a
    // pane that never changed directory pays for one base and not two.
    const proc = await sessionCwd(env.project, ctx.session())
    const cwds = proc === null ? [env.cwd] : [proc, env.cwd]
    const bases = { cwds, roots: env.roots }

    const unknown: string[] = []
    for (const candidate of candidates) {
      for (const path of candidatePaths(candidate.text, bases)) {
        if (!existence.has(path) && !unknown.includes(path)) unknown.push(path)
      }
    }
    if (unknown.length > 0) await probe(env.project, unknown)

    const isFile = (path: string): boolean => existence.get(path) === 'file'
    const links: ILink[] = []
    let cursor: Cursor = { y: topIdx, x: 0, idx: 0 }
    for (const candidate of candidates) {
      const resolution = resolveCandidate(candidate.text, { ...bases, isFile })
      if (resolution.kind === 'none') continue
      const range = rangeOf(candidate, cursor)
      if (range === null) continue
      // Advance only on success: a failed mapping leaves the cursor where it was, so the next
      // candidate still walks from a position we know is correct rather than from a guess.
      cursor = { y: range.start.y - 1, x: range.start.x - 1, idx: candidate.start }
      const act = (): void => activate(env, candidate, resolution)
      links.push({
        range,
        text: candidate.text,
        // Guarded on the modifier, so a plain click — or the mouseup of a right click, which
        // `Linkifier._handleMouseUp` does not distinguish — opens nothing. This path only runs
        // for a press the capture listener below did not swallow.
        activate: (event: MouseEvent) => {
          if (event.ctrlKey || event.metaKey) act()
        },
        hover: () => {
          hovered = { act }
        },
        leave: () => {
          hovered = null
        },
      })
    }
    return links.length > 0 ? links : undefined
  }

  /** A position in the wrapped block, paired with the string offset it corresponds to. */
  interface Cursor {
    y: number
    x: number
    idx: number
  }

  /**
   * Map a candidate's string offsets back to a 1-based buffer range.
   *
   * `from` is a running cursor, not the top of the wrapped block, and that is the whole point.
   * Walking from `topIdx` for every candidate made the mapping O(start) per candidate and so
   * quadratic over the line — and the input is untrusted output, so the worst case is chosen by
   * whatever the pane happens to print. A wrapped logical line dense with real paths is the
   * shape that hurts, and `resolveCandidate` gates this call on the file *existing*, which a
   * cloned repo decides. Candidates arrive in increasing `start` order, so one cursor carried
   * across them makes the whole line linear.
   */
  function rangeOf(candidate: Candidate, from: Cursor): ILink['range'] | null {
    const [startY, startX] = mapStrIdx(from.y, from.x, candidate.start - from.idx)
    if (startY === -1 || startX === -1) return null
    const [endY, endX] = mapStrIdx(startY, startX, candidate.end - candidate.start)
    if (endY === -1 || endX === -1) return null
    // 1-based, right side including — thus +1 everywhere except `end.x`, which is already
    // exclusive on the way in. Transcribed from `addon-web-links`' `LinkComputer`.
    return { start: { x: startX + 1, y: startY + 1 }, end: { x: endX, y: endY + 1 } }
  }

  /**
   * Map a string index in the joined logical line back to `[lineIndex, columnIndex]`, 0-based.
   *
   * Transcribed from `@xterm/addon-web-links`' `LinkComputer._mapStrIdx`, wide-character
   * correction included, because that addon exports only `WebLinksAddon` — the algorithm we
   * need is not importable. `[-1, -1]` when the walk runs off the end of the buffer.
   */
  function mapStrIdx(lineIndex: number, rowIndex: number, stringIndex: number): [number, number] {
    const buf = term.buffer.active
    const cell = buf.getNullCell()
    let start = rowIndex
    let index = stringIndex
    let line = lineIndex
    while (index) {
      const bufferLine = buf.getLine(line)
      if (!bufferLine) return [-1, -1]
      for (let i = start; i < bufferLine.length; ++i) {
        bufferLine.getCell(i, cell)
        const chars = cell.getChars()
        const width = cell.getWidth()
        if (width) {
          index -= chars.length || 1
          // A wide character that wrapped early leaves an empty last cell; the cells to its
          // right are reset with `chars=''` and `width=1`. Without this correction every
          // range after such a cell is one column out.
          if (i === bufferLine.length - 1 && chars === '') {
            const next = buf.getLine(line + 1)
            if (next && next.isWrapped) {
              next.getCell(0, cell)
              if (cell.getWidth() === 2) index += 1
            }
          }
        }
        if (index < 0) return [line, i]
      }
      line++
      start = 0
    }
    return [line, start]
  }

  /*
   * The gate. Capture phase, on the host element, before every listener xterm owns — see the
   * header for what is downstream of it and why merely being first is not enough.
   */
  let swallowed = false

  const onMouseDown = (ev: MouseEvent): void => {
    // Cleared on *every* press, not only on the ones taken. A ctrl+press whose release lands
    // outside the window fires no `click` at all, and a flag left standing from it would eat
    // the next ordinary click in this pane — a swallowed press that opens nothing is the same
    // dead control this whole file is careful to avoid.
    swallowed = false
    if (ev.button !== 0) return
    if (!ev.ctrlKey && !ev.metaKey) return
    const target = hovered
    if (target === null) return
    ev.preventDefault()
    ev.stopPropagation()
    swallowed = true
    target.act()
  }

  const onClick = (ev: MouseEvent): void => {
    if (swallowed) {
      // The press was ours; its click is not a gesture anything else should see. React 19
      // delegates to the root container, so stopping it here is what keeps the pane's own
      // handlers from also reacting to the press that opened a file.
      swallowed = false
      ev.preventDefault()
      ev.stopPropagation()
      return
    }
    if (ev.button !== 0 || ev.ctrlKey || ev.metaKey) return
    if (hovered === null) return
    // A drag that selected the path is not a click that missed; staying silent there is the
    // whole reason this is on `click` and reads the selection.
    if (term.getSelection() !== '') return
    notify('Ctrl+click a path in a terminal to open it.', { kind: 'info' })
  }

  el.addEventListener('mousedown', onMouseDown, true)
  el.addEventListener('click', onClick, true)

  let registration: IDisposable | null = null
  try {
    registration = term.registerLinkProvider(provider)
  } catch {
    // A terminal disposed between `ensureTerminal` and here. The listeners still come off.
  }

  return () => {
    el.removeEventListener('mousedown', onMouseDown, true)
    el.removeEventListener('click', onClick, true)
    hovered = null
    try {
      registration?.dispose()
    } catch {
      // Disposing a provider on an already-disposed terminal throws; nothing to do about it.
    }
  }
}

/**
 * Do what the link says, or explain why nothing can be done.
 *
 * The `many` arm is the one worth reading. It refuses, and it names every file. The
 * alternative that lost was a context menu with one item per candidate: better in isolation,
 * but it needs the pane's React menu plumbing threaded down into a native DOM listener, and the
 * case is rare enough (two roots holding the same relative path, or a cwd that agrees with a
 * root) that a sentence naming the candidates lets the user act through the file tree or Ctrl+P
 * without a new surface. What it must never do is take the first one — `resolveCandidate` went
 * to the trouble of distinguishing `one` from `many` precisely so this arm could exist.
 */
function activate(env: PathLinkEnv, candidate: Candidate, resolution: Resolution): void {
  if (resolution.kind === 'many') {
    notify(
      `${candidate.text} names ${resolution.paths.length} files in this project, so cide will ` +
        'not guess which one you meant.',
      {
        kind: 'info',
        detail: resolution.paths.join('\n'),
        hint: 'Open the one you want from the file tree, or with Ctrl+P.',
      },
    )
    return
  }
  if (resolution.kind !== 'one') return
  env.open?.(
    resolution.path,
    candidate.line === null ? null : { line: candidate.line, column: candidate.column ?? 1 },
  )
}

/**
 * Rebuild the logical line the pointer is on, following the terminal's own wrapping.
 *
 * Transcribed from `@xterm/addon-web-links`' `LinkComputer._getWindowedLineStrings`: expand up
 * while the line `isWrapped` and holds no space, expand down likewise, stop at 2048 characters
 * in each direction. `trimRight=true` on purpose, which is what makes an early-wrapped wide
 * character match — and what makes [`mapStrIdx`]'s correction necessary.
 *
 * This handles the *terminal's* wrapping and nothing else. A TUI that draws its own line
 * breaks — Claude Code's does — leaves `isWrapped` false, and a path it split is not
 * recoverable from the buffer by anyone. `pathMatch.ts` says so in its "deliberately not
 * matched" list rather than half-handling it.
 */
function windowedLineStrings(
  lineIndex: number,
  term: TerminalHandle['term'],
): [string[], number] {
  let line: IBufferLine | undefined
  let topIdx = lineIndex
  let bottomIdx = lineIndex
  let length = 0
  let content = ''
  const lines: string[] = []

  if ((line = term.buffer.active.getLine(lineIndex))) {
    const current = line.translateToString(true)

    if (line.isWrapped && current[0] !== ' ') {
      length = 0
      while ((line = term.buffer.active.getLine(--topIdx)) && length < 2048) {
        content = line.translateToString(true)
        length += content.length
        lines.push(content)
        if (!line.isWrapped || content.indexOf(' ') !== -1) break
      }
      lines.reverse()
    }

    lines.push(current)

    length = 0
    while ((line = term.buffer.active.getLine(++bottomIdx)) && line.isWrapped && length < 2048) {
      content = line.translateToString(true)
      length += content.length
      lines.push(content)
      if (content.indexOf(' ') !== -1) break
    }
  }
  return [lines, topIdx]
}
