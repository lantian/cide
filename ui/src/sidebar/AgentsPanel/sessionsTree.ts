/**
 * The Sessions tab's two lists, its drop rules and its keyboard steps — pure. (M199)
 *
 * **Import-free but for types** from `sessionsModel.ts` (erased at compile), so
 * `check-sessions.mjs` drives every rule here under node. `useSessionsDrag.ts` is the pointer
 * mechanism and `SessionsPanel.tsx` the drawing; neither decides anything this file does.
 *
 * # Where a session is, and the one-place rule
 *
 * - **In a group** when the journal files it in one that exists — live or ended, it stays there.
 * - **Loose in *Opened*** when it is live and in no group: the panes' sessions and the background
 *   ones together, newest first.
 * - **In *Closed*** otherwise: ended and unfiled.
 *
 * Each row is in exactly one of the three. A membership naming a group that no longer exists
 * (another window deleted it a frame ago) reads as no group, so the row lands loose or in Closed
 * rather than nowhere.
 *
 * # A search opens every folder it finds something in
 *
 * While the box has words, a group is drawn when its name matches or anything under it does, and
 * drawn **open** whatever it was folded to — a hit inside a folded group is a hit nobody can see.
 * Its folded state is untouched, so clearing the search puts the tree back the way it was.
 */
import type { GroupView, SessionStatus, SessionView } from './sessionsModel'

/** One line of the *Opened* tree. */
export type TreeItem =
  | {
      type: 'group'
      id: string
      depth: number
      group: GroupView
      /** Drawn open: the group's own state, or a search's. */
      open: boolean
      /** Sessions anywhere under it. */
      count: number
      /** For a folded group: something inside works, and how many wait for the user. */
      working: boolean
      waiting: number
    }
  | { type: 'session'; id: string; depth: number; view: SessionView }
  /** The thin line between the groups and the loose sessions. */
  | { type: 'rule'; id: string; depth: 0 }

/** What the search decided about each row and how each row reads — the tree's two inputs. */
export interface TreeLens {
  /** The search keeps this row. */
  keep: (v: SessionView) => boolean
  /** What its glyph says — for the folded groups' roll-up. */
  status: (v: SessionView) => SessionStatus
  /** The box has words: folders open to show what they hold. */
  searching: boolean
  /** The search's words, to match a group's own name. */
  words: readonly string[]
}

/** The group a row is filed in, when that group exists. */
function groupOf(v: SessionView, known: ReadonlySet<string>): string | null {
  return v.group !== null && known.has(v.group) ? v.group : null
}

/** Where a row is: a group's id, `'loose'`, or `'closed'`. See the header. */
export function placeOf(v: SessionView, groups: readonly GroupView[]): string {
  const g = groupOf(v, new Set(groups.map((x) => x.id)))
  if (g !== null) return g
  return v.live ? 'loose' : 'closed'
}

/** Whether `id` is `ancestor` or under it. Bounded by the group count against a cycle. */
export function isWithin(groups: readonly GroupView[], id: string, ancestor: string): boolean {
  const byId = new Map(groups.map((g) => [g.id, g]))
  let at: string | null = id
  for (let i = 0; i <= groups.length && at !== null; i += 1) {
    if (at === ancestor) return true
    at = byId.get(at)?.parent ?? null
  }
  return false
}

/** "Release 0.18 / Menus" — a group named where it sits, for a hint. */
export function groupPath(groups: readonly GroupView[], id: string): string {
  const byId = new Map(groups.map((g) => [g.id, g]))
  const parts: string[] = []
  let at: string | null = id
  for (let i = 0; i <= groups.length && at !== null; i += 1) {
    const g = byId.get(at)
    if (g === undefined) break
    parts.unshift(g.name)
    at = g.parent
  }
  return parts.join(' / ')
}

/**
 * The *Opened* tree, flattened in drawing order: groups (siblings in the user's order, each
 * followed by its subgroups and then its sessions newest first), a rule, then the loose live
 * sessions newest first. `views` arrive newest first from Rust and keep that order.
 */
export function buildOpened(
  views: readonly SessionView[],
  groups: readonly GroupView[],
  lens: TreeLens,
): TreeItem[] {
  const known = new Set(groups.map((g) => g.id))
  // A parent that does not exist reads as the top level, so a stale nesting cannot hide a group.
  const parentOf = (g: GroupView) => (g.parent !== null && known.has(g.parent) ? g.parent : null)
  const children = new Map<string | null, GroupView[]>()
  for (const g of groups) {
    const p = parentOf(g)
    const list = children.get(p) ?? []
    list.push(g)
    children.set(p, list)
  }
  const members = new Map<string, SessionView[]>()
  for (const v of views) {
    const g = groupOf(v, known)
    if (g === null) continue
    const list = members.get(g) ?? []
    list.push(v)
    members.set(g, list)
  }
  const deep = (id: string, seen = new Set<string>()): SessionView[] => {
    if (seen.has(id)) return []
    seen.add(id)
    return [
      ...(members.get(id) ?? []),
      ...(children.get(id) ?? []).flatMap((c) => deep(c.id, seen)),
    ]
  }
  const nameHit = (g: GroupView) =>
    lens.words.length > 0 && lens.words.every((w) => g.name.toLowerCase().includes(w))
  const shown = (g: GroupView): boolean =>
    !lens.searching || nameHit(g) || deep(g.id).some(lens.keep)

  const out: TreeItem[] = []
  const walk = (parent: string | null, depth: number, seen: Set<string>) => {
    for (const g of children.get(parent) ?? []) {
      if (seen.has(g.id) || !shown(g)) continue
      seen.add(g.id)
      const all = deep(g.id)
      const open = lens.searching || !g.collapsed
      out.push({
        type: 'group',
        id: g.id,
        depth,
        group: g,
        open,
        count: all.length,
        working: all.some((v) => lens.status(v) === 'working'),
        waiting: all.filter((v) => statusWaits(lens.status(v))).length,
      })
      if (!open) continue
      walk(g.id, depth + 1, seen)
      for (const v of members.get(g.id) ?? [])
        if (!lens.searching || lens.keep(v) || nameHit(g))
          out.push({ type: 'session', id: v.id, depth: depth + 1, view: v })
    }
  }
  walk(null, 0, new Set())
  const loose = views.filter((v) => groupOf(v, known) === null && v.live && lens.keep(v))
  if (out.length > 0 && loose.length > 0) out.push({ type: 'rule', id: ' rule', depth: 0 })
  for (const v of loose) out.push({ type: 'session', id: v.id, depth: 0, view: v })
  return out
}

/** The attention statuses — `sessionsModel.isWaiting`, restated so this file needs no value import. */
function statusWaits(s: SessionStatus): boolean {
  return s === 'awaitingInput' || s === 'question' || s === 'permission'
}

/** *Closed*: ended, unfiled rows the search keeps, under the day captions, newest first. */
export function buildClosed(
  views: readonly SessionView[],
  groups: readonly GroupView[],
  keep: (v: SessionView) => boolean,
  dayOf: (v: SessionView) => 'today' | 'yesterday' | 'earlier',
): { day: 'today' | 'yesterday' | 'earlier'; views: SessionView[] }[] {
  const buckets = { today: [] as SessionView[], yesterday: [] as SessionView[], earlier: [] as SessionView[] }
  for (const v of views) if (placeOf(v, groups) === 'closed' && keep(v)) buckets[dayOf(v)].push(v)
  return (['today', 'yesterday', 'earlier'] as const)
    .filter((day) => buckets[day].length > 0)
    .map((day) => ({ day, views: buckets[day] }))
}

/** The sub-tabs' figures: rows the search keeps in each. */
export function tabCounts(
  views: readonly SessionView[],
  groups: readonly GroupView[],
  keep: (v: SessionView) => boolean,
): { opened: number; closed: number } {
  let opened = 0
  let closed = 0
  for (const v of views) {
    if (!keep(v)) continue
    if (placeOf(v, groups) === 'closed') closed += 1
    else opened += 1
  }
  return { opened, closed }
}

// --- dragging ------------------------------------------------------------------------------

export type DragLoad = { type: 'session'; id: string } | { type: 'group'; id: string }

/** Where the pointer is, as the drop rules see it. */
export type DropTarget =
  /** Into a group: its row, or any session row inside it. */
  | { kind: 'group'; id: string }
  /** Beside a group — its top or bottom edge — to reorder groups. */
  | { kind: 'line'; id: string; where: 'before' | 'after' }
  /** The loose area: a loose session's row, the rule, the space below. */
  | { kind: 'root' }
  /** The *Opened* sub-tab, from *Closed*: hold to switch. */
  | { kind: 'tab' }

/** `cide_ipc::sessions::SessionEdit`'s two arms a drop makes, restated (import-free). */
export type DropEdit =
  | { op: 'assign'; conversation: string; group: string | null }
  | { op: 'move'; id: string; parent: string | null; before: string | null }

export type DropOutcome =
  | { kind: 'move'; hint: string; edit: DropEdit }
  | { kind: 'noop'; hint: string }
  | { kind: 'refuse'; hint: string }
  /** Over the *Opened* tab: nothing drops, the tab opens after a rest. */
  | { kind: 'spring'; hint: string }

/** The verdict for carrying `load` to `target`, with the sentence the ghost shows. */
export function dropOutcome(
  load: DragLoad,
  target: DropTarget | null,
  views: readonly SessionView[],
  groups: readonly GroupView[],
): DropOutcome | null {
  if (target === null) return null
  const byId = new Map(groups.map((g) => [g.id, g]))
  const name = (id: string) => `“${groupPath(groups, id)}”`
  if (load.type === 'session') {
    const v = views.find((x) => x.id === load.id)
    if (v === undefined) return null
    const current = v.group !== null && byId.has(v.group) ? v.group : null
    switch (target.kind) {
      case 'tab':
        return { kind: 'spring', hint: 'Hold to open Opened, then drop it on a group' }
      case 'group':
        if (current === target.id) return { kind: 'noop', hint: `Already in ${name(target.id)}` }
        return {
          kind: 'move',
          hint: `Move to ${name(target.id)}`,
          edit: { op: 'assign', conversation: v.id, group: target.id },
        }
      case 'root':
        if (current === null)
          return v.live
            ? { kind: 'noop', hint: 'Already in no group' }
            : { kind: 'refuse', hint: 'A closed session goes into a group' }
        return {
          kind: 'move',
          hint: v.live
            ? `Take it out of ${name(current)}`
            : `Take it out of ${name(current)} — it has ended, so it goes to Closed`,
          edit: { op: 'assign', conversation: v.id, group: null },
        }
      case 'line':
        return null
    }
  }
  const g = byId.get(load.id)
  if (g === undefined) return null
  const itself = { kind: 'refuse' as const, hint: `“${g.name}” cannot go inside itself` }
  switch (target.kind) {
    case 'tab':
      return { kind: 'refuse', hint: 'Groups live in Opened' }
    case 'group':
      if (isWithin(groups, target.id, g.id)) return itself
      if (g.parent === target.id) return { kind: 'noop', hint: `Already in ${name(target.id)}` }
      return {
        kind: 'move',
        hint: `Move into ${name(target.id)}`,
        edit: { op: 'move', id: g.id, parent: target.id, before: null },
      }
    case 'line': {
      const h = byId.get(target.id)
      if (h === undefined) return null
      if (h.id === g.id) return { kind: 'noop', hint: 'Already here' }
      if (h.parent !== null && isWithin(groups, h.parent, g.id)) return itself
      const siblings = groups.filter((x) => x.parent === h.parent && x.id !== g.id)
      const at = siblings.findIndex((x) => x.id === h.id)
      const before = target.where === 'before' ? h.id : (siblings[at + 1]?.id ?? null)
      return {
        kind: 'move',
        hint: `Place it ${target.where} “${h.name}”`,
        edit: { op: 'move', id: g.id, parent: h.parent, before },
      }
    }
    case 'root':
      if (g.parent === null) return { kind: 'noop', hint: 'Already at the top level' }
      return {
        kind: 'move',
        hint: 'Move to the top level',
        edit: { op: 'move', id: g.id, parent: null, before: null },
      }
  }
}

/**
 * What a rest of the pointer opens: a folded group (so the drag can go into what it holds), or
 * the *Opened* tab. A key, so the hook can tell "still on the same thing" from "moved on".
 */
export function springKey(target: DropTarget | null, groups: readonly GroupView[]): string | null {
  if (target === null) return null
  if (target.kind === 'tab') return 'tab'
  if (target.kind === 'group' && groups.find((g) => g.id === target.id)?.collapsed === true)
    return `group:${target.id}`
  return null
}

// --- the keyboard ----------------------------------------------------------------------------

/**
 * The row an arrow, Home or End selects, among `ids` in drawing order, or `null` for a key that
 * moves nothing. From no selection, Down and Home take the first row and Up and End the last.
 */
export function stepSelection(ids: readonly string[], current: string | null, key: string): string | null {
  if (ids.length === 0) return null
  const at = current === null ? -1 : ids.indexOf(current)
  switch (key) {
    case 'ArrowDown':
      return ids[at < 0 ? 0 : Math.min(ids.length - 1, at + 1)] ?? null
    case 'ArrowUp':
      return ids[at < 0 ? ids.length - 1 : Math.max(0, at - 1)] ?? null
    case 'Home':
      return ids[0] ?? null
    case 'End':
      return ids[ids.length - 1] ?? null
    default:
      return null
  }
}
