/**
 * Where a new file or folder goes, and whether the name the user typed can be one.
 *
 * Pure and **import-free**, like `rowPaths.ts` and `clickSemantics.ts` beside it, so
 * `check-new-entry.mjs` can hold every edge under node. That is not tidiness: the rules below
 * are the ones a user meets *while typing*, and the alternative — letting `mkdir(2)` decide —
 * is what makes a context-menu item feel like a trapdoor. `EEXIST`, `EISDIR` and `ENOENT`
 * arrive after the gesture, phrased as syscalls, at which point the typing is already gone.
 *
 * Rust runs the same list in `cide_fs::ops::check_name`. Two copies on purpose and not by
 * accident: this one exists to *tell the user*, that one exists because a name reaching a
 * `#[tauri::command]` has been over IPC and nothing about it can be assumed. The Rust tests
 * and `check-new-entry.mjs` pin the same cases so the two cannot drift silently.
 */

/** The directory a new entry will be created in, and how to name it to the user. */
export interface NewEntryTarget {
  /** Absolute path of the directory the entry is created in. */
  readonly parent: string
  /**
   * What the menu item and the draft row call that directory.
   *
   * The basename, except at a project root in a multi-root project, where the basename is
   * the least useful thing to say — two roots called `src` are the whole reason the label
   * exists. See [`targetFor`].
   */
  readonly label: string
  /**
   * True when `parent` is a project root **and** the tree draws no row for it.
   *
   * A single-root project hides its root (`Index::show_roots`), so there is no row to anchor
   * a draft under and the draft belongs at row 0. With several roots each is a row and the
   * draft goes under it like any other folder. Nothing else in the frontend knows this rule,
   * so it is computed here where a check script can hold it.
   */
  readonly atTop: boolean
}

/** The row a *New File…* gesture started from. Only the two fields the rule reads. */
export interface AnchorRow {
  readonly path: string
  readonly isDir: boolean
}

/** The directory part of an absolute POSIX path, without its trailing slash. */
function dirnameOf(path: string): string {
  const cut = path.lastIndexOf('/')
  // `/a` → `/`, not `''`. An empty parent would be sent to Rust and refused as a relative
  // path, which is a correct refusal to a question nobody meant to ask.
  if (cut <= 0) return '/'
  return path.slice(0, cut)
}

/** The last segment of a path, or the path itself when it has no separator. */
export function basenameOf(path: string): string {
  const cut = path.lastIndexOf('/')
  return cut < 0 || cut === path.length - 1 ? path : path.slice(cut + 1)
}

/**
 * Which directory a new entry goes in, given the row the gesture started from.
 *
 * **A file row means its parent, not the file.** This is the edge that decides whether the
 * feature feels right: right-clicking `src/main.rs` and asking for a new file is asking for a
 * sibling of `main.rs`, and the alternative reading — "inside the thing I clicked" — has no
 * meaning for a file at all. Every editor resolves it this way and users rely on it without
 * ever being told.
 *
 * **A directory row means that directory**, whether it is expanded or not; the caller expands
 * it so the new row is visible.
 *
 * **No row means a project root.** A project may have several, so the answer names which:
 * `roots[0]`, the project's first root, carried out in `label` so the menu item can say
 * `New File in cide…` rather than silently picking one of two. Nothing here reaches for a
 * "current" root, because there is no such thing — the tree has no notion of a focused root
 * and inventing one would put files in a different place depending on what was scrolled.
 *
 * `null` when there is nowhere to put anything: no row and no roots, which is the panel before
 * the bootstrap lands.
 */
export function targetFor(row: AnchorRow | null, roots: readonly string[]): NewEntryTarget | null {
  if (row !== null) {
    const parent = row.isDir ? row.path : dirnameOf(row.path)
    const rootIndex = roots.indexOf(parent)
    return {
      parent,
      label: labelForDirectory(parent, roots),
      // A root row is only drawn when there are several; with one, the root is the top of the
      // list rather than a row of its own.
      atTop: rootIndex >= 0 && roots.length <= 1,
    }
  }
  const first = roots[0]
  if (first === undefined || first === '') return null
  return { parent: first, label: labelForDirectory(first, roots), atTop: roots.length <= 1 }
}

/**
 * What to call a directory in a menu item.
 *
 * A project root keeps its basename too — it is the only name the user has for it — but the
 * *caller* is the one that decides whether to say it. `targetFor` returns it for every target
 * so a menu can choose to name the destination only when it is not obvious, which is the
 * empty-space case and the multi-root case.
 */
function labelForDirectory(path: string, roots: readonly string[]): string {
  const name = basenameOf(path)
  // `/` has no basename worth showing, and a root opened at the filesystem root is a thing
  // people do to look at `/etc`.
  if (name === '' || name === '/') return path
  // Two roots can share a basename — `~/work/a/src` and `~/work/b/src` is the ordinary
  // monorepo shape — and a label that names both the same way is worse than a long one.
  const clashes = roots.filter((r) => basenameOf(r) === name).length > 1
  return clashes && roots.includes(path) ? path : name
}

/** The verdict on a typed name: a blocking `error`, a non-blocking `note`, or neither. */
export interface NameVerdict {
  /** Why this name cannot be used. `null` when it can. Blocks the commit. */
  readonly error: string | null
  /** Something true and worth saying that does not block anything. */
  readonly note: string | null
}

const OK: NameVerdict = { error: null, note: null }

/**
 * Check a name the user is typing, against the names already in the target directory.
 *
 * Returns a verdict rather than a boolean because two of the edges are genuinely not errors:
 *
 * * **A leading dot is allowed.** `.gitignore` is the single most likely thing anybody creates
 *   from this menu, and refusing it would be inventing a rule the filesystem does not have.
 *   What is true and worth saying is that a dotfile may be hidden by the project's own ignore
 *   rules and so may not get a row — the note says so, and the panel says it again afterwards
 *   if the row really did not appear.
 * * **A trailing space is trimmed, not refused.** Which is why `siblings` is compared against
 *   the trimmed name: `"main.rs "` and `"main.rs"` are the same intention.
 *
 * The blocking ones, and the answer each needs:
 *
 * * empty, or only whitespace — the same message, because to the user they are the same
 *   mistake and "whitespace" is a word about the second one only;
 * * `/` — refused with the reason, *not* silently rewritten to `_` the way the rename box
 *   does it. Rename is editing a name in place, where a stray `/` is a typo; this is choosing
 *   a location, where `src/main.rs` is plainly meant and would not be what happened;
 * * `.` and `..` — they already name this directory and its parent, which is also the shape
 *   an escape attempt takes;
 * * a name that exists — refused here so the user sees it before pressing Enter. Rust refuses
 *   it again, because the directory can gain the name between the keystroke and the command.
 */
export function checkName(
  name: string,
  siblings: readonly string[],
  directory: boolean,
): NameVerdict {
  const trimmed = name.trim()
  const what = directory ? 'folder' : 'file'

  if (trimmed === '') {
    // Not "a name cannot be empty" — the box *is* empty when it opens, and a red message on
    // an untouched field reads as a failure before anything was attempted. The caller draws
    // this as a hint; only Enter treats it as a refusal.
    return { error: `Type a ${what} name.`, note: null }
  }
  if (trimmed.includes('/')) {
    return {
      error: 'A name cannot contain “/”. Create the folder first, then the file inside it.',
      note: null,
    }
  }
  // Not reachable from a keyboard, and very reachable from a paste.
  if (trimmed.includes('\0')) {
    return { error: 'A name cannot contain a NUL character.', note: null }
  }
  if (trimmed === '.' || trimmed === '..') {
    return { error: '“.” and “..” already name this folder and the one above it.', note: null }
  }
  if (siblings.includes(trimmed)) {
    return { error: `“${trimmed}” already exists here.`, note: null }
  }

  // Case-only clashes are not an error on Linux and are a silent overwrite on macOS and
  // Windows. Said rather than guessed at, because this app cannot know which filesystem the
  // project sits on — a case-insensitive one will refuse the create and the panel will show
  // Rust's `already exists`, which is much less useful arriving with no warning.
  const lower = trimmed.toLowerCase()
  const clash = siblings.find((s) => s !== trimmed && s.toLowerCase() === lower)
  if (clash !== undefined) {
    return { error: null, note: `“${clash}” differs only in case; some filesystems treat these as one name.` }
  }

  if (trimmed.startsWith('.')) {
    return { error: null, note: 'A dot-file may be hidden by this project’s ignore rules.' }
  }
  return OK
}

/**
 * The name to send, given what is in the box. `null` when there is nothing to send.
 *
 * Trimming lives here rather than at the call site so that the name being *checked* and the
 * name being *created* are produced by the same function. When they were two expressions, a
 * name with a trailing space passed the sibling check against its trimmed form and was then
 * sent untrimmed.
 */
export function nameToSend(raw: string, siblings: readonly string[], directory: boolean): string | null {
  if (checkName(raw, siblings, directory).error !== null) return null
  return raw.trim()
}
