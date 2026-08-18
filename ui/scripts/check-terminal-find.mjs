/**
 * Checks `src/terminal/findModel.ts` — the terminal find bar's arithmetic, its scope note and
 * its keyboard — and pins it against `src/editor/findMatches.ts`, which is the same arithmetic
 * for the other find bar in this app.
 *
 * # What is actually at risk here
 *
 * The bar itself cannot be compiled by anything under `ui/scripts/`: it needs React, a mounted
 * pane and a live `Terminal` with a rendered element. What *can* be compiled is everything a
 * user reads, and every one of those is a sentence that is wrong in a way review does not catch:
 *
 * * **The ordinal.** `@codemirror/search` and `@xterm/addon-search` both wrap silently in both
 *   directions, so a bare total (`12 matches`) makes "you have walked past the last match and
 *   are back at the first" indistinguishable from "the key did nothing" — the number does not
 *   move either way. `editor/findMatches.ts` carries the whole story; the fix is `3 of 12`, and
 *   an off-by-one in it is invisible until somebody counts.
 * * **The cap.** xterm's addon saturates its result list *at* `highlightLimit`, where
 *   CodeMirror's walk stops one *past* `COUNT_CAP`. Two different mechanisms, one label. A `>`
 *   where a `>=` belongs makes the terminal bar claim `999 matches` for a query with ten
 *   thousand.
 * * **The two bars agreeing.** They are deliberately separate modules — `findModel.ts` imports
 *   nothing so that this script can compile it, and importing `countLabel` would end that — so
 *   the only thing holding their wording together is the cross-check below. Without it the two
 *   drift into `3 of 12` and `3/12`, which is the kind of difference nobody files a bug about
 *   and everybody notices.
 * * **The alt-screen note.** The one piece of honesty in the feature: on the alternate buffer
 *   there is no scrollback, so the search covers the visible screen and says so. A note that
 *   went missing would leave the bar reporting "no results" for a word the user can see in the
 *   transcript ten lines up.
 *
 * Both modules import nothing, deliberately, which is what lets a bare `tsc` compile them side
 * by side and node run them.
 *
 * Run: `pnpm --dir ui run check:terminal-find`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

/** A source file, read as text. The wiring section below reads rather than imports. */
const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')

const out = mkdtempSync(join(tmpdir(), 'cide-term-find-'))
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
      'src/terminal/findModel.ts',
      'src/editor/findMatches.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const {
    FIND_HIGHLIGHT_LIMIT,
    NO_RESULTS,
    findCountLabel,
    findFieldKey,
    findScopeNote,
  } = await import(`file://${join(out, 'terminal/findModel.js')}`)
  const { COUNT_CAP, countLabel } = await import(`file://${join(out, 'editor/findMatches.js')}`)

  /* ----------------------------------------------------------------------------- the count */

  eq(NO_RESULTS, { count: 0, index: -1 }, 'the opening state is "nothing found, nowhere"')

  eq(
    findCountLabel('', { count: 0, index: -1 }),
    '',
    'an empty query says NOTHING. The bar opens with an empty field, and "0 matches" there ' +
      'reports a failure that has not happened yet',
  )
  eq(
    findCountLabel('', { count: 7, index: 2 }),
    '',
    '...even if a previous search left numbers behind — the query is what the label is about',
  )

  eq(findCountLabel('x', { count: 0, index: -1 }), '0 matches', 'nothing found says so')
  eq(findCountLabel('x', { count: 1, index: -1 }), '1 match', 'the singular is singular')
  eq(findCountLabel('x', { count: 12, index: -1 }), '12 matches', 'a total, with no caret on one')
  eq(
    findCountLabel('x', { count: 12, index: 2 }),
    '3 of 12',
    'the ordinal is 1-BASED and the addon index is 0-based — the one arithmetic in this module',
  )
  eq(findCountLabel('x', { count: 12, index: 0 }), '1 of 12', 'index 0 is the first match')
  eq(findCountLabel('x', { count: 12, index: 11 }), '12 of 12', 'index n-1 is the last')

  /*
   * The wrap, said out loud, because it is the reason the ordinal exists at all: walking off
   * the end of the list comes back to `1 of 12`, and a bar showing a bare total would have
   * printed the same string twice and looked broken.
   */
  eq(
    [findCountLabel('x', { count: 3, index: 2 }), findCountLabel('x', { count: 3, index: 0 })],
    ['3 of 3', '1 of 3'],
    'walking past the last match wraps to the first, and the label MOVES — the whole point',
  )

  /* ------------------------------------------------------------------------------- the cap */

  eq(
    FIND_HIGHLIGHT_LIMIT,
    COUNT_CAP,
    'the two find bars cap at the same number, so `999+` means the same thing in both. This is ' +
      'a claim about the product, not an implementation detail: they are handed to two ' +
      'different libraries and only this line keeps them equal',
  )
  eq(
    findCountLabel('x', { count: FIND_HIGHLIGHT_LIMIT, index: 4 }),
    `${FIND_HIGHLIGHT_LIMIT}+`,
    'AT the limit is already capped — xterm saturates its result list *at* `highlightLimit` ' +
      '(`ResultTracker.updateResults` slices to it), where CodeMirror stops one past `COUNT_CAP`',
  )
  eq(
    findCountLabel('x', { count: FIND_HIGHLIGHT_LIMIT - 1, index: 4 }),
    `5 of ${FIND_HIGHLIGHT_LIMIT - 1}`,
    'one below the limit is an exact count with an ordinal',
  )
  eq(
    findCountLabel('x', { count: FIND_HIGHLIGHT_LIMIT, index: -1 }),
    `${FIND_HIGHLIGHT_LIMIT}+`,
    'and the cap wins over the ordinal branch either way round',
  )

  /* ------------------------------------------- the two bars word the same fact the same way */

  /*
   * The cross-check that pays for the duplication. `findModel.ts` cannot import `countLabel`
   * without losing its standalone compile, so every sentence is written twice; this walks the
   * cross-product and demands the two agree.
   *
   * The cap is excluded from the sweep and asserted separately above, because the two modules
   * genuinely disagree about *where* it lands — a `Tally` at the cap has walked one past it and
   * an xterm count at the limit has not — and folding that into "they agree" would either hide
   * the difference or fail for a reason that is not drift.
   */
  let agreed = 0
  for (const total of [0, 1, 2, 12, 100, COUNT_CAP - 1]) {
    for (const index of [-1, 0, 1, total - 1]) {
      if (index >= total) continue
      const ordinal = index >= 0 ? index + 1 : null
      eq(
        findCountLabel('q', { count: total, index }),
        countLabel({ total, capped: false, ordinal }),
        `the terminal bar and the editor bar agree for total=${total} index=${index}`,
      )
      agreed += 1
    }
  }
  ok(agreed >= 18, `${agreed} label pairs compared — the cross-product still expands`)

  /* ------------------------------------------------------------------------- the scope note */

  eq(
    findScopeNote(false),
    null,
    'on the normal buffer there is nothing to explain — `buffer.active` is the scrollback plus ' +
      'the screen, which is what "search this terminal" plainly means',
  )
  const note = findScopeNote(true)
  ok(typeof note === 'string' && note.length > 0, 'on the ALTERNATE buffer the bar says something')
  ok(
    /screen/i.test(note ?? ''),
    'and what it says names the screen, because that is exactly what the search covers: a ' +
      'full-screen TUI has no scrollback, so finding nothing quietly would be a lie about the ' +
      'transcript the user was reading a minute ago',
  )

  /* -------------------------------------------------------------------------- the field keys */

  const key = (spec) => ({
    key: spec.key,
    shiftKey: spec.shift === true,
    ctrlKey: spec.ctrl === true,
    altKey: spec.alt === true,
    metaKey: spec.meta === true,
  })

  eq(findFieldKey(key({ key: 'Enter' })), 'next', 'Enter walks forward')
  eq(findFieldKey(key({ key: 'Enter', shift: true })), 'previous', 'Shift+Enter walks back')
  eq(findFieldKey(key({ key: 'Escape' })), 'close', 'Escape dismisses')
  eq(findFieldKey(key({ key: 'f', ctrl: true })), 'refocus', 'a second Ctrl+F re-selects the field')
  eq(
    findFieldKey(key({ key: 'F', ctrl: true })),
    'refocus',
    '...whatever case the layout reports, because Shift is not held and `key` follows the layout',
  )

  eq(findFieldKey(key({ key: 'a' })), null, 'an ordinary character is left to the field')
  eq(findFieldKey(key({ key: 'v', ctrl: true })), null, "Ctrl+V is the field's own native paste")
  eq(findFieldKey(key({ key: 'c', ctrl: true })), null, '...and Ctrl+C its native copy')
  eq(
    findFieldKey(key({ key: 'a', ctrl: true })),
    null,
    'Ctrl+A selects all IN THE FIELD. Claiming it would take select-all from the one input in ' +
      'this app that is inside a terminal pane',
  )
  eq(
    findFieldKey(key({ key: 'f', ctrl: true, shift: true })),
    null,
    'Ctrl+Shift+F is find-in-files and belongs to the key gate, in this field as everywhere else',
  )
  eq(
    findFieldKey(key({ key: 'F3' })),
    null,
    "F3 is the EDITOR find bar's, kept out of the global keymap by " +
      '`keymap::nothing_binds_the_find_bars_f_keys`; claiming it here would give one key two ' +
      'meanings in two panes for a gesture nobody asked for',
  )
  for (const mod of ['alt', 'meta']) {
    for (const k of ['Enter', 'Escape', 'f']) {
      eq(
        findFieldKey(key({ key: k, [mod]: true, ctrl: k === 'f' })),
        null,
        `${mod}+${k} is not claimed — the field takes only the four chords it draws buttons for`,
      )
    }
  }
  eq(
    findFieldKey(key({ key: 'Escape', shift: true })),
    null,
    'Shift+Escape is not close. Escape is the one key whose unmodified form has to stay exact, ' +
      'because a modified spelling closing the bar would also close it for a chord the user ' +
      'meant for something else',
  )

  /* ------------------------------------------------------- the feature is actually plugged in */

  /*
   * A source scan, and a weak gate by construction — it proves a call is *written*, not that it
   * runs. It earns its place because the defect it is aimed at is the one this whole feature
   * started as: `@xterm/addon-search` had been a pinned, installed, version-locked dependency of
   * this app for eighteen milestones and **nothing imported it**. Nothing was broken, nothing
   * failed, no gate went red; the capability simply did not exist. `check-commands.mjs` reads
   * `dispatch.ts` as text for exactly the same reason and says so at length.
   *
   * Each line below is one link in the chain from a keystroke to a highlighted match. A break in
   * any of them leaves a bar that opens and finds nothing, or a chord that does nothing at all.
   */
  {
    const xterm = read('../src/terminal/xterm.ts')
    const dispatch = read('../src/keys/dispatch.ts')
    const pane = read('../src/panes/TerminalPane.tsx')
    const bar = read('../src/panes/TerminalFindBar.tsx')
    const pkg = JSON.parse(read('../package.json'))

    ok(
      typeof pkg.dependencies['@xterm/addon-search'] === 'string',
      'the search addon is still a dependency',
    )
    ok(
      xterm.includes("from '@xterm/addon-search'"),
      'and something IMPORTS it — the whole defect this feature began as was a pinned ' +
        'dependency that no module in the app ever mentioned',
    )
    ok(
      xterm.includes('new SearchAddon(') && xterm.includes('loadAddon(addon)'),
      'the addon is constructed and loaded into a terminal',
    )
    ok(
      /highlightLimit:\s*FIND_HIGHLIGHT_LIMIT/.test(xterm),
      "and it is given this module's limit, which is what makes the `999+` label true",
    )

    ok(
      xterm.includes('terminalOpensFind(ev)') && xterm.includes('openTerminalFind(paneId)'),
      'the Ctrl+F chord reaches the store from the terminal key handler — the focus-scoped ' +
        'route, which is the only one that ships',
    )
    ok(
      /case 'terminal\.find':/.test(dispatch) && dispatch.includes('openTerminalFind(on.pane)'),
      'and the palette row reaches the SAME store write, so the command and the keystroke are ' +
        'one feature rather than two',
    )
    ok(
      pane.includes('<TerminalFindBar paneId={paneId} />'),
      'the pane renders the bar. Without this line the store is written and nothing reads it — ' +
        'a state produced by nobody, which is this project\'s recurring defect exactly',
    )
    ok(
      bar.includes('ensureSearch(') && bar.includes('searchOptions()'),
      'the bar runs its searches through the loaded addon, with decorations',
    )
    ok(
      bar.includes('onDidChangeResults'),
      'and subscribes to the counts — the event fires only for a search that carried a ' +
        '`decorations` block, which is why `searchOptions` is not optional',
    )
    ok(
      bar.includes('findCountLabel(') && bar.includes('findScopeNote('),
      "and draws this module's label and scope note rather than wording them again",
    )
    ok(
      bar.includes('clearDecorations()'),
      'and clears the highlights when it closes, or a pane keeps them for the rest of its life',
    )
    /*
     * What a match is painted with, pinned as a source scan because `searchOptions` needs a
     * `getComputedStyle` and cannot be compiled here.
     *
     * Both of these shipped wrong once and neither would have failed a gate. `--sel` is not a
     * neutral wash in a terminal: `settings/theme.ts::TERMINAL_SLOTS` maps
     * `selectionBackground → --sel`, so using it for matches paints a hit in exactly the colour
     * of selected text — and the addon *selects* the active match, so the hit that most needs to
     * stand out wore the selection's colour twice. And a solid `--accent` for the current hit is
     * the thing `EditorSurface.module.css` records as tried and unusable, because the decoration
     * cannot choose the ink drawn over it; xterm applies `backgroundColor` as the cell background
     * with the glyph's own foreground on top, and in a terminal that ink is an arbitrary ANSI
     * palette a child chose. The ring is the idiom that survives both.
     */
    ok(
      !/hexToken\(style, '--sel'/.test(xterm),
      "the match wash is NOT `--sel` — that token IS the terminal's `selectionBackground` slot " +
        '(`settings/theme.ts::TERMINAL_SLOTS`), so a match painted with it is indistinguishable ' +
        'from selected text, and the addon selects the active match',
    )
    ok(
      /activeMatchBorder:/.test(xterm),
      'the current hit is marked with a RING (`activeMatchBorder`, which the addon renders as ' +
        '`outline: 1px solid`) rather than by a loud fill alone — `EditorSurface.module.css` ' +
        'records what a solid `--accent` fill costs when the decoration cannot choose its ink, ' +
        'and a terminal cell has strictly less control over that ink than a code buffer does',
    )

    ok(
      /id: 'find',[\s\S]{0,400}openTerminalFind\(paneId\)/.test(pane),
      "the pane's context menu offers it too, and reaches the same store write. This is the " +
        'feature\'s only *visible* affordance: the chord is focus-scoped rather than bound in ' +
        '`cide_core::keymap`, so Settings → Keymap lists no shortcut for it and the palette ' +
        'draws no chip',
    )
  }

  /* --------------------------------------------------- Ctrl+V reaches one paste, with a kind */

  /*
   * Item 8's half of the same claim. `terminal/clipboard.ts` is the one implementation of paste
   * and the pane kind is what decides its no-text branch; a call site that forgot to pass one
   * would not compile, but a call site that passed the *wrong* one would, and it is the branch
   * that hands `^V` back to the Claude CLI — the only path an image has into a prompt.
   */
  {
    const clipboard = read('../src/terminal/clipboard.ts')
    const xterm = read('../src/terminal/xterm.ts')
    const dispatch = read('../src/keys/dispatch.ts')
    const pane = read('../src/panes/TerminalPane.tsx')

    ok(
      clipboard.includes("const CTRL_V = '\\x16'"),
      'the byte handed back to the Claude CLI is written once, as a named constant',
    )
    ok(
      clipboard.includes('term.input(CTRL_V)'),
      'and `pasteIntoTerminal` re-emits it — without this line, a Claude pane with an image on ' +
        'the clipboard pastes nothing and image paste is gone',
    )
    ok(
      /if \(kind === 'claude'\)/.test(clipboard),
      "...and only in a Claude pane: `^V` in a shell is readline's quoted-insert, which would " +
        'eat the next keystroke',
    )
    ok(
      xterm.includes('pasteIntoTerminal(term, kind)'),
      'the keystroke path passes the pane kind',
    )
    ok(
      dispatch.includes("pasteIntoTerminal(term, target?.pane.kind === 'claude' ? 'claude' : 'shell')"),
      'the `terminal.paste` command passes it too',
    )
    ok(
      pane.includes('pasteIntoTerminal(term, kind)') &&
        pane.includes('pasteInto(paneId, runKind)'),
      "and so does the pane's own context menu — the one route that used to work, and the " +
        'reason the bug was reported as an asymmetry rather than as a broken paste',
    )
  }

  if (failed > 0) {
    console.error(`\ncheck-terminal-find: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-terminal-find: ok (${agreed} labels cross-checked against the editor's find bar)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
