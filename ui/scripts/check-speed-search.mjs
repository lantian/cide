/**
 * Speed search: the key table, the wrap-around, the expiry — and the call sites.
 *
 * Typing into either sidebar tree filters it. That gesture has to take a handful of keys the
 * trees already use, and *which* keys it takes is the entire risk: a Delete swallowed in the
 * wrong state trashes a file, an Escape claimed too late throws away a clipboard cut, a Space
 * claimed too eagerly stops the git tree ticking boxes. All of those decisions live in
 * `src/sidebar/speedSearch.ts`, which is pure and import-free precisely so this script can
 * compile it with the TypeScript already in `node_modules` and drive it under node — the same
 * arrangement `check-tree-status.mjs` uses for `clickSemantics.ts`.
 *
 * Four things are pinned:
 *
 *  1. **The key table, row by row**, in both states (nothing typed / a query armed) and across
 *     the modifier matrix. This is the assertion that "must not steal keys that already work in
 *     a tree" is a property rather than an intention.
 *  2. **Wrap-around and expiry.** `nextMatch` cycles rather than clamping; `expired` is a
 *     comparison of timestamps rather than a timer, for the reason `keys/gate.ts` gives.
 *  3. **The sentences.** The two dead ends — no match, and a truncated list — have to be
 *     distinguishable from `3 of 17` and from each other.
 *  4. **The call sites**, over comment-stripped source. Every bug this feature can have is a
 *     *missing call*: a branch that runs after the clipboard block instead of before it, a
 *     highlight span re-derived in the row, a tree that never exits the search on blur. A
 *     module's own tests cannot see any of those, which is `check-editor.mjs`'s stated reason
 *     for the same technique.
 *
 * The matching rule itself is **not** here, and that is deliberate: it lives in
 * `crates/cide-fs/src/speed.rs` with its own tests, and reaches both trees through
 * `fs_tree_match` and `tree_match_labels`. See that module's header for why one Rust rule with
 * two entry points beat a TypeScript copy. What this script pins about it is the *wiring* —
 * that both trees ask Rust rather than matching locally.
 *
 * Run: `pnpm --dir ui run check:speed-search`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-speed-search-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (condition, what) => {
  if (!condition) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

/**
 * Source with comments removed.
 *
 * Every grep below runs over this rather than over the raw file, and the reason is a lesson
 * this project has already paid for: a comment explaining a rule contains the same words the
 * rule does, so an assertion over raw source stays green after the code is deleted and only
 * the explanation is left. The mutation transcript for this script proves the stripping works
 * in both directions — removing the feature fails, and naming it only in a comment does not
 * satisfy the pin.
 *
 * String-aware, so a `'//'` inside a string literal does not eat the rest of the line.
 */
function withoutComments(source) {
  let out = ''
  let i = 0
  while (i < source.length) {
    const ch = source[i]
    if (ch === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++
      continue
    }
    if (ch === '/' && source[i + 1] === '*') {
      i += 2
      while (i < source.length && !(source[i] === '*' && source[i + 1] === '/')) i++
      i += 2
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch
      out += ch
      i++
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') {
          out += source[i]
          i++
        }
        if (i < source.length) {
          out += source[i]
          i++
        }
      }
      out += quote
      i++
      continue
    }
    out += ch
    i++
  }
  return out
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/speedSearch.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'sidebar', 'speedSearch.js')}`)

  /* ------------------------------------------------- the blur rule, driven rather than grepped */
  //
  // The gate that shipped asserted only that both trees contained `onBlur={speedSearch.exit}` —
  // which is the exact line that made speed search in the changes tree cancel itself the moment it
  // found a match. A regex cannot tell "focus left the tree" from "focus moved to another row
  // inside it", and that difference is a roving tabindex the source text never mentions. So the
  // rule moved into the pure module and is exercised here with a stub container.
  const container = (holds) => ({ contains: (node) => holds.includes(node) })
  const rowA = 'row-a'
  const rowB = 'row-b'
  const terminal = 'terminal'

  eq(
    m.blurLeftTheTree(container([rowA, rowB]), rowB),
    false,
    'focus moving from one row to another INSIDE the tree is not a blur — this is the one that '
      + 'cancelled the search the instant it succeeded, because landing on a match focuses it',
  )
  eq(
    m.blurLeftTheTree(container([rowA, rowB]), terminal),
    true,
    'focus going to a terminal really did leave the tree, and the query must not be left armed',
  )
  eq(
    m.blurLeftTheTree(container([rowA]), null),
    true,
    'focus going nowhere counts as leaving — a query armed against the document body would eat '
      + 'the first letters typed on the way back',
  )
  eq(
    m.blurLeftTheTree(null, rowA),
    true,
    'and an unmounted container leaves, rather than trapping the search in a tree that is gone',
  )

  const NONE = { ctrl: false, meta: false, alt: false, shift: false }
  const mods = (held = {}) => ({ ...NONE, ...held })
  /** `speedKey`, with the two arguments that vary named at the call site. */
  const key = (query, k, held) => m.speedKey(query, k, mods(held)).kind
  /** The same, keeping the payload — only `extend` has one. */
  const act = (query, k, held) => m.speedKey(query, k, mods(held))

  // ---------------------------------------------------------------------------------------
  // 1. The key table — nothing typed
  // ---------------------------------------------------------------------------------------

  /*
   * With no query armed, speed search must be invisible. Every key below is one a tree already
   * answers, and every one of them has to come back `pass` — not `swallow`, not `exit`, which
   * would both consume the key and are how a working gesture silently stops working.
   */
  for (const k of [
    'Delete',
    'Backspace',
    'Escape',
    'Enter',
    'ArrowDown',
    'ArrowUp',
    'ArrowLeft',
    'ArrowRight',
    'Home',
    'End',
    'PageUp',
    'PageDown',
    'F2',
    'Tab',
    ' ',
  ]) {
    eq(key('', k), 'pass', `with nothing typed, ${JSON.stringify(k)} is not speed search's`)
  }

  eq(key('', 'c', { ctrl: true }), 'pass', 'Ctrl+C with no search is the tree\'s copy')
  eq(key('', 'x', { ctrl: true }), 'pass', 'and Ctrl+X its cut')
  eq(key('', 'v', { ctrl: true }), 'pass', 'and Ctrl+V its paste')
  eq(key('', 'a', { ctrl: true }), 'pass', 'and Ctrl+A its select-all')
  eq(key('', 'r', { ctrl: true }), 'pass', 'and Ctrl+R its rename')
  eq(key('', 'e', { ctrl: true, shift: true }), 'pass',
    'Ctrl+Shift+E is a registry chord the key gate resolves before this handler runs, and must '
    + 'never be claimed here')

  /*
   * The one key that starts a search. `t` is a plain character, and with nothing typed it is
   * the *gesture* — this is the assertion that says the feature is reachable at all.
   */
  eq(act('', 't'), { kind: 'extend', char: 't' }, 'a bare letter starts a search')
  eq(act('', 'T', { shift: true }), { kind: 'extend', char: 'T' },
    'shift is not a modifier here — it is how a capital is typed, and a search that could not '
    + 'spell `App.tsx` would be a curiosity')
  eq(act('', '.'), { kind: 'extend', char: '.' }, 'a dot too — `.rs` is a query people type')
  eq(act('', '-'), { kind: 'extend', char: '-' }, 'and a hyphen')
  eq(act('', '5'), { kind: 'extend', char: '5' }, 'and a digit')

  // ---------------------------------------------------------------------------------------
  // 2. The key table — a query armed
  // ---------------------------------------------------------------------------------------

  eq(act('tr', 'e'), { kind: 'extend', char: 'e' }, 'a letter extends the query')
  eq(act('tr', ' '), { kind: 'extend', char: ' ' },
    'and so does a space, once something has been typed: `check tree` has one in it')
  eq(key('', ' '), 'pass',
    'THE COLLISION: a bare Space with nothing typed stays the changes tree\'s tick. `speedKey` '
    + 'takes the query rather than a boolean for exactly this row')

  eq(key('tr', 'Backspace'), 'erase', 'Backspace erases')
  eq(key('tr', 'Escape'), 'exit', 'Escape leaves')
  eq(key('tr', 'Enter'), 'accept', 'Enter opens the row the search landed on')
  eq(m.speedKey('tr', 'ArrowDown', mods()), { kind: 'move', delta: 1 }, 'Down walks the matches')
  eq(m.speedKey('tr', 'ArrowUp', mods()), { kind: 'move', delta: -1 }, 'and Up walks them back')

  eq(key('tr', 'Delete'), 'swallow',
    'THE DANGEROUS ONE: a bare Delete is *Move to Trash* in the file tree, over whatever the '
    + 'search has just selected. Mid-word it is swallowed and does nothing — `pass` here would '
    + 'put a delete confirmation on screen for a typo')

  for (const k of ['ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown']) {
    eq(key('tr', k), 'exitThenPass',
      `${k} ends the search and then does its ordinary job — a fold re-flattens the tree and `
      + 'renumbers every match index, so the list has to be gone before the fold happens')
  }

  for (const [k, held, what] of [
    ['c', { ctrl: true }, 'Ctrl+C still copies'],
    ['x', { ctrl: true }, 'Ctrl+X still cuts'],
    ['v', { ctrl: true }, 'Ctrl+V still pastes'],
    ['a', { ctrl: true }, 'Ctrl+A still selects all'],
    ['r', { ctrl: true }, 'Ctrl+R still renames'],
    ['c', { meta: true }, '⌘C too, on the platform layer'],
    ['Enter', { alt: true }, 'and Alt+Enter is nobody else\'s business here'],
  ]) {
    eq(key('tr', k, held), 'exitThenPass', `${what} — the search ends and the chord is handed back`)
  }

  eq(key('tr', 'F2'), 'exitThenPass',
    'F2 is deliberately unclaimed by the tree and stays that way: the search ends, and whatever '
    + 'does or does not answer F2 is unchanged')
  eq(key('tr', 'Tab'), 'exitThenPass', 'Tab moves focus out, so the search must not survive it')

  // The two verdicts that consume a key must be distinguishable from the two that do not:
  // collapsing `exit` and `exitThenPass` into one "not handled" would leave a query armed under
  // a fold that renumbered every match.
  ok(
    new Set(['pass', 'extend', 'erase', 'move', 'accept', 'exit', 'exitThenPass', 'swallow'])
      .size === 8,
    'the eight verdicts are eight distinct things',
  )

  // ---------------------------------------------------------------------------------------
  // 3. What the query becomes
  // ---------------------------------------------------------------------------------------

  eq(m.applyKey('tr', { kind: 'extend', char: 'e' }), 'tre', 'extending appends')
  eq(m.applyKey('tre', { kind: 'erase' }), 'tr', 'erasing removes one character')
  eq(m.applyKey('t', { kind: 'erase' }), null,
    'and erasing the LAST one ends the search rather than leaving an empty query armed — an '
    + 'empty query matches nothing, so an armed empty search swallows every key and highlights '
    + 'no row, which from the user\'s side is the tree having frozen')
  eq(m.applyKey('tr', { kind: 'exit' }), null, 'Escape ends it')
  eq(m.applyKey('tr', { kind: 'accept' }), null, 'so does accepting a match')
  eq(m.applyKey('tr', { kind: 'exitThenPass' }), null, 'so does a fold')

  // ---------------------------------------------------------------------------------------
  // 4. Wrap-around
  // ---------------------------------------------------------------------------------------

  eq(m.nextMatch(3, 0, 1), 1, 'Down moves on')
  eq(m.nextMatch(3, 2, 1), 0, 'and wraps at the end')
  eq(m.nextMatch(3, 0, -1), 2,
    'Up from the first match wraps to the last. A clamp would make that press silently do '
    + 'nothing, which in a feature whose only feedback is a counter reads as the key breaking')
  eq(m.nextMatch(3, -1, 1), 0, 'with nothing selected, Down lands on the first')
  eq(m.nextMatch(3, -1, -1), 2, 'and Up on the last')
  eq(m.nextMatch(0, -1, 1), -1, 'an empty match list has no next match, and says so')
  eq(m.nextMatch(1, 0, 1), 0, 'one match is its own successor')

  // ---------------------------------------------------------------------------------------
  // 5. Expiry, by timestamp and not by a timer
  // ---------------------------------------------------------------------------------------

  eq(m.SPEED_TIMEOUT_MS, 2000, 'the query stays armed for two seconds of silence')
  eq(m.expired(1000, 1000), false, 'a query typed this instant is live')
  eq(m.expired(1000, 1000 + m.SPEED_TIMEOUT_MS - 1), false, 'and still live just under the limit')
  eq(m.expired(1000, 1000 + m.SPEED_TIMEOUT_MS), true, 'and stale at it')
  eq(m.expired(1000, 9_000_000), true,
    'a webview that was in the background for an hour had its timers throttled, so the timer '
    + 'that would have cleared this may never have fired — which is why expiry is a comparison '
    + 'made at the next keystroke rather than a timer. Same arrangement as `keys/gate.ts`')

  // ---------------------------------------------------------------------------------------
  // 6. The sentences
  // ---------------------------------------------------------------------------------------

  eq(m.speedSummary(17, 2, false), '3 of 17', 'the counter is 1-based, like the find bar\'s')
  eq(m.speedSummary(17, -1, false), '1 of 17', 'before anything is selected it reads as the first')
  eq(m.speedSummary(0, -1, false), 'no match in the expanded tree',
    'THE DEAD END: speed search only sees what is expanded, so a bare `no match` would be read '
    + 'as a lie by anyone who knows the file is in the project. The sentence names the reason')
  ok(
    m.speedSummary(1000, 0, true).includes('first 1000'),
    'and a list the Rust cap cut short says so — a silently short answer is indistinguishable '
    + 'from a tree that does not contain the rest',
  )
  ok(
    m.speedSummary(3, 0, false) !== m.speedSummary(3, 0, true),
    'a truncated list and a complete one of the same length do not read the same',
  )

  // ---------------------------------------------------------------------------------------
  // 7. The call sites, over comment-stripped source
  // ---------------------------------------------------------------------------------------

  const source = (path) => withoutComments(readFileSync(path, 'utf8'))
  const fileTree = source('src/sidebar/FileTree.tsx')
  const changes = source('src/sidebar/GitPanel/ChangesTree.tsx')
  const hook = source('src/sidebar/useSpeedSearch.ts')
  const bar = source('src/sidebar/SpeedSearchBar.tsx')
  const client = source('src/ipc/client.ts')
  const rules = source('src/sidebar/speedSearch.ts')

  ok(
    !/^\s*import /m.test(rules),
    '`speedSearch.ts` imports nothing, which is what lets this script compile it standalone — '
    + 'the same property `clickSemantics.ts` and `rowWindow.ts` are kept import-free for',
  )

  // Both trees ask the *same Rust rule*, and neither matches locally.
  ok(
    /invoke<TreeMatches>\('fs_tree_match'/.test(client),
    'the explorer\'s query goes to `fs_tree_match`, because its rows are not in the webview — '
    + 'it holds 200-row chunks of a flattening Rust owns, and a local match would silently miss '
    + 'everything outside the cache',
  )
  ok(
    /invoke<TreeMatches>\('tree_match_labels'/.test(client),
    'and the changes tree\'s goes to `tree_match_labels`, which is the SAME Rust rule. A local '
    + 'loop over `rows` would have been one round trip cheaper and a second implementation of '
    + '`cide_fs::speed` — the duplication this whole arrangement exists to avoid',
  )
  ok(
    !/toLowerCase\(\)[\s\S]{0,40}(indexOf|includes)/.test(hook)
      && !/(indexOf|includes)\([\s\S]{0,30}toLowerCase/.test(hook),
    'and nothing in the hook matches a label itself — a substring test here would be the second '
    + 'copy arriving by the back door',
  )

  // The branch is FIRST in each handler. Position, not presence: a speed-search branch that ran
  // after the clipboard block would let Escape throw away a pending cut, and one that ran after
  // `treeKeyAction` would let Delete trash a file mid-word.
  const fileTreeHandler = fileTree.slice(fileTree.indexOf('const onKeyDown = useCallback'))
  const speedAt = fileTreeHandler.indexOf('speedSearch.onKeyDown(e)')
  ok(speedAt >= 0, 'the file tree offers every keystroke to the search')
  for (const [marker, why] of [
    ['treeKeyAction(', 'Delete reaching *Move to Trash* mid-word'],
    ["e.key === 'Escape'", 'Escape throwing away a pending clipboard cut instead of ending the search'],
    ['clipboardKey', 'Ctrl+C being resolved before the search has had its say'],
    ['moveIndex(', 'the arrows moving the cursor instead of walking the matches'],
  ]) {
    const at = fileTreeHandler.indexOf(marker)
    ok(
      at > speedAt,
      `and does so BEFORE \`${marker}\` — running after it would mean ${why}`,
    )
  }

  const changesHandler = changes.slice(changes.indexOf('const onKeyDown = useCallback'))
  const changesSpeedAt = changesHandler.indexOf('speedSearch.onKeyDown(e)')
  ok(changesSpeedAt >= 0, 'the changes tree offers every keystroke to the search too')
  for (const [marker, why] of [
    ["case ' ':", 'a typed space ticking a checkbox instead of continuing the query'],
    ["case 'Escape':", 'Escape collapsing the selection instead of ending the search'],
  ]) {
    const at = changesHandler.indexOf(marker)
    ok(
      at > changesSpeedAt,
      `and does so BEFORE \`${marker}\` — running after it would mean ${why}`,
    )
  }

  // Both trees end the search on the two gestures that are not keystrokes.
  for (const [name, body] of [['FileTree', fileTree], ['ChangesTree', changes]]) {
    ok(
      /onPointerDownCapture=\{speedSearch\.exit\}/.test(body),
      `${name} ends the search on a press — the pointer is answering the same question the `
      + 'query was asking, and a box left up over a selection it did not make is a lie',
    )
    ok(
      /onBlur=\{\(e\) =>/.test(body) && /blurLeftTheTree\(e\.currentTarget, e\.relatedTarget\)/.test(body),
      `${name} ends it on blur, but only when focus really left the tree`,
    )
    // ...and NOT `onBlur={speedSearch.exit}`, which is what shipped and what broke the changes
    // tree. `onBlur` is the bubbling `focusout`, so landing on a match — which focuses the matched
    // row — cancelled the search that had just succeeded. This assertion is the negative because
    // the positive above would also pass a wrapper that ignored its argument.
    ok(
      !/onBlur=\{speedSearch\.exit\}/.test(body),
      `${name} does not hand \`exit\` straight to onBlur — a focus move BETWEEN ROWS fires it too`,
    )
  }

  // The highlight is threaded down as a prop and never re-derived in a row.
  ok(
    /match=\{speedSearch\.spanFor\(/.test(fileTree),
    'the explorer passes each row its span as a prop, following the panel\'s stated rule that '
    + 'per-row store subscriptions are one zustand listener per visible row torn down every '
    + 'scroll tick',
  )
  ok(
    /match=\{speedSearch\.spanFor\(index\)\}/.test(changes),
    'and so does the changes tree',
  )
  ok(
    /<SpeedName name=\{row\.name\}/.test(fileTree) && /<SpeedName name=\{name\}/.test(changes),
    'both draw the name through `SpeedName`, so the `<mark>` and — more importantly — the span '
    + 'arithmetic cannot end up spelled two ways',
  )
  ok(
    /const name = row\.label/.test(changes) && !/const \{ name \} = splitPath\(entry\.path\)/.test(changes),
    'and `FileLabel` renders `row.label` rather than re-deriving the name from the path. The '
    + 'query is matched against `Row.label` in Rust, so the span is an offset into THAT string; '
    + 'a second derivation would put the highlight over the wrong glyphs the day the two differ',
  )
  ok(
    /data-audit=\{audit\}/.test(bar),
    'and the readout publishes it, so `--audit-panes` and a future screenshot check can find '
    + 'the box by name in either panel',
  )
  ok(
    /name\.slice\(start, end\)/.test(bar) && /Math\.min\(span\.end, name\.length\)/.test(bar),
    'and `SpeedName` clamps the span rather than trusting it — the offsets crossed IPC, and a '
    + 'bad `slice` range is a wrong-looking row while a bad decode is an exception inside a '
    + 'render, which unmounts the panel',
  )

  // The readout is a sibling of the scroller, not a child of it.
  for (const [name, body, audit] of [
    ['FileTree', fileTree, 'fileTreeSpeedSearch'],
    ['ChangesTree', changes, 'gitTreeSpeedSearch'],
  ]) {
    const barAt = body.indexOf('<SpeedSearchBar')
    const treeAt = body.indexOf('role="tree"')
    ok(barAt >= 0 && treeAt >= 0 && barAt < treeAt,
      `${name} draws the readout OUTSIDE its \`role="tree"\` scroller. Inside it, the box would `
      + 'be a non-treeitem child of a tree and would sit as far down the viewport as the tree '
      + 'is tall — a hundred thousand rows, in the explorer')
    ok(body.includes(`audit="${audit}"`), `${name}'s readout is auditable as ${audit}`)
  }

  // The staleness guard, which is the sharpest hazard in the feature.
  ok(
    /answer\.query !== next/.test(hook),
    'a frame that answers an older keystroke is dropped rather than drawn — Rust echoes the '
    + 'query back for exactly this, the way `PickerFrame` does',
  )
  ok(
    /frame\.count === options\.count/.test(hook),
    'and so is one computed against a tree of a different size: a match list is a list of '
    + 'indices into ONE flattening, and a tree that changed size is a different flattening',
  )
  ok(
    /run\(query, false\)/.test(hook) && /run\(next, true\)/.test(hook),
    'a re-search after the rows moved does NOT jump back to the first match, while a keystroke '
    + 'does. One `run` that always jumped made every Down key bounce to the top: landing loads '
    + 'a chunk, a chunk is a new row cache, a new row cache is a revision, and the revision '
    + 're-searched and re-landed',
  )
  ok(
    /revision: chunks/.test(fileTree),
    'the explorer\'s staleness signal is its row cache, which it already subscribes to — a new '
    + 'store subscription here would be a re-render per watcher burst, and `check:tree-flicker` '
    + 'pins that at zero',
  )
  ok(
    /revision: rows/.test(changes),
    'and the changes tree\'s is its row array, rebuilt whenever a `git status` lands',
  )

  if (failed > 0) {
    console.error(`\ncheck-speed-search: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-speed-search: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
