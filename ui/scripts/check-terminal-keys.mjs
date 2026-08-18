/**
 * Checks `src/terminal/keys.ts` — the two decisions a terminal makes about a keystroke on its
 * own, after the key gate has passed it through.
 *
 * The module was written to be pure and testable ("keeping the decision separate from the xterm
 * wiring is what makes it testable without a DOM") and **had no test at all**: nothing under
 * `ui/scripts/` mentioned it, so the Shift+Enter encoding — a hand-shake with one specific
 * program, where the wrong escape shows up as a literal `[13;2u` in the user's prompt — was
 * covered by nothing but review.
 *
 * The clipboard half is the part that must not be got wrong twice:
 *
 * * **Ctrl+C with nothing selected has to stay the interrupt.** It is the primary escape from a
 *   runaway command and from a mid-turn `claude`, and a copy feature that swallows it is worse
 *   than no copy feature. The rule returns `null` there, which is what makes xterm send `ETX`
 *   exactly as it always did; the sweep below asserts that for every modifier combination.
 * * **Ctrl+V is claimed in both pane kinds, and image paste still has to survive it.** The rule
 *   used to return `null` for a Claude pane so `^V` reached the CLI, which binds it and reads
 *   the system clipboard out of `xclip`/`wl-paste` — the only path an *image* has into a prompt.
 *   That handler exists at the CLI's main prompt and nowhere else, so at a numbered-choice
 *   question the keystroke was swallowed and nothing happened; the reported bug. The chord is
 *   now always intercepted and `terminal/clipboard.ts` re-emits `^V` when the clipboard held no
 *   text, which is where the image path lives. This file pins the half that is a pure function:
 *   the rule no longer branches on the kind.
 * * **Ctrl+F opens the pane's find bar**, in both kinds, and only for a plain-Ctrl keydown. It
 *   is here rather than in `cide_core::keymap` for the reason the two above are, and
 *   `keymap::ctrl_f_is_not_bound_here_because_two_panes_mean_two_things_by_it` is the other end
 *   of that claim.
 *
 * Compiled standalone with the TypeScript already in `node_modules`, the same shape as
 * `check-exit-marker.mjs`: this module imports nothing, deliberately, so that this can.
 *
 * Run: `pnpm --dir ui run check:terminal-keys`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-term-keys-'))
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
      'src/terminal/keys.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const { SHIFT_ENTER, terminalKeyBytes, terminalClipboardAction, terminalOpensFind } =
    await import(`file://${join(out, 'keys.js')}`)

  /** A `TerminalKeyEvent`, with every modifier off unless named. */
  const ev = (spec) => ({
    type: spec.type ?? 'keydown',
    key: spec.key,
    ctrlKey: spec.ctrl === true,
    altKey: spec.alt === true,
    shiftKey: spec.shift === true,
    metaKey: spec.meta === true,
  })

  /* ------------------------------------------------------------------------- Shift+Enter */

  eq(SHIFT_ENTER, '\x1b\r', 'Shift+Enter is ESC CR — what `claude /terminal-setup` writes')
  eq(
    terminalKeyBytes(ev({ key: 'Enter', shift: true })),
    SHIFT_ENTER,
    'Shift+Enter is re-encoded, because a PTY has no way to tell it from Enter',
  )
  eq(
    terminalKeyBytes(ev({ key: 'Enter' })),
    null,
    'plain Enter is left alone — xterm already sends the `\\r` a shell is waiting for',
  )
  for (const extra of ['ctrl', 'alt', 'meta']) {
    eq(
      terminalKeyBytes(ev({ key: 'Enter', shift: true, [extra]: true })),
      null,
      `${extra}+shift+enter is not claimed — claiming it would shadow a future binding`,
    )
  }
  for (const type of ['keyup', 'keypress']) {
    eq(
      terminalKeyBytes(ev({ key: 'Enter', shift: true, type })),
      null,
      `${type} resolves nothing — xterm offers all three to one handler, and three sends is ` +
        'three newlines for one press',
    )
  }

  /* ---------------------------------------------------------- the interrupt, exhaustively */

  /*
   * Every modifier combination against C and V, in both pane kinds, with and without a
   * selection. Testing only the two chords that copy would pass on a rule that also fired for
   * ⌃⇧C or for a keyup — and the branch that matters most is the one that must do *nothing*.
   */
  const SELECTION = 'lines the user dragged over'
  let swept = 0
  for (const kind of ['claude', 'shell']) {
    for (const selection of ['', SELECTION]) {
      for (const key of ['c', 'v', 'C', 'x', 'Enter']) {
        for (let bits = 0; bits < 16; bits++) {
          const mods = {
            ctrl: (bits & 1) !== 0,
            alt: (bits & 2) !== 0,
            shift: (bits & 4) !== 0,
            meta: (bits & 8) !== 0,
          }
          const plainCtrl = mods.ctrl && !mods.alt && !mods.shift && !mods.meta
          const letter = key.toLowerCase()
          const expected =
            !plainCtrl || (letter !== 'c' && letter !== 'v')
              ? null
              : letter === 'c'
                ? selection === ''
                  ? null
                  : { kind: 'copy', text: selection }
                : { kind: 'paste' }

          swept += 1
          eq(
            terminalClipboardAction(ev({ key, ...mods }), { selection, kind }),
            expected,
            `${JSON.stringify(mods)} ${key} in a ${kind} pane with ` +
              `${selection === '' ? 'no selection' : 'a selection'}`,
          )
        }
      }
    }
  }
  eq(swept, 2 * 2 * 5 * 16, 'swept both pane kinds × both selection states × the key set × 16')

  /* ------------------------------------------------- the four claims, said in their own words */

  eq(
    terminalClipboardAction(ev({ key: 'c', ctrl: true }), { selection: '', kind: 'shell' }),
    null,
    'Ctrl+C with nothing selected is the INTERRUPT: `null` is what lets xterm send ETX with ' +
      'its own side effects, and no branch of this feature may take that away',
  )

  // A selection xterm reports but the user never made.
  //
  // `getSelection()` is derived from coordinates and never consults `hasSelection()`, so a
  // one-pixel drag across a row boundary while clicking into the pane returns "\n", and a
  // double-click that misses a word by a cell returns the spaces beside it. Against an
  // `=== ''` test both of those are "a selection", and the next Ctrl+C — the one aimed at a
  // mid-turn `claude` — silently becomes a clipboard write instead of an interrupt.
  //
  // This shipped in the first version of this feature and is the reason the check exists in
  // this shape: the interrupt is the keystroke that must never regress, so every string that
  // carries no visible characters is asserted to interrupt, one by one.
  for (const selection of ['\n', '\n\n', ' ', '   ', '\t', ' \n ', '\u00a0']) {
    for (const kind of ['shell', 'claude']) {
      eq(
        terminalClipboardAction(ev({ key: 'c', ctrl: true }), { selection, kind }),
        null,
        `Ctrl+C over a selection of only whitespace (${JSON.stringify(selection)}, ${kind}) is ` +
          'still the INTERRUPT — xterm reports these for gestures the user did not make',
      )
    }
  }

  // ...and the boundary is visible characters, not length: whitespace AROUND real text is part
  // of what the user selected and must be copied verbatim, indentation included.
  eq(
    terminalClipboardAction(ev({ key: 'c', ctrl: true }), { selection: '  fn main() {\n', kind: 'shell' }),
    { kind: 'copy', text: '  fn main() {\n' },
    'a selection with real text copies exactly what was selected, leading indentation and all',
  )
  eq(
    terminalClipboardAction(ev({ key: 'c', ctrl: true }), { selection: 'x', kind: 'claude' }),
    { kind: 'copy', text: 'x' },
    'Ctrl+C with a selection copies, in a Claude pane as much as in a shell — the CLI does not ' +
      'bind it and cide never sent ^C-as-copy anywhere',
  )
  eq(
    terminalClipboardAction(ev({ key: 'v', ctrl: true }), { selection: '', kind: 'claude' }),
    { kind: 'paste' },
    'Ctrl+V in a Claude pane pastes too. It used to resolve to `null` so the CLI got `^V`, and ' +
      'that only worked at the main prompt — at a numbered-choice question nothing was ' +
      'listening and the keystroke vanished. `terminal/clipboard.ts` hands `^V` back to the CLI ' +
      'when the clipboard holds no text, so image paste is unchanged',
  )
  eq(
    terminalClipboardAction(ev({ key: 'v', ctrl: true }), { selection: 'x', kind: 'claude' }),
    { kind: 'paste' },
    'and a selection does not change it — only Ctrl+C consults the selection',
  )
  eq(
    terminalClipboardAction(ev({ key: 'v', ctrl: true }), { selection: '', kind: 'shell' }),
    { kind: 'paste' },
    "Ctrl+V in a shell pane pastes: `bash` binds ^V to readline's quoted-insert, which swallows " +
      'it, which is the reported "nothing happens"',
  )

  // keydown only, for the clipboard half as well. A copy on the keyup would write the clipboard
  // twice per press; a `false` return on the keyup of an interrupt would be worse.
  for (const type of ['keyup', 'keypress']) {
    eq(
      terminalClipboardAction(ev({ key: 'c', ctrl: true, type }), {
        selection: SELECTION,
        kind: 'shell',
      }),
      null,
      `${type} is not a chord`,
    )
  }

  // The two chords that are conventionally copy/paste in a terminal are deliberately NOT
  // claimed. They are inert in xterm today (its ctrl branch requires `!shiftKey`), they are
  // reserved in two comments in this repository, and nobody has asked for them — so the rule
  // leaves them free rather than inventing a binding.
  for (const key of ['c', 'v']) {
    eq(
      terminalClipboardAction(ev({ key, ctrl: true, shift: true }), {
        selection: SELECTION,
        kind: 'shell',
      }),
      null,
      `ctrl+shift+${key} is left unclaimed`,
    )
  }

  /* ------------------------------------------------------------------------------ Ctrl+F */

  /*
   * The find chord, swept exactly like the clipboard pair and for the same reason: the claim
   * worth measuring is not that ⌃F resolves, it is that the *other fifteen* modifier
   * combinations on that key — and every other key — do not.
   *
   * ⌃F costs a child a real byte (`^F` is readline's `forward-char` and vim's page-forward), so
   * a rule that fired one modifier wide would take a second key from every shell in every pane.
   */
  let findSwept = 0
  for (const key of ['f', 'F', 'g', 'v', 'Enter']) {
    for (let bits = 0; bits < 16; bits++) {
      const mods = {
        ctrl: (bits & 1) !== 0,
        alt: (bits & 2) !== 0,
        shift: (bits & 4) !== 0,
        meta: (bits & 8) !== 0,
      }
      const plainCtrl = mods.ctrl && !mods.alt && !mods.shift && !mods.meta
      findSwept += 1
      eq(
        terminalOpensFind(ev({ key, ...mods })),
        plainCtrl && key.toLowerCase() === 'f',
        `terminalOpensFind: ${JSON.stringify(mods)} ${key}`,
      )
    }
  }
  eq(findSwept, 5 * 16, 'swept the find chord across the key set × 16 modifier combinations')

  for (const type of ['keyup', 'keypress']) {
    eq(
      terminalOpensFind(ev({ key: 'f', ctrl: true, type })),
      false,
      `${type} is not a chord — xterm offers all three to one handler, and opening the bar on ` +
        'three of them would re-select the field twice for one press',
    )
  }

  // Ctrl+Shift+F is `sidebar.search` in `cide_core::keymap::defaults()`, resolved by the key
  // gate before this module is ever reached. Claiming it here would be a second meaning for a
  // chord that already has one, in the one surface where the gate's decision arrives second.
  eq(
    terminalOpensFind(ev({ key: 'f', ctrl: true, shift: true })),
    false,
    'ctrl+shift+f is find-in-files and is left to the key gate',
  )

  if (failed > 0) {
    console.error(`\ncheck-terminal-keys: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-terminal-keys: ok (${swept} clipboard chords and ${findSwept} find chords swept, ` +
      'Shift+Enter table pinned)',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
