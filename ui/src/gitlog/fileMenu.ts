/**
 * The right-click menu on a changed file in the details pane, and what Ctrl+D does.
 *
 * Import-free and compiled by `ui/scripts/check-log.mjs`, for the reason `logMenu.ts` gives about
 * the commit menu: enablement is where a menu goes wrong, and a disabled row whose reason is a
 * guess is the `repoOpen` mistake in miniature.
 *
 * # The four actions, and why they are four
 *
 * They are two questions crossed with two answers. The question is *which pair of revisions*, and
 * the answers are "what this commit did" versus "what is on disk now":
 *
 * | item | older side | newer side |
 * | --- | --- | --- |
 * | Show diff | the commit's first parent | the commit |
 * | Compare with local | the commit | the working tree |
 * | Compare before with local | the commit's first parent | the working tree |
 *
 * *Show diff* is the commit read on its own — what the double-click already does. The other two
 * both end at the working tree and differ only in where they start, which is exactly the
 * distinction a reader needs when asking "has my change survived": *Compare with local* answers
 * "what happened to this file **after** this commit", and *Compare before with local* answers
 * "what has happened **since just before** it", which includes the commit's own change.
 *
 * `RevSide::WorkingTree` is legal only as the *newer* side and `cide_git::revision` refuses the
 * other arrangement by name, so both working-tree items put it there. There is deliberately no
 * "compare local with X": it would be the same diff drawn backwards and a typed error from Rust.
 *
 * *Open file* is the odd one out and is not a diff at all — it opens the path in an editor on the
 * working tree, which is what a reader wants after deciding the diff is the thing they were
 * looking for.
 */

/** Why an item cannot be clicked, or `null`. */
export type FileMenuRefusal = string | null

/** A file the menu was opened on. Structural — see the header. */
export interface FileMenuTarget {
  readonly path: string
  /** The pre-rename path, or `null`. */
  readonly oldPath: string | null
}

export type FileMenuId = 'showDiff' | 'openFile' | 'compareLocal' | 'compareBeforeLocal'

export interface FileMenuItem {
  readonly id: FileMenuId
  readonly label: string
  /** Present exactly when the item is drawn greyed. */
  readonly disabledReason?: string | undefined
}

/**
 * A file that a commit **deleted** has nothing to open and nothing on disk to compare against.
 *
 * Stated as one reason reused by three items rather than three wordings, because they are one
 * fact. Note the asymmetry that makes *Show diff* still work: the deletion itself is a perfectly
 * readable diff — content on the old side, nothing on the new — and it is often the only thing
 * the reader wants to see.
 */
export const DELETED_NO_LOCAL = 'This commit deleted the file, so there is nothing on disk to read'

/**
 * A comparison against the working tree needs a *file*, and the pane can be showing a range whose
 * newer side already is the working tree.
 *
 * Comparing the working tree with itself is not an error Rust would refuse — it is a diff with no
 * hunks, which is worse: an empty pane that looks like a failed request.
 */
export const ALREADY_LOCAL = 'The newer side of this comparison is already the working tree'

/**
 * The menu, in order.
 *
 * `deleted` is the caller's answer, not something derivable from a path: `CommitFile` carries the
 * change kind and this module is given the conclusion, the same division `commitMenu` uses for
 * `sameRepo`. Deriving it from "the file is missing on disk" would be wrong for a file deleted in
 * this commit and re-created by a later one.
 */
export function fileMenu(input: {
  readonly target: FileMenuTarget
  /** The commit deleted this file. */
  readonly deleted: boolean
  /** The pane is showing a range whose newer side is the working tree. */
  readonly newerIsWorkingTree: boolean
}): FileMenuItem[] {
  const local: FileMenuRefusal = input.deleted
    ? DELETED_NO_LOCAL
    : input.newerIsWorkingTree
      ? ALREADY_LOCAL
      : null

  return [
    { id: 'showDiff', label: 'Show diff' },
    {
      id: 'openFile',
      label: 'Open file',
      // Only the deletion stops this one. A range already ending at the working tree has no
      // bearing on whether the path can be opened — that refusal is about a *comparison*.
      ...(input.deleted ? { disabledReason: DELETED_NO_LOCAL } : {}),
    },
    {
      id: 'compareLocal',
      label: 'Compare with local',
      ...(local === null ? {} : { disabledReason: local }),
    },
    {
      id: 'compareBeforeLocal',
      label: 'Compare before with local',
      ...(local === null ? {} : { disabledReason: local }),
    },
  ]
}

/**
 * What Ctrl+D does: the same thing as a double-click, which is *Show diff*.
 *
 * A constant rather than a literal at the two call sites, because "the keyboard does what the
 * mouse does" is a claim that stops being true the moment one of them is edited alone — and the
 * gesture pair is the whole reason the shortcut was asked for.
 */
export const PRIMARY_FILE_ACTION: FileMenuId = 'showDiff'

/**
 * Whether a keydown is the diff shortcut.
 *
 * `ctrlKey` and not `metaKey` as well: this is a panel binding rather than a registered command,
 * so it must not claim ⌘D on a Mac, where that is *duplicate* in most editors. When the tool
 * window's bindings become real commands in `cide-core::commands` this moves there and the
 * platform layer answers the question properly.
 *
 * `altKey` is refused so that Ctrl+Alt+D — unbound here, bound elsewhere by the desktop — is not
 * swallowed. `shiftKey` likewise: Ctrl+Shift+D is a different chord and claiming it here would
 * take it from every other surface in the window.
 */
export function isDiffShortcut(e: {
  readonly key: string
  readonly ctrlKey: boolean
  readonly metaKey: boolean
  readonly altKey: boolean
  readonly shiftKey: boolean
}): boolean {
  return e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && (e.key === 'd' || e.key === 'D')
}
