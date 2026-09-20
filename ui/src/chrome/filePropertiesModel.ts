/**
 * What the properties card says, and how it says it. (M70)
 *
 * > *"Need a file properties modal window that will show OS stats and git history if applicable
 * > and other useful stuff"*
 *
 * `FileProperties.tsx` is the markup; every decision and every sentence is here, and that split
 * is the same one `closeConfirmModel.ts` and `pasteConfirmModel.ts` make for the same reason:
 * the mistakes are all in this half, and in this half they can be tested.
 * `ui/scripts/check-properties.mjs` compiles this file on its own, so it is DOM-free and its
 * only import is `../panes/imageKinds`, which is itself import-free — `tsc` follows the relative
 * path and compiles both.
 *
 * The `…Model` suffix is not decoration: `fileProperties.ts` beside `FileProperties.tsx` is one
 * path on macOS, which is the build break `ui/scripts/check-casing.mjs` exists to prevent.
 *
 * # The rules a properties dialog usually gets wrong
 *
 * 1. **An absent fact gets a sentence, never an empty row.** Every `Option` Rust sends can be
 *    `None` for a reason the reader can act on — a filesystem with no birth time, a uid with no
 *    passwd entry, a file too large to count lines in. A blank value next to a label reads as a
 *    bug in the card; [`NOT_RECORDED`] and the `text_skipped` sentence read as facts.
 * 2. **A bounded count never looks exact.** [`formatCount`] prefixes *at least* whenever the
 *    walk was truncated. An exact-looking lower bound is worse than no number at all in a panel
 *    whose only job is being believed — `PasteCollision::truncated`'s rule, and `DirSummary`'s.
 * 3. **The card reads the disk, so it says so when the buffer differs.** [`sizeNote`] returns
 *    *on disk* for a path with unsaved edits open. Without it the card silently contradicts the
 *    status bar six inches away on the same screen, and neither one says which is stale.
 * 4. **An unknown git status is not a clean one.** See [`statusLabel`]: `TreeStatusMap` is
 *    capped at 20,000 entries and absence from it normally means clean — but absence from a
 *    *truncated* map means nothing at all, and a green *Clean* on a modified file is the one
 *    wrong answer here that costs the user something.
 */
import { formatBytes } from '../panes/imageKinds'

/** What a row says when the filesystem simply does not carry the fact. */
export const NOT_RECORDED = 'Not recorded'

/** What the git status row says when the status map cannot answer. See [`statusLabel`]. */
export const STATUS_UNKNOWN = 'Not reported'

/**
 * The subset of the generated `FileProperties` this module reads.
 *
 * Declared structurally rather than imported, for `pasteConfirmModel.ts`'s reason: the check
 * script compiles this file standalone and cannot reach `ipc/generated.ts`. The real type is
 * checked against this one at the single call site in `FileProperties.tsx`, so a
 * `cargo xtask codegen` rename stops the build rather than silently producing a card of empty
 * rows.
 */
export interface PropertiesLike {
  path: string
  name: string
  kind: 'file' | 'dir' | 'symlink' | 'other'
  len: number
  modifiedUnixMs?: number | undefined
  changedUnixMs?: number | undefined
  createdUnixMs?: number | undefined
  readonly: boolean
  mode?: number | undefined
  modeString?: string | undefined
  owner?: OwnerLike | undefined
  symlinkTarget?: string | undefined
  symlinkBroken?: boolean | undefined
  text?: TextFactsLike | undefined
  textSkipped?: string | undefined
}

export interface OwnerLike {
  uid: number
  gid: number
  user?: string | undefined
  group?: string | undefined
}

export interface TextFactsLike {
  lines: number
  ending: 'lf' | 'crlf' | 'cr' | 'mixed' | 'none'
  utf8: boolean
}

/** The subset of the generated `DirSummary` this module reads. */
export interface DirSummaryLike {
  files: number
  dirs: number
  bytes: number
  truncated: boolean
}

/** One commit, as much of `CommitRow` as the card draws. */
export interface CommitLike {
  oid: string
  shortOid: string
  summary: string
  author: string
  /**
   * Unix seconds, and `bigint` is not a mistake.
   *
   * ts-rs renders `CommitRow::authored`'s `i64` as a `bigint` — a claim about the wire that is
   * not true, since Tauri's transport is JSON and `JSON.parse` produces a `number`.
   * `gitlog/logModel.ts` narrows with `Number()` at one point and explains why that is lossless
   * for every second this side of the year 285616. This accepts both and narrows the same way,
   * rather than forcing every caller through a cast — which is how one of them forgets and gets
   * `NaN` in a date field with nothing logged anywhere.
   */
  authored: bigint | number
}

/** The subset of the generated `FilePropertiesGit` this module reads. */
export interface GitLike {
  /** Which repository the path resolved to — `cide_git::repo::locate`'s answer, not a guess. */
  repo: string
  relPath: string
  firstCommit?: CommitLike | undefined
  firstTruncated: boolean
  lastCommit?: CommitLike | undefined
  recent: CommitLike[]
  more: boolean
}

/**
 * The heading's second line: what kind of thing this is, in words.
 *
 * A symlink says so **before** anything else, because every other row on the card is about the
 * link rather than its target and a reader who has not noticed will misread all of them.
 */
export function kindLabel(props: PropertiesLike): string {
  switch (props.kind) {
    case 'dir':
      return 'Folder'
    case 'symlink':
      return props.symlinkBroken === true ? 'Broken symlink' : 'Symlink'
    case 'other':
      // Named rather than called a file: a fifo whose "size" is 0 and whose "lines" are absent
      // makes sense once you know what it is, and looks like a broken card until then.
      return 'Special file'
    case 'file':
      return 'File'
  }
}

/**
 * `7,412 B (7 KiB)` — the exact count first, the human one in brackets.
 *
 * Both, because a properties dialog is where the exact number is looked up. The status bar shows
 * `7 KiB` alone and is right to: it has one line and the reader is not auditing anything. Here
 * the reader often is, and `7 KiB` cannot be compared against `ls -l`.
 *
 * The bracket is dropped under a kibibyte, where it would repeat the number it is explaining.
 */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return NOT_RECORDED
  const exact = `${Math.round(bytes).toLocaleString('en-US')} B`
  if (bytes < 1024) return exact
  return `${exact} (${formatBytes(bytes)})`
}

/**
 * `20 Sep 2026, 14:02:11`, or [`NOT_RECORDED`].
 *
 * Absolute and to the second, which is the opposite of the log's relative `3 days ago` and is
 * deliberate: the log is a narrative and this is a record. A card that said *3 days ago* for an
 * mtime would be unusable for the thing mtimes are used for, which is comparing two of them.
 *
 * The milliseconds arrive as a plain number and can be arithmetic'd — see
 * `cide_ipc::properties`, which explains at length why they are **not** a `FileStamp`, whose
 * nanoseconds do not survive a JavaScript number and are carried as a string for that reason.
 */
export function formatTime(ms: number | undefined, locale = 'en-GB'): string {
  if (ms === undefined || !Number.isFinite(ms)) return NOT_RECORDED
  const at = new Date(ms)
  if (Number.isNaN(at.getTime())) return NOT_RECORDED
  const date = at.toLocaleDateString(locale, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
  })
  const time = at.toLocaleTimeString(locale, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  })
  return `${date}, ${time}`
}

/**
 * `ivan (1000) · users (100)`, or the numbers alone.
 *
 * A missing name is the ordinary state inside a container and on an NFS mount whose directory
 * service is unreachable, so it costs a word and never the fact. The numbers are always shown
 * even when the names resolve, because a uid is what the reader compares against `id -u`.
 */
export function formatOwner(owner: OwnerLike | undefined): string | null {
  if (owner === undefined) return null
  const user = owner.user === undefined ? `${owner.uid}` : `${owner.user} (${owner.uid})`
  const group = owner.group === undefined ? `${owner.gid}` : `${owner.group} (${owner.gid})`
  return `${user} · ${group}`
}

/** `LF`, `CRLF`, `Mixed`, `None`. The status bar's vocabulary exactly. */
export function endingLabel(ending: TextFactsLike['ending']): string {
  switch (ending) {
    case 'lf':
      return 'LF'
    case 'crlf':
      return 'CRLF'
    case 'cr':
      return 'CR'
    case 'mixed':
      return 'Mixed'
    case 'none':
      // Not `LF`: a file with no break has no ending, and claiming one is a guess on a row
      // people read to decide whether a tool mangled a checkout.
      return 'No line breaks'
  }
}

/**
 * `214 lines · LF · UTF-8`, or the sentence saying why there is no such row.
 *
 * Exactly one of the two is always available — `cide_core::properties` guarantees it and its
 * test `exactly_one_of_text_and_its_excuse_is_present` pins it — so this never returns `null`
 * for a file. It returns `null` only for a directory, which has no lines and needs no apology
 * for not having them.
 */
export function textLine(props: PropertiesLike): string | null {
  if (props.text !== undefined) {
    const { lines, ending, utf8 } = props.text
    const parts = [count(lines, 'line'), endingLabel(ending)]
    // `UTF-8` is stated only when true. `document::read` refuses a non-UTF-8 file outright, so
    // this is the one place in cide that can explain why a file will not open, and the words
    // have to be the explanation rather than a label.
    parts.push(utf8 ? 'UTF-8' : 'Not valid UTF-8 — this file will not open in the editor')
    return parts.join(' · ')
  }
  if (props.textSkipped !== undefined) return props.textSkipped
  return null
}

/**
 * *on disk*, when the path has unsaved edits open, and `null` otherwise.
 *
 * The card reads the file, not the buffer. That is the right choice — a properties card is about
 * what is on the filesystem, and every other row on it (size, mode, mtime) is unambiguously the
 * disk's — but the size and the line count are the two rows a reader will compare against the
 * editor in front of them, and while a buffer is dirty the two legitimately differ.
 *
 * One word fixes it. Without it the card quietly contradicts the status bar and neither says
 * which is stale.
 */
export function sizeNote(dirty: boolean): string | null {
  return dirty ? 'on disk' : null
}

/**
 * `42 files, 7 folders`, or `at least 50,000 files, 3,001 folders`.
 *
 * `null` while the walk is still running, so the caller shows a pending row rather than `0
 * files` — which is a real answer for an empty directory and must not also be the word for
 * *not yet known*.
 */
export function entriesLine(summary: DirSummaryLike | null): string | null {
  if (summary === null) return null
  const body = `${count(summary.files, 'file')}, ${count(summary.dirs, 'folder')}`
  return summary.truncated ? `at least ${body}` : body
}

/** The directory's recursive size, hedged the same way. */
export function dirSizeLine(summary: DirSummaryLike | null): string | null {
  if (summary === null) return null
  const size = formatSize(summary.bytes)
  return summary.truncated ? `at least ${size}` : size
}

/**
 * `at least 4,096` or `4,096`, for a count that may be a lower bound.
 *
 * The hedge is not decoration. An exact-looking number that is really a lower bound is worse
 * than no number at all in a panel whose only job is being believed — `pasteConfirmModel.ts`
 * says the same thing about the same hazard, and this is the second place it arises.
 */
export function formatCount(n: number, noun: string, truncated: boolean): string {
  const counted = count(n, noun)
  return truncated ? `at least ${counted}` : counted
}

/** `1 file` / `3 files`, with the separator a four-digit count needs. */
function count(n: number, noun: string): string {
  const shown = n.toLocaleString('en-US')
  return n === 1 ? `1 ${noun}` : `${shown} ${noun}s`
}

/**
 * The git status row.
 *
 * `status` is the letter `sidebar/treeStatus.ts` resolved, or `null` when the path is not in the
 * map. **`truncated` is what makes the difference between the two readings of absence**, and
 * getting it wrong is the one silent failure on this card that costs the user something.
 *
 * `TreeStatusMap` is capped at 20,000 entries. Below the cap, absence genuinely means clean —
 * that is the map's documented encoding. Above it, absence means the walk never got to this
 * path, and rendering *Clean* there is a green tick on a file with uncommitted work in it. So a
 * truncated map answers [`STATUS_UNKNOWN`], which is true and which the reader can act on by
 * looking at the Git panel.
 */
export function statusLabel(
  status: string | null,
  truncated: boolean,
): string {
  if (status === null || status === 'clean') {
    return truncated ? STATUS_UNKNOWN : 'Clean — no local changes'
  }
  switch (status) {
    case 'modified':
      return 'Modified'
    case 'added':
      return 'Added'
    case 'deleted':
      return 'Deleted'
    case 'untracked':
      return 'Untracked'
    case 'ignored':
      return 'Ignored by git'
    case 'conflicted':
      return 'Conflicted'
    default:
      // A status a newer build reports and this one does not know renders as itself rather than
      // as a blank — `cide_docker`'s "carried as a String and never an enum" rule.
      return status
  }
}

/**
 * The *tracked since* row: a date, or the reason there is not one.
 *
 * Three outcomes and they must stay three. `first_truncated` is what separates *this path has
 * never been committed* from *there is more history than we were willing to read*, and a card
 * that collapsed them would tell somebody their file is untracked because their repository is
 * large. `cide_git::properties::oldest` carries the same argument on the producing side.
 */
export function trackedSince(git: GitLike | null, locale = 'en-GB'): string | null {
  if (git === null) return null
  if (git.firstCommit !== undefined) {
    return formatTime(Number(git.firstCommit.authored) * 1000, locale).split(',')[0] ?? null
  }
  if (git.firstTruncated) return 'More history than the card reads'
  return 'Never committed'
}

/**
 * Whether the card offers *Open full history*.
 *
 * Only when there is history to open. The button routes to `git.history.file`, which opens a
 * tool-window tab — and a tab that opens onto *"Nothing in this repository's history touches
 * …"* is a worse answer than a button that was never drawn, because the card already said the
 * same thing in one line.
 */
export function canOpenHistory(git: GitLike | null): boolean {
  return git !== null && git.recent.length > 0
}

/**
 * The line under the recent list: how much was not shown.
 *
 * `null` when the list is the whole history, so a file with three commits gets no footnote
 * claiming there might be more. Read from `git.more`, which Rust derives from one row beyond
 * the limit rather than from the list's length — the off-by-one that would otherwise draw this
 * line on a file whose entire history is already on screen.
 */
export function moreLine(git: GitLike | null, shown: number): string | null {
  if (git === null || !git.more) return null
  return `${shown} of more — open the full history for the rest`
}

/** The card's accessible name, and its heading. */
export function cardLabel(props: PropertiesLike): string {
  return `Properties of ${props.name}`
}

/**
 * The path row's value: the project-relative path when there is one, else the absolute path.
 *
 * Relative is what the reader recognises — it is what the tree, the tabs and every commit
 * message call the file — and the absolute one is a click away on the copy button. A path
 * outside every root has no relative form and shows absolute, which is itself the information
 * that it is outside the project.
 */
export function displayPath(absolute: string, roots: readonly string[]): string {
  let best: string | null = null
  for (const root of roots) {
    const prefix = root.endsWith('/') ? root : `${root}/`
    if (absolute.startsWith(prefix) && (best === null || prefix.length > best.length)) {
      best = prefix
    }
  }
  return best === null ? absolute : absolute.slice(best.length)
}
