/**
 * The held-modifier MRU switcher, as arithmetic. **Two of them now, and one implementation.**
 *
 * Ctrl+Tab (tabs) and Ctrl+` (projects) are both the Windows / IDEA / browser switcher — not a
 * two-item toggle and not a step through the strip on screen:
 *
 * ```
 * Ctrl down, Tab            → a popup opens on MRU[1], the tab used before this one
 * Ctrl still down, Tab      → MRU[2], MRU[3], … wrapping through the whole list
 * Ctrl still down, Shift+Tab→ back one
 * Ctrl up                   → the selection is activated and moves to the front of the stack
 * Escape                    → closed, nothing activated, the stack untouched
 * ```
 *
 * Everything above is decided here, by pure functions over a frozen list of **opaque id
 * strings**. Nothing in this module knows what a project or a tab is, which is what let the
 * second switcher be a second *caller* rather than a second copy — see `keys/switcherStore.ts`
 * for the part that does know, and for why one store and one keyup latch is not a
 * simplification but a correctness requirement. No DOM, no store, no React and no `@/` import,
 * for the same reason `chords.ts` and `keymap.ts` have none: `ui/scripts/check-switcher.mjs`
 * compiles this module on its own and drives the whole gesture through it — over both targets —
 * and `ui/scripts/check-key-gate.mjs` compiles it beside the gate to prove the stroke capture
 * below behaves the same at both of the gate's entry points.
 *
 * # The two invariants that make the walk correct
 *
 * 1. **The order is frozen at open.** [`Walk::order`] is a snapshot taken when the popup
 *    appears, and Tab moves an index inside it and nothing else. Reordering as the user walks
 *    — the obvious implementation, and the wrong one — moves the list under them: the second
 *    Tab lands on whatever the first one just promoted, so a third entry is unreachable and
 *    the popup shows a list that reshuffles under the highlight.
 * 2. **Nothing is activated until the release.** [`commit`] is the only function that
 *    produces a new stack, and [`cancel`] does not exist as a function at all — cancelling is
 *    dropping the `Walk`, which is why Escape provably cannot reorder anything.
 *
 * # Why `hold` is a mask and not the word "ctrl"
 *
 * The chord that opens a switcher comes from the keymap, and a user may rebind it — the shipped
 * defaults are `ctrl+tab` and `` ctrl+` `` on every platform (macOS keeps ctrl for both,
 * because `⌘⇥` is the system application switcher and `⌘\`` walks an application's windows),
 * but `alt+tab` or `meta+tab` are things a `keymap.json` can say. So the modifiers to *watch
 * for the release of* are read off the opening chord rather than named here. Shift is
 * deliberately excluded from the mask: it is the direction, and releasing it mid-walk must not
 * commit.
 *
 * # And why the walk carries its *key* as well
 *
 * [`capture`] used to compare against the literal `'tab'`, which was the one line in this
 * module that knew which switcher it was serving. With two of them that is no longer merely
 * inelegant: a second `` ctrl+` `` during a project walk would fall through to the keymap (and
 * be rescued by accident, as one more dispatch), and `` ctrl+shift+` `` would resolve to
 * `terminal.splitBelow` and split a terminal in the middle of the gesture. Reading the key off
 * the opening chord is what makes the claim in [`capture`]'s note — *the switcher owns its own
 * key for as long as it is up* — true for both.
 */
import { parseStroke, type Chord } from './chords'

/**
 * The modifiers whose release commits the walk.
 *
 * Shift is not one of them — see the module note. `false` for all three is not representable
 * as a hold: [`openingOf`] answers `null` there, which is what makes a switcher opened from the
 * command palette (no keyboard event at all) take the immediate path instead of opening a
 * popup nothing can close.
 */
export interface Hold {
  ctrl: boolean
  alt: boolean
  meta: boolean
}

/**
 * The chord that opened a switcher, reduced to the two things a walk needs from it: what to
 * watch for the release of, and which key keeps walking.
 *
 * `null` rather than an `Opening` with an empty hold is the palette case — see [`Hold`].
 */
export interface Opening {
  readonly hold: Hold
  /** Canonical key name, e.g. `tab` or `backquote`. Never a modifier. */
  readonly key: string
}

/** The modifier state of a key event, structurally. `KeyboardEvent` satisfies it. */
export interface ModifierState {
  readonly ctrlKey: boolean
  readonly altKey: boolean
  readonly metaKey: boolean
}

/** One walk in progress. Immutable; every transition returns a new one. */
export interface Walk {
  /**
   * The MRU stack as it was when the popup opened. **Never rewritten while the walk runs** —
   * see invariant 1 in the module note.
   */
  readonly order: readonly string[]
  /** Index into [`Self::order`]. Always in range; the walk wraps rather than clamping. */
  readonly index: number
  /** The modifiers being held. Releasing all of them commits. */
  readonly hold: Hold
  /**
   * The key that opened this walk, and therefore the one that keeps walking it.
   *
   * Read off the opening chord rather than named, so the two switchers are one implementation
   * and a rebind moves the walk key with the binding. See the module note.
   */
  readonly key: string
}

/** What [`begin`] decided to do about one press of the switcher command. */
export type Begin =
  /** A modifier is down: open the popup and wait for the release. */
  | { kind: 'walk'; walk: Walk }
  /**
   * No modifier to hold — the palette ran the command, or it is bound to a bare key. Activate
   * the neighbour immediately and show nothing.
   *
   * This is not a lesser fallback, it is the only terminating behaviour available: a popup
   * that commits on the release of a modifier nobody is holding never closes, and would
   * swallow Tab for the rest of the session. "Switch to the most recently used project" is a
   * perfectly good thing for a palette row to do.
   */
  | { kind: 'activate'; id: string }
  /** Fewer than two entries: there is nowhere to switch to. */
  | { kind: 'nothing' }

/** What the gate should do with a keystroke that arrived while a walk is open. */
export type Capture =
  /** The walk's own key (with or without Shift) and the hold still down. Swallowed. */
  | { kind: 'advance'; step: 1 | -1; consumed: true }
  /**
   * Close without activating. `consumed` is the difference between Escape — which is ours,
   * and must not also reach the terminal as `^[` — and a stroke that revealed the hold has
   * been lost, which is not ours and has to go on to whatever it was bound to.
   */
  | { kind: 'cancel'; consumed: boolean }
  /** Not the switcher's business. The keymap resolves it as usual and the walk stays open. */
  | { kind: 'ignore'; consumed: false }

/**
 * Move `id` to the front, dropping any earlier occurrence. The whole MRU stack, in one line.
 *
 * Total: an id that is not in `order` is prepended, which is what makes "a project opened"
 * need no separate case.
 */
export function touch(order: readonly string[], id: string): string[] {
  return [id, ...order.filter((other) => other !== id)]
}

/**
 * Reconcile a stored stack against the projects that actually exist now.
 *
 * Known ids keep their remembered order; ids the stack has never seen are appended at the
 * back, *not* the front. Both halves matter and both are the answer to "the stack has to
 * survive a project closing, and a project opening":
 *
 * * A closed project has to leave, or Ctrl+Tab eventually walks onto an id with no header tab
 *   and `project_activate` fails on a project that is gone.
 * * A newly opened project arrives at the back rather than the front because opening it also
 *   *activates* it, and the activation is what promotes it — see `store/workspace.ts`, which
 *   calls [`touch`] with the active id on every snapshot. Appending at the front as well
 *   would be the same promotion written twice, and the second copy would fire for a project
 *   opened in another window that this one never switched to.
 */
export function reconcile(order: readonly string[], live: readonly string[]): string[] {
  const alive = new Set(live)
  const kept = order.filter((id) => alive.has(id))
  const known = new Set(kept)
  return [...kept, ...live.filter((id) => !known.has(id))]
}

/**
 * The stack one window's snapshot implies, or `previous` unchanged when it has no say in it.
 *
 * `strip` is this window's header strip and `active` the project it is showing. The identity of
 * `previous` is returned when nothing moved, so a caller can use `next !== previous` as "this is
 * worth persisting" and a subscriber does not re-render on every snapshot.
 *
 * # A window that cannot walk the stack must not rewrite it
 *
 * The guard on `strip.length` is the whole reason this is a function rather than three lines at
 * the call site, and it is not a degenerate-input check. Two live cases reach here with a strip
 * the switcher can do nothing with:
 *
 * * a **detached tab or pane window**, whose strip is `[]` — see `keys/target.ts`;
 * * **`WindowMode::PerProject`**, where every shell window's strip holds exactly one project.
 *
 * In both, reconciling would read the missing projects as *closed* and answer a stack of one
 * entry or none — and the store persists this answer to a `localStorage` key that every window
 * of the origin shares. Opening a single detached pane would therefore wipe the shell window's
 * remembered order, and the next launch would start Ctrl+Tab from header order: exactly the
 * behaviour the switcher exists to replace, undone by a window that has no Ctrl+Tab at all.
 *
 * Freezing instead is safe in the direction that matters. The stale ids a frozen stack keeps are
 * dropped by [`reconcile`] the moment a window with two projects sees a snapshot, and no walk can
 * start before then — [`begin`] needs two entries as well.
 *
 * # The tab stack uses this too, and the guard means something else there
 *
 * `store/workspace.ts` calls this once per project with that project's tab ids and its
 * `activeTab`, and a reader who has just read the paragraph above will assume the guard is
 * doing the same job. It is not. `active_tab` is *workspace* state, so every window's snapshot
 * carries the same answer for a given project — a detached-pane window and a `PerProject` shell
 * compute an identical stack, and there is no "may this window speak" question and no wipe
 * hazard at all. The guard still earns its place for a plainer reason: a project holding only
 * its pinned console has nothing to walk, and returning `previous` unchanged is how nothing is
 * written back for it.
 */
export function restack(
  previous: readonly string[],
  strip: readonly string[],
  active: string | null,
): readonly string[] {
  if (strip.length < 2) return previous
  const reconciled = reconcile(previous, strip)
  const next = active === null ? reconciled : touch(reconciled, active)
  const unchanged = next.length === previous.length && next.every((id, at) => id === previous[at])
  return unchanged ? previous : next
}

/**
 * The chord that is opening a switcher, or `null` when it holds no modifier.
 *
 * `null` is the palette case and the bare-key-binding case; [`begin`] turns it into an
 * immediate activation. Shift alone is not a hold, so `shift+tab` answers `null` as well —
 * releasing Shift must never commit, so a walk held open by Shift alone could not end.
 */
export function openingOf(stroke: string | null): Opening | null {
  if (stroke === null) return null
  const chord = parseStroke(stroke)
  if (chord === null) return null
  if (!chord.ctrl && !chord.alt && !chord.meta) return null
  return {
    hold: { ctrl: chord.ctrl, alt: chord.alt, meta: chord.meta },
    key: chord.key,
  }
}

/**
 * True while **every** modifier in `hold` is still down.
 *
 * So letting go of any one of them ends the hold. That only differs from "all of them" for a
 * rebound multi-modifier chord like `ctrl+alt+tab`, and it is the right way round there:
 * releasing Ctrl and Alt a beat apart is ordinary, and waiting for the last one would leave the
 * popup up after the user has visibly finished with it.
 */
export function stillHeld(hold: Hold, state: ModifierState): boolean {
  if (hold.ctrl && !state.ctrlKey) return false
  if (hold.alt && !state.altKey) return false
  if (hold.meta && !state.metaKey) return false
  return true
}

/**
 * One keyup, structurally: the modifier flags the event reports, plus the key that came *up*.
 *
 * The two are separate fields for the reason [`endsHold`] exists — on this app's own platform
 * they disagree about the single event the whole gesture is waiting for.
 */
export interface KeyRelease extends ModifierState {
  readonly key: string
}

/**
 * `KeyboardEvent.key` for each modifier a [`Hold`] can name, folded onto the flag it sets.
 *
 * Shift is deliberately absent, as it is from `Hold`: it is the direction, and letting go of it
 * mid-walk must not commit.
 */
const RELEASED_MODIFIER: Readonly<Record<string, keyof Hold>> = {
  Control: 'ctrl',
  Alt: 'alt',
  AltGraph: 'alt',
  Meta: 'meta',
  OS: 'meta',
  Super: 'meta',
  Hyper: 'meta',
}

/**
 * Does this keyup end the hold? The commit condition — and deliberately **not** `!stillHeld`.
 *
 * # WebKitGTK reports the modifier state from *before* the release
 *
 * `!stillHeld(hold, ev)` was the first implementation and on the platform this app ships on it
 * is false for ever, which made the released Ctrl do nothing: the popup stayed up asking for
 * more Tabs, and the only ways out were Escape or clicking away.
 *
 * GDK fills a key event's `state` with the modifier mask as it was *immediately before* the
 * event, and WebKit's GTK/WPE ports hand that mask straight to the DOM for a key **release**.
 * Their `modifiersForKeyboardEvent` compensates for presses only — it was added to fix
 * `ctrlKey` being false in the keydown of Ctrl itself (WebKit r272489 for GTK, r272771 for WPE)
 * and returns early when `!event->pressed`. So the keyup of Ctrl arrives here with
 * `ctrlKey === true` and `stillHeld` answers "still down". Chromium and Firefox do compensate,
 * which is exactly why the naive version passes in a browser and fails in the shipped app.
 *
 * So the name of the key that came up is the primary signal, and the reported flags are only
 * the fallback:
 *
 * * `key` names a modifier this walk is holding → the hold is over, whatever the flags claim;
 * * otherwise the flags decide, which is what still catches a hold released while we were not
 *   listening and revealed by some later, unrelated keyup.
 *
 * The cost is one edge: holding *both* Control keys and letting go of one commits, where a
 * browser would wait for the second. That is the same trade [`stillHeld`] already makes across
 * a multi-modifier hold, and it is the right way round — a popup that outlives the release of
 * the key the user was visibly holding is the bug they can see.
 */
export function endsHold(hold: Hold, ev: KeyRelease): boolean {
  const released = RELEASED_MODIFIER[ev.key]
  if (released !== undefined && hold[released]) return true
  return !stillHeld(hold, ev)
}

/** [`stillHeld`] against a parsed chord rather than an event. */
function chordHolds(hold: Hold, chord: Chord): boolean {
  return stillHeld(hold, { ctrlKey: chord.ctrl, altKey: chord.alt, metaKey: chord.meta })
}

/**
 * Open the switcher, or switch immediately when nothing is being held.
 *
 * `step` is `1` for the forward command and `-1` for the reverse one. The first press already
 * selects something — index `1` forwards, the *last* entry backwards — which is what makes one
 * press-and-release the two-item toggle everybody expects without special-casing it anywhere.
 */
export function begin(order: readonly string[], opening: Opening | null, step: 1 | -1): Begin {
  if (order.length < 2) return { kind: 'nothing' }
  const index = step === 1 ? 1 : order.length - 1
  if (opening === null) {
    // Checked rather than asserted: `order.length >= 2` makes both indices valid, and
    // `noUncheckedIndexedAccess` is on for a reason.
    const id = order[index]
    return id === undefined ? { kind: 'nothing' } : { kind: 'activate', id }
  }
  return { kind: 'walk', walk: { order, index, hold: opening.hold, key: opening.key } }
}

/** One more press of the walk key. Wraps both ways; the list is a ring, as in every switcher. */
export function advance(walk: Walk, step: 1 | -1): Walk {
  const size = walk.order.length
  return { ...walk, index: (walk.index + step + size) % size }
}

/** The id under the highlight. */
export function selection(walk: Walk): string | null {
  return walk.order[walk.index] ?? null
}

/**
 * Release. The one place a new stack is produced.
 *
 * `live` is the list *now* rather than the frozen one, because an entry can disappear while the
 * walk is open — a project closed in another window, a tab closed by a `cide://workspace-changed`
 * that landed mid-gesture. Reconciling here is what stops the commit writing a stack containing
 * an id that no longer exists.
 *
 * `selected` is `null` when the highlighted entry has gone in the meantime. The caller
 * activates nothing and keeps the reconciled order, which is the honest outcome: the user
 * released on a row that is not there any more.
 */
export function commit(
  walk: Walk,
  live: readonly string[],
): { selected: string | null; order: string[] } {
  const order = reconcile(walk.order, live)
  const chosen = selection(walk)
  const selected = chosen !== null && order.includes(chosen) ? chosen : null
  return { selected, order: selected === null ? order : touch(order, selected) }
}

/**
 * What an open walk wants done with one keystroke, before the keymap sees it.
 *
 * This is the stateful claim the gate consults, and its whole vocabulary is the three
 * variants of [`Capture`] — it never names a command and never reads the keymap, so it cannot
 * become a second way to resolve a chord.
 *
 * The order of the rules is the design:
 *
 * 1. **Escape cancels and is swallowed.** Unmodified or not: with Ctrl held, `ctrl+escape`
 *    is what a real keyboard reports, and letting either through to a focused terminal sends
 *    `^[` into the shell.
 * 2. **The walk's own key, with the hold still down, walks**, Shift deciding the direction.
 *    Claimed here rather than left to the keymap on purpose: the switcher owns that key for as
 *    long as it is up, exactly as the Windows one does, so it keeps working when the user has
 *    rebound the opening chord to something else and it cannot be broken by a `when` clause
 *    going false mid-walk.
 *
 *    `walk.key` and not the literal `'tab'`, which is what it used to be. That literal was
 *    harmless while one switcher existed and is not now: during a project walk on `` ctrl+` ``
 *    it would leave the reverse stroke `` ctrl+shift+` `` to the keymap, which resolves it to
 *    `terminal.splitBelow` — so walking backwards would split a terminal.
 * 3. **A stroke without the hold cancels without being consumed.** The user cannot type it
 *    while holding Ctrl, so the modifier is already up and we never saw the keyup — the
 *    alt-tab-away case. Cancelling is right (nothing was released deliberately) and *not*
 *    consuming is right too: the keystroke is a real one the user meant for something else.
 * 4. Anything else with the hold down is not ours. `ctrl+p` while the switcher is open opens
 *    the file picker, and the walk stays where it is.
 */
export function capture(walk: Walk, stroke: string): Capture {
  const chord = parseStroke(stroke)
  if (chord === null) return { kind: 'ignore', consumed: false }

  if (chord.key === 'escape') return { kind: 'cancel', consumed: true }
  if (!chordHolds(walk.hold, chord)) return { kind: 'cancel', consumed: false }
  if (chord.key === walk.key) {
    return { kind: 'advance', step: chord.shift ? -1 : 1, consumed: true }
  }
  return { kind: 'ignore', consumed: false }
}

/**
 * The popup's footer line, as text: which key ends the walk, and which one moves it.
 *
 * Here rather than in the component because it is *derived from the opening chord* and a user
 * can rebind that. The hint used to read "Release Ctrl … Tab / Shift+Tab to move", hard-coded,
 * which was true of the one switcher that existed; with two of them on different keys — and
 * with either rebindable — a hard-coded line is a sentence that tells the user to press
 * something that does nothing. Pure, so `check-switcher.mjs` can hold it to the bindings.
 *
 * The names are the ones printed on a keyboard rather than the canonical tokens: `backquote`
 * is not a thing anybody's key cap says.
 */
export function hintFor(walk: Walk): string {
  const held: string[] = []
  if (walk.hold.ctrl) held.push('Ctrl')
  if (walk.hold.alt) held.push('Alt')
  if (walk.hold.meta) held.push('Meta')
  const key = KEY_CAPS[walk.key] ?? (walk.key.length === 1 ? walk.key : capitalise(walk.key))
  return `Release ${held.join('+')} to switch · ${key} / Shift+${key} to move · Esc cancels`
}

/** Key caps for the walk keys a binding can plausibly name. Anything else prints its token. */
const KEY_CAPS: Readonly<Record<string, string>> = {
  tab: 'Tab',
  backquote: '`',
  space: 'Space',
  right: '→',
  left: '←',
  up: '↑',
  down: '↓',
}

function capitalise(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1)
}
