/**
 * Checks `src/windows/detachedPane.ts` — what a `pane:<uuid>` window may show — and
 * `src/windows/windowTabs.ts` — which tabs each window draws, and what a tab's only pane's
 * close/detach/maximize buttons act on. The two are one feature seen from both windows: an
 * editor can only leave the shell as a whole tab, and a torn-out tab must be drawn by
 * exactly one window.
 *
 * Worth a check because the failure it replaces was invisible to `tsc` and nearly invisible to
 * a reader. `DetachedPaneWindow` asked `needsSession(pane)`, got `false` for an `editor` pane —
 * correctly; an editor runs no child — and used that as *"so it is not orphaned"*, leaving
 * `TerminalPane` as the only remaining branch. A detached editor pane would have **spawned a
 * shell in a window titled `main.rs`**. Nothing about that is a type error, and the two lines
 * that produced it read fine in isolation.
 *
 * It was out of reach only because `layout::take_pane` refuses the last pane of a tab and a file
 * tab opens with one; splitting the file tab first — offered on the pane's own menu — puts
 * Detach right there. "Implemented and mis-wired", not "not implemented".
 *
 * Same shape as `check-window-controls.mjs`: a bare `tsc` over one import-free file, then import
 * the output and assert.
 *
 * Run: `pnpm --dir ui run check:detached`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-detached-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/windows/detachedPane.ts',
      'src/windows/windowTabs.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const { detachedContent, needsSession } = await import(`file://${join(out, 'detachedPane.js')}`)

  const kindOf = (pane) => detachedContent(pane).kind

  // --- the panes this window is for --------------------------------------------------------

  eq(
    kindOf({ kind: 'claude', session: 'a2f1' }),
    'terminal',
    'a torn-out Claude pane arrives with its session and attaches to it — the whole feature',
  )
  eq(kindOf({ kind: 'shell', session: 'a2f1' }), 'terminal', 'and so does a shell')

  // --- the pane that would have spawned a shell in a window titled main.rs ------------------

  eq(
    kindOf({ kind: 'editor', session: null }),
    'unsupported',
    'AN EDITOR IS REFUSED, not rendered as a terminal. `EditorPane` needs a TabId — the ' +
      'buffer is registered per tab in `openBuffers.ts` — and this window holds a pane that ' +
      'has been taken out of its tab. Falling through to `TerminalPane` spawns a shell.',
  )
  eq(
    kindOf({ kind: 'diff', session: null }),
    'unsupported',
    'and so is a diff, for the same reason and by the same default',
  )
  ok(
    detachedContent({ kind: 'editor', session: null }).message.includes('Redock'),
    'the refusal names the way out; the window offers exactly one other control',
  )
  ok(
    !detachedContent({ kind: 'editor', session: null }).message.includes('no session'),
    'and it does NOT say "this pane has no session, redock it to start one" — an editor will ' +
      'never have one, so that sentence is an instruction to press a button that fixes nothing',
  )

  // --- the ordering that makes the sentence right ------------------------------------------

  eq(
    kindOf({ kind: 'claude', session: null }),
    'orphaned',
    'a pane that DOES run a child and arrived without one is orphaned — the state that ' +
      'should not occur, rendered rather than ignored, because a fall-through to a spawn ' +
      'starts a child whose id no window records',
  )

  // --- a kind nobody has written yet -------------------------------------------------------

  ok(
    needsSession({ kind: 'notebook', session: null }),
    'an unknown kind is assumed to run a child, so a `PaneKind` added later ends up refused ' +
      'as orphaned rather than silently spawning a shell — the safe direction of the guess',
  )
  eq(
    kindOf({ kind: 'notebook', session: 'a2f1' }),
    'terminal',
    'and one that arrives with a session is treated as the terminal-ish thing it claims to be',
  )

  // --- the source pins ----------------------------------------------------------------------

  const window = readFileSync(join(UI, 'src/windows/DetachedPaneWindow.tsx'), 'utf8')
  ok(
    window.includes('detachedContent('),
    'the window asks this module rather than re-deriving the rule — re-deriving it is how it ' +
      'went wrong the first time',
  )
  ok(
    !/needsSession\(pane\)\s*&&/.test(window),
    'and the two-line version it grew out of is gone, not left beside its replacement',
  )
  ok(
    /content\.kind === 'terminal'/.test(window),
    'TerminalPane is rendered for the terminal answer ONLY. Any other shape of this branch — ' +
      'a negated orphan check, a truthiness test — is how a shell gets spawned for an editor',
  )

  // --- the detached-TAB rules: src/windows/windowTabs.ts --------------------------------------

  const { clusterPlan, detachedTabs, shownProjects, shownTabs } = await import(
    `file://${join(out, 'windowTabs.js')}`
  )

  const TABS = [{ id: 'console' }, { id: 'a' }, { id: 'b' }]
  const WINDOWS = {
    'shell:1': { kind: 'shell', projects: ['alpha'] },
    'tab:1': { kind: 'detachedTab', project: 'alpha', tab: 'b' },
    'pane:1': { kind: 'detachedPane', project: 'alpha', tab: 'a' },
  }
  const SHELL = WINDOWS['shell:1']

  eq(
    [...detachedTabs(WINDOWS)],
    ['b'],
    'a detachedTab role marks its tab as torn out; a detachedPane role marks NOTHING — its ' +
      '`tab` is the home the pane re-docks into, still drawn by the shell',
  )
  eq(
    shownTabs(SHELL, TABS, WINDOWS).map((t) => t.id),
    ['console', 'a'],
    'THE LOAD-BEARING FILTER: the shell must not draw a torn-out tab. The tab stays in ' +
      '`project.tabs` (quit guards and restore plans walk that list), so this filter is the ' +
      'only thing between one file tab and two live buffers over one file',
  )
  eq(
    shownTabs(WINDOWS['tab:1'], TABS, WINDOWS).map((t) => t.id),
    ['b'],
    'a tab window draws exactly its own tab',
  )
  eq(
    shownTabs(WINDOWS['pane:1'], TABS, WINDOWS).map((t) => t.id),
    [],
    'a pane window draws no tab at all — its pane lives outside every tree',
  )
  eq(
    shownTabs(SHELL, TABS, { 'shell:1': SHELL }),
    TABS,
    'with nothing torn out the shell draws every tab',
  )

  // --- which PROJECTS a window draws ---------------------------------------------------------

  const PROJECTS = [{ id: 'alpha' }, { id: 'beta' }, { id: 'gamma' }]

  eq(
    shownProjects({ kind: 'shell', projects: ['alpha', 'beta', 'gamma'] }, PROJECTS).map(
      (p) => p.id,
    ),
    ['alpha', 'beta', 'gamma'],
    'the stacked shell draws every project — one window, one strip, which is the default mode',
  )
  eq(
    shownProjects({ kind: 'shell', projects: ['beta'] }, PROJECTS).map((p) => p.id),
    ['beta'],
    'THE PER-PROJECT FILTER: each window names exactly one project and must draw exactly that ' +
      'one. Rendering the workspace map instead gave three windows the same three-tab strip, ' +
      'and the two tabs a window does not draw are inert — `activate_project` only moves ' +
      '`active` on shells whose `projects` holds the id, so the click landed in another window',
  )
  eq(
    shownProjects({ kind: 'shell', projects: ['gamma', 'alpha'] }, PROJECTS).map((p) => p.id),
    ['alpha', 'gamma'],
    'and the order is the CALLER\'s — workspace insertion order, which is what the strip draws ' +
      'and `reorder_project` rewrites — never the role list\'s',
  )
  eq(
    shownProjects({ kind: 'shell', projects: ['alpha', 'ghost'] }, PROJECTS).map((p) => p.id),
    ['alpha'],
    'a role naming a project the workspace no longer has draws nothing for it, rather than a ' +
      'tab over a project that is gone',
  )
  eq(
    shownProjects(WINDOWS['tab:1'], PROJECTS),
    [],
    'a detached window has no project strip. `keys/target.ts::windowProjectsOf` is these ids, ' +
      'so this is also what stops Ctrl+` walking a list the window does not draw',
  )
  eq(
    shownProjects(WINDOWS['pane:1'], PROJECTS),
    [],
    'and neither does a pane window',
  )

  eq(
    clusterPlan(1, false),
    { maximize: false, close: 'tab', detach: 'tab' },
    "a tab's only pane: maximize is withheld (it already fills the tab), and close/detach " +
      "act on the TAB — `layout::close`/`take_pane` refuse a last pane, and these buttons " +
      "used to run into that refusal and print `lastPane`",
  )
  eq(
    clusterPlan(2, false),
    { maximize: true, close: 'pane', detach: 'pane' },
    'a pane with a sibling keeps all three pane-scoped actions',
  )
  eq(
    clusterPlan(1, true),
    { maximize: false, close: 'pane', detach: 'pane' },
    "the pinned console's sole pane stays pane-scoped: the console cannot close or detach " +
      'as a tab, and the pane-scoped arms keep the role-based refusals that word the buttons',
  )

  // --- the source pins for the tab rules ------------------------------------------------------

  const app = readFileSync(join(UI, 'src/App.tsx'), 'utf8')
  ok(
    /<TabContent[\s\S]{0,600}tabs=\{visibleTabs\}/.test(app),
    'TabContent is fed the FILTERED list. It mounts every tab it is handed, hidden or not, ' +
      'so `tabs={activeProject.tabs}` here would put a second live editor behind the shell ' +
      'for every torn-out file',
  )
  ok(
    app.includes('shownTabs(boot.role, activeProject.tabs, boot.workspace.windows)'),
    'and the filtered list comes from this module rather than being re-derived in place',
  )
  ok(
    /<AppHeader[\s\S]{0,400}projects=\{projects\}/.test(app) &&
      app.includes('shownProjects(boot.role, Object.values(boot.workspace.projects))'),
    'the header strip is fed the FILTERED project list from this module. ' +
      '`Object.values(workspace.projects)` on its own is every project in the PROCESS, which is ' +
      'what put three identical strips in three windows under *One window per project*',
  )
  const targets = readFileSync(join(UI, 'src/keys/target.ts'), 'utf8')
  ok(
    /export function windowProjectsOf[\s\S]{0,200}role\.projects/.test(targets),
    'and the switcher reads the same `role.projects` this module filters by. It is a second ' +
      'function (that module is compiled alone by check-commands, so it may not import a value) ' +
      'but it must not become a second SOURCE: Ctrl+` walking `workspace.projects` is the same ' +
      'dead keystroke one ring out',
  )
  ok(
    app.includes('clusterPlan('),
    "the pane cluster's handlers are routed by clusterPlan, not by a re-derived pane count",
  )

  const dispatch = readFileSync(join(UI, 'src/keys/dispatch.ts'), 'utf8')
  ok(
    /case 'pane\.close': \{[\s\S]{0,900}clusterPlan\(/.test(dispatch),
    "the pane.close chord follows the same clusterPlan the button does — the chord closing " +
      'the pane while the button closes the tab is the drift this module exists to prevent',
  )
  ok(
    /case 'pane\.detachToWindow': \{[\s\S]{0,900}clusterPlan\(/.test(dispatch),
    'and so does pane.detachToWindow',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('detached pane window: ok')
  console.log('detached tab rules: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
