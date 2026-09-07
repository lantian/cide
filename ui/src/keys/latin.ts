/**
 * Non-Latin keyboard layouts: what a chord looks like under one, and the one rewrite that makes
 * every reader of `KeyboardEvent.key` and `keyCode` agree with the key gate.
 *
 * # The three facts, and where they come from
 *
 * Under a Russian layout the physical Z key reports, in WebKitGTK 2.52.3
 * (`Source/WebKit/Shared/gtk/WebKeyboardEventGtk.cpp`):
 *
 * | property  | value  | derived from                                                      |
 * | --------- | ------ | ----------------------------------------------------------------- |
 * | `code`    | `KeyZ` | the hardware keycode (`keyCodeStringForGdkKeycode`) — physical    |
 * | `key`     | `я`    | the keyval's Unicode (`keyValueStringForGdkKeyval`) — the layout  |
 * | `keyCode` | `0`    | `windowsKeyCodeForGdkKeyval`, whose cases are Latin only, `default: return 0` |
 *
 * There is no Latin-group fallback anywhere in WebKitGTK, and `navigator.keyboard.getLayoutMap`
 * does not exist in WebKit. Chromium, by contrast, falls back to the hardware key's US-layout
 * value for `keyCode` when the keyval is not Latin, which is why every web app's Ctrl+Z works
 * under a Russian layout in Chrome and none of them did here.
 *
 * `keys/chords.ts` was already right: `strokeFromEvent` reads `code` first, so every binding in
 * `cide_core::keymap` resolved under Russian before this module existed. What did not were the
 * readers that resolve by the other two properties, and there are three kinds of them:
 *
 * * **xterm** encodes Ctrl+letter and Alt+letter from `keyCode` (`Keyboard.ts`, the `default:`
 *   branch: `keyCode 65..90 → fromCharCode(keyCode - 64)`). With `0` nothing is written, so in a
 *   terminal pane Ctrl+C no longer interrupted and Ctrl+Z no longer suspended — the report.
 * * **CodeMirror** resolves `keyName(event)` — `event.key` — and, for a printable character with
 *   a modifier held, falls back to `base[event.keyCode]` (`@codemirror/view`'s `runHandlers`).
 *   With `я`/`0` nothing matches: undo, select-all, duplicate-line, toggle-comment, find, all
 *   dead in an editor pane. Worse than dead on Hebrew, where `KeyQ` produces `/` with a real
 *   `keyCode 191`, so Ctrl+Q ran *toggle comment*.
 * * **cide's own focus-scoped rules** that match a chord on `ev.key` — the terminal's Ctrl+C /
 *   Ctrl+V / Ctrl+F (`terminal/keys.ts`), the file tree's clipboard chords and Ctrl+R, the git
 *   log's Ctrl+D, the merge pane's undo. Each read `с` where it wanted `c`.
 *
 * # The rewrite
 *
 * [`latiniseKeyEvent`] rewrites a **chord keydown** in place to what a US layout would have
 * reported: `key`, `keyCode` and `which` become own data properties on the event, shadowing the
 * prototype getters, and `code` is never touched. It runs once, from both keyboard entry points
 * of the gate (`keys/gate.ts`), which is upstream of every reader above: the window capture
 * listener fires before xterm's textarea listener, before CodeMirror's `contentDOM` handler and
 * before React's root listener copies `nativeEvent.key` into its synthetic event.
 *
 * In place rather than a synthetic event, for two reasons. The browser's default action — native
 * undo in a plain `<input>`, which WebKit decides in the UI process from the GDK event before
 * JavaScript ever runs — rides the *original* event, and a cancelled original plus a synthetic
 * twin would lose it. And the gate memoises its decision against the event object, so one object
 * must remain one object.
 *
 * Both `key` and `keyCode`, because the readers split: xterm reads only `keyCode`, cide's rules
 * read only `key`, CodeMirror reads both — and on a US layout it needs both, since
 * `Ctrl-Shift-z` is registered lowercase and `event.key` is `Z`, so the `base[keyCode]` fallback
 * is what makes every shifted letter chord resolve there. A rewrite that set one and not the
 * other would fix half the surfaces.
 *
 * # What is, and is not, rewritten
 *
 * Only a keystroke that is a *chord* — Ctrl, Meta, or (on Linux) Alt held. An unmodified key, or
 * Shift alone, is text and keeps the character the layout produced: the sidebar's type-ahead,
 * the terminal's textarea path and every text field read `key` for typing. AltGr sets none of the
 * three flags under GDK (ISO_Level3_Shift is Mod5; WebKit maps only Mod1 to `altKey`), so `€`,
 * `@` and `ą` typed through it are never touched. Composition (`isComposing`, `keyCode 229`,
 * `Process`, `Dead`) belongs to the input method — the same exclusions as
 * `terminal/inputRouting.ts`'s `imeFiltered`.
 *
 * Two rungs decide whether a chord's `key` is the layout's character rather than the US one:
 *
 * 1. **Stateless:** `key` is a single non-ASCII code point. Every letter and most punctuation on a
 *    Cyrillic, Greek, Hebrew or Arabic layout. Also German Ctrl+ü on `BracketLeft` → `[`/219,
 *    deliberately: Chromium's hardware fallback gives 219 there too, and CodeMirror's `base[219]`
 *    lands on `Ctrl-[` in Chrome as well.
 * 2. **Latched:** `key` is ASCII but not the US face of that physical key, and the active layout
 *    is known to be non-Latin. Russian's physical `/` produces `.`, so Ctrl+/ (toggle comment)
 *    would be unreachable on rung 1 alone; Shift+digits differ (`"№;:?` against `@#$^&`); Hebrew
 *    and Greek put `/`, `'` and `;` on letter keys, which is the wrong-command case above. The
 *    latch is what keeps Dvorak, QWERTZ and AZERTY untouched — Latin layouts, where `key` is the
 *    right answer and the printed letter is the one the user means.
 *
 * The latch is learned from typing ([`learnsLayout`]): a keydown whose `key` is a letter outside
 * the Latin script sets it, an ASCII letter clears it, anything else leaves it alone — so Turkish
 * `ı` and `ş` (Latin script) cannot flip it, and Hebrew's `/` on a letter key cannot clear it.
 * Its known failure is a stale `true` between switching from a non-Latin layout to a Latin non-US
 * one and the next Latin letter typed: for that window a chord on a punctuation key is spelled
 * the US way. Transient, and cheaper than the alternative, which is VS Code's per-layout table
 * set. One latch per window, because one module instance per webview.
 *
 * # One behaviour change, named
 *
 * Alt+letter in a terminal under Russian used to *type the Cyrillic letter* (xterm encoded
 * nothing for `keyCode 0` and the textarea path delivered the character). It now sends
 * `ESC` + the Latin letter, which is what Chrome, VS Code and IDEA do and what readline's
 * Meta-bindings want. Alt+letter with no chord semantics on the layout in question was never a
 * way to type that letter without Alt.
 *
 * # Why the module is shaped like this
 *
 * Import-free and DOM-type-free, like `chords.ts`, so `ui/scripts/check-latin.mjs` can compile it
 * standalone and drive every rule with plain objects, and `check-key-gate.mjs` can sweep it
 * through both entry points beside the gate. The decisions ([`latinRewrite`], [`learnsLayout`])
 * are pure and take the latch as an argument; only [`latiniseKeyEvent`] holds state.
 */

/** The parts of a `KeyboardEvent` the rewrite is decided from. `KeyboardEvent` satisfies it. */
export interface LatinFacts {
  /** Absent in a hand-built fixture, which is treated as a keydown — as `gate.ts` treats it. */
  readonly type?: string | undefined
  readonly key: string
  readonly code: string
  /** Absent in a fixture; `229` is the input method's placeholder. */
  readonly keyCode?: number | undefined
  readonly isComposing?: boolean | undefined
  readonly ctrlKey: boolean
  readonly altKey: boolean
  readonly shiftKey: boolean
  readonly metaKey: boolean
}

/** What a physical key reports on a US layout: its Windows virtual-key code and both faces. */
export interface LatinFace {
  readonly keyCode: number
  readonly base: string
  readonly shift: string
}

/** The values the rewrite installs on the event. */
export interface LatinRewrite {
  readonly key: string
  readonly keyCode: number
}

/**
 * The eleven punctuation keys, with the virtual-key codes xterm and w3c-keyname agree on.
 *
 * Literal because none of them is derivable from its `code`. `KeyA`–`KeyZ` and `Digit0`–`Digit9`
 * are generated below rather than listed, for the reason `chords.ts` gives: a 36-entry table is
 * 36 chances to mistype one. `check-latin.mjs` holds every entry — generated and literal — to
 * w3c-keyname's `base`/`shift` tables, which are what CodeMirror's fallback consults.
 */
const PUNCTUATION: readonly (readonly [code: string, keyCode: number, base: string, shift: string])[] = [
  ['Backquote', 192, '`', '~'],
  ['Minus', 189, '-', '_'],
  ['Equal', 187, '=', '+'],
  ['BracketLeft', 219, '[', '{'],
  ['BracketRight', 221, ']', '}'],
  ['Backslash', 220, '\\', '|'],
  ['Semicolon', 186, ';', ':'],
  ['Quote', 222, "'", '"'],
  ['Comma', 188, ',', '<'],
  ['Period', 190, '.', '>'],
  ['Slash', 191, '/', '?'],
]

/** The shifted face of `Digit0`…`Digit9`, in that order. */
const SHIFTED_DIGITS = ')!@#$%^&*('

function buildFaces(): Record<string, LatinFace> {
  const faces: Record<string, LatinFace> = {}
  for (let i = 0; i < 26; i++) {
    const upper = String.fromCharCode(65 + i)
    faces[`Key${upper}`] = { keyCode: 65 + i, base: upper.toLowerCase(), shift: upper }
  }
  for (let digit = 0; digit < 10; digit++) {
    faces[`Digit${digit}`] = {
      keyCode: 48 + digit,
      base: String(digit),
      shift: SHIFTED_DIGITS.charAt(digit),
    }
  }
  for (const [code, keyCode, base, shift] of PUNCTUATION) faces[code] = { keyCode, base, shift }
  return faces
}

/**
 * Every `code` the rewrite knows, and what a US layout reports for it.
 *
 * Exactly the codes `chords.ts`'s `nameFromCode` names — which is what makes the gate's verdict
 * invariant under the rewrite: for these codes `strokeFromEvent` never consults `key`, so an
 * event that arrives as `я` and leaves as `z` resolves to the same chord either way.
 * `check-key-gate.mjs` measures that for every code and every modifier set.
 */
export const US_FACES: Readonly<Record<string, LatinFace>> = buildFaces()

/** The keys of [`US_FACES`], for a check that wants to walk them. */
export const LATIN_CODES: readonly string[] = Object.keys(US_FACES)

/** What a US layout reports for this physical key with Shift held or not, or `null`. */
export function usFace(code: string, shift: boolean): LatinRewrite | null {
  const face = US_FACES[code]
  if (face === undefined) return null
  return { key: shift ? face.shift : face.base, keyCode: face.keyCode }
}

/**
 * The code point of a `key` that is exactly one character, or `null`.
 *
 * "One code point" rather than `length === 1`, so an astral character is one character and a
 * key *name* (`Process`, `Dead`, `Control`, `ArrowLeft`) is never one. That single test is what
 * excludes every modifier and every composition placeholder without a list to maintain.
 */
function singleCodePoint(key: string): number | null {
  const point = key.codePointAt(0)
  if (point === undefined) return null
  return String.fromCodePoint(point).length === key.length ? point : null
}

/** A keydown that is not the input method's, with a `key` that is one character. */
function chordCandidate(facts: LatinFacts): number | null {
  if (facts.type !== undefined && facts.type !== 'keydown') return null
  if (facts.isComposing === true || facts.keyCode === 229) return null
  return singleCodePoint(facts.key)
}

/**
 * What to install on this event, or `null` to leave it exactly as the browser built it.
 *
 * `nonLatinLayout` is the latch, passed in so the decision is pure. `altIsChord` is whether Alt
 * on its own makes a chord: true on Linux and Windows, false on macOS, where Option is a level-3
 * shift and `⌥o` is how `ø` is typed — CodeMirror carries the same guard, for the same reason.
 * A parameter rather than a `navigator` sniff so the module stays free of the DOM and the check
 * can drive both answers.
 */
export function latinRewrite(
  facts: LatinFacts,
  nonLatinLayout: boolean,
  altIsChord = true,
): LatinRewrite | null {
  const point = chordCandidate(facts)
  if (point === null) return null

  const chord = facts.ctrlKey || facts.metaKey || (altIsChord && facts.altKey)
  if (!chord) return null

  const face = usFace(facts.code, facts.shiftKey)
  if (face === null) return null

  // Rung 1: the layout produced something outside ASCII. Not a Latin layout's answer.
  if (point > 0x7f) return face
  // Rung 2: ASCII, but not this key's US face — Russian's `.` on the `/` key, Hebrew's `/` on
  // Q. Only while typing has shown the layout to be non-Latin, or Dvorak's `;` on Z would be
  // rewritten too.
  if (nonLatinLayout && facts.key !== face.key) return face
  return null
}

/** A letter of any script. One code point, `u` so an astral letter is one match. */
const LETTER = /^\p{L}$/u
/** The Latin script, which includes `ı`, `ş`, `ü`, `ø` — every letter a Latin layout can type. */
const LATIN_LETTER = /^\p{Script=Latin}$/u
const ASCII_LETTER = /^[A-Za-z]$/

/**
 * What this keydown says about the active layout: `true` non-Latin, `false` Latin, `null` nothing.
 *
 * Evidence is a *letter*: one outside the Latin script sets the latch, an ASCII one clears it.
 * Punctuation, digits, symbols and non-ASCII Latin letters say nothing, so a Turkish `ı`, a
 * Hebrew `/` on a letter key, `№` on Shift+3 and `€` through AltGr all leave the latch where it
 * was. Every keydown is evidence, chorded or not — Ctrl+Z on `я` sets it as surely as typing
 * does — because the chord that needs the latch is often the first thing pressed after a
 * layout switch.
 */
export function learnsLayout(facts: LatinFacts): boolean | null {
  if (chordCandidate(facts) === null) return null
  if (ASCII_LETTER.test(facts.key)) return false
  if (LETTER.test(facts.key) && !LATIN_LETTER.test(facts.key)) return true
  return null
}

/*
 * The latch. Module scope because there is one keyboard per window and one module instance per
 * webview; exported accessors rather than the variable so a check can reset it between sections
 * and assert its state before a sweep.
 */
let nonLatinLayout = false

/** Whether the last letter typed in this window came from a non-Latin layout. */
export function latinLayoutIsNonLatin(): boolean {
  return nonLatinLayout
}

/** Forget what has been learned. For checks; nothing in the app calls it. */
export function resetLatinLayout(): void {
  nonLatinLayout = false
}

/**
 * Install `key`, `keyCode` and `which` on the event, shadowing the prototype's getters.
 *
 * All four descriptor attributes stated, so a property this creates (`keyCode` on a fixture,
 * `key` on a real event) and one it redefines (`key` on a fixture, an own writable) end up
 * indistinguishable, and a later test can overwrite either. The read-back is the contract:
 * a frozen object throws, a sealed one silently keeps the old value, and either way the caller
 * must not believe a rewrite happened that did not.
 */
function shadow(target: LatinFacts, rewrite: LatinRewrite): boolean {
  const entries: readonly (readonly [name: string, value: string | number])[] = [
    ['key', rewrite.key],
    ['keyCode', rewrite.keyCode],
    ['which', rewrite.keyCode],
  ]
  try {
    for (const [name, value] of entries) {
      Object.defineProperty(target, name, {
        value,
        writable: true,
        enumerable: true,
        configurable: true,
      })
    }
  } catch {
    return false
  }
  return target.key === rewrite.key && target.keyCode === rewrite.keyCode
}

/*
 * Decisions, memoised against the event object.
 *
 * Both entry points of the gate see the *same* event for a keystroke typed into a terminal, so
 * the second call must neither rewrite twice nor — the trap — learn from its own rewrite: after
 * `я` has become `z`, a second pass reads an ASCII letter and clears the latch, and a terminal
 * pane un-learns the layout on every chord. "After a rewrite the key equals its face" made the
 * rewrite idempotent on its own; it is the learning that needs the memo. A `WeakMap`, as the
 * gate's own memo is, so nothing is held past the event's life.
 */
const decided = new WeakMap<object, LatinRewrite | null>()

/**
 * The entry point: learn from this keydown, then rewrite it if it is a chord the layout spelled
 * its own way. Returns what was installed — or `null` when the event was left alone, including
 * when a rewrite was attempted and did not take.
 *
 * Learn first, from the event as the browser built it — the rewrite would erase the evidence.
 * A second call for the same event returns the first answer and changes nothing.
 */
export function latiniseKeyEvent(ev: LatinFacts): LatinRewrite | null {
  const cached = decided.get(ev)
  if (cached !== undefined) return cached

  const learned = learnsLayout(ev)
  if (learned !== null) nonLatinLayout = learned

  const rewrite = latinRewrite(ev, nonLatinLayout)
  const applied = rewrite !== null && shadow(ev, rewrite) ? rewrite : null
  decided.set(ev, applied)
  return applied
}
