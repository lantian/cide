/**
 * Checks `src/chrome/menuModel.ts` — every enablement decision the header's and the tab
 * strip's menus make.
 *
 * These decisions are the feature. A context menu is a list of sentences saying what a user may
 * do here and, when they may not, why; get one of them wrong and the symptom is either a line
 * that silently does nothing or a line that quietly acts on the wrong thing. Neither shows up
 * as a crash, neither is caught by `tsc`, and there is no browser in this harness to click one
 * in. So the rules live in a DOM-free module and this script exercises them directly.
 *
 * Same shape as `check-menus.mjs`, which does the same for the menu *system*: a bare `tsc` over
 * one file, then import the output and assert. `menuModel.ts` imports only types, so the
 * compile pulls in no React, no CSS module and no `@tauri-apps/api`.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the menu opens. `useContextMenu` owns that, and `check-menus.mjs` pins its geometry
 *     and keyboard behaviour.
 *   - that `⊞ bash row` is *in* the header. That is JSX, checked by `tsc` and by eye.
 *   - that the picker opens in front of the window. That needs a running GUI and a compositor;
 *     see `cmd::project::project_pick`, which says so in as many words.
 *
 * Run: `pnpm --dir ui run check:menu-model`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-menu-model-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

/** The item with this id, or `undefined`. Separators have no id and never match. */
const item = (entries, id) => entries.find((e) => e.id === id)
/** Ids in order, separators shown as `--`, which is what makes grouping assertable. */
const shape = (entries) => entries.map((e) => (e.kind === 'separator' ? '--' : e.id))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/menuModel.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The project sets both, and this module is written for them.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const {
    MAXIMIZED_REASON,
    MISSING_REASON,
    NOT_ACTIVE,
    NO_CLIPBOARD,
    NO_HOST,
    NO_PROJECT_REASON,
    NO_RECENTS_REASON,
    ONLY_PROJECT,
    PINNED_REASON,
    closable,
    overflowEntries,
    pathOf,
    projectTabEntries,
    recentEntries,
    rowGate,
    rowTarget,
    tabMenuEntries,
  } = await import(`file://${join(out, 'chrome/menuModel.js')}`)

  const NOTHING = { browse() {}, reopen() {}, forgetMissing() {}, clear() {} }
  const recent = (path, exists) => ({
    project: { path, displayPath: path, name: path, openedAt: 0n },
    exists,
  })

  // =====================================================================================
  // 1. Recent projects — the entry whose folder has gone
  // =====================================================================================

  {
    const entries = recentEntries([recent('~/a', true), recent('~/gone', false)], NOTHING)

    ok(typeof item(entries, 'recent:~/a').run === 'function', 'a live project can be reopened')
    // The requirement, verbatim: showing it is fine, silently failing to open it is not.
    eq(item(entries, 'recent:~/gone').run, undefined, 'a missing folder is not clickable')
    eq(
      item(entries, 'recent:~/gone').disabledReason,
      MISSING_REASON,
      'a missing folder says why, rather than being greyed out in silence',
    )
    ok(
      item(entries, 'recent:~/gone') !== undefined,
      'a missing folder is still listed — dropping it is indistinguishable from forgetting it',
    )
    eq(
      item(entries, 'forget-missing').label,
      'Remove 1 missing project',
      'the removal offer is singular for one',
    )
  }

  {
    const entries = recentEntries([recent('~/x', false), recent('~/y', false)], NOTHING)
    eq(item(entries, 'forget-missing').label, 'Remove 2 missing projects', '…and plural for two')
  }

  {
    const entries = recentEntries([recent('~/a', true)], NOTHING)
    eq(item(entries, 'forget-missing'), undefined, 'nothing missing, nothing to remove')
    ok(item(entries, 'clear').danger === true, 'clearing the list is destructive and painted so')
  }

  {
    // The empty list must still be an *answer*. `useContextMenu` refuses to open a menu with
    // no items at all, so returning `[]` here would make the caret look broken on first launch.
    const entries = recentEntries([], NOTHING)
    eq(shape(entries), ['browse', '--', 'empty'], 'an empty list still offers Open folder…')
    eq(item(entries, 'empty').disabledReason, NO_RECENTS_REASON, 'and says why it is empty')
    ok(typeof item(entries, 'browse').run === 'function', 'Open folder… is always live')
  }

  {
    // Two checkouts of one repository share a basename. Labelling by name would put `cide` in
    // the menu twice with no way to tell them apart.
    const entries = recentEntries([recent('~/work/cide', true), recent('~/tmp/cide', true)], NOTHING)
    eq(
      [item(entries, 'recent:~/work/cide').label, item(entries, 'recent:~/tmp/cide').label],
      ['~/work/cide', '~/tmp/cide'],
      'entries are labelled by path, not by basename',
    )
  }

  // =====================================================================================
  // 2. Project tabs
  // =====================================================================================

  {
    const one = { id: 'p1', displayPath: '~/a' }
    const two = { id: 'p2', displayPath: '~/b' }
    const closed = []
    const entries = projectTabEntries([one, two], one, {
      close: (id) => closed.push(id),
      reveal: () => {},
      copy: () => {},
    })

    eq(
      shape(entries),
      ['close', 'close-others', '--', 'reveal', 'copy-path'],
      'the project tab menu is the four items the request named',
    )
    item(entries, 'close-others').run()
    eq(closed, ['p2'], 'Close others closes every project but this one')
  }

  {
    const only = { id: 'p1', displayPath: '~/a' }
    const entries = projectTabEntries([only], only, { close: () => {} })
    eq(item(entries, 'close-others').disabledReason, ONLY_PROJECT, 'nothing else to close')
    // No clipboard in this webview ⇒ the item is disabled with a reason, never drawn and dead.
    eq(item(entries, 'copy-path').disabledReason, NO_CLIPBOARD, 'Copy path needs a clipboard')
  }

  // =====================================================================================
  // 3. Workspace tabs
  // =====================================================================================

  const tab = (id, kind) => ({ id, kind, tree: {} })
  const console_ = tab('t0', { kind: 'claudeHome' })
  const file = tab('t1', { kind: 'file', path: '/repo/src/main.rs', dirty: false })
  const extra = tab('t2', { kind: 'claudeFull', title: 'Claude' })
  const tabs = [console_, file, extra]

  /**
   * Record every bulk close as **one call with a list**, not as N calls.
   *
   * The list-of-lists shape is the assertion, not a convenience: the defect being fixed is three
   * menu items that fired one close per tab and so raised one confirmation dialog per unsaved
   * file, for a gesture the user made once. `[['t1','t2']]` and `[['t1'],['t2']]` both flatten to
   * the same ids, so a check that only looked at the ids would pass against the bug.
   */
  const recorder = () => {
    const calls = []
    return { calls, closeMany: (ids) => calls.push(ids) }
  }

  ok(!closable(console_), 'the pinned console is not closable')
  ok(closable(file) && closable(extra), 'everything else is')
  eq(pathOf(file), '/repo/src/main.rs', 'a file tab is about its path')
  eq(pathOf(console_), null, 'a console tab is about nothing on disk')

  {
    const bulk = recorder()
    const entries = tabMenuEntries(tabs, file, 't1', {
      close: () => {},
      closeMany: bulk.closeMany,
      split: () => {},
      detach: () => {},
      copy: () => {},
    })

    eq(
      shape(entries),
      [
        'close',
        'close-others',
        'close-left',
        'close-right',
        '--',
        'split',
        'detach',
        '--',
        'copy-path',
      ],
      'the workspace tab menu is the seven items, in three groups, with left before right',
    )
    item(entries, 'close-right').run()
    eq(bulk.calls, [['t2']], 'Close to the right closes only what follows this tab, in one call')

    bulk.calls.length = 0
    item(entries, 'close-others').run()
    eq(bulk.calls, [['t2']], 'Close others skips the pinned console as well as this tab')
  }

  // ---------------------------------------------------------------- Close to the left
  //
  // The whole point of the item, and the case the naive `slice(0, index)` gets wrong: the
  // console is at index 0, so it is to the left of *everything*. An implementation that did not
  // filter by `closable` would offer this on `t1` and then fail against `CoreError::TabPinned` —
  // a menu entry that errors, which is worse than one that is not offered.
  {
    const bulk = recorder()
    const four = [console_, file, extra, tab('t3', { kind: 'settings', section: 'general' })]
    const entries = tabMenuEntries(four, four[2], 't0', { close: () => {}, ...bulk })
    item(entries, 'close-left').run()
    eq(bulk.calls, [['t1']], 'Close to the left closes what precedes this tab and never the console')
  }
  {
    const bulk = recorder()
    const entries = tabMenuEntries(tabs, file, 't1', { close: () => {}, ...bulk })
    eq(
      item(entries, 'close-left').disabledReason,
      'Nothing to the left can be closed',
      'the first closable tab has only the pinned console to its left, and the line says so '
        + 'rather than being absent or present-and-inert',
    )
    ok(item(entries, 'close-left').run === undefined, 'and it genuinely cannot be run')
  }
  {
    const bulk = recorder()
    const entries = tabMenuEntries(tabs, console_, 't0', { close: () => {}, ...bulk })
    eq(
      item(entries, 'close-left').disabledReason,
      'Nothing to the left can be closed',
      'and the console itself has nothing to its left at all',
    )
  }

  {
    // Offered and disabled with a reason rather than dropped: an item that comes and goes with
    // the tab order is one the user has to hunt for.
    const entries = tabMenuEntries(tabs, extra, 't2', { close: () => {}, closeMany: () => {} })
    eq(
      item(entries, 'close-right').disabledReason,
      'Nothing to the right can be closed',
      'the last tab still shows Close to the right, and says why it is off',
    )
  }

  /*
   * A host that supplies `close` but not `closeMany` gets three disabled lines, not three lines
   * that quietly close one tab at a time.
   *
   * This is the assertion that stops the loop coming back. The tempting "fallback to `close`"
   * would leave two behaviours in the app for one gesture — one asking once, one asking per file
   * — and the wrong one would survive in whichever surface nobody re-tested.
   */
  {
    // A tab with something on *both* sides, so all three sets are non-empty and the only reason
    // any of them could be off is the missing door. On `t1` the left set is legitimately empty
    // (only the console precedes it) and that more specific sentence wins, which is right — and
    // would make this assertion pass for the wrong reason.
    const four = [console_, file, extra, tab('t3', { kind: 'settings', section: 'general' })]
    const entries = tabMenuEntries(four, extra, 't2', { close: () => {} })
    for (const id of ['close-others', 'close-left', 'close-right']) {
      eq(item(entries, id).disabledReason, NO_HOST, `${id} needs the bulk door, not \`close\``)
      ok(item(entries, id).run === undefined, `and ${id} is not runnable without it`)
    }
    ok(typeof item(entries, 'close').run === 'function', 'while single Close still works')
  }

  {
    const bulk = recorder()
    const entries = tabMenuEntries(tabs, console_, 't0', { close: () => {}, ...bulk })
    eq(
      item(entries, 'close').disabledReason,
      PINNED_REASON,
      'the console refuses to close and says it is pinned, not that it is unavailable',
    )
    const right = item(entries, 'close-right')
    ok(typeof right.run === 'function', 'the console can close the tabs to its right')
    right.run()
    eq(bulk.calls, [['t1', 't2']], 'and it closes both of them in one gesture, not two')
  }

  {
    // The mismatch this guard exists for: `split`/`detach` act on the *active* tab's focused
    // pane, so offering them on any other tab would act somewhere the user was not pointing.
    const entries = tabMenuEntries(tabs, extra, 't1', {
      close: () => {},
      split: () => {},
      detach: () => {},
    })
    eq(item(entries, 'split').disabledReason, NOT_ACTIVE, 'Split is refused on an inactive tab')
    eq(item(entries, 'detach').disabledReason, NOT_ACTIVE, 'so is Detach')
  }

  {
    // No handler from the host — which is exactly `App.tsx`'s state for detach today.
    const entries = tabMenuEntries(tabs, file, 't1', { close: () => {} })
    eq(item(entries, 'detach').disabledReason, NO_HOST, 'an unwired detach says so')
    eq(item(entries, 'copy-path').disabledReason, NO_CLIPBOARD, 'and so does an absent clipboard')
  }

  {
    const entries = tabMenuEntries(tabs, console_, 't0', { close: () => {}, copy: () => {} })
    eq(
      item(entries, 'copy-path').disabledReason,
      'This tab is not a file',
      'a console tab has no path to copy',
    )
  }

  // =====================================================================================
  // 4. The `▾` list of tabs the strip is hiding
  // =====================================================================================
  //
  // The geometry — *which* tabs are out of view — is `chrome/tabOverflow.ts` and is exercised by
  // `check:tab-overflow`. What is checked here is the half that decides what a user reads and
  // what they may do: the list is reachability only, it never offers to close anything, and it
  // does not lie about which file a line names.

  {
    const modRs = tab('t3', { kind: 'file', path: '/repo/src/lib/mod.rs', dirty: false })
    const modRs2 = tab('t4', { kind: 'file', path: '/repo/src/net/mod.rs', dirty: true })
    const settings = tab('t5', { kind: 'settings' })
    const all = [console_, file, extra, modRs, modRs2, settings]

    const activated = []
    const entries = overflowEntries(all, ['t1', 't5'], { activate: (id) => activated.push(id) })

    eq(shape(entries), ['overflow:t1', 'overflow:t5'], 'one line per hidden tab, and only those')
    eq(
      entries.map((e) => e.label),
      ['main.rs', 'Settings'],
      'labelled the way the strip and the switcher label them',
    )
    entries.forEach((e) => e.run())
    eq(activated, ['t1', 't5'], 'and picking a line activates that tab')

    /*
     * The requirement, expressed as an assertion rather than as a comment: this control only
     * activates. `OverflowActions` has no `close` field at all, so there is no door for one to
     * come through — but a future edit could still add a `danger`-flagged line, and the strip is
     * the one surface in this app where a mis-click is a lost buffer.
     */
    ok(
      entries.every((e) => e.danger === undefined),
      'no line in the overflow list is destructive',
    )
    ok(
      !JSON.stringify(entries.map((e) => [e.id, e.label])).toLowerCase().includes('close'),
      'and none of them so much as mentions closing',
    )

    // Strip order, not measurement order. The list has to read the way the strip reads or it is
    // one more thing to translate.
    eq(
      shape(overflowEntries(all, ['t5', 't0', 't3'], { activate: () => {} })),
      ['overflow:t0', 'overflow:t3', 'overflow:t5'],
      'the list is in strip order whatever order the ids were measured in',
    )

    /*
     * The pinned console is listed like anything else. Its pin is about closing and moving; a
     * strip scrolled far enough right to hide it is exactly when a user needs it in the list.
     */
    ok(
      typeof item(overflowEntries(all, ['t0'], { activate: () => {} }), 'overflow:t0').run
        === 'function',
      'the pinned console is reachable from the list: the pin is about closing, not reaching',
    )

    /*
     * Duplicate basenames. Four clipped `mod.rs` lines answer nothing, and a Rust or Go tree is
     * full of them. Only the colliders expand — `main.rs` beside them stays short.
     */
    const dupes = overflowEntries(all, ['t1', 't3', 't4'], { activate: () => {} })
    eq(
      dupes.map((e) => e.label),
      ['main.rs', '/repo/src/lib/mod.rs', '/repo/src/net/mod.rs'],
      'same-basename tabs fall back to their whole path, and only they do',
    )

    // The strip is measured at layout time and the menu is built at open time; a
    // `cide://workspace-changed` in between can close a tab the measurement still names.
    eq(
      shape(overflowEntries(all, ['t1', 'gone'], { activate: () => {} })),
      ['overflow:t1'],
      'an id naming no tab is skipped rather than drawn as a line onto nothing',
    )
    eq(shape(overflowEntries(all, [], { activate: () => {} })), [], 'nothing hidden, no lines')

    // The chrome audit renders a handler-free `<TabStrip>`. A live-looking line that does
    // nothing is the failure this whole model exists to make unrepresentable.
    const inert = overflowEntries(all, ['t1', 't5'], {})
    ok(
      inert.every((e) => e.run === undefined && e.disabledReason === NO_HOST),
      'with no activate handler every line is disabled and says why',
    )
  }

  // =====================================================================================
  // 5. The header's row controls
  // =====================================================================================

  const boot = (project) => ({
    role: { kind: 'shell', projects: ['p1'], active: 'p1' },
    workspace: { projects: { p1: project } },
  })
  const project = (maximized) => ({
    id: 'p1',
    activeTab: 't1',
    tabs: [{ id: 't1', tree: { focused: 'pane-a', maximized } }],
  })

  eq(rowGate(null), NO_PROJECT_REASON, 'before the bootstrap lands there is nothing to add to')
  eq(
    rowGate({ role: { kind: 'shell', projects: [], active: null }, workspace: { projects: {} } }),
    NO_PROJECT_REASON,
    'an empty workspace is a real state, and the buttons say what it needs',
  )
  eq(rowGate(boot(project(null))), null, 'with a project open the buttons are live')
  eq(
    rowGate(boot(project('pane-a'))),
    MAXIMIZED_REASON,
    'a maximized pane refuses out loud, where SplitTree used to hide the strip',
  )

  eq(
    rowTarget(boot(project(null))),
    { project: 'p1', tab: 't1', after: 'pane-a', maximized: null },
    'a row is anchored under the focused pane, not at the bottom of the tab',
  )
  eq(
    rowTarget({
      role: { kind: 'detachedTab', project: 'p1', tab: 't1' },
      workspace: { projects: { p1: project(null) } },
    }).project,
    'p1',
    'a window showing one project still names it',
  )

  // =====================================================================================
  // 6. The bulk close, as source — because everything above passes with nothing wired
  // =====================================================================================
  //
  // Section 3 drives `tabMenuEntries` against a fixture `closeMany`, so all of it stays green
  // while the app hands the model nothing: the three items would simply read *"Not available in
  // this window"* for ever, on a surface where the reason sounds plausible. That is this
  // project's signature defect wearing its most convincing disguise, and only a grep over the
  // wiring catches it.
  //
  // Stripped of comments first. `TabActions.closeMany` and `TabStripProps.onCloseMany` are each
  // documented in a paragraph that names the identifier several times, so a raw grep would
  // certify the prose of a feature that had been removed.
  {
    const strip = (src) =>
      src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')
    const tabStrip = strip(readFileSync('src/chrome/TabStrip.tsx', 'utf8'))
    const app = strip(readFileSync('src/App.tsx', 'utf8'))
    const store = strip(readFileSync('src/store/workspace.ts', 'utf8'))
    const model = strip(readFileSync('src/chrome/menuModel.ts', 'utf8'))

    ok(
      /closeMany: onCloseMany/.test(tabStrip),
      'the strip hands the model its bulk-close door — without it all three items are disabled ' +
        'with a reason that reads like a legitimate state',
    )
    ok(
      /onCloseMany=\{\(ids\) => void closeTabs\(activeProject\.id, ids\)\}/.test(app),
      'and App.tsx supplies it, which is the one call site and the whole difference between a ' +
        'menu item and a sentence about one',
    )
    /*
     * The store asks ONCE. This is the assertion that stops the loop coming back: the model can
     * hand over a list and the store can still spend it one `closeTab` at a time, which is the
     * behaviour being replaced — one risk round trip per tab, one refusal per dirty file, and
     * `closeConfirmStore` queueing a modal for each.
     */
    ok(
      /tabsCloseRisk\(get\(\)\.boot, project, tabs\)/.test(store) && /scope: 'tabs'/.test(store),
      'and the store asks about the whole batch once, under the plural scope — not once per tab',
    )
    ok(
      !/forEach\(\(t\) => close\(t\.id\)\)/.test(model),
      'and the model no longer closes tabs in a loop of its own: two doors for one gesture is ' +
        'how the wrong one survives in whichever surface nobody re-tested',
    )
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-menu-model: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-menu-model: OK')
