/**
 * Keystroke spelling — the one place a `KeyboardEvent` becomes a string the keymap can be
 * looked up with.
 *
 * The spelling has to agree byte-for-byte with `cide-core::keymap`, which normalises every
 * binding to `ctrl+alt+shift+meta` modifier order followed by a lowercased key name. If the
 * two disagree the symptom is not an error: a binding simply never fires, which is the
 * exact failure mode `keymap.rs` documents at length. So both sides of the comparison go
 * through [`canonicalKeyName`] here — event-derived names *and* the binding strings that
 * arrive from Rust — rather than trusting that Rust's normalisation and the browser's key
 * names happen to coincide.
 *
 * # Why `code` and not `key`
 *
 * `KeyboardEvent.key` is the *produced character*, so it changes under modifiers: pressing
 * shift and the backtick key reports `~`, which means the shipped default
 * `ctrl+shift+` + backtick (`terminal.splitBelow`) would never match. `code` is the
 * physical key and is stable under shift, so it is the primary source and `key` is only the
 * fallback for codes this table does not name.
 *
 * The cost is layout independence: on a Dvorak or AZERTY layout `KeyP` is not where `p` is
 * printed. VS Code solves that with a per-layout mapping table shipped for every keyboard
 * layout it knows; that is a large amount of data for a problem no one has reported here
 * yet, and the fallback to `key` keeps unnamed keys layout-correct in the meantime. Worth
 * revisiting if a non-US-layout user complains — the fix belongs in this function alone.
 *
 * This module is deliberately free of DOM types, React and `@/` imports so it can be
 * compiled and executed standalone by `ui/scripts/check-key-gate.mjs`.
 */

/** The parts of a `KeyboardEvent` a chord is derived from. `KeyboardEvent` satisfies it. */
export interface KeyStroke {
  readonly ctrlKey: boolean
  readonly altKey: boolean
  readonly shiftKey: boolean
  readonly metaKey: boolean
  readonly key: string
  readonly code: string
}

/**
 * Keys that are only ever modifiers.
 *
 * A keydown for one of these is not a chord — it is the first half of one — and treating it
 * as a chord would make every `ctrl+p` first resolve `ctrl` on its own.
 */
const MODIFIER_KEYS = new Set([
  'Control',
  'Alt',
  'AltGraph',
  'Shift',
  'Meta',
  'OS',
  'CapsLock',
  'NumLock',
  'ScrollLock',
  'Fn',
  'FnLock',
  'Hyper',
  'Super',
  // Composition in progress (fcitx5, ibus). The keystroke belongs to the input method.
  'Dead',
  'Process',
  'Unidentified',
])

/**
 * `KeyboardEvent.code` values with a stable name, for keys whose `code` is not derivable.
 *
 * `KeyA`-`KeyZ`, `Digit0`-`Digit9` and `F1`-`F24` are handled by pattern below rather than
 * listed, because a 62-entry table is 62 chances to mistype one.
 */
const CODE_NAMES: Readonly<Record<string, string>> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
  Comma: 'comma',
  Period: 'period',
  Semicolon: 'semicolon',
  Quote: 'quote',
  BracketLeft: 'bracketleft',
  BracketRight: 'bracketright',
  Backslash: 'backslash',
  Slash: 'slash',
  Minus: 'minus',
  Equal: 'equal',
  Backquote: 'backquote',
  IntlBackslash: 'backslash',
  Escape: 'escape',
  Enter: 'enter',
  NumpadEnter: 'enter',
  Tab: 'tab',
  Space: 'space',
  Backspace: 'backspace',
  Delete: 'delete',
  Insert: 'insert',
  Home: 'home',
  End: 'end',
  PageUp: 'pageup',
  PageDown: 'pagedown',
  NumpadAdd: 'plus',
  NumpadSubtract: 'minus',
  NumpadMultiply: 'multiply',
  NumpadDivide: 'slash',
  NumpadDecimal: 'period',
}

/**
 * Spellings that mean the same key, folded onto one name.
 *
 * Both directions need this. A user writing `ctrl+,` in `keymap.json` means the binding the
 * defaults spell `ctrl+comma`; Rust's `normalize_key` lowercases and orders modifiers but
 * deliberately does not rename keys, so folding them is this side's job — and it must be
 * applied to the binding string as well as to the event, or the two halves land on
 * different names and the binding silently does nothing.
 */
const ALIASES: Readonly<Record<string, string>> = {
  ',': 'comma',
  '.': 'period',
  ';': 'semicolon',
  "'": 'quote',
  '[': 'bracketleft',
  ']': 'bracketright',
  '\\': 'backslash',
  '/': 'slash',
  '-': 'minus',
  '=': 'equal',
  '`': 'backquote',
  '+': 'plus',
  '*': 'multiply',
  ' ': 'space',
  arrowleft: 'left',
  arrowright: 'right',
  arrowup: 'up',
  arrowdown: 'down',
  esc: 'escape',
  del: 'delete',
  ins: 'insert',
  pgup: 'pageup',
  pgdn: 'pagedown',
  pgdown: 'pagedown',
  return: 'enter',
  spacebar: 'space',
  bslash: 'backslash',
  tilde: 'backquote',
  grave: 'backquote',
  // The *shifted* face of the same physical key, and the one a user is most likely to write:
  // the project switcher was asked for as "ctrl+~". `strokeFromEvent` reads `code` first, so no
  // keystroke ever produces the token `~` and a binding spelled with it would be inert —
  // exactly the silent failure this table exists to prevent. Folding it here is what makes the
  // literal spelling mean the chord the user meant. It also means a `keymap.json` cannot
  // distinguish Ctrl+` from Ctrl+Shift+` by writing the tilde; Shift is a modifier here and is
  // written as one.
  '~': 'backquote',
}

/** Fold one key name onto its canonical spelling. Total: unknown names pass through lowercased. */
export function canonicalKeyName(name: string): string {
  const lowered = name.toLowerCase()
  return ALIASES[lowered] ?? lowered
}

/** The canonical name for a physical `code`, or `null` when the table does not cover it. */
function nameFromCode(code: string): string | null {
  const named = CODE_NAMES[code]
  if (named !== undefined) return named

  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase()
  if (/^Digit[0-9]$/.test(code)) return code.slice(5)
  if (/^Numpad[0-9]$/.test(code)) return `numpad${code.slice(6)}`
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code.toLowerCase()
  return null
}

/** True when this keystroke is a modifier being held rather than a chord being pressed. */
export function isModifierStroke(ev: KeyStroke): boolean {
  return MODIFIER_KEYS.has(ev.key)
}

/**
 * The canonical spelling of one keystroke, or `null` for a bare modifier.
 *
 * Never returns a partial chord: a caller can treat `null` as "nothing happened yet".
 */
export function strokeFromEvent(ev: KeyStroke): string | null {
  if (isModifierStroke(ev)) return null
  const name = nameFromCode(ev.code) ?? canonicalKeyName(ev.key)
  if (name === '') return null
  return renderStroke({
    ctrl: ev.ctrlKey,
    alt: ev.altKey,
    shift: ev.shiftKey,
    meta: ev.metaKey,
    key: name,
  })
}

/** One parsed keystroke. Mirrors `cide_core::keymap::Chord`. */
export interface Chord {
  ctrl: boolean
  alt: boolean
  shift: boolean
  meta: boolean
  key: string
}

/** Canonical text for a chord: modifiers in `ctrl alt shift meta` order, then the key. */
export function renderStroke(chord: Chord): string {
  let out = ''
  if (chord.ctrl) out += 'ctrl+'
  if (chord.alt) out += 'alt+'
  if (chord.shift) out += 'shift+'
  if (chord.meta) out += 'meta+'
  return out + chord.key
}

/**
 * Parse one written stroke, e.g. `Ctrl+Shift+P`.
 *
 * Mirrors `parse_stroke` in `keymap.rs`, including its handling of `+` as a bindable key,
 * and returns `null` instead of throwing — this parses data that reached us from a config
 * file, and one bad line must not cost the user the rest of their keymap.
 */
export function parseStroke(text: string): Chord | null {
  const lowered = text.toLowerCase()

  let modifiers: string | null
  let key: string
  if (lowered === '+' || lowered === '++') {
    modifiers = null
    key = '+'
  } else if (lowered.endsWith('++')) {
    modifiers = lowered.slice(0, -2)
    key = '+'
  } else {
    const cut = lowered.lastIndexOf('+')
    if (cut < 0) {
      modifiers = null
      key = lowered
    } else {
      modifiers = lowered.slice(0, cut)
      key = lowered.slice(cut + 1)
    }
  }

  if (key === '') return null

  const chord: Chord = {
    ctrl: false,
    alt: false,
    shift: false,
    meta: false,
    key: canonicalKeyName(key),
  }
  if (modifiers === null) return chord

  for (const modifier of modifiers.split('+')) {
    switch (modifier) {
      case 'ctrl':
      case 'control':
        chord.ctrl = true
        break
      case 'alt':
      case 'option':
        chord.alt = true
        break
      case 'shift':
        chord.shift = true
        break
      case 'meta':
      case 'cmd':
      case 'command':
      case 'super':
      case 'win':
        chord.meta = true
        break
      default:
        return null
    }
  }
  return chord
}

/**
 * Canonical spelling of a whole binding key, which may be a multi-stroke sequence.
 *
 * Unparseable input is folded to a whitespace- and case-normalised copy of itself, exactly
 * as `normalize_key` does in Rust: it compares equal to other spellings of the same broken
 * input and to no real keystroke, so a typo is inert rather than binding something else.
 */
export function normalizeSequence(key: string): string {
  const strokes = key.split(/\s+/).filter((s) => s !== '')
  if (strokes.length === 0) return ''

  const parsed: string[] = []
  for (const stroke of strokes) {
    const chord = parseStroke(stroke)
    if (chord === null) return strokes.join(' ').toLowerCase()
    parsed.push(renderStroke(chord))
  }
  return parsed.join(' ')
}

/** Every proper prefix of a sequence, shortest first. `a b c` → [`a`, `a b`]. */
export function prefixesOf(sequence: string): string[] {
  const strokes = sequence.split(' ')
  const out: string[] = []
  for (let i = 1; i < strokes.length; i++) out.push(strokes.slice(0, i).join(' '))
  return out
}

/**
 * Symbols for the palette's key chip.
 *
 * The mock draws `⌘P`-style chips. On Linux these bind to Ctrl (§2 of the plan), so the
 * transcription keeps the mock's compact symbol form and swaps the modifier: `⌃` control,
 * `⌥` alt, `⇧` shift, `⌘` meta. Spelling them out as `Ctrl+Alt+Right` was the alternative
 * and it loses — the chip is a fixed badge at the right edge of a 620px row, and the longest
 * shipped binding would be more than twice the width of the widest symbol chip.
 */
const MODIFIER_SYMBOLS: readonly (readonly ['ctrl' | 'alt' | 'shift' | 'meta', string])[] = [
  ['ctrl', '⌃'],
  ['alt', '⌥'],
  ['shift', '⇧'],
  ['meta', '⌘'],
]

/** Printable forms for keys whose canonical name is a word. */
const KEY_GLYPHS: Readonly<Record<string, string>> = {
  left: '←',
  right: '→',
  up: '↑',
  down: '↓',
  enter: '⏎',
  escape: 'Esc',
  tab: '⇥',
  space: '␣',
  backspace: '⌫',
  delete: '⌦',
  pageup: 'PgUp',
  pagedown: 'PgDn',
  home: 'Home',
  end: 'End',
  insert: 'Ins',
  comma: ',',
  period: '.',
  semicolon: ';',
  quote: "'",
  bracketleft: '[',
  bracketright: ']',
  backslash: '\\',
  slash: '/',
  minus: '-',
  equal: '=',
  backquote: '`',
  plus: '+',
  multiply: '*',
  // The mouse's thumb buttons, which the keymap carries as pseudo-keys — see
  // `cide_core::keymap::defaults`. Without these the palette's chip would print the raw token
  // `mouseback`, which is a spelling only this codebase uses. `Mouse ←` says what to press.
  mouseback: 'Mouse ←',
  mouseforward: 'Mouse →',
}

/** Render a binding key as the palette's chip text, e.g. `ctrl+shift+p` → `⌃⇧P`. */
export function chipLabel(key: string): string {
  return key
    .split(/\s+/)
    .filter((s) => s !== '')
    .map((stroke) => {
      const chord = parseStroke(stroke)
      if (chord === null) return stroke
      let out = ''
      for (const [flag, symbol] of MODIFIER_SYMBOLS) if (chord[flag]) out += symbol
      const glyph = KEY_GLYPHS[chord.key]
      return out + (glyph ?? (chord.key.length === 1 ? chord.key.toUpperCase() : chord.key))
    })
    .join(' ')
}
