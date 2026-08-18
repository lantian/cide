/**
 * Which gestures a file-tree row answers, now that not every row is a file.
 *
 * Until M13 the tree had two kinds and the question "is this a directory" answered everything:
 * `kind === 'dir'` decided the twisty, the click, the icon, the context menu and what Enter did.
 * The *External Libraries* group adds two kinds that are not files at all — a header that
 * expands and does nothing else, and a sentence that does nothing whatsoever — and it adds a
 * population of ordinary `dir`/`file` rows that live **outside every project root**, where
 * Rename, Cut and Move to Trash are refused by Rust (`cide_fs::ops::check_within`).
 *
 * Six places in `FileTree.tsx` asked `kind === 'dir'` and every one of them meant something
 * different by it. This module is where those questions are answered instead, because a rule
 * that lives in a React event handler is a rule no check script can compile — which is where
 * every shipped bug in this panel has hidden. Pure and **import-free**, the same arrangement as
 * `clickSemantics.ts`, `rowPaths.ts` and `rowWindow.ts`; `ui/scripts/check-groups.mjs` compiles
 * it standalone and drives it. Do not add an import.
 *
 * # The interface a second group uses
 *
 * Nothing below names either group except the two id constants. *Scratches* landed after this
 * module was written and needed **no change to a single rule in it**: it ships rows of kind
 * `group`, `note` and `file` like the first one, and the one way it differs — its rows are
 * writable — is already expressed, because `rowVerbs` takes `inProject` as a parameter rather
 * than deriving it. The caller passes "is this path in a directory cide may write in", which is
 * the project's roots for one group and the roots plus the scratch drawer for the other, and
 * `fs_writable_roots` is the single place that answers it.
 *
 * A third group needs its own id, its own Rust-side `Groups::show`, and a `case` in
 * `dispatch.ts` if it wants a command; see `crates/cide-fs/src/groups.rs` for that half of the
 * contract.
 *
 * *Project Notes* is the third id and the one that **did** need a rule here, because it is not a
 * group at all: it is a `pin`, a single top-level row that *opens* instead of expanding. That is
 * a new `RowKind`, one arm in `rowVerbs` and one branch in `groupIcon`, and nothing else in this
 * module moved — every existing rule keyed on kind, which is why widening the kind was the
 * cheap change and why `kind: 'file'` with a sentinel path was not (it would have made *Copy
 * Path* live on a row whose path names nothing).
 */

/**
 * The five things a tree row can be. Structurally `TreeRowKind` from the generated wire types.
 *
 * Hand-maintained, because this module is import-free so that `check:groups` can compile it
 * standalone — which is exactly why `check:notes` reads both this union and `TreeRowKind` in
 * `ui/src/ipc/generated.ts` as text and asserts they name the same set. That assertion is the
 * only thing keeping the two in step.
 */
export type RowKind = 'dir' | 'file' | 'group' | 'note' | 'pin'

/**
 * The scheme every synthetic row's path uses.
 *
 * Chosen so the path is **not absolute**, which is what `cide_fs::ops::check_within` already
 * refuses: a frontend bug that sent a group header to `fs_rename` gets an error from the check
 * that was already there rather than from a special case somebody had to remember to write. An
 * empty string was the alternative and is worse — the context menu's `rowFacts` would have
 * answered `{path: ''}` and offered *Rename…* on it.
 */
const GROUP_PREFIX = 'cide://group/'

/** *External Libraries*. Matches `cide_app::libraries::GROUP_ID` exactly. */
export const EXTERNAL_LIBRARIES = 'externalLibraries'

/**
 * *Scratches*. Matches `cide_app::scratches::GROUP_ID` exactly.
 *
 * The second group, and the first that is **writable**: its rows are files cide itself created,
 * in a drawer under `$XDG_STATE_HOME`, and Rename, Cut and *Move to Trash* all work on them.
 * Nothing in this module needs to know that — `rowVerbs` takes `inProject` as an argument
 * precisely so the caller can answer "may cide change things here" from the list Rust gave it
 * (`fs_writable_roots`), rather than from a second copy of the rule kept in step by memory.
 */
export const SCRATCHES = 'scratches'

/**
 * *Project Notes*. Matches `cide_app::notes::GROUP_ID` exactly.
 *
 * The third id, and the first that is a **pin**: one top-level row that *opens* rather than
 * expanding. It carries the same `cide://group/<id>` sentinel a header does — so
 * `isSyntheticPath`, `mutationRefusal` and `treeDrag` refuse it by the rules they already
 * apply — and the file it opens is **named by Rust and never by this string**: the frontend
 * calls `fs_notes_ensure`, which creates the file on the first call and answers its path. That
 * separation is what makes it impossible to open the sentinel by mistake; a handler that passed
 * `cide://group/projectNotes` to `file.open` would fail loudly at `check_within` rather than
 * quietly opening the wrong thing.
 */
export const PROJECT_NOTES = 'projectNotes'

/** The `path` a group header carries. Mirrors `cide_fs::groups::group_path`. */
export function groupPath(id: string): string {
  return `${GROUP_PREFIX}${id}`
}

/** The group id inside a header's path, or `null` for anything else. */
export function groupIdOf(path: string): string | null {
  return path.startsWith(GROUP_PREFIX) ? path.slice(GROUP_PREFIX.length) : null
}

/**
 * A path that names nothing on disk: a group header, or a note.
 *
 * Tested by *shape*, not by prefix list. Rust's containment check keys on exactly this — "is it
 * absolute" — so a synthetic scheme added later (`cide://note/`, and whatever a future group
 * invents) is covered here the moment it exists, rather than the day somebody remembers to add
 * it to a list. A Windows path is not a concern: cide is Linux-first and every path on the wire
 * is a POSIX absolute path.
 */
export function isSyntheticPath(path: string): boolean {
  return !path.startsWith('/')
}

/**
 * What a row of this kind lets the user do.
 *
 * Five booleans and not one enum, for the reason `RowAction` in `clickSemantics.ts` gives: these
 * are independent, and an enum would need a member per combination.
 */
export interface RowVerbs {
  /** Draw a twisty; a click on it, Enter, and `→` fold or unfold the row. */
  readonly expandable: boolean
  /** A double-click or Enter opens an editor tab. */
  readonly openable: boolean
  /** May be part of the set Cut, Copy and *Move to Trash* act on. */
  readonly actionable: boolean
  /**
   * The context menu offers the verbs that change the disk: *New File*, *Cut*, *Paste*,
   * *Rename…*, *Move to Trash*.
   *
   * False for everything outside a project root. Rust refuses those paths anyway, so the cost of
   * getting this wrong is a menu item that errors rather than data loss — but an item that is
   * enabled and always fails is exactly the dead control this panel keeps shipping, and a greyed
   * row with a reason on it is the house treatment.
   */
  readonly mutable: boolean
  /** The verbs that only *name* the row: *Copy Path*, *Copy Relative Path*, *Reveal*. */
  readonly addressable: boolean
}

const NOTHING: RowVerbs = {
  expandable: false,
  openable: false,
  actionable: false,
  mutable: false,
  addressable: false,
}

/**
 * The rules, one row per kind.
 *
 * `inProject` is "some project root contains this path" — `rootOf(path, roots) !== null` from
 * `rowPaths.ts`. It is passed rather than derived because this module is import-free, and
 * because the caller has the roots in hand already.
 *
 * | kind | expandable | openable | actionable | mutable | addressable |
 * | --- | --- | --- | --- | --- | --- |
 * | `group` | yes | no | no | no | no |
 * | `note` | no | no | no | no | no |
 * | `pin` | no | **yes** | no | no | no |
 * | `dir` in a project | yes | no | yes | yes | yes |
 * | `dir` outside | yes | no | yes | **no** | yes |
 * | `file` in a project | no | yes | yes | yes | yes |
 * | `file` outside | no | yes | yes | **no** | yes |
 *
 * A group header is deliberately **not actionable**: the arrows may land on it (that is how a
 * keyboard reaches the twisty at all) but Ctrl+X on the cursor must not put `cide://group/…` on
 * the clipboard, and *Move 3 Items to Trash* must not count it.
 *
 * A **pin** — *Project Notes* — is the mirror image: `openable` and nothing else. The three
 * `false`s are each a decision rather than a default. `expandable: false` because it has no
 * children and Rust refuses to mark it expanded, so a twisty on it would fold nothing.
 * `actionable: false` for the header's reason exactly: Ctrl+X on the cursor must not put a
 * sentinel on the clipboard and *Move 3 Items to Trash* must not count it. `addressable: false`
 * because *Copy Path* would copy the literal string `cide://group/projectNotes` — a
 * correct-looking answer about the wrong thing, which is the defect class this panel keeps
 * paying for; the file's real path is known only to Rust, and the row is not a way to learn it.
 *
 * A pin's verbs do **not** depend on `inProject`, unlike a `file`'s: it has no path on disk for
 * containment to be a question about. The *file it opens* is outside every root and is saved
 * anyway, because `file_write` applies no containment check — that asymmetry is argued in
 * `crates/cide-app/src/notes.rs`, and nothing in this module needs to know about it.
 *
 * A library file **is openable**, and that is the decision worth stating. The tab it opens is
 * read-only — `cide_core::toolchain::read_only_reason` clears `FileDoc::writable` for anything
 * under a toolchain's dependency cache — so reading a dependency's source costs nothing and can
 * change nothing. It is also not new: Go to definition has opened these files since M12.
 *
 * There is deliberately **no confirmation** on that open, unlike the ctrl+click on an
 * out-of-project path in a terminal pane. That dialog defends against *attacker-chosen bytes on
 * screen plus one unsuspecting click* — a build log naming `~/.claude/.credentials.json`. A row
 * under *External Libraries* is neither: cide produced it, from the project's own lockfile, under
 * a header the user expanded by name, and every row under it is a dependency source by
 * construction. A dialog per click there would be noise, and noise is what teaches people to
 * click through the dialog that matters.
 */
export function rowVerbs(kind: RowKind, inProject: boolean): RowVerbs {
  switch (kind) {
    case 'group':
      return { ...NOTHING, expandable: true }
    case 'note':
      return NOTHING
    case 'pin':
      return { ...NOTHING, openable: true }
    case 'dir':
      return {
        expandable: true,
        openable: false,
        actionable: true,
        mutable: inProject,
        addressable: true,
      }
    case 'file':
      return {
        expandable: false,
        openable: true,
        actionable: true,
        mutable: inProject,
        addressable: true,
      }
    default:
      // A kind this build has never heard of — an older webview against a newer backend. It
      // draws as a row with no verbs rather than throwing inside a render, which would unmount
      // the whole tree. The same defence `STATUS[tone] ?? CLEAN` makes one file over.
      return NOTHING
  }
}

/**
 * The icon stem a synthetic row draws, or `null` to use the ordinary filename lookup.
 *
 * `id` is [`groupIdOf`] of the row's path — `null` for anything that is not a header, which is
 * every row this returns `null` for anyway.
 *
 * Here rather than in `icons/iconFor.ts` because it is a *tree* decision, not an icon-theme one:
 * the Material Icon Theme has no association for "a group header", and inventing one inside the
 * transcribed table is exactly what that file's header forbids. `folder-lib` is the theme's own
 * library folder and ships in `public/icons/` in both variants, which `check-icons.mjs` proves.
 */
export function groupIcon(kind: RowKind, expanded: boolean, id: string | null): string | null {
  // A pin has no open and closed variant — it never folds — so it takes its stem whole and
  // before the `-open` suffix below could be appended to it. An unknown pin id falls back to
  // the plain document glyph rather than to a folder: a pin stands for one file.
  if (kind === 'pin') return STEMS[id ?? ''] ?? 'document'
  // A note draws no icon at all. It is a sentence, and a document glyph in front of it would
  // read as a file called "cargo is not on PATH".
  if (kind !== 'group') return null
  // `-open` is the Material theme's own suffix convention for a folder, applied here rather than
  // by the caller so the two halves of one stem cannot be assembled in two places.
  const stem = STEMS[id ?? ''] ?? 'folder'
  return expanded ? `${stem}-open` : stem
}

/**
 * The Material Icon Theme folder each group borrows, by id.
 *
 * `folder-lib` is upstream's library folder and `folder-temp` its temporary-files one, which is
 * as close as that theme comes to a drawer of scratch buffers — and closer than the alternative,
 * which was to draw both headers with the same library icon and make the two groups look like
 * two halves of one thing. Both ship in `public/icons/`, which `check:scratch` re-checks on disk
 * because a stem with no file is a 404 and a blank row rather than an error.
 *
 * An unknown id falls back to the plain folder rather than throwing: a group this build has
 * never heard of is an older webview against a newer backend, the same case `rowVerbs`'s
 * `default` arm covers.
 *
 * *Project Notes* is the one entry here that is not a folder, because the row is not a drawer:
 * `markdown` is the Material theme's own glyph for a `.md` file, which is what the row opens, so
 * the pin looks like the tab it produces. Both variants ship in `public/icons/`, re-checked on
 * disk by `check:notes` for the same reason `check:scratch` re-checks its two.
 */
const STEMS: Readonly<Record<string, string>> = {
  [EXTERNAL_LIBRARIES]: 'folder-lib',
  [SCRATCHES]: 'folder-temp',
  [PROJECT_NOTES]: 'markdown',
}
