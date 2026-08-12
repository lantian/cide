/**
 * What Copy, Cut and Paste mean in the file tree — every decision, none of the plumbing.
 *
 * Pure, and importing nothing but `newEntry.ts` beside it, so `check-fs-clipboard.mjs` can
 * hold each rule under node. The rules are worth holding: every one of them is invisible in a
 * screenshot, and each has a wrong version that type-checks and quietly loses a file.
 *
 * `cide_fs::copy` is the other half. It owns what happens on disk — the collision rename, the
 * recursion, the refusals — and this owns what the user is told *before* they commit to it.
 * The two overlap on purpose in exactly two places, both pinned by tests on both sides:
 * `candidateName` (so the menu can say what the copy will be called) and `isInside` (so the
 * "into itself" refusal is a greyed menu item rather than a rejected command).
 *
 * # What is on which clipboard
 *
 * There are two, and conflating them is the bug this file exists to avoid.
 *
 * * The **file clipboard** is in-app state: a mode, a project and some paths (see [`FileClip`]).
 *   It is what Paste reads. Nothing is copied on disk when it is written — a Cut that is never
 *   pasted must cost nothing, which is only true if Ctrl+X touches no files at all.
 * * The **system clipboard** gets the paths as plain text, one per line ([`clipboardText`]), so
 *   that a Copy in the tree is also useful in a terminal pane, an editor, or a browser.
 *
 * What this does **not** support, stated because a file manager user will expect it: pasting
 * *into* cide from Dolphin or Nautilus, and pasting cide's Copy into them as files rather than
 * as text. Those need the desktop's own clipboard flavours — `text/uri-list` plus
 * `x-special/gnome-copied-files` for GNOME, `application/x-kde-cutselection` for KDE — which a
 * webview cannot put on the clipboard: `navigator.clipboard.write` refuses custom MIME types,
 * and serving them properly means owning the X11/Wayland selection from the native side for as
 * long as the clipboard lives. That is a real feature, and it is not this one. Copy here is
 * cide-to-cide, plus text for everyone else.
 */
import { basenameOf, targetFor, type AnchorRow, type NewEntryTarget } from './newEntry'

/** Copy leaves the source alone; Cut removes it **when the paste happens**, never before. */
export type ClipMode = 'copy' | 'cut'

/** What the tree is holding, ready to be pasted. */
export interface FileClip {
  readonly mode: ClipMode
  /**
   * The project the paths came from.
   *
   * Carried because a paste into a *different* project would send Rust paths that are outside
   * that project's roots, and `ops::check_within` refuses those — correctly, since containment
   * is the only thing standing between a webview-supplied path and `/etc`. So the refusal is
   * made here, where it can be a sentence in a greyed menu item instead of a rejected command
   * the user has to interpret.
   */
  readonly project: string
  /**
   * Absolute paths, in the order they were picked.
   *
   * An array although the tree's selection is a single row today: `fs_paste` and `fs_delete`
   * both take a list, so multi-select arrives as a frontend change with no wire change and no
   * second Rust path to keep in step.
   */
  readonly paths: readonly string[]
}

/** The parts of Rust's `PastedEntry` this module reads. Structural, so the model imports
 *  nothing from the generated types — see the header. `fileClipboard.ts` passes the real
 *  `PastedEntry` in, which is where a renamed field would fail to compile. */
export interface PasteOutcome {
  readonly source: string
  readonly dest: string
  readonly renamed: boolean
  readonly skipped: number
}

/**
 * Which folder a paste lands in — **deliberately the same rule as *New File…***.
 *
 * A directory row means that directory; a **file** row means its *parent*; no row means the
 * project's first root. Sharing `targetFor` rather than restating it is the point: two rules
 * that agree today and are written twice are two rules that disagree after the next change,
 * and "paste went somewhere else than New File would have" is a difference nobody would think
 * to test for.
 */
export function pasteTargetFor(
  row: AnchorRow | null,
  roots: readonly string[],
): NewEntryTarget | null {
  return targetFor(row, roots)
}

/**
 * Is `path` `ancestor`, or inside it?
 *
 * Component-wise, like Rust's `Path::starts_with`, which is why the separator is appended
 * before the prefix test: the string version answers `true` for `/a/src` inside `/a/s`, and
 * would refuse a perfectly good paste into a sibling whose name shares a prefix.
 */
export function isInside(path: string, ancestor: string): boolean {
  if (path === ancestor) return true
  const base = ancestor.endsWith('/') ? ancestor : `${ancestor}/`
  return path.startsWith(base)
}

/**
 * Why this paste cannot happen, or `null` when it can.
 *
 * A sentence rather than a boolean, because the menu item is drawn *disabled with the reason
 * on it* — the house treatment for "why not", and the difference between a control that looks
 * broken and one that explains itself. Every refusal here is also enforced in Rust; this copy
 * exists to say so before the gesture rather than after it.
 */
export function pasteRefusal(
  clip: FileClip | null,
  project: string | null,
  target: NewEntryTarget | null,
): string | null {
  if (project === null) return 'No project is open.'
  if (clip === null || clip.paths.length === 0) {
    return 'Nothing has been copied yet — use Copy or Cut first.'
  }
  if (clip.project !== project) {
    return 'Those paths were copied in another project; paste them where they came from.'
  }
  if (target === null) return 'This project has no folder to paste into.'
  const inside = clip.paths.find((path) => isInside(target.parent, path))
  if (inside !== undefined) {
    return `“${basenameOf(inside)}” cannot be pasted into itself.`
  }
  return null
}

/** The *Paste* item's label, naming the destination whenever it is not what was clicked. */
export function pasteLabel(clip: FileClip | null, target: NewEntryTarget | null): string {
  if (clip === null || clip.paths.length === 0 || target === null) return 'Paste'
  const what =
    clip.paths.length === 1
      ? `“${basenameOf(clip.paths[0] ?? '')}”`
      : `${clip.paths.length} items`
  return `Paste ${what} into ${target.label}`
}

/** The *Copy* / *Cut* item's label. Plural only when it would otherwise lie. */
export function copyLabel(mode: ClipMode, count: number): string {
  const verb = mode === 'cut' ? 'Cut' : 'Copy'
  return count > 1 ? `${verb} ${count} Items` : verb
}

/**
 * The strip under the tree while something is on the clipboard, or `null`.
 *
 * Only for a **cut**, and that asymmetry is the whole reason this function exists rather than
 * a flag. A copy changes nothing until it is pasted and needs no announcement; a cut is a
 * *pending removal*, and a user who cuts a folder and then forgets has a row that looks
 * ordinary and is about to move. The cut rows are dimmed as well; the strip is what says why,
 * and says how to call it off.
 */
export function pendingNote(clip: FileClip | null): string | null {
  if (clip === null || clip.mode !== 'cut' || clip.paths.length === 0) return null
  const what =
    clip.paths.length === 1 ? `“${basenameOf(clip.paths[0] ?? '')}”` : `${clip.paths.length} items`
  return `${what} will move when you paste. Esc cancels.`
}

/** Whether a row should be drawn as pending-cut. */
export function isCutPending(clip: FileClip | null, path: string): boolean {
  return clip !== null && clip.mode === 'cut' && clip.paths.includes(path)
}

/** What Copy puts on the *system* clipboard: one absolute path per line. */
export function clipboardText(paths: readonly string[]): string {
  return paths.join('\n')
}

/**
 * What to say after a paste, or `null` to say nothing.
 *
 * **`null` is the common answer, and that is the design.** The tree selects and scrolls to
 * what it just made, so "Pasted main.rs" is a sentence describing something already on screen.
 * A message is only worth the user's attention when the result differs from what they asked
 * for, and there are exactly three ways it can:
 *
 * * a **rename**, because the name was taken. This is the one that must be said: the user
 *   asked for `main.rs` in a folder that has one, and the row they were looking for still
 *   holds the older file — so a silent success reads as a paste that did nothing;
 * * **skipped** entries — sockets, fifos, devices — which are absent from the copy;
 * * a **cut pasted where it already was**, which is a legitimate no-op and looks exactly like
 *   a broken Ctrl+V unless it says so.
 */
export function pastedSummary(entries: readonly PasteOutcome[], mode: ClipMode): string | null {
  if (entries.length === 0) return null
  const parts: string[] = []

  const noop = entries.filter((entry) => entry.source === entry.dest)
  if (noop.length > 0) {
    parts.push(
      noop.length === 1
        ? `“${basenameOf(noop[0]?.source ?? '')}” is already in this folder.`
        : `${noop.length} items are already in this folder.`,
    )
  }

  const renamed = entries.filter((entry) => entry.renamed)
  if (renamed.length === 1) {
    const only = renamed[0]
    if (only !== undefined) {
      const verb = mode === 'cut' ? 'moved file' : 'copy'
      parts.push(
        `“${basenameOf(only.source)}” was already here, so the ${verb} is called “${basenameOf(only.dest)}”.`,
      )
    }
  } else if (renamed.length > 1) {
    parts.push(`${renamed.length} names were already taken, so those were renamed.`)
  }

  const skipped = entries.reduce((total, entry) => total + entry.skipped, 0)
  if (skipped > 0) {
    parts.push(
      skipped === 1
        ? '1 socket or pipe was left behind — those cannot be copied.'
        : `${skipped} sockets or pipes were left behind — those cannot be copied.`,
    )
  }

  return parts.length === 0 ? null : parts.join(' ')
}

/**
 * The name a copy will take when `name` is already used — the frontend's copy of
 * `cide_fs::copy::candidate_name`.
 *
 * Two copies of one rule, for the reason `checkName` is two copies of one rule: this one tells
 * the user what is about to happen while they can still change their mind, and that one is
 * what actually claims the name — atomically, in a loop, against a directory that can gain the
 * name between the two. They are pinned to the same table on both sides.
 */
export function candidateName(name: string, attempt: number, directory: boolean): string {
  if (attempt === 0) return name
  const suffix = attempt === 1 ? ' copy' : ` copy ${attempt}`
  const dot = directory ? -1 : name.lastIndexOf('.')
  // `> 0`, not `>= 0`: a leading dot is the whole name of a dot-file, not an extension, so
  // `.gitignore` copies to `.gitignore copy` rather than to ` copy.gitignore`.
  if (dot > 0) return `${name.slice(0, dot)}${suffix}${name.slice(dot)}`
  return `${name}${suffix}`
}
