/**
 * Settings → Keymap: the arithmetic of the screen, and the wiring that makes it reachable.
 *
 * Two halves, and the second is the one this project keeps paying for.
 *
 * **The rules.** `src/settings/keymapModel.ts` and `src/keys/recorder.ts` are import-free so
 * the TypeScript already in `node_modules` can compile them alone and this script can drive
 * every case: which rows exist, which are customised, what a chord would collide with before
 * it is written, and what a keystroke means while one is being recorded. Those rules were in a
 * React component in the first draft, where no check could reach them — the shape five shipped
 * bugs came in through.
 *
 * **The wiring.** A recorder nothing arms, an edit command nothing calls and an event nothing
 * subscribes to are the seventeen-and-counting features this codebase has built and left
 * reachable from nothing. So the second half greps the call sites — with comments stripped,
 * because a gate that matched its own rationale went vacuous exactly that way, and deleting
 * the feature left it green.
 *
 * Run: `pnpm --dir ui run check:keymap`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-keymap-'))
let failed = 0

const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

/**
 * Source with block and line comments removed. See the module note for why.
 *
 * The line-comment pass deliberately does **not** match a `//` preceded by a colon, because
 * this codebase's event names are `cide://keymap-changed` and the naive regex truncates the
 * line at the scheme separator — which would make every assertion about an event name fail for
 * a reason that has nothing to do with the code. (A string literal genuinely containing `//`
 * after a non-colon would still be clipped; nothing here has one.)
 */
const readCode = (rel) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^[ \t]*\/\/[^\n]*$/gm, '')
    .replace(/([^:])\/\/[^\n]*/g, '$1')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/keys/chords.ts',
      'src/keys/recorder.ts',
      'src/settings/keymapModel.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      // `dom` for the same reason `check-key-gate.mjs` needs it: nothing here touches a DOM,
      // but tsc pulls every `@types` package in `node_modules` into the program by default and
      // `@types/react-dom` does not compile without the DOM declarations.
      '--lib', 'es2023,dom',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const model = require(join(out, 'settings/keymapModel.js'))
  const recorder = require(join(out, 'keys/recorder.js'))

  /* ------------------------------------------------------------------------ the fixture */

  /*
   * A registry slice, deliberately including the three shapes the old screen could not draw:
   * a command with no binding at all, a command that is `unavailable`, and a binding that
   * carries a `when`.
   */
  const COMMANDS = [
    { id: 'picker.files', title: 'Open file…', group: 'Navigate' },
    { id: 'palette.commands', title: 'Command palette', group: 'Navigate' },
    { id: 'file.save', title: 'Save', group: 'File' },
    // Palette-only: no default binding anywhere. Roughly half the registry looks like this,
    // and every one of them was absent from the screen before rows were built from commands.
    { id: 'file.saveAll', title: 'Save all', group: 'File' },
    { id: 'git.pull', title: 'Pull', group: 'Git' },
    { id: 'tab.console', title: 'Go to Claude console', group: 'Project' },
    {
      id: 'pane.promoteToTab',
      title: 'Promote pane to tab',
      group: 'Window',
      unavailable: 'no domain operation behind it yet',
    },
    // A command whose *palette* clause exists. It must never become a binding's `when`: one
    // gates a list, the other gates the keyboard.
    { id: 'sidebar.toggle', title: 'Toggle panel', group: 'View', when: 'shellWindow' },
  ]

  const BINDINGS = [
    { key: 'ctrl+p', command: 'picker.files', when: null, layer: 'default' },
    { key: 'ctrl+shift+p', command: 'palette.commands', when: null, layer: 'default' },
    { key: 'ctrl+s', command: 'file.save', when: null, layer: 'default' },
    { key: 'ctrl+t', command: 'git.pull', when: null, layer: 'default' },
    { key: 'ctrl+1', command: 'tab.console', when: 'shellWindow', layer: 'default' },
    { key: 'f4', command: 'sidebar.toggle', when: 'shellWindow', layer: 'default' },
  ]

  /* ---------------------------------------------------------------------------- the rows */

  {
    const rows = model.buildRows(COMMANDS, BINDINGS, [])
    eq(rows.length, COMMANDS.length, 'every command gets a row, bound or not')

    const byId = new Map(rows.map((row) => [row.id, row]))
    eq(byId.get('picker.files').bindings, [{ key: 'ctrl+p', when: null, layer: 'default' }], 'a bound row carries its binding')
    eq(byId.get('file.saveAll').bindings.length, 0, 'a palette-only command has a row and no binding')
    eq(byId.get('file.saveAll').unbound, false, 'and is not "unbound by you" — nobody unbound it')
    eq(model.customised(byId.get('file.saveAll')), false, 'nor customised')

    // The scoped binding keeps its clause, normalised, all the way to the row.
    eq(byId.get('tab.console').bindings[0].when, 'shellWindow', 'a scoped binding keeps its when')
    // …and the *command*'s own clause never becomes one.
    eq(byId.get('sidebar.toggle').bindings[0].when, 'shellWindow', 'the binding’s clause is the binding’s')
    eq(
      model.editTarget(byId.get('file.saveAll'), null).when,
      null,
      'a command with no binding is bound unconditionally — Command::when gates the palette, not the keyboard',
    )

    ok(!model.bindable(byId.get('pane.promoteToTab')), 'an unavailable command is not bindable')
    ok(model.bindable(byId.get('file.save')), 'an available one is')

    // Group, then title, which is the palette's order.
    const order = rows.map((row) => `${row.group}/${row.title}`)
    eq(order, [...order].sort((a, b) => a.localeCompare(b)), 'rows are grouped and then alphabetical')
  }

  /* ------------------------------------------------------- what the user's file adds to it */

  {
    // A rebind: two entries in the file, one resolved binding on the new key.
    const overrides = [
      { key: 'ctrl+p', command: '-picker.files', when: null },
      { key: 'ctrl+shift+o', command: 'picker.files', when: null },
    ]
    const resolved = [
      ...BINDINGS.filter((b) => b.command !== 'picker.files'),
      { key: 'ctrl+shift+o', command: 'picker.files', when: null, layer: 'user' },
    ]
    const row = model.buildRows(COMMANDS, resolved, overrides).find((r) => r.id === 'picker.files')
    eq(row.overrides, 2, 'both halves of a rebind count as this command’s overrides')
    ok(model.customised(row), 'so the row reads as customised')
    eq(row.unbound, false, 'it is bound, just not where it shipped')
    eq(row.bindings[0].layer, 'user', 'and the layer says whose it is')
  }
  {
    /*
     * The row a resolved-bindings list cannot draw at all.
     *
     * A `-command` entry produces **no** resolved binding, so before `KeymapReport.overrides`
     * existed a user who unbound Ctrl+T saw the row silently revert to "Ctrl+T — default"…
     * except it did not, because `git.pull` then had no binding and the screen listed
     * bindings. The row vanished, and with it the only place to press *Restore default*.
     */
    const overrides = [{ key: 'ctrl+t', command: '-git.pull', when: null }]
    const resolved = BINDINGS.filter((b) => b.command !== 'git.pull')
    const row = model.buildRows(COMMANDS, resolved, overrides).find((r) => r.id === 'git.pull')
    eq(row.bindings.length, 0, 'nothing resolves')
    eq(row.unbound, true, 'and the screen can still say who did that')
    ok(model.customised(row), 'and offer to undo it')
  }
  {
    // A `keymap.json` naming an id this build does not have. Invisible everywhere else.
    const overrides = [{ key: 'ctrl+j', command: 'pane.doTheThing', when: null }]
    const rows = model.buildRows(COMMANDS, BINDINGS, overrides)
    const row = rows.find((r) => r.id === 'pane.doTheThing')
    ok(row !== undefined, 'an unknown command id still gets a row')
    eq(row.unknown, true, 'marked as unknown')
    eq(row.group, model.UNKNOWN_GROUP, 'under its own group')
    eq(rows[rows.length - 1].id, 'pane.doTheThing', 'sorted last, whatever it is called')
  }

  /* ---------------------------------------------------------- one line per binding, not per row */

  {
    const twice = [
      { key: 'ctrl+p', command: 'picker.files', when: 'editorFocused', layer: 'user' },
      { key: 'ctrl+o', command: 'picker.files', when: 'terminalFocused', layer: 'user' },
    ]
    const rows = model.buildRows(COMMANDS, twice, [])
    const entries = model.entriesOf(rows).filter((e) => e.row.id === 'picker.files')
    eq(entries.length, 2, 'a command bound twice gets two lines')
    eq(
      entries.map((e) => model.editTarget(e.row, e.binding).when),
      ['editorFocused', 'terminalFocused'],
      'and each line edits its own context — a removal matches on when, so guessing removes nothing',
    )
    eq(
      model.entriesOf(rows).filter((e) => e.row.id === 'file.saveAll').length,
      1,
      'a command with no binding still gets exactly one line',
    )
  }

  /* -------------------------------------------------------------- what a chord would run into */

  {
    const clashes = model.clashesFor(BINDINGS, 'ctrl+p', null, 'file.saveAll')
    eq(clashes.length, 1, 'a taken chord is reported')
    eq(clashes[0].command, 'picker.files', 'naming who holds it')
    eq(clashes[0].kind, 'same', 'as the same keystroke')
    eq(
      clashes[0].contested,
      false,
      'and a default is *shadowed*, not contested — nothing will be reported afterwards',
    )
  }
  {
    const mine = [{ key: 'ctrl+p', command: 'picker.files', when: null, layer: 'user' }]
    eq(
      model.clashesFor(mine, 'ctrl+p', null, 'file.saveAll')[0].contested,
      true,
      'a chord taken from the user’s own layer is a real conflict: both survive',
    )
  }
  {
    /*
     * An UNSCOPED binding overlaps every scope, in both directions.
     *
     * This assertion used to read "a different context is not a clash" and expect `[]`, which is
     * true of two *scoped* contexts and false whenever either side is null: `picker.files` on
     * ctrl+p carries no clause, so it applies in an editor too, and binding something else to
     * ctrl+p `when editorFocused` really does give that keystroke two commands there.
     *
     * The direction below it is the one that shipped a bug: with no clause at all, an added
     * binding took F4 from the panel toggle with no dialog, no conflict row and no diagnostic —
     * the one collision a keymap editor exists to catch, invisible because two `when` strings
     * were compared for equality and `null` contains all of them.
     */
    eq(
      model.clashesFor(BINDINGS, 'ctrl+p', 'editorFocused', 'file.saveAll').map((c) => c.command),
      ['picker.files'],
      'an unscoped standing binding is contested by a scoped candidate on the same key',
    )
    eq(
      model.clashesFor(BINDINGS, 'f4', null, 'file.saveAll').map((c) => c.command),
      ['sidebar.toggle'],
      'and an unscoped CANDIDATE contests a scoped standing binding — the F4 case that shipped',
    )
    // ...while two genuinely different scopes stay disjoint, which is the mechanism that lets one
    // key mean different things in a terminal and in an editor. Widening this would put a
    // confirmation in front of the ordinary case.
    eq(
      model.clashesFor(
        [{ key: 'f4', command: 'sidebar.toggle', when: 'shellWindow', layer: 'default' }],
        'f4',
        'editorFocused',
        'file.saveAll',
      ),
      [],
      'two different non-null contexts are not a clash',
    )
    eq(model.clashesFor(BINDINGS, 'ctrl+p', null, 'picker.files'), [], 'a command does not clash with itself')
    eq(model.clashesFor(BINDINGS, 'ctrl+alt+shift+f9', null, 'file.saveAll'), [], 'a free chord is free')
    // Spelling is not the question — the keystroke is. Rust cannot see this one: `normalize_key`
    // renames no keys, so ``ctrl+` `` and `ctrl+backquote` are two keys there and one here.
    const backquote = [{ key: 'ctrl+`', command: 'project.switcher.next', when: null, layer: 'default' }]
    eq(
      model.clashesFor(backquote, 'ctrl+backquote', null, 'file.saveAll').length,
      1,
      'the alias table folds the two spellings of one physical key',
    )
    eq(model.clashesFor(backquote, 'CTRL+~', null, 'file.saveAll').length, 1, 'including the shifted face of it')
  }
  {
    /*
     * The two silent ones. `keys/keymap.ts` prefers an applicable *continuation* over an exact
     * match — which is what makes `ctrl+k ctrl+s` reachable — so a chord and a sequence that
     * starts with it cannot both fire, whichever order they were bound in.
     */
    const sequence = [{ key: 'ctrl+k ctrl+s', command: 'settings.keymap', when: null, layer: 'user' }]
    const asPrefix = model.clashesFor(sequence, 'ctrl+k', null, 'file.saveAll')
    eq(asPrefix.length, 1, 'binding the first stroke of an existing sequence is reported')
    eq(asPrefix[0].kind, 'prefix', 'as a prefix clash — the new binding could never fire')

    const single = [{ key: 'ctrl+k', command: 'git.pull', when: null, layer: 'user' }]
    const asExtension = model.clashesFor(single, 'ctrl+k ctrl+s', null, 'file.saveAll')
    eq(asExtension.length, 1, 'and so is the reverse')
    eq(asExtension[0].kind, 'extends', 'where it is the existing binding that stops firing')
  }

  /* ------------------------------------------------------- the chord that costs everyone else */

  {
    ok(model.bareKeyWarning('a') !== null, 'an unmodified letter is worth a warning')
    ok(model.bareKeyWarning('shift+a') !== null, 'and shift alone is still typing')
    eq(model.bareKeyWarning('ctrl+a'), null, 'a modifier makes it a shortcut')
    eq(model.bareKeyWarning('alt+a'), null, 'any of the three')
    ok(model.bareKeyWarning('space') !== null, 'space is typing')
    ok(model.bareKeyWarning('tab') !== null, 'so is tab')
    eq(model.bareKeyWarning('f4'), null, 'an f-key is not')
    eq(model.bareKeyWarning('escape'), null, 'nor is escape')
    eq(model.bareKeyWarning('up'), null, 'nor an arrow')
    // Only the first stroke matters: the rest are read while a prefix is armed, which the user
    // entered deliberately a keystroke earlier.
    eq(model.bareKeyWarning('ctrl+k a'), null, 'the second stroke of a sequence is safe')
    ok(model.bareKeyWarning('a ctrl+k') !== null, 'the first one is not')
  }

  /* ------------------------------------------------------------------------ the two footers */

  {
    ok(model.overridesSummary(0).includes('No overrides'), 'an empty file says so')
    ok(model.overridesSummary(1).includes('1 override'), 'one is singular')
    ok(model.overridesSummary(7).includes('7 overrides'), 'seven are not')
    ok(
      model.editSummary(0, 0).toLowerCase().includes('nothing changed'),
      'an edit that matched nothing must say so — silence here is the failure the counts exist for',
    )
    ok(model.editSummary(2, 0).includes('2 entries removed'), 'a reset reports what it deleted')
    ok(model.editSummary(1, 2).includes('2 entries written'), 'and a rebind what it wrote')
  }

  /* --------------------------------------------------------------------------- the recorder */

  {
    const empty = recorder.EMPTY
    eq(recorder.canSave(empty), false, 'nothing recorded is not saveable')
    eq(recorder.capture(empty, 'ctrl+p'), { kind: 'record', next: { strokes: ['ctrl+p'], extending: false } }, 'a chord is data')

    const one = recorder.record(empty, 'ctrl+p')
    eq(recorder.sequenceOf(one), 'ctrl+p', 'one stroke')
    eq(recorder.canSave(one), true, 'and it can be saved')

    // Replace, not append: a fumbled chord must not silently become a two-stroke sequence.
    eq(recorder.record(one, 'ctrl+o').strokes, ['ctrl+o'], 'a second press replaces')
    eq(recorder.canExtend(one), true, 'until a second stroke is asked for')
    eq(recorder.record(recorder.extend(one), 'ctrl+s').strokes, ['ctrl+p', 'ctrl+s'], 'and then it appends')

    const full = recorder.record(recorder.extend(one), 'ctrl+s')
    eq(recorder.canExtend(full), false, 'two strokes is the cap')
    eq(recorder.extend(full), full, 'asking for a third changes nothing')
    eq(recorder.record(full, 'ctrl+x').strokes, ['ctrl+x'], 'a press past the cap starts over rather than being ignored')
    eq(recorder.canExtend(empty), false, 'and there is nothing to extend before the first press')
  }
  {
    // The two verbs, and exactly how narrow they are.
    eq(recorder.capture(recorder.EMPTY, 'escape').kind, 'cancel', 'bare escape cancels')
    eq(recorder.capture(recorder.record(recorder.EMPTY, 'ctrl+p'), 'enter').kind, 'commit', 'bare enter saves')
    eq(recorder.capture(recorder.EMPTY, 'enter').kind, 'wait', 'unless there is nothing to save')
    for (const stroke of ['shift+escape', 'ctrl+escape', 'shift+enter', 'ctrl+enter', 'tab', 'space', 'f4']) {
      eq(
        recorder.capture(recorder.EMPTY, stroke).kind,
        'record',
        `${stroke} is recordable — only the *bare* two are spent`,
      )
    }
  }
  {
    eq(recorder.chipFor(recorder.EMPTY), '', 'an empty recording shows nothing')
    eq(recorder.chipFor(recorder.record(recorder.EMPTY, 'ctrl+shift+p')), '⌃⇧P', 'the chip is the palette’s')
    eq(
      recorder.chipFor(recorder.extend(recorder.record(recorder.EMPTY, 'ctrl+k'))),
      '⌃K …',
      'and an owed second stroke is the readout `onPending` was written for',
    )
    eq(recorder.MAX_STROKES, 2, 'the cap is a UI decision, not a format one')
  }

  /* ----------------------------------------------------------- and now: is any of it reachable */

  {
    const useGate = readCode('../src/keys/useKeyGate.ts')
    ok(
      /capture:\s*\(stroke\)\s*=>\s*recorderCapture\(stroke\)\s*\|\|\s*switcherCapture\(stroke\)/.test(useGate),
      'the gate’s one capture hook is composed of both claims, recorder first ' +
        '(the recorder is modal; a switcher walk cannot legitimately be open behind it)',
    )

    const store = readCode('../src/keys/recorderStore.ts')
    ok(
      store.includes('currentKeyGate()?.reset()'),
      'startRecording clears any armed chord prefix — an armed prefix outranks the capture, so ' +
        'without this the first stroke the user records completes somebody else’s sequence',
    )
    ok(
      store.includes('cancelWalk()'),
      'and cancels an open switcher walk — its keyup latch is not the gate’s and would activate ' +
        'a tab out from under the popup',
    )
    /*
     * The claim is **total** while armed, which is one line and the whole of the feature's
     * safety: a chord being recorded must not also run. Asserted as "the function ends with
     * `return true`" rather than "contains it", because a per-stroke answer would be exactly
     * the plausible-looking regression — Ctrl+P recorded *and* the file picker opened.
     */
    const claim = store.slice(store.indexOf('export function recorderCapture'))
    const afterGuard = claim.slice(claim.indexOf('return false') + 'return false'.length)
    eq(
      (afterGuard.match(/\breturn\b[^\n]*/g) ?? []).map((line) => line.trim()),
      ['return true'],
      'and the claim is total while it is armed: past the closed-recorder guard, recorderCapture ' +
        'has exactly one exit and it is `return true` (a per-stroke answer is the plausible ' +
        'regression — Ctrl+P recorded *and* the file picker opened)',
    )

    const section = readCode('../src/settings/KeymapSection.tsx')
    ok(section.includes('startRecording('), 'the screen arms the recorder')
    ok(section.includes('settingsApi.keymapEdit('), 'and calls the write command')
    ok(section.includes('useEffect(() => stopRecording'), 'and disarms it when the section unmounts')
    ok(section.includes('buildRows('), 'and builds its rows from the registry, not from the bindings')
    ok(section.includes('clashesFor('), 'and warns before it writes')
    for (const edit of ['rebind', 'unbind', 'reset', 'resetAll']) {
      ok(section.includes(`kind: '${edit}'`), `and every edit the wire offers has a button: ${edit}`)
    }

    /*
     * **The frontend never assembles a `keymap.json` entry.**
     *
     * A removal is `-command` with the losing binding's own `when`, and working out which
     * removals a change needs requires the compiled-in defaults. Building one here would be a
     * second implementation of layering, in the language that cannot see the defaults.
     */
    for (const rel of ['../src/settings/KeymapSection.tsx', '../src/settings/keymapModel.ts']) {
      const code = readCode(rel)
      ok(
        !/[`'"]-\$\{/.test(code) && !/'-'\s*\+/.test(code),
        `${rel} does not build a \`-command\` removal directive — that is cide_core::keymap::apply_edit's job`,
      )
    }

    const client = readCode('../src/ipc/client.ts')
    ok(client.includes("invoke<KeymapEditResult>('keymap_edit'"), 'client.ts is the seam for the write')
    ok(client.includes("'cide://keymap-changed'"), 'and for the broadcast')

    const workspace = readCode('../src/store/workspace.ts')
    ok(
      workspace.includes('events.onKeymapChanged('),
      'the store subscribes to the keymap broadcast — applySnapshot keeps the old keymap array, ' +
        'so cide://workspace-changed is precisely the path that cannot refresh a binding',
    )
    ok(
      /set\(\{\s*boot:\s*\{\s*\.\.\.boot,\s*keymap\s*\}\s*\}\)/.test(workspace),
      'and installs a *new* array, which is what makes the gate rebuild its indexed keymap',
    )

    const commands = readFileSync(fileURLToPath(new URL('../../contract/commands.json', import.meta.url)), 'utf8')
    ok(JSON.parse(commands).includes('keymap_edit'), 'the command is in the contract')
    const events = readFileSync(fileURLToPath(new URL('../../contract/events.json', import.meta.url)), 'utf8')
    ok(JSON.parse(events).includes('cide://keymap-changed'), 'and so is the event')
  }

  if (failed > 0) {
    console.error(`\ncheck-keymap: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-keymap: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
