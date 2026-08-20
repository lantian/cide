/**
 * *Send this to Claude ▸* — the list of conversations, numbered the way the user sees them.
 *
 * > *"we should show all session that is opened for current project with naming like
 * > `1: <session-name>` — where 1 is number of the panel/session and `<session-name>` is the
 * > name of the session (that I actually giving via /rename command in claude code)"*
 *
 * Import-free on purpose, like `sendToClaude.ts` and `revealTarget.ts` beside it:
 * `check-editor.mjs` compiles the editor's pure modules with `tsc` and runs them under node,
 * so anything reaching `@/store` or `@/ipc/client` falls out of that pipe. The half with the
 * numbers in it is the half worth pinning, and here that is *the order*, which is the one
 * thing in this feature a user can catch being wrong at a glance.
 *
 * # Two halves, from two places, and only one of them is durable
 *
 * The **pane** half comes from the workspace mirror, which Rust owns: which tabs a project
 * has, which panes are in them, and where each one sits in the split tree.
 *
 * The **name** half does not exist in cide at all. `/rename` is typed into the CLI, which
 * records it under `~/.claude/sessions/<pid>.json`; `cide_claude::roster` reads it and
 * `claude_session_names` puts it on the wire. So a name here is a fact about another program's
 * state, arriving asynchronously and legitimately absent — for a conversation nobody has
 * named, for a session the CLI has not written a record for yet, for a `claude` that is not
 * running at all. Every one of those falls back to the pane's own title rather than to a gap,
 * because a row a user cannot identify is worse than one identified coarsely.
 *
 * # Why the number is a position and not an id
 *
 * A `PaneId` is a UUID and means nothing to a reader. The user asked for *"number of the
 * panel"*, and a panel's number is where it sits: tabs left to right, and inside each tab the
 * split tree in reading order — leftmost/topmost first, which is exactly a pre-order walk of
 * `LayoutNode`, because a split's `a` child *is* its left or top half.
 *
 * It is therefore **positional and unstable by construction**: close a pane and everything
 * after it renumbers. That is the right trade for a menu that is read and clicked in one
 * gesture, and it is why the number is never persisted, never used as a key, and never sent
 * anywhere — the row carries the `PaneId` for that, and the number is only ever shown.
 *
 * `panes` is an insertion-ordered map on the wire and walking *that* would have been one line
 * shorter and wrong: insertion order is the order panes were *created*, so splitting the first
 * pane in a tab puts the new one at the end of the map and in the middle of the screen. The
 * numbering would then disagree with the layout for every user who has ever split a pane
 * anywhere but at the right-hand edge.
 */

/** The parts of a `Pane` a row needs. `kind` is compared against the string `'claude'`. */
export interface PaneLike {
  readonly id: string
  readonly kind: string
  /** cide's handle on the session. The id it was spawned under, stable for the pane's life. */
  readonly session?: string | null | undefined
  /**
   * The conversation the CLI is actually on, when `/clear` or a resume has moved it.
   *
   * Consulted **first**, because the CLI files its session record under the conversation it is
   * running — so after a `/clear` the name the user gave lives under this id and not under
   * [`session`]. Looking only at `session` would silently lose the name for exactly the panes
   * that have been used the longest.
   */
  readonly conversation?: string | null | undefined
  /** cide's own label, e.g. `cide : claude`. The fallback when there is no name. */
  readonly title: string
}

/** The parts of a `LayoutNode` the walk reads. `a` is the left or top half of a split. */
export type NodeLike =
  | { readonly kind: 'leaf'; readonly pane: string }
  | { readonly kind: 'split'; readonly a: NodeLike; readonly b: NodeLike }

export interface TreeLike {
  readonly root: NodeLike
  readonly panes: Readonly<Record<string, PaneLike>>
}

export interface TabLike {
  readonly id: string
  readonly tree: TreeLike
}

export interface ProjectLike {
  /** `tabs[0]` is the pinned console, and so holds the project's first Claude panes. */
  readonly tabs: readonly TabLike[]
  /** Panes torn out into windows of their own. Still this project's conversations. */
  readonly detached: Readonly<Record<string, PaneLike>>
}

/** One row of the submenu. */
export interface ClaudeSession {
  readonly pane: string
  /** 1-based, in reading order across the whole project. Shown, never stored or sent. */
  readonly index: number
  /** The pane's own title, kept so a caller can name the destination without re-deriving it. */
  readonly title: string
  /** What `/rename` called it, or `null` when nothing has. */
  readonly name: string | null
  /** What the menu row says: `1: agents`. */
  readonly label: string
}

/** Pane ids in reading order: a split's `a` half — its left or top — before its `b`. */
function walk(node: NodeLike, out: string[]): void {
  if (node.kind === 'leaf') {
    out.push(node.pane)
    return
  }
  walk(node.a, out)
  walk(node.b, out)
}

/**
 * Every Claude pane in a project, numbered and labelled.
 *
 * Detached panes come last and are numbered like any other. They are a real destination — a
 * conversation torn into its own window is still this project's, `claude_send_lines` will
 * happily address it, and `revealPane` knows how to bring its window forward — so leaving them
 * out would hide the pane a user most deliberately put somewhere they could see.
 *
 * A pane with no session id at all still gets a row. That is a Claude pane sitting at a resume
 * splash with no process behind it, and it cannot receive a mention today; it is listed anyway
 * because the alternative is a numbering with holes in it, and because whether a pane's
 * `claude` is on the IDE server is not something this webview can see — the send reports that,
 * with a sentence, which is the split `useSendToClaude` already argues for at length.
 */
export function claudeSessions(
  project: ProjectLike,
  names: Readonly<Record<string, string>>,
): ClaudeSession[] {
  const out: ClaudeSession[] = []

  const push = (pane: PaneLike): void => {
    if (pane.kind !== 'claude') return
    // The conversation first: after a `/clear` the CLI files its record under the new id, so
    // that is where the name the user typed will be found.
    const id = pane.conversation ?? pane.session ?? null
    const name = id === null ? undefined : names[id]
    const index = out.length + 1
    out.push({
      pane: pane.id,
      index,
      title: pane.title,
      name: name ?? null,
      label: `${index}: ${name ?? pane.title}`,
    })
  }

  for (const tab of project.tabs) {
    const order: string[] = []
    walk(tab.tree.root, order)
    for (const id of order) {
      const pane = tab.tree.panes[id]
      if (pane !== undefined) push(pane)
    }
  }
  for (const pane of Object.values(project.detached)) push(pane)

  return out
}
