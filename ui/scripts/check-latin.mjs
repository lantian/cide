/**
 * Checks `src/keys/latin.ts` — the rewrite that makes a chord under a non-Latin layout read as
 * it would on a US one, so xterm, CodeMirror and every `ev.key` matcher in this app agree with
 * the key gate.
 *
 * The facts this rests on are WebKitGTK's and cannot be produced here: under a Russian layout
 * the Z key reports `code KeyZ`, `key я`, `keyCode 0`. So the script works from the layout side
 * instead — the whole Russian layout, every key, both Shift faces — and asserts what each chord
 * must be rewritten to, plus the four scripts that behave the same way and the two Latin ones
 * that must be left alone.
 *
 * Two of the assertions are differential rather than hand-written, and those are the ones that
 * matter most:
 *
 * * **The face table is held to w3c-keyname's `base` and `shift` tables**, resolved out of
 *   `@codemirror/view`'s own dependency tree. Those are the tables CodeMirror's `runHandlers`
 *   consults through `base[event.keyCode]`, so a virtual-key code that disagrees with them is a
 *   chord CodeMirror resolves to the wrong key — with no error, in an editor that looks fine.
 * * **Every letter's code satisfies xterm's control-byte arithmetic**, `fromCharCode(keyCode -
 *   64)`, and the three bracket codes are the three xterm special-cases — because that branch is
 *   the whole reason Ctrl+C stopped interrupting.
 *
 * Compiled standalone with the TypeScript already in `node_modules`, the same shape as
 * `check-terminal-keys.mjs`: the module imports nothing, deliberately, so that this can.
 *
 * Run: `pnpm --dir ui run check:latin`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, realpathSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-latin-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => eq(cond, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/keys/latin.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    US_FACES,
    LATIN_CODES,
    usFace,
    latinRewrite,
    learnsLayout,
    latiniseKeyEvent,
    latinLayoutIsNonLatin,
    resetLatinLayout,
  } = await import(`file://${join(out, 'latin.js')}`)

  /** A `LatinFacts`, with every modifier off unless named and no `type` unless given. */
  const ev = (spec) => ({
    ...(spec.type === undefined ? {} : { type: spec.type }),
    key: spec.key,
    code: spec.code,
    ...(spec.keyCode === undefined ? {} : { keyCode: spec.keyCode }),
    ...(spec.composing === undefined ? {} : { isComposing: spec.composing }),
    ctrlKey: spec.ctrl === true,
    altKey: spec.alt === true,
    shiftKey: spec.shift === true,
    metaKey: spec.meta === true,
  })

  /* ------------------------------------------------------- the table, held to its readers */

  /*
   * w3c-keyname is not a dependency of this package; it is `@codemirror/view`'s, and under pnpm
   * it lives beside the *real* view package in the virtual store, not under `ui/node_modules`.
   * Resolving from the view package's own path is what finds the copy CodeMirror actually
   * loads — which is the one that matters, since a second copy could disagree.
   */
  const viewDir = realpathSync('node_modules/@codemirror/view')
  const w3c = createRequire(join(viewDir, 'package.json'))('w3c-keyname')

  eq(LATIN_CODES.length, 47, '26 letters, 10 digits and 11 punctuation keys, no more and no fewer')
  eq(Object.keys(US_FACES).length, LATIN_CODES.length, 'LATIN_CODES is exactly the table')
  for (const code of LATIN_CODES) {
    const face = US_FACES[code]
    eq(w3c.base[face.keyCode], face.base, `${code}: w3c-keyname base[${face.keyCode}] is the unshifted face`)
    eq(w3c.shift[face.keyCode], face.shift, `${code}: w3c-keyname shift[${face.keyCode}] is the shifted face`)
    eq(usFace(code, false), { key: face.base, keyCode: face.keyCode }, `usFace(${code}, unshifted)`)
    eq(usFace(code, true), { key: face.shift, keyCode: face.keyCode }, `usFace(${code}, shifted)`)
  }
  for (let i = 0; i < 26; i++) {
    const code = `Key${String.fromCharCode(65 + i)}`
    // xterm: `String.fromCharCode(ev.keyCode - 64)` is the control byte for 65..90.
    eq(
      String.fromCharCode(US_FACES[code].keyCode - 64),
      String.fromCharCode(1 + i),
      `${code}: xterm's ctrl arithmetic lands on ^${String.fromCharCode(65 + i)}`,
    )
  }
  eq(US_FACES.BracketLeft.keyCode, 219, 'Ctrl+[ is ESC to xterm, and xterm tests keyCode 219 for it')
  eq(US_FACES.Backslash.keyCode, 220, 'Ctrl+\\ is FS to xterm, keyCode 220')
  eq(US_FACES.BracketRight.keyCode, 221, 'Ctrl+] is GS to xterm, keyCode 221')
  eq(usFace('ArrowLeft', false), null, 'a named key has no face')
  eq(usFace('IntlBackslash', false), null, 'the ISO extra key is outside the US table')
  eq(usFace('', false), null, 'an empty code has no face')

  /* --------------------------------------------- the whole Russian layout, both rungs */

  /** `xkb` `ru`: each physical key and both of its faces. */
  const RU = [
    ['Backquote', 'ё', 'Ё'],
    ['Digit1', '1', '!'],
    ['Digit2', '2', '"'],
    ['Digit3', '3', '№'],
    ['Digit4', '4', ';'],
    ['Digit5', '5', '%'],
    ['Digit6', '6', ':'],
    ['Digit7', '7', '?'],
    ['Digit8', '8', '*'],
    ['Digit9', '9', '('],
    ['Digit0', '0', ')'],
    ['Minus', '-', '_'],
    ['Equal', '=', '+'],
    ['KeyQ', 'й', 'Й'],
    ['KeyW', 'ц', 'Ц'],
    ['KeyE', 'у', 'У'],
    ['KeyR', 'к', 'К'],
    ['KeyT', 'е', 'Е'],
    ['KeyY', 'н', 'Н'],
    ['KeyU', 'г', 'Г'],
    ['KeyI', 'ш', 'Ш'],
    ['KeyO', 'щ', 'Щ'],
    ['KeyP', 'з', 'З'],
    ['BracketLeft', 'х', 'Х'],
    ['BracketRight', 'ъ', 'Ъ'],
    ['Backslash', '\\', '/'],
    ['KeyA', 'ф', 'Ф'],
    ['KeyS', 'ы', 'Ы'],
    ['KeyD', 'в', 'В'],
    ['KeyF', 'а', 'А'],
    ['KeyG', 'п', 'П'],
    ['KeyH', 'р', 'Р'],
    ['KeyJ', 'о', 'О'],
    ['KeyK', 'л', 'Л'],
    ['KeyL', 'д', 'Д'],
    ['Semicolon', 'ж', 'Ж'],
    ['Quote', 'э', 'Э'],
    ['KeyZ', 'я', 'Я'],
    ['KeyX', 'ч', 'Ч'],
    ['KeyC', 'с', 'С'],
    ['KeyV', 'м', 'М'],
    ['KeyB', 'и', 'И'],
    ['KeyN', 'т', 'Т'],
    ['KeyM', 'ь', 'Ь'],
    ['Comma', 'б', 'Б'],
    ['Period', 'ю', 'Ю'],
    ['Slash', '.', ','],
  ]
  eq(RU.length, LATIN_CODES.length, 'the Russian table covers every physical key the rewrite knows')
  ok(
    RU.every(([code]) => LATIN_CODES.includes(code)),
    'and names no key the rewrite does not',
  )

  const nonAscii = (s) => s.codePointAt(0) > 0x7f

  let swept = 0
  for (const [code, plain, shifted] of RU) {
    for (const shift of [false, true]) {
      const key = shift ? shifted : plain
      const face = usFace(code, shift)
      for (const mods of [{ ctrl: true }, { alt: true }, { meta: true }, { ctrl: true, alt: true }]) {
        const label = `${code} ${JSON.stringify(key)} ${JSON.stringify({ ...mods, shift })}`
        // Rung 1 needs nothing but the character; rung 2 needs the latch and a mismatch.
        const unlatched = nonAscii(key) ? face : null
        const latched = key !== face.key ? face : null
        eq(latinRewrite(ev({ code, key, shift, ...mods }), false), unlatched, `unlatched: ${label}`)
        eq(latinRewrite(ev({ code, key, shift, ...mods }), true), latched, `latched: ${label}`)
        swept += 2
      }
      // Text: the layout's character stays, whatever the latch says.
      eq(latinRewrite(ev({ code, key, shift }), true), null, `${code} ${key} with no chord modifier is text`)
    }
  }
  ok(swept === RU.length * 2 * 4 * 2, 'swept every Russian key, both faces, four chords, both latch states')

  // The two the design was argued from, spelled out.
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true }), false), { key: 'z', keyCode: 90 }, 'Ctrl+Z on Russian')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'Я', ctrl: true, shift: true }), false), { key: 'Z', keyCode: 90 }, 'Ctrl+Shift+Z on Russian is the uppercase face, as a US layout reports it')
  eq(latinRewrite(ev({ code: 'KeyC', key: 'с', ctrl: true }), false), { key: 'c', keyCode: 67 }, 'Ctrl+C on Russian — the interrupt')
  eq(latinRewrite(ev({ code: 'Slash', key: '.', ctrl: true }), false), null, 'Ctrl+/ on Russian produces `.`, which rung 1 cannot see')
  eq(latinRewrite(ev({ code: 'Slash', key: '.', ctrl: true }), true), { key: '/', keyCode: 191 }, 'and rung 2 can, once the layout is known — toggle comment')
  eq(latinRewrite(ev({ code: 'Slash', key: ',', ctrl: true, shift: true }), true), { key: '?', keyCode: 191 }, 'Ctrl+Shift+/ on Russian')
  eq(latinRewrite(ev({ code: 'Digit2', key: '"', ctrl: true, shift: true }), true), { key: '@', keyCode: 50 }, 'Ctrl+Shift+2 on Russian is `^@` to xterm again')
  eq(latinRewrite(ev({ code: 'Digit3', key: '№', ctrl: true, shift: true }), false), { key: '#', keyCode: 51 }, 'Shift+3 on Russian is `№`, non-ASCII, rung 1')
  eq(latinRewrite(ev({ code: 'Digit1', key: '!', ctrl: true, shift: true }), true), null, 'a face that already matches is left alone, latched or not')
  eq(latinRewrite(ev({ code: 'Minus', key: '-', ctrl: true }), true), null, 'so is Ctrl+-')

  /* ------------------------------------------------------------ other scripts, and Latin ones */

  eq(latinRewrite(ev({ code: 'KeyZ', key: 'ζ', ctrl: true }), false), { key: 'z', keyCode: 90 }, 'Greek')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'ז', ctrl: true }), false), { key: 'z', keyCode: 90 }, 'Hebrew')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'ئ', ctrl: true }), false), { key: 'z', keyCode: 90 }, 'Arabic')
  eq(latinRewrite(ev({ code: 'BracketRight', key: 'ї', ctrl: true }), false), { key: ']', keyCode: 221 }, 'Ukrainian, on a punctuation key')
  // Hebrew puts `/` on Q: real keyCode 191, and CodeMirror ran toggle-comment on Ctrl+Q. The
  // wrong command, not a dead key — the case rung 2 covers letter codes for.
  eq(latinRewrite(ev({ code: 'KeyQ', key: '/', keyCode: 191, ctrl: true }), false), null, 'Hebrew Ctrl+Q before the layout is known')
  eq(latinRewrite(ev({ code: 'KeyQ', key: '/', keyCode: 191, ctrl: true }), true), { key: 'q', keyCode: 81 }, 'Hebrew Ctrl+Q once it is')
  eq(latinRewrite(ev({ code: 'KeyW', key: "'", ctrl: true }), true), { key: 'w', keyCode: 87 }, "Hebrew Ctrl+W (`'` on W)")
  // Chrome parity, pinned as a decision: a non-ASCII *Latin* letter on a punctuation key is
  // rewritten too. Chromium's hardware fallback gives 219 for this key as well.
  eq(latinRewrite(ev({ code: 'BracketLeft', key: 'ü', ctrl: true }), false), { key: '[', keyCode: 219 }, 'German Ctrl+ü is Ctrl+[ — as in Chrome')
  // Latin layouts that move keys: `key` is the right answer and the latch is never set by one.
  eq(latinRewrite(ev({ code: 'KeyZ', key: ';', ctrl: true }), false), null, 'Dvorak Ctrl+; on the Z key is Ctrl+;')
  eq(latinRewrite(ev({ code: 'KeyY', key: 'z', ctrl: true }), false), null, 'QWERTZ Ctrl+Z on the Y key is Ctrl+Z')
  eq(latinRewrite(ev({ code: 'KeyA', key: 'q', ctrl: true }), false), null, 'AZERTY Ctrl+Q on the A key is Ctrl+Q')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'z', ctrl: true }), true), null, 'a US letter is never rewritten, even under a stale latch')

  /* -------------------------------------------------------------------------- not chords */

  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я' }), true), null, 'typing `я` is text')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'Я', shift: true }), true), null, 'Shift alone is a capital, not a chord')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', alt: true }), false, false), null, 'with altIsChord=false (macOS) Option alone is a level-3 shift')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', alt: true, ctrl: true }), false, false), { key: 'z', keyCode: 90 }, 'but Ctrl+Option still is a chord there')
  eq(latinRewrite(ev({ code: 'KeyZ', key: '𐐷', ctrl: true }), false), { key: 'z', keyCode: 90 }, 'an astral single character (Deseret) is one code point and non-ASCII')

  /* ------------------------------------------------------------------------- exclusions */

  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true, keyCode: 229 }), false), null, 'keyCode 229 is the input method')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true, composing: true }), false), null, 'so is isComposing')
  for (const key of ['Process', 'Dead', 'Unidentified', 'Control', 'AltGraph', 'Shift', 'Meta']) {
    eq(latinRewrite(ev({ code: 'KeyZ', key, ctrl: true }), true), null, `\`${key}\` is a name, not a character`)
  }
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true, type: 'keyup' }), false), null, 'keyup is not rewritten')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true, type: 'keypress' }), false), null, 'nor keypress')
  eq(latinRewrite(ev({ code: 'KeyZ', key: 'я', ctrl: true, type: 'keydown' }), false), { key: 'z', keyCode: 90 }, 'an explicit keydown is')
  for (const code of ['ArrowLeft', 'F3', 'Enter', 'Space', 'NumpadAdd', 'IntlBackslash', '']) {
    eq(latinRewrite(ev({ code, key: 'я', ctrl: true }), true), null, `${JSON.stringify(code)} is outside the table`)
  }
  eq(latinRewrite(ev({ code: 'KeyZ', key: '', ctrl: true }), true), null, 'an empty key is not a character')

  /* ------------------------------------------------------------------------- the latch */

  for (const key of ['я', 'ζ', 'ז', 'ئ', 'ї']) eq(learnsLayout(ev({ code: 'KeyZ', key })), true, `${key} is a non-Latin letter`)
  eq(learnsLayout(ev({ code: 'KeyZ', key: 'я', ctrl: true })), true, 'a chorded non-Latin letter is evidence too')
  eq(learnsLayout(ev({ code: 'KeyF', key: 'f' })), false, 'an ASCII letter clears it')
  eq(learnsLayout(ev({ code: 'KeyF', key: 'F', shift: true })), false, 'so does a capital')
  for (const key of ['№', '€', '/', '.', '1', ' ', 'ı', 'ş', 'ü', 'ø', 'Process', 'Dead', 'Control', 'ArrowLeft', '']) {
    eq(learnsLayout(ev({ code: 'KeyQ', key })), null, `${JSON.stringify(key)} says nothing about the layout`)
  }
  eq(learnsLayout(ev({ code: 'KeyZ', key: 'я', keyCode: 229 })), null, 'the input method says nothing')
  eq(learnsLayout(ev({ code: 'KeyZ', key: 'я', composing: true })), null, 'nor does a composition')
  eq(learnsLayout(ev({ code: 'KeyZ', key: 'я', type: 'keyup' })), null, 'nor a keyup')

  /* ------------------------------------------------------------------------ the applier */

  resetLatinLayout()
  eq(latinLayoutIsNonLatin(), false, 'fresh: Latin')

  {
    const e = ev({ code: 'Slash', key: '.', ctrl: true })
    eq(latiniseKeyEvent(e), null, 'cold start: the first Ctrl+/ on Russian is missed, by design')
    eq(e.key, '.', 'and the event is untouched')
    ok(!Object.hasOwn(e, 'keyCode'), 'no keyCode was invented')
  }
  {
    const e = ev({ code: 'KeyZ', key: 'я', ctrl: true })
    eq(latiniseKeyEvent(e), { key: 'z', keyCode: 90 }, 'Ctrl+Z on Russian is rewritten')
    eq([e.key, e.keyCode, e.which, e.code], ['z', 90, 90, 'KeyZ'], 'key, keyCode and which installed; code untouched')
    for (const name of ['key', 'keyCode', 'which']) {
      const d = Object.getOwnPropertyDescriptor(e, name)
      eq([d.writable, d.enumerable, d.configurable], [true, true, true], `${name} is an ordinary own property`)
    }
    eq(latinLayoutIsNonLatin(), true, 'and the latch learned from it')
    // The same event object reaches both entry points of the gate for a terminal keystroke.
    eq(latiniseKeyEvent(e), { key: 'z', keyCode: 90 }, 'a second pass — the other entry point — answers the same')
    eq([e.key, e.keyCode], ['z', 90], 'and leaves the rewrite in place')
    eq(
      latinLayoutIsNonLatin(),
      true,
      'and does not read its own `z` as Latin evidence — the first version did, and a terminal un-learned the layout on every chord',
    )
  }
  {
    const e = ev({ code: 'Slash', key: '.', ctrl: true })
    eq(latiniseKeyEvent(e), { key: '/', keyCode: 191 }, 'now Ctrl+/ resolves — rung 2 through the applier')
  }
  {
    // A real event: `key`/`keyCode` are getters on the prototype, not own properties.
    const proto = {
      get key() {
        return 'я'
      },
      get keyCode() {
        return 0
      },
      get which() {
        return 0
      },
    }
    const e = Object.assign(Object.create(proto), { code: 'KeyZ', ctrlKey: true, altKey: false, shiftKey: false, metaKey: false })
    eq(latiniseKeyEvent(e), { key: 'z', keyCode: 90 }, 'prototype getters are shadowed')
    eq([e.key, e.keyCode, e.which], ['z', 90, 90], 'and the own properties win')
    eq(proto.key, 'я', 'the prototype is not what was written to')
  }
  {
    const e = ev({ code: 'KeyZ', key: 'я', ctrl: true, type: 'keyup' })
    eq(latiniseKeyEvent(e), null, 'keyup through the applier is left alone')
    eq(e.key, 'я', 'untouched')
  }
  {
    const e = Object.freeze(ev({ code: 'KeyZ', key: 'я', ctrl: true }))
    eq(latiniseKeyEvent(e), null, 'a frozen event reports no rewrite rather than a rewrite that did not take')
    eq(e.key, 'я', 'and is unchanged')
  }
  {
    const e = ev({ code: 'KeyZ', key: 'я' })
    eq(latiniseKeyEvent(e), null, 'typing is never rewritten')
    eq(e.key, 'я', 'the character reaches the text field')
  }
  {
    eq(latiniseKeyEvent(ev({ code: 'KeyF', key: 'f' })), null, 'a Latin letter typed')
    eq(latinLayoutIsNonLatin(), false, 'clears the latch')
    const e = ev({ code: 'Slash', key: '.', ctrl: true })
    eq(latiniseKeyEvent(e), null, 'and Ctrl+/ is Ctrl+. again on a layout that has one')
  }
  resetLatinLayout()
  eq(latinLayoutIsNonLatin(), false, 'reset forgets')

  if (failed > 0) {
    console.error(`\ncheck-latin: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`check-latin: ok (${LATIN_CODES.length} keys held to w3c-keyname, ${swept} Russian chords swept)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
