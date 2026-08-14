/**
 * Settings → Keymap, as arithmetic: what the screen shows, what is customised, and what a
 * chord would collide with **before** it is written.
 *
 * No React, no store, no `@/` import — only the two pure key modules beside it, so
 * `ui/scripts/check-keymap.mjs` can compile this standalone and drive every rule. That split is
 * not tidiness: the two rules here that matter are the conflict preview and the arithmetic of
 * "is this row customised", and a rule that lives in a component is a rule no check can reach.
 * This project has paid for that five times.
 *
 * # What is *not* here
 *
 * The edit itself. Turning "put this command on this chord" into entries in `keymap.json` is
 * `cide_core::keymap::apply_edit`, because it needs the compiled-in defaults to know what to
 * suppress and because a second implementation of layering is a second set of the bugs
 * `keymap.rs` documents at length. This module never builds a `Binding`; it names a command,
 * a context and a key, and Rust works out the file.
 *
 * # Why the rows are built from commands and not from bindings
 *
 * The screen used to map `report.bindings`, which is the *resolved table*: 32 rows for a
 * registry of 59 commands. Half the app was therefore unbindable from the screen whose job is
 * binding — not disabled, not explained, simply absent — and an unbind was invisible too,
 * because a `-command` entry produces no resolved binding at all. So the join runs the other
 * way: every command gets a row, its bindings hang off it, and `overrides` is counted from the
 * user's file rather than inferred from what survived layering.
 */
import { normalizeSequence, parseStroke, prefixesOf } from '../keys/chords'

/** Which layer a binding came from. Mirrors `KeymapLayer`. */
export type LayerName = 'default' | 'platform' | 'user'

/**
 * The registry entry, structurally.
 *
 * Structural rather than the generated `Command`, for the reason `keys/keymap.ts` gives for
 * `KeyBinding`: this module has to compile with no `@/` import so its check script can run it,
 * and the generated type is assignable to this one so nothing is lost at the call site.
 */
export interface CommandInfo {
  id: string
  title: string
  group: string
  unavailable?: string | null | undefined
}

/** A resolved binding, structurally. `ResolvedBinding` satisfies it. */
export interface ResolvedInfo {
  key: string
  command: string
  when?: string | null | undefined
  layer: LayerName
}

/** One entry of the user's `keymap.json`, structurally. `Binding` satisfies it. */
export interface OverrideInfo {
  key: string
  command: string
  when?: string | null | undefined
}

/** One binding under a row, with its key normalised the way the gate compares it. */
export interface RowBinding {
  key: string
  when: string | null
  layer: LayerName
}

/** One command, and everything the screen knows about it. */
export interface KeymapRow {
  id: string
  title: string
  group: string
  /** Why the command cannot run in this build, or `null`. Not bindable while it is set. */
  unavailable: string | null
  /** Its bindings, in resolution order. Usually one; empty for a palette-only command. */
  bindings: readonly RowBinding[]
  /** How many entries in `keymap.json` name this command — adds and `-removals` alike. */
  overrides: number
  /**
   * The user's file names this command and nothing resolves: they took its chord away.
   *
   * The row a resolved-bindings list cannot draw, and the reason `KeymapReport` carries the
   * raw overrides at all. Without it, unbinding Ctrl+T removes the row from the screen
   * entirely and there is nothing left to press *Restore default* on.
   */
  unbound: boolean
  /** The user's file names a command the registry does not have. */
  unknown: boolean
}

/**
 * A command may be bound iff it can actually run.
 *
 * Binding a key to an `unavailable` command is strictly worse than leaving it unbound: the
 * gate resolves the chord, swallows the keystroke and calls a dispatcher that has nothing to
 * do, so the key stops reaching the terminal or the editor underneath *and* does nothing.
 * `cide_core::keymap`'s `nothing_binds_a_key_to_an_unavailable_command` forbids it for the
 * shipped table; this is the same rule for the screen that lets a user write one.
 */
export function bindable(row: KeymapRow): boolean {
  return row.unavailable === null
}

/** True when the user's file has anything to say about this row. */
export function customised(row: KeymapRow): boolean {
  return row.overrides > 0
}

/**
 * The (command, `when`) pair that identifies a binding for an edit.
 *
 * A function rather than an inline object literal because it is *the* load-bearing rule of the
 * whole screen: a removal matches on key, command **and** `when`, so the context has to be the
 * one the binding actually carries. The resolved binding already knows it, normalised, which
 * makes the row the user clicked the removal recipe — as long as it is copied and never
 * re-derived from the command's own `when`, which gates the palette and is a different thing.
 */
export function editTarget(
  row: KeymapRow,
  binding: RowBinding | null,
): { command: string; when: string | null } {
  return { command: row.id, when: binding === null ? null : binding.when }
}

/** Trim and collapse a `when`, the way `cide_core::keymap::normalize_when` does. */
function normalizeWhen(when: string | null | undefined): string | null {
  if (when === null || when === undefined) return null
  const text = when.trim().replace(/\s+/g, ' ')
  return text === '' ? null : text
}

/**
 * Build one row per command, with the user's file joined onto it.
 *
 * `bindings` is the resolved table and `overrides` the raw file. Both are needed and neither
 * is derivable from the other: the resolved table cannot show a removal, and the file cannot
 * show what a default binds.
 *
 * Sorted by group and then title, which is the palette's own order, so a user looking for
 * "the git ones" finds them together. Rows for command ids the registry does not have come
 * last, under their own group: a `keymap.json` naming a command that was renamed years ago
 * would otherwise be invisible on the one screen that could explain it.
 */
export function buildRows(
  commands: readonly CommandInfo[],
  bindings: readonly ResolvedInfo[],
  overrides: readonly OverrideInfo[],
): KeymapRow[] {
  const byCommand = new Map<string, RowBinding[]>()
  for (const binding of bindings) {
    const key = normalizeSequence(binding.key)
    if (key === '') continue
    // A `-command` entry is a removal directive Rust has already applied; one reaching a
    // resolved table would be a bug there, and drawing it as a binding would show the user a
    // chord bound to a command called `-picker.files`.
    if (binding.command.startsWith('-')) continue
    const list = byCommand.get(binding.command)
    const row: RowBinding = { key, when: normalizeWhen(binding.when), layer: binding.layer }
    if (list === undefined) byCommand.set(binding.command, [row])
    else list.push(row)
  }

  const overrideCounts = new Map<string, number>()
  for (const entry of overrides) {
    const id = entry.command.startsWith('-') ? entry.command.slice(1) : entry.command
    if (id === '') continue
    overrideCounts.set(id, (overrideCounts.get(id) ?? 0) + 1)
  }

  const rows: KeymapRow[] = commands.map((command) => {
    const bound = byCommand.get(command.id) ?? []
    const count = overrideCounts.get(command.id) ?? 0
    return {
      id: command.id,
      title: command.title,
      group: command.group,
      unavailable: command.unavailable ?? null,
      bindings: bound,
      overrides: count,
      unbound: count > 0 && bound.length === 0,
      unknown: false,
    }
  })

  const known = new Set(rows.map((row) => row.id))
  for (const [id, count] of overrideCounts) {
    if (known.has(id)) continue
    rows.push({
      id,
      title: id,
      group: UNKNOWN_GROUP,
      unavailable: null,
      bindings: byCommand.get(id) ?? [],
      overrides: count,
      unbound: (byCommand.get(id) ?? []).length === 0,
      unknown: true,
    })
  }

  rows.sort((a, b) => {
    if (a.group !== b.group) {
      // Unknown ids last, whatever they sort as alphabetically.
      if (a.group === UNKNOWN_GROUP) return 1
      if (b.group === UNKNOWN_GROUP) return -1
      return a.group.localeCompare(b.group)
    }
    return a.title.localeCompare(b.title)
  })
  return rows
}

/** The group heading rows for ids the registry does not know are filed under. */
export const UNKNOWN_GROUP = 'Not in this build'

/** One line of the table: a command with one of its bindings, or a command with none. */
export interface KeymapEntry {
  row: KeymapRow
  /** The binding this line is about. `null` for a command that has none. */
  binding: RowBinding | null
}

/**
 * Flatten rows into table lines, one per binding.
 *
 * A command bound to two chords gets two lines rather than one line listing both, because
 * every action on this screen is *about a binding*: Edit moves one, Unbind removes one, and a
 * removal matches on that binding's own `when`. A single line would have to pick one of them
 * silently, which is precisely the class of guess the `when` trap punishes.
 */
export function entriesOf(rows: readonly KeymapRow[]): KeymapEntry[] {
  const out: KeymapEntry[] = []
  for (const row of rows) {
    if (row.bindings.length === 0) out.push({ row, binding: null })
    else for (const binding of row.bindings) out.push({ row, binding })
  }
  return out
}

/** Does this row match what the user typed into the filter box? */
export function matchesFilter(row: KeymapRow, needle: string): boolean {
  const text = needle.trim().toLowerCase()
  if (text === '') return true
  if (row.title.toLowerCase().includes(text)) return true
  if (row.id.toLowerCase().includes(text)) return true
  if (row.group.toLowerCase().includes(text)) return true
  return row.bindings.some((binding) => binding.key.includes(text))
}

// --- what a chord would cost, before it is written -------------------------------------------

/** How a candidate chord and an existing binding get in each other's way. */
export type ClashKind =
  /** The same keystroke in the same context. One of the two will never fire. */
  | 'same'
  /**
   * The candidate is the first stroke of a binding that already exists.
   *
   * `keys/keymap.ts` prefers an applicable *continuation* over an exact match, which is what
   * makes `ctrl+k ctrl+s` reachable at all. The consequence is that a binding on `ctrl+k`
   * alone can then never fire — the stroke arms the prefix machine instead.
   */
  | 'prefix'
  /** The candidate is a sequence whose first stroke is already bound on its own — see above,
   *  with the roles reversed: it is the *existing* binding that stops firing. */
  | 'extends'

/** One thing a candidate chord would run into. */
export interface KeyClash {
  kind: ClashKind
  /** The existing binding's normalised key. */
  key: string
  /** The command that holds it now. */
  command: string
  when: string | null
  layer: LayerName
  /**
   * True when saving would leave **both** bindings alive and `conflicts` would report them.
   *
   * The asymmetry worth telling a user about: within the user's own layer duplicates coexist
   * deliberately, so a chord taken from another *user* entry is a real conflict. A chord taken
   * from a **default** is not — `apply_layer` drops an earlier layer's binding on the same
   * (key, `when`) — so nothing will be reported, and the only visible consequence is that the
   * default's command has quietly lost its shortcut. Same warning, opposite fine print.
   */
  contested: boolean
}

/**
 * What binding `key` in context `when` for `command` would run into.
 *
 * Compared against the **resolved** table, normalised on both sides through the same
 * `normalizeSequence` the gate uses, so the answer is about the keystroke rather than about
 * its spelling: `ctrl+backquote` and `` ctrl+` `` are one key here, which they are not in
 * `cide_core::keymap` (it renames no keys by design, and the alias table is this side's).
 * That makes this preview strictly better informed than the post-hoc report — the one place in
 * the app where the frontend knows something Rust does not, and the reason the warning is
 * raised *before* the write rather than left to the banner afterwards.
 *
 * Bindings for the same command are not clashes: binding a command to a chord it already
 * answers to is a no-op, and a command deliberately bound to two chords is the user's business.
 */
export function clashesFor(
  bindings: readonly ResolvedInfo[],
  key: string,
  when: string | null,
  command: string,
): KeyClash[] {
  const candidate = normalizeSequence(key)
  if (candidate === '') return []
  const context = normalizeWhen(when)
  const out: KeyClash[] = []

  for (const binding of bindings) {
    if (binding.command === command || binding.command.startsWith('-')) continue
    const existing = normalizeSequence(binding.key)
    if (existing === '') continue
    /*
     * A different context is not a clash — that is the mechanism by which one key means
     * different things in a terminal and in an editor.
     *
     * **Unless one of them is unscoped.** An entry with no `when` applies *everywhere*, so it
     * overlaps every scoped binding on the same key rather than sitting beside it, and the gate
     * resolves last-applicable-wins: an unscoped User entry silently kills the scoped Default.
     * Binding an unbound command to F4 with no context therefore took the panel toggle away with
     * no dialog, no conflict row and no diagnostic — a comparison of two `when` strings cannot see
     * a containment relationship, and `null` contains all of them.
     *
     * Only the null cases are widened. Two *different* non-null scopes really are disjoint as far
     * as anything here can tell, and treating them as clashing would put a confirmation in front
     * of the ordinary editor-versus-terminal split this whole mechanism exists for.
     */
    const existingWhen = normalizeWhen(binding.when)
    const overlaps = existingWhen === context || existingWhen === null || context === null
    if (!overlaps) continue

    let kind: ClashKind | null = null
    if (existing === candidate) kind = 'same'
    else if (prefixesOf(existing).includes(candidate)) kind = 'prefix'
    else if (prefixesOf(candidate).includes(existing)) kind = 'extends'
    if (kind === null) continue

    out.push({
      kind,
      key: existing,
      command: binding.command,
      when: normalizeWhen(binding.when),
      layer: binding.layer,
      contested: kind === 'same' && binding.layer === 'user',
    })
  }
  return out
}

/**
 * Why binding this chord might be a bad idea even when nothing else holds it.
 *
 * The key gate's second entry point is a **window capture** listener, so an unmodified
 * printable key is not "a shortcut with a low bar" — it is that character removed from every
 * text field, every rename box, every commit message and every terminal in every window. The
 * screen warns and does not refuse, for the same reason it warns about conflicts: this is the
 * user's keyboard, and `keymap.json` can express it either way.
 *
 * Only the first stroke is examined. Later strokes of a sequence are only ever read while the
 * prefix machine is armed, which is a state the user entered deliberately a moment earlier.
 */
export function bareKeyWarning(key: string): string | null {
  const first = normalizeSequence(key).split(' ')[0]
  if (first === undefined || first === '') return null
  const chord = parseStroke(first)
  if (chord === null) return null
  if (chord.ctrl || chord.alt || chord.meta) return null
  const typing = chord.key.length === 1 || TYPING_KEYS.has(chord.key)
  if (!typing) return null
  return chord.shift
    ? 'This chord holds only Shift, so it is an ordinary typed character. The key gate is a window capture listener, so binding it takes that character from every text field and every terminal in the app.'
    : 'This chord has no modifier. The key gate is a window capture listener, so binding it takes the key from every text field and every terminal in the app.'
}

/** Keys that are typing even though their name is a word. */
const TYPING_KEYS = new Set(['space', 'tab', 'enter', 'backspace'])

// --- the footer -------------------------------------------------------------------------------

/**
 * The line under the table, which has to be true of the file rather than of the table.
 *
 * It used to read "Overrides: <path> — user overrides only; the defaults above are compiled
 * in", written that way because a bare path reads as "this exists and you have edited it".
 * Now that the screen can write the file, the qualifier can be the truth instead of a
 * disclaimer — and the count comes from `KeymapReport.overrides`, never from the resolved
 * list, because the most interesting override produces no resolved binding at all.
 */
export function overridesSummary(count: number): string {
  if (count === 0) return 'No overrides yet — every binding above is compiled in.'
  if (count === 1) return '1 override in your file; everything else is compiled in.'
  return `${count} overrides in your file; everything else is compiled in.`
}

/** What an edit did, as the sentence the screen shows after it. */
export function editSummary(removed: number, added: number): string {
  if (removed === 0 && added === 0) {
    // The failure this whole result type exists for: an edit that matched nothing looks
    // exactly like one that worked unless it says so.
    return 'Nothing changed — no entry in your file matched.'
  }
  const parts: string[] = []
  if (removed > 0) parts.push(`${removed} ${removed === 1 ? 'entry' : 'entries'} removed`)
  if (added > 0) parts.push(`${added} ${added === 1 ? 'entry' : 'entries'} written`)
  return `${parts.join(', ')}.`
}
