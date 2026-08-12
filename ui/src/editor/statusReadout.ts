/**
 * `Markdown · UTF-8 · LF · Ln 7, Col 48` — the file readout, and the wire it travels down.
 *
 * The line used to be printed at the right-hand end of the editor's own breadcrumb bar,
 * where it spent a third of a 28px row that the path also wants and repeated `UTF-8` once
 * per open pane. The status bar is where these four facts belong: it is the row a user
 * already looks along for them, it is 24px that exist whether or not anything is written in
 * them, and it has a right-hand group sitting empty next to two placeholders.
 *
 * The bar lives in `chrome/`, the facts live inside a CodeMirror state in `editor/`, and
 * nothing between the two can reach both — `App.tsx` renders the bar and does not know a
 * caret from a scrollbar. So this module is that reach, the same shape as `openBuffers.ts`
 * and `revealRequest.ts`, and for the same reason: a live editor outlives its position in
 * the React tree, so the thing that holds it is a module and not a component.
 *
 * # Why the text is pushed, and not held in state
 *
 * A caret readout in React state re-renders whatever holds it on every caret move, which on
 * a held arrow key is 30 renders a second. In `EditorSurface` that was one component; from
 * here it would be `App` and the entire pane grid under it — every terminal, every diff,
 * every tree — thirty times a second while a key is down. So the value never enters React:
 * subscribers are handed the string and write it into a DOM node they own. `StatusBar`
 * renders an empty span once and this module fills it, exactly as the breadcrumb bar did.
 *
 * # Why claims are a stack
 *
 * A split shows two editors at once and the bar has one slot, so the slot belongs to the
 * last editor the user was in. Mounting claims it (a freshly opened file is the one being
 * looked at), and focus takes it back — which is what stops a second pane mounting beside a
 * buffer somebody is typing in from stealing the readout out from under them. Releasing
 * hands the slot to the editor under it rather than blanking the bar, so closing one half of
 * a split leaves the other half's position on screen instead of nothing.
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
 * The readout as one line.
 *
 * `UTF-8` is a literal because it is a fact and not a guess: `cide-fs` refuses a file it
 * cannot decode as UTF-8 (`FsError::NotUtf8`), so a buffer on screen is a buffer that was
 * UTF-8. If that ever stops being true the encoding becomes a fourth field here, and this is
 * the one place that would have to change.
 */
export function formatReadout({ language, ending, cursor }: Readout): string {
  return `${language} · UTF-8 · ${ending} · ${cursor}`
}

/** Receives the owning editor's line, and `''` when no editor holds the slot. */
export type ReadoutListener = (text: string) => void

/** One mounted editor's hold on the bar's single slot. */
export interface ReadoutSlot {
  /** Replace this editor's line. Reaches the bar only while this slot is on top. */
  set(text: string): void
  /** Take the slot — the user is in this editor now. A no-op when it is already held. */
  focus(): void
  /** Give it up on unmount. The editor under this one gets it back. */
  release(): void
}

interface Claim {
  text: string
}

/** Claim order, oldest first; the last entry owns the slot. */
const claims: Claim[] = []
const listeners = new Set<ReadoutListener>()

/**
 * The line the last published notification carried.
 *
 * Kept so a change that does not move the visible text — a caret walking inside a line that
 * is already `Ln 7, Col 48`, an unfocused pane repainting — costs nothing. `set` is called
 * from a CodeMirror update listener, so "cheap when nothing changed" is the common case and
 * not an optimisation for a rare one.
 */
let published = ''

/** What the bar should be showing right now. */
export function statusReadout(): string {
  return claims.at(-1)?.text ?? ''
}

function publish(): void {
  const text = statusReadout()
  if (text === published) return
  published = text
  // Copied before the walk: a listener is free to unsubscribe itself, and mutating the set
  // under its own iterator is how the next listener silently stops being told.
  for (const listen of [...listeners]) listen(text)
}

/**
 * Take a slot for a newly mounted editor, and answer with the handle that drives it.
 *
 * A handle rather than an `(id, text)` pair keyed on the path, because the key would not be
 * unique: two panes can show one file, and the one being closed would disconnect the one
 * that stays. Same reasoning as `registerReveal`'s disposer.
 *
 * The handle is inert after `release`, so a listener still firing out of a torn-down
 * CodeMirror update cannot write into a bar that has moved on to another buffer.
 */
export function claimStatusReadout(text = ''): ReadoutSlot {
  const claim: Claim = { text }
  claims.push(claim)
  publish()
  let live = true

  const top = (): boolean => claims.at(-1) === claim

  return {
    set(next: string): void {
      if (!live || claim.text === next) return
      claim.text = next
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
 * The immediate call is what makes a bar that mounts after an editor — a detached window, a
 * remount — show the position it missed instead of staying blank until the next keystroke.
 */
export function subscribeStatusReadout(listen: ReadoutListener): () => void {
  listeners.add(listen)
  listen(statusReadout())
  return () => {
    listeners.delete(listen)
  }
}

/** Every live claim, oldest first. For tests and diagnostics. */
export function readoutClaims(): string[] {
  return claims.map((claim) => claim.text)
}
