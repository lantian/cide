/**
 * `crates › cide-core › src › lib.rs · Rust · UTF-8 · LF · Ln 7, Col 48` — everything the
 * status bar says about the open file, and the wire it travels down.
 *
 * Both halves used to be a 28px row of their own above every buffer: the trail on the left
 * of it, the readout on the right. That row is gone. It repeated what the tab strip already
 * says (`lib.rs`, with a language badge and the full path on hover), it repeated `UTF-8`
 * once per open pane, and it cost 28px of every editor in a split to do it. The status bar
 * is where these facts belong — a row that exists whether or not anything is written in it.
 *
 * The bar lives in `chrome/`, the facts live in a CodeMirror state in `editor/`, and nothing
 * between the two can reach both — `App.tsx` renders the bar and does not know a caret from
 * a scrollbar. So this module is that reach, the same shape as `openBuffers.ts` and
 * `revealRequest.ts`, and for the same reason: a live editor outlives its position in the
 * React tree, so the thing that holds it is a module and not a component.
 *
 * # Why the line comes in two pieces
 *
 * They change at completely different rates, and the bar treats them accordingly. The trail
 * changes when the user switches file — rare, and it renders as React elements because the
 * `›` separators and the bidi handling below want real markup. `detail` changes on every
 * caret move, which under a held arrow key is 30 times a second, and is written straight
 * into a DOM node the bar owns: through React it would re-render the bar, and there is no
 * version of that which is worth 30 renders a second.
 *
 * # Why claims are a stack
 *
 * A split shows two editors at once and the bar has one slot, so the slot belongs to the
 * last editor the user was in. Mounting claims it (a freshly opened file is the one being
 * looked at), and focus takes it back — which is what stops a second pane mounting beside a
 * buffer somebody is typing in from stealing the readout out from under them. Releasing
 * hands the slot to the editor under it rather than blanking the bar, so closing one half of
 * a split leaves the other half's file on screen instead of nothing.
 */

/** The four facts, each owned by a different part of the editor. */
export interface Readout {
  /** What `languages.ts` calls this file: `Markdown`, `TSX`, `Plain Text`. */
  readonly language: string
  /** `lineEndings.ts`'s classification: `LF`, `CRLF`, `CR` or `Mixed`. */
  readonly ending: string
  /** `Ln 7, Col 48`, already formatted — the caret is the editor's to describe. */
  readonly cursor: string
}

/**
 * The half of the line that moves: `Rust · UTF-8 · LF · Ln 7, Col 48`.
 *
 * `UTF-8` is a literal because it is a fact and not a guess: `cide-fs` refuses a file it
 * cannot decode as UTF-8 (`FsError::NotUtf8`), so a buffer on screen is a buffer that was
 * UTF-8. If that ever stops being true the encoding becomes a fourth field here, and this is
 * the one place that would have to change.
 */
export function formatReadout({ language, ending, cursor }: Readout): string {
  return `${language} · UTF-8 · ${ending} · ${cursor}`
}

/**
 * `crates › cide-core › src › lib.rs`, as segments the bar can put separators between.
 *
 * Relative to `root` when the file is inside it, and absolute when it is not — showing
 * `home › lantian › work › cide › crates › …` for a file in the open project is how a trail
 * becomes noise. A missing root, an empty one, or a path outside it all fall through to the
 * whole path, which is the honest answer in each case.
 *
 * Lived in `EditorSurface` as `breadcrumbSegments` while there was a breadcrumb bar to name
 * it after. It composes a status bar line now, so it is described where the rest of that line
 * is — and, being pure, it is reachable by `scripts/check-editor.mjs`, which the component is
 * not.
 */
export function pathTrail(path: string, root?: string): string[] {
  let rest = path
  if (root !== undefined && root.length > 0) {
    const base = root.endsWith('/') ? root : `${root}/`
    if (path.startsWith(base)) rest = path.slice(base.length)
  }
  return rest.split(/[/\\]/).filter((s) => s.length > 0)
}

/** What the bar shows for the file the user is in. */
export interface ReadoutLine {
  /** The trail. Empty when no editor holds the slot. */
  readonly trail: readonly string[]
  /** `Rust · UTF-8 · LF · Ln 7, Col 48`, or `''` for the same reason. */
  readonly detail: string
}

/** No editor open. Frozen and shared, so "nothing here" is one object and one comparison. */
const NOTHING: ReadoutLine = Object.freeze({ trail: Object.freeze([]), detail: '' })

/** Receives the owning editor's line, and `NOTHING` when no editor holds the slot. */
export type ReadoutListener = (line: ReadoutLine) => void

/** One mounted editor's hold on the bar's single slot. */
export interface ReadoutSlot {
  /** Replace the moving half. Reaches the bar only while this slot is on top. */
  set(detail: string): void
  /**
   * Replace the trail, for a file whose *root* moved under it.
   *
   * Not the same thing as opening another file, which takes a fresh claim. This is the boot
   * race: `PaneBody` passes `roots[0]?.path ?? PROJECT_ROOT`, so an editor restored before
   * its project record arrives computes its trail against the fallback and would otherwise
   * show an absolute path for the rest of the session.
   */
  setTrail(trail: readonly string[]): void
  /** Take the slot — the user is in this editor now. A no-op when it is already held. */
  focus(): void
  /** Give it up on unmount. The editor under this one gets it back. */
  release(): void
}

interface Claim {
  trail: readonly string[]
  detail: string
}

/** Claim order, oldest first; the last entry owns the slot. */
const claims: Claim[] = []
const listeners = new Set<ReadoutListener>()

/**
 * The line the last notification carried.
 *
 * Kept so a change that does not move the visible text — a caret walking inside a line that
 * is already `Ln 7, Col 48`, an unfocused pane repainting — costs nothing. `set` is called
 * from a CodeMirror update listener, so "cheap when nothing changed" is the common case and
 * not an optimisation for a rare one.
 */
let published: ReadoutLine = NOTHING

/** What the bar should be showing right now. */
export function statusReadout(): ReadoutLine {
  return claims.at(-1) ?? NOTHING
}

/**
 * Segment-by-segment equality.
 *
 * Exported because `StatusBar` needs the same answer this module does: the trail is the one
 * piece of the line that goes through React state, and the setter has to bail on an
 * unchanged trail or every keystroke re-renders the bar.
 */
export function sameTrail(a: readonly string[], b: readonly string[]): boolean {
  if (a === b) return true
  return a.length === b.length && a.every((seg, i) => seg === b[i])
}

function same(a: ReadoutLine, b: ReadoutLine): boolean {
  return a.detail === b.detail && sameTrail(a.trail, b.trail)
}

function publish(): void {
  const line = statusReadout()
  if (same(line, published)) return
  // A snapshot, not the claim: the claim is mutable and a listener holding it would watch
  // `detail` change under it without ever being told.
  published = { trail: line.trail, detail: line.detail }
  // Copied before the walk: a listener is free to unsubscribe itself, and mutating the set
  // under its own iterator is how the next listener silently stops being told.
  for (const listen of [...listeners]) listen(published)
}

/**
 * Take a slot for a newly mounted editor, and answer with the handle that drives it.
 *
 * A handle rather than an `(id, line)` pair keyed on the path, because the key would not be
 * unique: two panes can show one file, and the one being closed would disconnect the one
 * that stays. Same reasoning as `registerReveal`'s disposer.
 *
 * The trail is handed over once and changes only through `setTrail`. `EditorSurface` rebuilds
 * its view — and so takes a fresh claim — whenever the *path* changes, so the only thing that
 * can move it afterwards is the root it is drawn relative to.
 *
 * The handle is inert after `release`, so a listener still firing out of a torn-down
 * CodeMirror update cannot write into a bar that has moved on to another buffer.
 */
export function claimStatusReadout(trail: readonly string[], detail = ''): ReadoutSlot {
  const claim: Claim = { trail: [...trail], detail }
  claims.push(claim)
  publish()
  let live = true

  const top = (): boolean => claims.at(-1) === claim

  return {
    set(next: string): void {
      if (!live || claim.detail === next) return
      claim.detail = next
      if (top()) publish()
    },
    setTrail(next: readonly string[]): void {
      if (!live || sameTrail(claim.trail, next)) return
      claim.trail = [...next]
      if (top()) publish()
    },
    focus(): void {
      if (!live || top()) return
      const at = claims.indexOf(claim)
      if (at < 0) return
      claims.splice(at, 1)
      claims.push(claim)
      publish()
    },
    release(): void {
      if (!live) return
      live = false
      const at = claims.indexOf(claim)
      if (at >= 0) claims.splice(at, 1)
      publish()
    },
  }
}

/**
 * Watch the slot. Fires immediately with the current line, then on every change.
 *
 * The immediate call is what makes a bar that mounts after an editor — StrictMode's second
 * pass, a hot reload — show the file it missed instead of staying blank until the next
 * keystroke.
 */
export function subscribeStatusReadout(listen: ReadoutListener): () => void {
  listeners.add(listen)
  listen(statusReadout())
  return () => {
    listeners.delete(listen)
  }
}

/** Every live claim, oldest first. For tests and diagnostics. */
export function readoutClaims(): ReadoutLine[] {
  return claims.map(({ trail, detail }) => ({ trail, detail }))
}
