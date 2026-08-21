/**
 * Renders both halves of the extension UI under node and checks what came out.
 *
 * `tsc` proves they compile and `check-ext.mjs` proves the pure rules decide the right things.
 * Neither can see whether the components *paint* — this repository has shipped a panel that
 * compiled, mounted and drew nothing, twice, which is why `check-git-render.mjs` exists and why
 * this is its fifth instance.
 *
 * It matters more here than for a panel cide wrote. Every other render check guards markup this
 * repository controls end to end; this one guards a renderer whose **input comes from somebody
 * else's code**. A `ViewBody` kind that fell out of the switch would not be a bug in cide that a
 * user could see and report — it would be an extension that looks broken, reported to its author,
 * who cannot fix it.
 *
 * Each assertion below names a failure this design can actually have:
 *
 *  - **Every body kind draws something.** A kind with no `case` renders `undefined`, which React
 *    prints as nothing at all: an empty panel under a lit rail button.
 *  - **`failed` is drawn as an error and `empty` is not.** "This extension broke" and "there is
 *    nothing here" are the two answers a user must not have to tell apart by guessing, and they
 *    are one class name apart.
 *  - **A short table row is padded, not dropped.** A worker that miscounted its own columns should
 *    cost a blank cell and not the panel.
 *  - **A collapsed tree row hides its children and an expanded one shows them.** The seed comes
 *    from the worker and the state does not; a renderer that re-seeded on every post would slam
 *    the tree shut every time the user typed.
 *  - **With no handlers, no control is drawn.** The panel convention, and here it is load-bearing
 *    twice: a dead toolbar button is an extension that looks broken.
 *  - **The manager's first-launch screen says how to connect one.** It is what every user sees the
 *    first time, and an empty list with no prose is the one state nobody can act on.
 *  - **The consent line is on every row.** An install copies somebody else's code onto this
 *    machine, and `process:spawn` means it may run a program against their project. That is drawn
 *    beside the button, not behind one more click.
 *  - **A marketplace that would not clone shows git's own words.** The user is used to reading
 *    them, and cide paraphrasing them helps nobody.
 *
 * `window` is faked before the import, because `@tauri-apps/api` touches it on load and the
 * manager panel's host imports the client — nothing is faked deeper than that.
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(UI, 'node_modules', '.cache', 'cide-ext-render-'))
let failed = 0
let checked = 0

const eq = (actual, expected, what) => {
  checked += 1
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => eq(cond === true, true, what)
/**
 * How many elements carry this CSS-module class.
 *
 * Matched on the *hashed* name Vite emits — `.action` becomes `_action_2o2l5_68` — so the needle
 * is the underscore-wrapped base name and nothing else. No `\b` around it: `_` is a word
 * character, so a boundary assertion after `_action_` fails against the hash that follows it, and
 * the count silently comes back zero. The underscores are the delimiters.
 */
const count = (html, cls) => html.split(`class="`).filter((part) => part.includes(cls)).length

try {
  globalThis.window ??= globalThis

  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/ext/smokeEntry.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  try {
    await import(`file://${resolve(out, 'smokeEntry.js')}`)
  } finally {
    console.log = log
  }
  const stories = Object.fromEntries(
    JSON.parse(printed.at(-1)).map((story) => [story.story, story.html]),
  )

  // --- the contributed-panel renderer ---------------------------------------------------------

  for (const kind of ['empty', 'loading', 'failed', 'list', 'tree', 'table', 'markdown']) {
    const html = stories[`panel-${kind}`]
    ok(
      typeof html === 'string' && html.length > 0,
      `the \`${kind}\` body rendered — a kind with no case renders undefined, which React prints `
        + 'as nothing at all: an empty panel under a lit rail button',
    )
  }

  ok(
    /_failed_/.test(stories['panel-failed']),
    'a failed view is drawn as an error and not as an empty list — "this extension broke" and '
      + '"there is nothing here" are the two answers a user must not tell apart by guessing',
  )
  ok(
    !/_failed_/.test(stories['panel-empty']),
    'and an empty one is not, or every quiet panel would read as a fault',
  )
  ok(
    stories['panel-empty'].includes('Open a .sql file'),
    "the empty view prints the extension's own sentence rather than a generic one",
  )

  ok(stories['panel-list'].includes('CREATE TABLE users'), 'a list row draws its label')
  ok(stories['panel-list'].includes('line 9'), 'and its detail')
  eq(count(stories['panel-list'], '_action_'), 2, 'both toolbar buttons are drawn')
  ok(
    stories['panel-list'].includes('disabled=""'),
    'and a disabled one is drawn *disabled* rather than omitted — an action an extension declares '
      + 'and cide silently drops is an extension that looks broken to its author',
  )

  ok(stories['panel-tree'].includes('web'), 'an expanded tree row shows its children')
  ok(
    !stories['panel-tree'].includes('>data<'),
    'and a collapsed one hides them — the worker seeds the expansion and does not own it, or '
      + 'every re-post would slam the tree shut while the user was reading it',
  )

  eq(
    count(stories['panel-table'], '_table_'),
    1,
    'a table renders as one table',
  )
  ok(
    stories['panel-table'].includes('email'),
    'a row shorter than the column list still renders — a worker that miscounted its own columns '
      + 'costs a blank cell, not the panel',
  )

  ok(stories['panel-markdown'].includes('Some prose.'), 'markdown renders its text')
  ok(
    !stories['panel-markdown'].includes('<h1'),
    'and does *not* render it as markup. A panel whose content comes from a third party is the '
      + 'one place in cide where rendering somebody else\'s HTML would be a real hole',
  )

  eq(
    count(stories['panel-read-only'], '_action_'),
    0,
    'with no `onAction`, the toolbar is not drawn at all — never drawn dead',
  )

  // --- the manager panel ------------------------------------------------------------------------

  ok(
    stories['manager-none'].includes('No marketplace is connected'),
    'the first-launch screen says what a marketplace is and how to connect one — it is what '
      + 'every user sees first, and an empty list with no prose is the one state nobody can act on',
  )
  ok(
    stories['manager-none'].includes('Connect'),
    'and offers the field to do it in, already open — that screen is the one place where '
      + 'connecting is the only useful control, so it must not need a click to reveal',
  )
  ok(
    !/_search_/.test(stories['manager-none']),
    'and has no search box: nothing is connected, so there is nothing to search, and a field over '
      + 'an empty list is a control that cannot do anything',
  )

  const ready = stories['manager-ready']
  ok(ready.includes('SQL') && ready.includes('YAML'), 'both catalog rows are drawn')
  ok(ready.includes('>Install<'), 'a row that is not installed offers Install')
  ok(ready.includes('>Update<'), 'and one behind its marketplace offers Update')
  ok(ready.includes('>Disable<'), 'an installed, enabled one offers Disable')
  /*
   * What an extension may do is on the row, beside the button that installs it — an install copies
   * somebody else's code onto this machine, so it must not be behind one more click.
   *
   * As *tags* and not as a sentence, which is a change from the first version of this panel: the
   * whole consent sentence in a 252px column is four wrapped lines per extension, which is why the
   * panel read as a wall. The sentence is still what a person decides by — it is the tag's tooltip
   * here and the body of the extension's page there — so the assertion is that the tag and the
   * wording travel together, not that either exists alone.
   */
  eq(
    count(ready, '_perm_'),
    6,
    'every permission is tagged on its row — three each for two extensions that ask for three',
  )
  ok(
    ready.includes('runs programs'),
    'in words rather than as a capability string, because `process:spawn` tells a user nothing',
  )
  ok(
    ready.includes('title="run programs on your machine"'),
    'and the full sentence is the tag\'s tooltip, so the short form is never the only form',
  )
  ok(
    count(ready, '_permStrong_') === 2,
    '`process:spawn` is toned apart from the rest — every other capability is a read of something '
      + 'already on screen, and this one means the manifest names a binary and cide runs it '
      + "against the user's project",
  )

  // The row's one-glance answer, and a shape difference as well as a colour one.
  eq(count(ready, '_dot_'), 2, 'every row has a status dot')
  ok(
    ready.includes('aria-label="enabled"') && ready.includes('aria-label="not installed"'),
    'and it is named, because a coloured circle is not a name — a screen reader would otherwise '
      + 'be told nothing about which of four states a row is in',
  )
  ok(
    count(ready, '_dotOn_') === 1 && count(ready, '_dotOff_') === 0,
    'the enabled row is filled and nothing is drawn for one that is merely offered',
  )
  ok(ready.includes('local'), 'a local marketplace says so, so the user need not infer it from a URL')

  // The readout that answers "why is my .sql file coloured like that".
  ok(
    ready.includes('cide-marketplace.sql'),
    'a language a contribution won names the extension that won it — cide ships grammars of its '
      + 'own, so the ordinary state after installing one is that a builtin lost, and a user who '
      + 'cannot see that has no way to tell a working extension from an inert one',
  )
  ok(
    ready.includes('was cide'),
    'and says what it displaced',
  )
  ok(
    !ready.includes('Markdown'),
    'a builtin nothing displaced is not listed — all eleven beside two contributed ones would '
      + 'bury the answer in a table nobody needs to read',
  )

  // --- search, filtering, and the link into a page ---------------------------------------------

  ok(
    ready.includes('Search extensions'),
    'the search field is drawn, and its accessible name says what it searches',
  )
  eq(
    count(ready, '_chip_'),
    7,
    'one chip per filter — each is a question somebody actually asks, and each is one press',
  )
  ok(
    count(ready, '_chipCount_') === 7,
    'and every chip carries its count, so a user can see what pressing it would show before they '
      + 'press it',
  )
  ok(
    /aria-selected="true"/.test(ready),
    'the active filter is marked selected, or a screen reader is told nothing about which of the '
      + 'seven is in force',
  )
  ok(
    !/<button[^>]*class="[^"]*_chip_[^"]*"[^>]*disabled/.test(ready),
    'and no chip is disabled, even at zero: a chip the user cannot press is a count they cannot '
      + 'confirm, and pressing an empty one shows the sentence that explains it',
  )
  eq(
    count(ready, '_nameLink_'),
    2,
    "every row's name opens that extension's page — the README is how somebody decides whether to "
      + 'install it, and it is not reachable from anywhere else',
  )

  const filtered = stories['manager-filtered']
  ok(
    filtered.includes('Nothing matches'),
    'a search that matches nothing says so, and an empty list under a search box says nothing '
      + 'about why',
  )
  ok(
    filtered.includes('sql') && filtered.includes('enabled'),
    'naming both the text and the filter, because "no results" under two constraints is ambiguous '
      + 'about which one to relax',
  )
  ok(
    filtered.includes('Languages'),
    'and the Languages readout survives the filter — it is a fact about the whole registry, and '
      + 'hiding it while somebody searches would take the answer away exactly when they are '
      + 'looking for it',
  )

  eq(
    count(stories['manager-read-only'], '_primary_') + count(stories['manager-read-only'], '_action_'),
    0,
    'with no handlers, the manager draws no buttons at all — the panel owns nothing and must not '
      + 'pretend it can act',
  )
  eq(
    count(stories['manager-read-only'], '_chip_'),
    0,
    'nor the filter chips, which are a control like any other',
  )
  eq(
    count(stories['manager-read-only'], '_headerAction_'),
    0,
    "nor the header's Connect and Refresh",
  )
  eq(
    count(stories['manager-read-only'], '_nameLink_'),
    0,
    "and a row's name is plain text rather than a dead link — a detached window has no tab strip "
      + 'to open a page in',
  )

  // --- the extension page ------------------------------------------------------------------------

  const page = stories['page']
  ok(
    page.includes('SQL'),
    "the page has a heading before its content arrives, from the name the tab was opened with — a "
      + 'tab that drew nothing until a round trip finished is a tab that looks broken',
  )
  ok(
    page.includes('Reading'),
    'and says it is reading rather than showing an empty document. `useEffect` does not run under '
      + 'SSR, so this is exactly the first frame a user sees',
  )
  ok(
    !/_markdown_/.test(page),
    'the README is not drawn before it has been read, which is what stops a flash of an empty '
      + 'document under a populated header',
  )

  const failedStory = stories['manager-failed']
  ok(
    failedStory.includes('could not read from remote repository'),
    "a marketplace that would not clone shows git's own words verbatim",
  )
  ok(
    failedStory.includes('>Refresh<'),
    'and still offers Refresh, or a transient network failure is a marketplace with no way back',
  )
  ok(
    failedStory.includes('has no source and is ignored'),
    'and a problem with `extensions.json` itself is drawn, because nothing else in the product '
      + 'would ever mention it',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`extension render: ok (${checked} checks over ${Object.keys(stories).length} stories)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
