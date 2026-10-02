/**
 * File links in terminal panes: the DOM and xterm half of `pathMatch.ts`.
 *
 * Parsing, buffer reconstruction and lookup caching live in testable modules next door, so
 * `ui/scripts/check-paths.mjs` can exercise them under node. What is here is
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
 * `buffer.active.getLine()` is 0-based — hence the `y - 1` passed to `readPathLine`. Ranges
 * handed back are 1-based with `end.x` inclusive.
 *
 * `OscLinkProvider` is registered in the terminal's constructor, so it is index 0 and outranks
 * this one; `Linkifier._removeIntersectingLinks` drops ours wherever an OSC 8 hyperlink
 * overlaps. That is the right precedence: a program that went to the trouble of emitting a
 * real hyperlink has said more than our matcher can infer.
 *
 * # Decision 1: the click gate is ours, in capture, on the host element — unconditionally
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
 * **The claim is unconditional**, and that is M15's correction. The gate used to require a
 * completed hover before it would swallow anything, and in a Claude Code pane that condition is
 * routinely false — the whole account is in `clickGate.ts`, which now owns the rule. The
 * consequence of the old spelling was not "the click does nothing": the press reached xterm,
 * xterm wrote an SGR report with the Ctrl bit to the pty, and `claude` answered it by forking
 * `dbus-send … FileManager1.ShowItems` — a desktop file manager, opened on the parent directory
 * of the file the user pointed at, from a gesture cide had declined and thereby handed over.
 *
 * So the sequence is now: claim the press synchronously, then work out what it landed on from
 * the *buffer*, at press time. That also fixes the mirror-image bug the old three lines had — a
 * stale `hovered` surviving a repaint, so the gate could act on a candidate from a line that had
 * since been overwritten.
 *
 * Consequence, and it decides the architecture: stopping the event in capture also hides it
 * from xterm's own `Linkifier`, so `ILink.activate` will not fire for the click we swallowed.
 * `activate` is still implemented and still guarded on the modifier, as the belt-and-braces path
 * for a press that reached xterm some other way.
 *
 * Nothing else is touched: middle-click paste, the pane's own `contextmenu`, shift-drag
 * force-selection and ordinary selection all see their events exactly as before. Alt+click in
 * particular is **not** claimed — see `clickGate.ts` for why, and `xterm.ts`'s XTVERSION reply
 * for what answers it instead.
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
 *
 * # Decision 3: a path outside the project is underlined only if it is really there
 *
 * M13 lets a ctrl+click open a file the project does not contain, once, after a confirmation
 * that names it. That could have been done with no hover-time work at all — offer the link for
 * any well-formed absolute path and let the click answer — and it was not, because a linker
 * error line names `/usr/bin/ld`, `/lib64/libc.so.6`, `/dev/null` and half a dozen `.so`s, and
 * underlining all of them is how an underline stops meaning "cide can open this".
 *
 * Index hits avoid disk work on hover. Index misses and outside-project candidates get a
 * real `stat` through `fs_stat_paths`; their answers expire because the project watcher does
 * not cover ignored directories or outside files. Activation refreshes disk answers.
 */
import type { IBufferCell, IBufferLine, ILink, IDisposable, Terminal } from '@xterm/xterm'
import { notify } from '@/chrome/notices'
import { isWebUrl, openWebLink } from '@/chrome/webLinks'
import { events, fs as fsApi, session as sessionApi, type TreeRowKind } from '@/ipc/client'
import { cellFromPoint, linkAtCell, pressVerdict, type Cell } from './clickGate'
import {
  candidatePaths,
  matchUrls,
  outsidePaths,
  type Candidate,
  type Resolution,
} from './pathMatch'
import { bufferPaths, pathRange, readPathLine, resolveBufferPaths } from './pathBuffer'
import { PathExistence } from './pathExistence'
import { matchTaskCodes } from './taskLinks'
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
  /**
   * Show a **directory** in the project's file tree, or `null` when this window has no tree.
   *
   * Wired to `runCommand('file.reveal', { path })` and to nothing else, so the whole policy —
   * open the Files panel first, then report in a sentence when the path has no row — stays in
   * `keys/dispatch.ts`'s one arm. A second call site straight into `treeStore.reveal` is how
   * three gestures come to behave in three ways, which that arm's own comment says out loud.
   *
   * `null` in a detached pane window, deliberately: that window has no sidebar, so a directory
   * there is offered no link at all rather than an underline whose command would refuse. Same
   * rule, same reason, one field over from [`open`].
   */
  readonly reveal: ((path: string) => void) | null
}

const envs = new Map<string, PathLinkEnv>()

/** Called from `TerminalPane`'s effect. Passing `null` on cleanup makes the pane's links inert. */
export function setPathLinkEnv(paneId: string, env: PathLinkEnv | null): void {
  if (env === null) envs.delete(paneId)
  else envs.set(paneId, env)
}

/** Index hits stay cheap; ignored and outside-project files use the disk fallback. */
const existence = new PathExistence(
  async (project, paths) => (await fsApi.pathsExist(project, paths)).map(linkable),
  async (paths) => (await fsApi.statPaths(paths)).map(linkable),
)

/** How long a pane’s `/proc` cwd is believed. */
const CWD_TTL_MS = 1_000

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
      existence.invalidate(change.paths)
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

/**
 * A probe answer, as this module's two-state cache stores it.
 *
 * `TreeRowKind` gained `group` and `note` in M13 — synthetic rows the *file tree* draws — and
 * neither can ever come back from a path probe, which answers about the disk. Narrowing here
 * rather than widening the cache is the point: a terminal link is offered for a file and for
 * nothing else, so an answer this module has never heard of has to land on `absent` rather than
 * on whatever the compiler was willing to infer.
 */
function linkable(answer: TreeRowKind | null | undefined): 'dir' | 'file' | 'absent' {
  return answer === 'dir' || answer === 'file' ? answer : 'absent'
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
   * Whether the pointer is over a link at all, as reported by `ILink.hover`/`leave`.
   *
   * A **boolean**, not the action. It used to carry the `act` closure, and that made it the
   * channel through which a swallowed press performed its open — which is precisely the design
   * M15 removed: a hover that had not happened (a TUI repainting under a stationary pointer) or
   * had happened for a line since overwritten made the gate either forward the press to the
   * child or act on the wrong candidate. The press now resolves against the buffer itself.
   *
   * What is left is the one thing hover legitimately knows and a press cannot cheaply re-derive:
   * whether an underline is currently drawn under the pointer, which is all
   * [`onClick`]'s "ctrl+click to open it" hint needs.
   */
  let hovered = false

  const provider = {
    provideLinks(y: number, callback: (links: ILink[] | undefined) => void): void {
      void scanLine(y).then(
        (scan) =>
          callback(scan.links.length > 0 ? scan.links.map((entry) => entry.link) : undefined),
        () => callback(undefined),
      )
    },
  }

  /**
   * What one logical line holds: the links to draw, and the candidates that resolved to nothing.
   *
   * The second half is not decoration. A press this module has claimed **must** end in something
   * the user can see, and "`src/foo.rs` does not name a file in this project" is a far better
   * answer than a generic shrug — but only the scan knows the text that failed. Keeping misses
   * costs a range per unresolved fragment. `pathBuffer.ts` maps displayed offsets to cells
   * in one pass, including wide characters and Markdown escapes.
   */
  interface Scan {
    /**
     * Each link, paired with the closure that performs it.
     *
     * Paired rather than reached through `ILink.activate`, because that takes a `MouseEvent` it
     * exists to read modifiers from — and the press handler would have to *fabricate* one to
     * call it. A synthetic event whose only purpose is to satisfy a guard is a guard that has
     * stopped meaning anything, and the next reader cannot tell which of the two callers is
     * real.
     */
    links: Array<{ link: ILink; act: () => void }>
    misses: Array<{ range: ILink['range']; text: string }>
  }

  const EMPTY_SCAN: Scan = { links: [], misses: [] }

  async function scanLine(y: number, refresh = false): Promise<Scan> {
    const current = readPathLine<IBufferCell>(term.buffer.active, y - 1)
    if (current === null) return EMPTY_SCAN

    // Web links first, and before any of the project checks below: a URL needs no project, no
    // roots and no `open` to mean something, so a pane with none of them still opens one.
    const scan: Scan = { links: webLinks(current), misses: [] }

    const env = envs.get(ctx.paneId)
    // No project, no roots, or nowhere to send an open. The first two mean there is nothing to
    // resolve against — `candidatePaths` would answer `[]` for every candidate anyway — and the
    // third means a link would be drawn that could not do anything. All three return before the
    // line is scanned for paths, which is what makes "no link" the visible state rather than "a
    // link that swallows clicks".
    //
    // `reveal` is deliberately NOT part of this test: a window with no file tree still opens
    // files perfectly well, it simply offers no directory links. Requiring both would take file
    // links away from every detached pane.
    if (env === undefined || env.project === '' || env.roots.length === 0) return scan
    if (env.open === null) return scan

    const candidates = bufferPaths<IBufferCell>(term.buffer.active, current)
    if (candidates.length === 0) return scan

    // The pane's own cwd first, the spawn cwd second. `candidatePaths` de-duplicates, so a
    // pane that never changed directory pays for one base and not two.
    const proc = await sessionCwd(env.project, ctx.session())
    const cwds = proc === null ? [env.cwd] : [proc, env.cwd]
    const bases = { cwds, roots: env.roots }

    const inside = candidates.flatMap(({ candidate }) => candidatePaths(candidate.text, bases))
    const outside = candidates.flatMap(({ candidate }) => outsidePaths(candidate.text, bases))
    await existence.lookup(env.project, inside, outside, refresh)

    const paths = resolveBufferPaths(candidates, bases,
      (path) => existence.kind(path) === 'file',
      (path) => env.reveal !== null && existence.kind(path) === 'dir')
    for (const { candidate, ranges, resolution, isDirectory } of paths) {
      for (const range of ranges) {
        if (resolution.kind === 'none') {
          scan.misses.push({ range, text: candidate.text })
          continue
        }
        const act = (): void => activate(env, candidate, resolution, isDirectory)
        scan.links.push({
          act,
          link: {
            range,
            text: candidate.text,
            // Guarded on the modifier, so a plain click — or the mouseup of a right click, which
            // `Linkifier._handleMouseUp` does not distinguish — opens nothing. This path only runs
            // for a press the capture listener below did not swallow.
            activate: (event: MouseEvent) => {
              if (event.ctrlKey || event.metaKey) {
                void scanLine(range.start.y, true).then((fresh) => actOnCell(fresh, range.start),
                  () => notify('cide could not read the line under the pointer.', { kind: 'warn' }))
              }
            },
            hover: () => {
              hovered = true
            },
            leave: () => {
              hovered = false
            },
          },
        })
      }
    }
    return scan
  }

  /**
   * The `http(s)://` links in one logical line, each opening in the user's browser.
   *
   * Same gesture as a path — ctrl+click, claimed by the gate below — and the same guarded
   * `activate` for a press that reached xterm some other way. The open goes through Rust
   * (`chrome/webLinks.ts`), never this webview. They use the same displayed-offset mapping
   * as file paths; the path matcher skips URL spans.
   */
  function webLinks(current: NonNullable<ReturnType<typeof readPathLine>>): Scan['links'] {
    const out: Scan['links'] = []
    for (const url of matchUrls(current.text)) {
      const range = pathRange(current, url.start, url.end)
      if (range === null) continue
      const act = (): void => openWebLink(url.text)
      out.push({
        act,
        link: {
          range,
          text: url.text,
          activate: (event: MouseEvent) => {
            if (event.ctrlKey || event.metaKey) act()
          },
          hover: () => {
            hovered = true
          },
          leave: () => {
            hovered = false
          },
        },
      })
    }
    return out
  }

  /**
   * Where in the buffer a press landed, in `ILink.range`'s own 1-based coordinates.
   *
   * The DOM half of `clickGate.ts`: this reads the rect and the grid, that does the arithmetic.
   * The split is the point — the arithmetic is what `check-paths.mjs` drives, and a division
   * written inline here would be back in the one place no check script can compile.
   */
  function cellUnder(ev: MouseEvent): Cell | null {
    // `undefined` before `term.open()`, which a press cannot precede — but the typing is
    // `HTMLElement | undefined` and a non-null assertion here would be the one place this file
    // guessed at a lifecycle it does not own.
    const el = term.element
    if (el === undefined) return null
    // The *screen* element, not the terminal element: `term.element` includes the viewport's
    // scrollbar and any padding, and dividing that by `cols` puts every column half a cell out
    // by the right-hand edge. `TerminalHandle.cellSize` looks in the same place for the same
    // reason.
    const screen = el.querySelector('.xterm-screen')
    if (!(screen instanceof HTMLElement)) return null
    return cellFromPoint(ev, screen.getBoundingClientRect(), {
      cols: term.cols,
      rows: term.rows,
      viewportY: term.buffer.active.viewportY,
    })
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
    if (pressVerdict(ev) === 'ignore') return

    /*
     * Claimed before anything is known about what is under the pointer, and that ordering is the
     * whole fix. Everything below this line is asynchronous — the cwd probe and the existence
     * probe are IPC round trips — so a design that decided first and swallowed afterwards would
     * have handed the press to xterm, and through xterm to the child, in the window between.
     */
    ev.preventDefault()
    ev.stopPropagation()
    swallowed = true

    const cell = cellUnder(ev)
    if (cell === null) {
      // The press was inside the pane but not over the grid — the scrollbar, or the padding
      // below the last row. Nothing to resolve and nothing worth a sentence: the user pointed at
      // no text.
      return
    }
    // An OSC 8 hyperlink outranks everything the scan could find, as it does in xterm's own
    // provider order — and it is read now, synchronously, from the cell the press landed on,
    // because the gate has just hidden this press from `OscLinkProvider` for good.
    const osc = oscLinkAt(term, cell)
    if (osc !== null && isWebUrl(osc)) {
      openWebLink(osc)
      return
    }
    void scanLine(cell.y, true).then(
      (scan) => actOnCell(scan, cell),
      () => {
        // The scan itself failed — a disposed terminal, or a probe that threw. The press is
        // already swallowed, so silence here would be the dead gesture this file exists to
        // avoid.
        notify('cide could not read the line under the pointer.', { kind: 'warn' })
      },
    )
  }

  /**
   * Do whatever the press pointed at, or say why there is nothing to do.
   *
   * **Every branch ends in an action or a sentence.** That is not politeness: the press has
   * already been taken away from the child, so a branch that returns quietly is a gesture the
   * user made, cide consumed, and nobody answered — which is the shape of the defect this
   * project has now found sixteen times.
   */
  function actOnCell(scan: Scan, cell: Cell): void {
    const hit = scan.links.find((entry) => linkAtCell(entry.link.range, cell))
    if (hit !== undefined) {
      hit.act()
      return
    }
    const miss = scan.misses.find((candidate) => linkAtCell(candidate.range, cell))
    if (miss !== undefined) {
      /*
       * A task code is a path candidate, because `pathMatch.ts`'s `BODY` includes `-`. (M60)
       *
       * It resolves to nothing, so it arrives here as a miss — and "does not name a file or
       * folder" is true of `t-503` and useless next to an underline that a *plain* click would
       * have opened. The contradiction is visible to the user, so the sentence names the other
       * gesture instead of the failed one.
       */
      if (matchTaskCodes(miss.text).length === 1) {
        notify(`${miss.text} is a task code, not a path.`, {
          kind: 'warn',
          hint: 'Click a task code without Ctrl to open the task.',
        })
        return
      }
      notify(`${miss.text} does not name a file or folder cide can reach from this pane.`, {
        kind: 'warn',
        hint: 'Paths resolve against the pane’s working directory and the project’s roots.',
      })
      return
    }
    notify('There is no file path or web link under the pointer.', {
      kind: 'warn',
      hint:
        'Ctrl+click a path in terminal output to open it, a folder to show it in the tree, or ' +
        'an http(s) link to open it in your browser.',
    })
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
    if (!hovered) return
    // A drag that selected the path is not a click that missed; staying silent there is the
    // whole reason this is on `click` and reads the selection.
    if (term.getSelection() !== '') return
    notify('Ctrl+click a path or link in a terminal to open it.', { kind: 'warn' })
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
    hovered = false
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
function activate(
  env: PathLinkEnv,
  candidate: Candidate,
  resolution: Resolution,
  isDirectory: boolean,
): void {
  if (resolution.kind === 'many') {
    const what = isDirectory ? 'folders' : 'files'
    notify(
      `${candidate.text} names ${resolution.paths.length} ${what} in this project, so cide will ` +
        'not guess which one you meant.',
      {
        kind: 'warn',
        detail: resolution.paths.join('\n'),
        hint: 'Open the one you want from the file tree, or with Ctrl+P.',
      },
    )
    return
  }
  if (resolution.kind !== 'one') return
  if (isDirectory) {
    /*
     * A directory is shown in the tree, not opened in an editor. `reveal` is
     * `runCommand('file.reveal', …)` — the same command Ctrl+Shift+E and the Explorer's ⌖ button
     * run — so the sidebar is brought to Files first and a path with no row reports itself,
     * both of which are that arm's rules and neither of which is restated here.
     *
     * `env.reveal` cannot be null on this path: `scanLine` only resolves a candidate as a
     * directory when it is non-null. The `?.` is the type system's, not a second opinion.
     */
    env.reveal?.(resolution.path)
    return
  }
  env.open?.(
    resolution.path,
    candidate.line === null ? null : { line: candidate.line, column: candidate.column ?? 1 },
  )
}

/** The one method of xterm's internal `IOscLinkService` that [`oscLinkAt`] calls. */
interface OscLinkService {
  getLinkData?: (id: number) => { uri?: unknown } | undefined
}

/**
 * The URI of the OSC 8 hyperlink covering `cell`, or `null`.
 *
 * **Reaches into xterm's internals, on purpose and in exactly one place.** OSC 8 links are
 * web links a program marked up itself (Claude Code's markdown links among them), and their
 * visible text need not be the URL — `[docs](https://…)` shows `docs`. xterm's
 * `OscLinkProvider` opens them on whatever click reaches it, which with this gate in place is
 * only ever a *plain* click; a web link that opens on the click a user makes to focus a pane
 * is a browser window nobody asked for. So xterm's handler refuses web links (`xterm.ts`) and
 * the ctrl+press this gate claims opens them — which needs the link under the press, and the
 * public API has no accessor for it.
 *
 * The two internals, both from `@xterm/xterm` 6.0.0 (pinned exactly, and `check:paths` pins
 * `XTERM_VERSION` to it): a buffer cell loaded through the public `getCell` *is* the internal
 * `CellData`, whose `extended.urlId` names the link, and `Terminal._core._oscLinkService` maps
 * that id to its URI — the same two reads `OscLinkProvider.provideLinks` makes. Both names
 * survive xterm's minified build. Every step is shape-checked, so an upgrade that renames
 * either degrades to "no OSC link here" and the text-URL path still works; `check:paths`
 * greps the built bundle for both names so that degradation is a red check, not a quiet one.
 */
function oscLinkAt(term: Terminal, cell: { x: number; y: number }): string | null {
  try {
    const buffer = term.buffer.active
    const scratch = buffer.getNullCell() as unknown as { extended?: { urlId?: unknown } }
    const line = buffer.getLine(cell.y - 1)
    if (line === undefined) return null
    line.getCell(cell.x - 1, scratch as unknown as Parameters<IBufferLine['getCell']>[1])
    const id = scratch.extended?.urlId
    if (typeof id !== 'number' || id === 0) return null
    const service = (term as unknown as { _core?: { _oscLinkService?: OscLinkService } })._core
      ?._oscLinkService
    const uri = service?.getLinkData?.(id)?.uri
    return typeof uri === 'string' && uri !== '' ? uri : null
  } catch {
    return null
  }
}
