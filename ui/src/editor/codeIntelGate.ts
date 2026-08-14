/**
 * The decisions behind Ctrl+hover and Ctrl+click, with nothing around them.
 *
 * # Why this file exists at all
 *
 * The two gestures are **one gesture seen twice**: the underline that appears under Ctrl is a
 * promise about what the click will do, and an underline that promises the wrong thing is worse
 * than no underline — it teaches the user to distrust the affordance and then to ignore it. So the
 * agreement between them is not a convention spread over a `mousemove` handler and a `mousedown`
 * handler; it is [`intent`], one function, called by both, over one cached answer.
 *
 * And it is *here*, import-free, because the two shipped bugs this project keeps re-paying for both
 * lived in exactly the place a rule cannot be compiled: a React hook and an event handler.
 * `ui/scripts/check-editor.mjs` compiles this module on its own and drives every function in it —
 * the ladder, the cache, the LRU, the backoff and the modifier-release rule — which is possible
 * only while it imports nothing. Adding a value import (CodeMirror, the IPC client, a store)
 * silently takes that away.
 *
 * # The cost problem, which is the design
 *
 * A `mousemove → invoke` is worse here than the generic "one request per frame" argument:
 *
 * * `diagnostics_probe` is `spawn_blocking`, so one tokio blocking thread is parked per in-flight
 *   hover;
 * * the outbound queue to a language server is `bounded(256)` and its comment says a `didChange`
 *   per debounce interval "is a trickle". A hover flood breaks that assumption, and the failure is
 *   not a slow underline: `LspHandle::send` **drops** notifications when the outbox is full, and a
 *   dropped `didChange` desyncs rust-analyzer's copy of the buffer permanently.
 *
 * So the budget is not politeness. It is: **the hover must not be able to starve document sync.**
 * Hence a ladder of six gates, of which only the last costs a round trip, and every earlier one is
 * a comparison in the webview:
 *
 * | # | gate | rejects |
 * | - | ---- | ------- |
 * | 0 | Ctrl is held, read off the move event itself | ~100% of pointer motion |
 * | 1 | the pointer is over a rendered glyph ([`onGlyph`]) | margins, past-EOL, the gutter |
 * | 2 | it is a word (CodeMirror's `wordAt`) | whitespace, punctuation runs |
 * | 3 | the token is identifier-shaped ([`askableToken`]) | keywords, comments, strings, numbers |
 * | 4 | not already answered ([`recall`], keyed on the **word range**) | re-entry, re-crossing |
 * | 5 | the pointer has settled ([`SETTLE_MS`]) | the whole of a drag across a line |
 * | 6 | ask | — |
 */

/**
 * What the server said about the position under the pointer.
 *
 * Mirrors `cide_ipc::ProbeAnswer`'s tags. Spelled out here rather than imported from
 * `ipc/generated.ts` because this module has to compile standalone — and because the spelling is
 * pinned by `check:editor` against the generated file, so the copy cannot drift silently.
 */
export type ProbeKind = 'definition' | 'declaration' | 'notFound' | 'unavailable'

/**
 * What a Ctrl+click would do. **The** answer to that question — the hover and the click both read
 * this and nothing else.
 *
 * `'jump'` — the caret is on a reference, so go to its declaration.
 * `'usages'` — the caret is on the declaration itself, so list the usages.
 * `'none'` — nothing actionable: the click reports one sentence, the hover draws nothing.
 *
 * Note what is deliberately *not* here: a fourth outcome for "unavailable" that draws a different
 * kind of underline. A server that cannot be asked is a server whose answer we do not have, and
 * drawing anything at all would be inventing one.
 */
export type Intent = 'jump' | 'usages' | 'none'

export function intent(kind: ProbeKind): Intent {
  switch (kind) {
    case 'definition':
      return 'jump'
    case 'declaration':
      return 'usages'
    default:
      return 'none'
  }
}

/**
 * Does this answer get an underline?
 *
 * Derived from [`intent`] rather than listed separately, and that is the whole guarantee: there is
 * no way to make the underline appear for a kind the click will not act on, because the two read
 * the same function. A separate list of "underlinable kinds" is exactly the second source of truth
 * that would drift.
 */
export function underlines(kind: ProbeKind): boolean {
  return intent(kind) !== 'none'
}

// --- gate 3: is this even an identifier? ------------------------------------------------------

/**
 * Token names the local tree-sitter/stream grammar emits that are **certainly not** identifiers.
 *
 * Gate 3, and the reason a hover over a page of Rust costs almost nothing: comments, string bodies,
 * keywords and punctuation are most of what a pointer crosses, and none of them can resolve.
 *
 * A reject-list and not an allow-list, deliberately. A grammar that gains a new token name must
 * fail *open* — asking about something unresolvable costs one round trip that is then cached,
 * while silently refusing to ask about a new kind of identifier is a feature that quietly stops
 * working with nothing to notice it. `check:editor` asserts that every name the grammars can emit
 * appears in one of these two sets, so "new" still has to be a deliberate edit.
 */
export const REJECTED_TOKENS: readonly string[] = [
  'comment',
  'string',
  'number',
  'keyword',
  'atom',
  'operator',
  'controlOperator',
  'bracket',
  'punctuation',
  'meta',
  'heading',
  'strong',
  'emphasis',
  'quote',
  'link',
  'monospace',
]

/**
 * And the names that are worth asking about.
 *
 * `labelName` is in here rather than in the reject list even though its only producers are a YAML
 * anchor and a loop label, neither of which any shipped server resolves: the list is "shaped like
 * an identifier", not "known to resolve". Asking and being told *no* is one cached round trip;
 * refusing to ask is a rule that has to be revisited the day a third language server appears.
 */
export const ASKED_TOKENS: readonly string[] = [
  'variableName',
  'variableName.function',
  'typeName',
  'propertyName',
  'macroName',
  'labelName',
]

const REJECTED = new Set(REJECTED_TOKENS)

/**
 * Gate 3. `null` means "no syntax tree here", which **must** fail open.
 *
 * Two real cases produce `null`, and refusing both would turn the feature off exactly where it is
 * most wanted: a buffer past `HIGHLIGHT_LIMIT_BYTES` loads no language at all, and `StreamLanguage`
 * parses lazily so the tree may not have reached the pointer yet. Neither is evidence that the
 * thing under the pointer is punctuation.
 *
 * Sub-tags are folded onto their base (`variableName.function` → also `variableName`), so a
 * grammar that starts qualifying a name it used to emit bare does not silently change the answer.
 */
export function askableToken(name: string | null): boolean {
  if (name === null) return true
  if (REJECTED.has(name)) return false
  const base = name.split('.')[0]
  return base === undefined || !REJECTED.has(base)
}

// --- gate 1: is the pointer actually over a glyph? --------------------------------------------

/** A rectangle, as `coordsAtPos` hands one back. */
export interface Rect {
  readonly left: number
  readonly right: number
  readonly top: number
  readonly bottom: number
}

/**
 * Gate 1, and it has to exist.
 *
 * `posAtCoords` in its precise mode still answers with the **line-end position** for a pointer
 * parked in the empty space to the right of a line, so without this, hovering blank space
 * underlines the last word of the line — a promise the click would then keep, by jumping somewhere
 * the user was not pointing at.
 *
 * The tolerance is one character width on each side horizontally and nothing vertically, which is
 * CodeMirror's own `HoverPlugin.startHover` rule. Wider is a mark that lights up before the pointer
 * reaches the word; narrower flickers as the pointer crosses a glyph boundary.
 */
export function onGlyph(x: number, y: number, rect: Rect, charWidth: number): boolean {
  if (y < rect.top || y > rect.bottom) return false
  return x >= rect.left - charWidth && x <= rect.right + charWidth
}

// --- gate 5, and the deadlines ----------------------------------------------------------------

/**
 * How long the pointer has to stand still before anything is asked.
 *
 * A **trailing debounce, not a throttle** — the opposite of `docSync.ts`'s choice, and correctly
 * so: starvation under continuous motion is the *desired* behaviour here, because there is nothing
 * worth showing while the pointer is moving.
 *
 * 150 ms rather than the 300 that `docSync`, `outlineStore` and CodeMirror's own `hoverTooltip`
 * use. Those three are following the user; this one is being *aimed with*, and 300 ms of nothing
 * after you have already stopped moving reads as the feature not working. It still collapses a
 * whole drag across a line into a single request.
 */
export const SETTLE_MS = 150

/**
 * The deadline a hover asks with.
 *
 * Five seconds is right behind a keystroke — `DEFINITION_TIMEOUT` argues it — and wrong behind a
 * pointer twice over: nobody wants an underline that arrives five seconds after they stopped
 * moving, and a blocking-pool thread is held for the whole of it. If a server cannot resolve a
 * definition in 600 ms it is indexing, and the honest answer is no underline.
 */
export const HOVER_TIMEOUT_MS = 600

/**
 * And the deadline a click asks with: the same one Go to definition has always used.
 *
 * Deliberately *not* shortened to match the hover. A click is a committed gesture with a visible
 * outcome, and telling a user who clicked "the server did not answer in 600 ms" when it would have
 * answered in 900 is a worse trade than making them wait.
 */
export const CLICK_TIMEOUT_MS = 5000

/**
 * How long an `unavailable` answer suppresses re-asking.
 *
 * `unavailable` is three very different situations behind one sentence — no server for this file
 * type (permanent), the server is not running (until a restart), the server did not answer in time
 * (transient, and *expected* for the first minute). Caching it as a definite "no" would mean a
 * `.md` buffer never underlining anything again after one hover; not caching it at all means every
 * settle during indexing burns another blocking thread. A short expiry is the honest middle: stay
 * quiet for ten seconds, then try again.
 */
export const UNKNOWN_BACKOFF_MS = 10_000

/**
 * How long a *definite* answer — a resolved declaration, or a confident "nothing here" — may be
 * reused before it is asked again.
 *
 * Definite answers used to be cached for ever, and the cache key carries only the *hovering*
 * file's generation. So an edit to the file the answer POINTS AT invalidated nothing: insert
 * thirty lines above a declaration in `util.rs`, go back to `main.rs`, and Ctrl+click kept
 * navigating to the old line without ever re-asking. The underline agreed with it, so there was
 * nothing on screen to suggest the answer was stale.
 *
 * Invalidating by target would mean a reverse index from every cached answer to the file it names,
 * maintained on every edit — real machinery for a case an expiry already bounds. Thirty seconds is
 * long enough that a drag across a line costs one request per identifier and not one per pass, and
 * short enough that a stale jump is a thing that happened once rather than a thing that persists
 * for the life of the window.
 *
 * It does NOT make the answer correct within the window. A jump inside thirty seconds of an edit
 * elsewhere can still land a few lines off, which is the same failure every editor with a
 * resolution cache has; what it removes is the unbounded version.
 */
export const DEFINITE_TTL_MS = 30_000

// --- gate 4: the cache ------------------------------------------------------------------------

/** Where a definition lives, in the units `jumpTo` takes. */
export interface Target {
  readonly path: string
  readonly line: number
  readonly column: number
}

/** One remembered answer. */
export interface Resolution {
  readonly kind: ProbeKind
  /** Set for `definition`, and read by the click's jump. */
  readonly target?: Target | undefined
  /** Set for `unavailable`: the server's own sentence, shown verbatim by the click. */
  readonly reason?: string | undefined
  /** The deadline this answer was produced under. See [`usable`]. */
  readonly askedMs: number
  /** When it landed, on whatever clock the caller passes to [`usable`]. */
  readonly at: number
}

/**
 * The cache key: **path, document generation, and the word range**.
 *
 * The word range and not the pointer position, which is the single biggest saving in the whole
 * ladder: crossing a fourteen-character identifier is one entry rather than fourteen, and
 * re-entering it later at a different pixel is free. Keying on `(line, column)` — the obvious
 * choice — multiplies the cache by the identifier's length and misses on every re-entry.
 *
 * It is also what makes the click and the hover share an answer at all: both normalise the pointer
 * to the same word range, so both compute the same key.
 *
 * `docGen` is a counter the caller bumps on every document change. Any edit invalidates every
 * answer for that file — an added `use`, a renamed local, an agent rewriting the buffer under the
 * user — so ranges are deliberately **not** mapped through the change set: an answer is invalidated
 * by an edit anywhere in the file, not only by one inside the word.
 *
 * The separator is [`KEY_SEP`], which is the one byte a path cannot contain.
 */
export const KEY_SEP = '\u0000'

/**
 * Everything remembered about one file, as a key prefix.
 *
 * Exported and paired with [`keyFor`] deliberately: `forgetCodeIntel` drops a file's answers by
 * prefix, and the first version of it built the prefix by hand with a *space* while `keyFor` used
 * a NUL. It matched nothing, so unmounting a buffer forgot nothing — a leak that is invisible on
 * screen and that no test of either function on its own could see. `check:editor` now asserts the
 * two agree.
 */
export function keyPrefix(path: string): string {
  return `${path}${KEY_SEP}`
}

export function keyFor(path: string, docGen: number, from: number, to: number): string {
  return `${path}${KEY_SEP}${docGen}${KEY_SEP}${from}-${to}`
}

/**
 * How many answers are kept. A few hundred covers every identifier in a file the user is reading;
 * the bound is what stops a long session accumulating one entry per word per edit.
 */
export const CACHE_MAX = 512

/**
 * Remember an answer, evicting the least recently used.
 *
 * **Delete before set**, so `Map`'s insertion order is a real LRU rather than a first-seen order —
 * the same shape `terminal/pathLinks.ts::remember` uses and for the same reason. Mutates in place
 * and returns nothing: the caller owns exactly one of these per module, and returning a new map
 * would invite somebody to keep the old one.
 */
export function remember(cache: Map<string, Resolution>, key: string, value: Resolution): void {
  cache.delete(key)
  cache.set(key, value)
  while (cache.size > CACHE_MAX) {
    const oldest = cache.keys().next()
    if (oldest.done === true) break
    cache.delete(oldest.value)
  }
}

/** Read an answer and mark it as freshly used, so an identifier the user keeps returning to stays. */
export function recall(cache: Map<string, Resolution>, key: string): Resolution | undefined {
  const hit = cache.get(key)
  if (hit === undefined) return undefined
  cache.delete(key)
  cache.set(key, hit)
  return hit
}

/**
 * May this cached answer be used by a request that would wait `wantMs`?
 *
 * **The invariant the whole feature rests on**, stated as a function so it can be driven:
 *
 * * a *definite* answer — `definition`, `declaration`, `notFound` — is usable by anybody. It cannot
 *   go stale except through an edit, and an edit changes the key.
 * * an `unavailable` is usable only by a request that would not have waited any longer than the one
 *   that produced it, and only inside the backoff window.
 *
 * The consequence, and it is the one to state out loud: **the hover can never underline something
 * the click will not act on, and the click may act where the hover stayed quiet.** A hover's
 * 600 ms `unavailable` is not an answer for a 5 s click, so the click re-asks properly rather than
 * inheriting a shrug. Wrong in the safe direction: the affordance under-promises, never over.
 */
export function usable(entry: Resolution, wantMs: number, now: number): boolean {
  if (entry.kind !== 'unavailable') return now - entry.at < DEFINITE_TTL_MS
  if (entry.askedMs < wantMs) return false
  return now - entry.at < UNKNOWN_BACKOFF_MS
}

// --- the ladder itself ------------------------------------------------------------------------

/**
 * What the hover should do about one pointer sample.
 *
 * * `clear` — nothing under the pointer. Take the mark down.
 * * `keep` — already drawn for exactly this range. The common case *within* one identifier: no
 *   dispatch, no timer, no request.
 * * `draw` / `hide` — the answer is already cached. Gate 4, and the reason re-entering an
 *   identifier is instant.
 * * `wait` — nothing is known. Restart the settle timer; a request happens **only if it fires**.
 *
 * Extracted from the plugin so the cost claim can be *measured* rather than asserted:
 * `check:editor` replays a synthetic drag through this function with a clock and counts the
 * requests it would produce. That number is the whole justification for the design, and a
 * justification that lives inside a `mousemove` handler is one nothing can check.
 */
export type HoverStep = 'clear' | 'keep' | 'draw' | 'hide' | 'wait'

export function hoverPlan(
  word: { readonly from: number; readonly to: number } | null,
  shown: { readonly from: number; readonly to: number } | null,
  cached: Resolution | undefined,
): HoverStep {
  if (word === null) return 'clear'
  if (shown !== null && shown.from === word.from && shown.to === word.to) return 'keep'
  if (cached !== undefined) return underlines(cached.kind) ? 'draw' : 'hide'
  return 'wait'
}

// --- releasing the modifier -------------------------------------------------------------------

/** The half of a keyboard event this module reads. */
export interface ModifierKey {
  readonly key: string
  readonly ctrlKey: boolean
  readonly metaKey: boolean
}

/** Is a Ctrl-family modifier held? `metaKey` too, because ⌘-click is the macOS equivalent. */
export function holdsCtrl(ev: { ctrlKey: boolean; metaKey: boolean }): boolean {
  return ev.ctrlKey || ev.metaKey
}

/**
 * Did this `keyup` end the Ctrl hold?
 *
 * **The key identity is checked first, and `!ctrlKey` only as a fallback.** WebKitGTK reports the
 * modifier mask from *before* the release, so on the Ctrl keyup itself `ev.ctrlKey` is still
 * `true` — a `!ev.ctrlKey` test alone would never fire on the platform this ships on, and the
 * underline would stay on screen until the pointer moved. That is written out at length in
 * `keys/switcher.ts`, which pays for the same engine behaviour, and CodeMirror's own
 * `crosshairCursor` guards the same way.
 *
 * The fallback still earns its place: a Ctrl+Alt chord released Alt-first delivers a keyup for
 * `Alt` whose mask no longer has Ctrl in it, and that is a genuine end of the hold.
 */
export function endsCtrlHold(ev: ModifierKey): boolean {
  if (ev.key === 'Control' || ev.key === 'Meta') return true
  return !holdsCtrl(ev)
}
