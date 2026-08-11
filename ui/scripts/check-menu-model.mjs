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
import { mkdtempSync, rmSync } from 'node:fs'
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

  ok(!closable(console_), 'the pinned console is not closable')
  ok(closable(file) && closable(extra), 'everything else is')
  eq(pathOf(file), '/repo/src/main.rs', 'a file tab is about its path')
  eq(pathOf(console_), null, 'a console tab is about nothing on disk')

  {
    const closed = []
    const entries = tabMenuEntries(tabs, file, 't1', {
      close: (id) => closed.push(id),
      split: () => {},
      detach: () => {},
      copy: () => {},
    })

    eq(
      shape(entries),
      ['close', 'close-others', 'close-right', '--', 'split', 'detach', '--', 'copy-path'],
      'the workspace tab menu is the six items the request named, in three groups',
    )
    item(entries, 'close-right').run()
    eq(closed, ['t2'], 'Close to the right closes only what follows this tab')

    closed.length = 0
    item(entries, 'close-others').run()
    eq(closed, ['t2'], 'Close others skips the pinned console as well as this tab')
  }

  {
    // Offered and disabled with a reason rather than dropped: an item that comes and goes with
    // the tab order is one the user has to hunt for.
    const entries = tabMenuEntries(tabs, extra, 't2', { close: () => {} })
    eq(
      item(entries, 'close-right').disabledReason,
      'Nothing to the right can be closed',
      'the last tab still shows Close to the right, and says why it is off',
    )
  }

  {
    const entries = tabMenuEntries(tabs, console_, 't0', { close: () => {} })
    eq(
      item(entries, 'close').disabledReason,
      PINNED_REASON,
      'the console refuses to close and says it is pinned, not that it is unavailable',
    )
    const right = item(entries, 'close-right')
    ok(typeof right.run === 'function', 'the console can close the tabs to its right')
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
  // 4. The header's row controls
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
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-menu-model: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-menu-model: OK')
