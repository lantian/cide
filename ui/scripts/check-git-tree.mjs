/**
 * Checks `src/sidebar/GitPanel/model.ts` — the git panel's pure core.
 *
 * Same shape as `check-status-format.mjs`, and for the same reason: this project has no JS
 * test runner, and adding one for a handful of pure functions would be a larger commitment
 * than the code it tests.
 *
 * # The failure this exists to prevent
 *
 * This check used to pass while the panel was **permanently empty against every real
 * repository**. It drove `normalizeStatus` with a fixture written in the shape the *panel*
 * assumed — a top-level `root`, a recursive `groups` tree — and asserted that that shape
 * survived. `git_status` returns `repos[].repo.{id,root,name}`, `repos[].branch`, and four
 * sibling lists, so every real repo failed `normalizeRepo`'s first check and was dropped. The
 * check was green because it was asking the panel to agree with itself.
 *
 * So the fixture below is a `cide_ipc::git::ChangesTree`, field for field, derived from
 * `crates/cide-ipc/src/git.rs` and not from anything in `ui/`. `WIRE_FIELDS` pins the field
 * names it depends on, so a rename in Rust that `cargo xtask codegen` propagates into
 * `ui/src/ipc/generated.ts` fails here too rather than silently emptying the panel again.
 *
 * What else is pinned: the repo level is elided with one repo and present with two (§5.3), a
 * submodule is a nested *repository* whose files roll up into its parent, tri-state
 * distinguishes "none of these" from "some of one of them", a collapsed group still commits
 * its ticked files, row ids carry `RepoId` rather than a path, and `normalizeStatus` never
 * throws whatever the backend sends.
 *
 * Run: `pnpm --dir ui run check:git`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-git-'))
try {
  /*
   * `model.ts` imports its types from `./types.ts`, which imports the generated wire types
   * through the `@/*` alias. A bare `tsc model.ts` has no `paths`, so the alias would not
   * resolve; a generated tsconfig carries it. The alternative — a relative `../../ipc/…`
   * import in one source file — would put a rule in the app's source purely to suit a test
   * script, and this codebase uses `@/` everywhere else.
   *
   * The imports are type-only, so the emitted `model.js` has no imports at all and node can
   * load it directly. `types.ts` must stay free of runtime values for that to hold; it says
   * so in a comment at the bottom of the file.
   */
  const tsconfig = join(out, 'tsconfig.json')
  writeFileSync(
    tsconfig,
    JSON.stringify({
      compilerOptions: {
        target: 'es2022',
        module: 'esnext',
        moduleResolution: 'bundler',
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        verbatimModuleSyntax: true,
        skipLibCheck: true,
        // Without this a configuration mistake here — a `rootDir` that does not contain every
        // source file, say — makes tsc report the error *and* fall back to emitting each
        // `.js` next to its `.ts`, littering `ui/src` with generated files that look tracked.
        noEmitOnError: true,
        outDir: out,
        // `src`, not the panel's own directory: `types.ts` reaches out to `@/ipc/generated`
        // through the alias, and tsc requires every source file to sit under `rootDir`. The
        // output therefore mirrors the source tree under `out`.
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      files: [
        join(UI, 'src', 'sidebar', 'GitPanel', 'model.ts'),
        // The drag-and-drop rules: what a grab carries, which targets accept it, what a
        // same-list drop does. Pure, and in a module of its own precisely so this script can
        // run them — the gesture that drives them needs a pointer and a DOM, so the rules are
        // the only half of that feature anything in this repo can execute.
        join(UI, 'src', 'sidebar', 'GitPanel', 'dragDrop.ts'),
        // The click rules. They are in a module of their own precisely so the three sidebar
        // check scripts can each hold the rule belonging to their tree; the git tree's is the
        // conditional one, and it is pinned at the foot of this file.
        join(UI, 'src', 'sidebar', 'clickSemantics.ts'),
      ],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  /*
   * `dragDrop.ts` imports two *values* from `model.ts` — `flatFiles` and `changelistIdOf` —
   * which `model.ts` itself deliberately does not do from anywhere (its imports are all
   * type-only, so it emits a file with no imports at all and node can load it directly).
   * TypeScript emits the specifier exactly as written, `'./model'`, and node refuses an
   * extensionless one.
   *
   * Rewriting it here beats the two alternatives: `'./model.js'` in the source is a lie in a
   * file the bundler reads, and duplicating the two functions into `dragDrop.ts` would put the
   * `cl:` prefix rule in two places, which is the one rule in this panel whose second copy
   * silently produces `NoSuchChangelist`.
   */
  const emitted = join(out, 'sidebar', 'GitPanel', 'dragDrop.js')
  writeFileSync(emitted, readFileSync(emitted, 'utf8').replace(/from '(\.\/[^']+)'/g, "from '$1.js'"))

  const m = await import(`file://${join(out, 'sidebar', 'GitPanel', 'model.js')}`)
  const d = await import(`file://${emitted}`)

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

  // --- the wire shape, pinned against the generated types ------------------------------

  /*
   * Every field the fixture below leans on, and where it lives. Read out of the generated
   * types rather than out of Rust directly: `cargo xtask codegen` is the gate that keeps the
   * two in step, so `generated.ts` is the frontend's copy of `crates/cide-ipc/src/git.rs` and
   * the one a TypeScript check can read. A rename that lands there and not here is exactly
   * the drift that emptied the panel.
   */
  const generated = readFileSync(join(UI, 'src', 'ipc', 'generated.ts'), 'utf8')
  const WIRE_FIELDS = {
    ChangesTree: ['repos'],
    RepoChanges: [
      'repo',
      'branch',
      'changelists',
      'unversioned',
      'ignored',
      'conflicts',
      'indexChangedExternally',
      'useStagingArea',
    ],
    RepoInfo: ['id', 'root', 'name', 'parent', 'isSubmodule'],
    ChangelistView: ['id', 'name', 'comment', 'active', 'changes'],
    ChangeEntry: ['path', 'origPath', 'index', 'worktree', 'staged', 'binary', 'submodule', 'changelist'],
  }
  for (const [type, fields] of Object.entries(WIRE_FIELDS)) {
    const decl = new RegExp(`export type ${type} = \\{[\\s\\S]*?\\};`).exec(generated)?.[0]
    ok(decl !== undefined, `the generated types still declare ${type}`)
    for (const field of fields) {
      ok(
        decl !== undefined && new RegExp(`(^|[\\s{,])${field}[?]?:`, 'm').test(decl),
        `${type}.${field} is still what Rust sends — the panel reads it by that name`,
      )
    }
  }

  // --- a fixture in that shape ---------------------------------------------------------

  const entry = (path, index, worktree, changelist, extra = {}) => ({
    path,
    origPath: null,
    index,
    worktree,
    staged: index !== 'unmodified',
    binary: false,
    submodule: false,
    changelist,
    ...extra,
  })

  const branch = { head: 'main', detached: false, upstream: null, ahead: 0, behind: 0, operation: null, unborn: false }

  /** One root: two changelists, plus unversioned and ignored files. */
  const repo = (id, root, name, parent = null) => ({
    repo: { id, root, name, parent, isSubmodule: parent !== null },
    branch,
    changelists: [
      {
        id: 'default',
        name: 'Changes',
        comment: '',
        active: true,
        changes: [
          entry('a.rs', 'unmodified', 'modified', 'default'),
          // Both sides dirty: partially staged, which is the leaf-level `–`.
          entry('src/b.rs', 'modified', 'modified', 'default'),
        ],
      },
      {
        id: 'fixes',
        name: 'fixes',
        comment: '',
        active: false,
        changes: [entry('c.rs', 'added', 'unmodified', 'fixes')],
      },
    ],
    unversioned: [entry('notes.md', 'unmodified', 'untracked', 'default')],
    ignored: [entry('target', 'unmodified', 'ignored', 'default')],
    conflicts: [],
    indexChangedExternally: false,
    useStagingArea: false,
  })

  const APP = 'app-0000-0000'
  const LIB = 'lib-0000-0000'
  const SUB = 'sub-0000-0000'

  const submodule = {
    ...repo(SUB, '/w/app/vendor/zlib', 'vendor/zlib', APP),
    changelists: [
      {
        id: 'default',
        name: 'Changes',
        comment: '',
        active: true,
        changes: [entry('z.c', 'unmodified', 'modified', 'default')],
      },
    ],
    unversioned: [],
    ignored: [],
  }

  // The whole point of the rewrite: this is what Rust sends, and repos must survive it.
  const one = m.normalizeStatus({ repos: [repo(APP, '/w/app', 'app')] })
  const two = m.normalizeStatus({
    repos: [repo(APP, '/w/app', 'app'), repo(LIB, '/w/lib', 'lib')],
  })
  const nested = m.normalizeStatus({ repos: [repo(APP, '/w/app', 'app'), submodule] })

  eq(
    one.repos.length,
    1,
    'a real `git_status` payload survives normalisation — the bug this whole check exists '
      + 'for was every repo being dropped here, which showed up as a permanently empty panel',
  )
  eq(one.repos[0].id, APP, 'and the repo carries its RepoId, which every git command needs')
  eq(one.repos[0].root, '/w/app', 'and its work tree, for the tooltip')
  eq(one.repos[0].name, 'app', 'and its name, which is what the row shows')
  eq(
    one.repos[0].groups.map((g) => [g.id, g.kind]),
    [
      ['cl:default', 'changelist'],
      ['cl:fixes', 'changelist'],
      ['unversioned', 'unversioned'],
      ['ignored', 'ignored'],
    ],
    'the four sibling lists become groups in one order, with empty ones dropped and '
      + 'changelist ids prefixed so a changelist named `ignored` cannot collide',
  )
  eq(
    m.normalizeStatus({
      repos: [{ ...repo(APP, '/w/app', 'app'), conflicts: [entry('x', 'conflicted', 'conflicted', 'default')] }],
    }).repos[0].groups[0].kind,
    'conflicts',
    'conflicts come first: they block the commit, and a group that has to be scrolled to is '
      + 'a group that gets committed around',
  )

  eq(nested.repos.length, 1, 'a submodule is re-nested under its parent, not left at the top')
  eq(nested.repos[0].children.map((c) => c.id), [SUB], 'under the repo its `parent` names')
  eq(nested.repos[0].children[0].isSubmodule, true, 'and it knows it is one')

  // --- shape ---------------------------------------------------------------------------

  const openAll = (view) => m.allGroups(view)
  const rows1 = m.buildRows(one, openAll(one))
  eq(
    rows1.filter((r) => r.kind === 'repo').length,
    0,
    'one repo: the repo level is elided, exactly as the mock has it',
  )
  eq(rows1[0].label, 'Changes', 'the first row is the first changelist')
  eq(rows1[0].depth, 0, 'with one repo, changelists sit flush at depth 0')
  eq(rows1[0].repo, APP, 'and every row carries a RepoId, never a path — see cmd/git.rs')

  const rows2 = m.buildRows(two, openAll(two))
  eq(rows2[0].kind, 'repo', 'two repos: a repo row appears above the changelists')
  eq(rows2[0].depth, 0, 'the repo row is the new depth 0')
  eq(rows2[1].depth, 1, 'and every changelist moves one level in')

  const rowsN = m.buildRows(nested, openAll(nested))
  eq(
    rowsN.find((r) => r.label === 'vendor/zlib').depth,
    1,
    'a submodule is a repository one level inside its parent — not an opaque row, and not a '
      + "group inside the parent's changelist, whose commit could never include its files",
  )
  eq(
    rowsN.find((r) => r.label === 'z.c').depth,
    3,
    "and its files sit under its own changelist: repo → submodule → Changes → file",
  )

  // --- counts --------------------------------------------------------------------------

  eq(rows1.find((r) => r.label === 'Changes').count, 2, "a changelist counts its own files")
  eq(rowsN[0].count, 6, "a repo's count includes its submodule's files")
  eq(rows2[0].count, 5, "and a plain repo counts every file under it")

  // --- tri-state -----------------------------------------------------------------------

  const partial = m.partialFiles(one)
  eq(
    [...partial],
    [m.fileRowId(APP, 'src/b.rs')],
    'partial is derived from the pair `ChangeEntry` carries — staged AND a dirty worktree — '
      + 'rather than from a flag the backend does not send',
  )
  const changes = rows1.find((r) => r.label === 'Changes')
  const none = new Set()
  eq(m.checkState(changes, none, (id) => partial.has(id)), 'unchecked', 'nothing ticked')

  const all = m.toggleRow(changes, none)
  eq(all.size, 2, 'ticking a changelist reaches every file in it')
  eq(
    m.checkState(changes, all, (id) => partial.has(id)),
    'partial',
    'one half-staged file makes the whole group partial, not checked — the group must not '
      + 'claim to contain more than the commit will',
  )
  eq(m.toggleRow(changes, all).size, 0, 'a second click clears rather than completing')

  const fixes = rows1.find((r) => r.label === 'fixes')
  eq(
    m.checkState(fixes, m.toggleRow(fixes, none), () => false),
    'checked',
    'a group with every file ticked and no partiality is checked',
  )

  // --- defaults ------------------------------------------------------------------------

  const defaults = m.defaultSelection(one)
  eq(defaults.size, 2, 'only the active changelist is ticked on open — `fixes` is not')
  eq(
    [...defaults].every((id) => !id.includes('notes.md') && !id.includes('target')),
    true,
    'unversioned and ignored files are never ticked by default',
  )
  eq(
    m.summarize(m.selectedFiles(one, defaults)),
    '2 modified',
    "the footer's summary counts by status and does not pluralise the adjective",
  )
  eq(m.summarize([]), 'nothing selected', 'an empty selection says so in words')
  eq(
    m.summarize([
      entry('a', 'unmodified', 'modified', 'd'),
      entry('b', 'added', 'unmodified', 'd'),
      entry('c', 'unmodified', 'untracked', 'd'),
      entry('d', 'unmodified', 'typeChange', 'd'),
    ]),
    '1 modified · 1 added · 1 type changed · 1 untracked',
    'several statuses join in SUMMARY_ORDER, and `typeChange` is spelled out rather than '
      + 'leaked as an identifier',
  )

  eq(
    m.entryStatus(entry('a', 'added', 'modified', 'd')),
    'added',
    'a file staged as added and then edited is still an addition: the index side wins',
  )
  /*
   * The conflict rule needs a fixture that can tell it apart from the fallback.
   *
   * This assertion used to pass `('conflicted', 'conflicted')`, where the `index` side is
   * already `conflicted` and the ordinary "index wins" branch returns `conflicted` on its own:
   * deleting the conflict rule outright left the check green. Only a pair whose *other* side
   * would otherwise win discriminates. Both are asserted, because the mixed pair is what makes
   * the rule testable and the doubled pair is what `cide_git::status` actually sends — libgit2
   * sets `is_conflicted()` for the whole entry, so `index_side` and `worktree_side` both report
   * `Conflicted`.
   */
  eq(
    m.entryStatus(entry('a', 'added', 'conflicted', 'd')),
    'conflicted',
    'a conflict outranks both sides — it is the state that blocks a commit, and a row drawn '
      + 'as `added` would invite a click on a checkbox that cannot commit',
  )
  eq(
    m.entryStatus(entry('a', 'conflicted', 'added', 'd')),
    'conflicted',
    'from either side',
  )
  eq(
    m.entryStatus(entry('a', 'conflicted', 'conflicted', 'd')),
    'conflicted',
    'including the doubled pair `cide_git::status` really sends',
  )

  const expanded = m.defaultExpanded(one)
  eq(
    expanded.has(m.groupRowId(APP, 'ignored')),
    false,
    'the ignored group starts collapsed, exactly as in IDEA',
  )
  eq(expanded.has(m.groupRowId(APP, 'cl:default')), true, 'the changelists do not')
  eq(
    m.isIgnoredGroupRow(m.groupRowId(APP, 'ignored')),
    true,
    "expanding it is what asks Rust for `includeIgnored`, so the row id has to be recognisable",
  )
  eq(m.isIgnoredGroupRow(m.groupRowId(APP, 'cl:default')), false, 'and nothing else is')

  // --- what a refresh carries over -----------------------------------------------------

  /*
   * `arrivals` is the whole of `useGitPanel::adopt`'s "is this row new" rule, and it is here
   * rather than in the hook because it was *wrong* in the hook in a way nothing could see: it
   * read the `seenFiles`/`seenGroups` refs from inside a `setSelected`/`setExpanded` updater
   * that React runs during the following render — after `adopt` has already replaced both refs
   * with the payload being adopted. Every id therefore tested as already-seen, nothing was ever
   * ticked, no group was ever opened, and the panel painted its rows and then sat there with
   * every changelist shut and Commit disabled. A pure function is testable; a ref read at a
   * moment React chooses is not.
   */
  const firstLoad = m.arrivals(one, new Set(), new Set())
  eq(
    firstLoad.files.sort(),
    [m.fileRowId(APP, 'a.rs'), m.fileRowId(APP, 'src/b.rs')].sort(),
    'a first payload arrives with the active changelist ticked — this is the assertion that '
      + 'fails when the panel opens with nothing selected and a dead Commit button',
  )
  eq(
    firstLoad.groups.sort(),
    [...m.defaultExpanded(one)].sort(),
    'and with its groups open, the ignored one excepted',
  )
  eq(
    m.arrivals(one, new Set(m.allFiles(one)), m.allGroups(one)),
    { files: [], groups: [] },
    'a payload that brings nothing new adds nothing: a refresh must not re-tick what the user '
      + 'unticked nor re-open what they shut, and this repaints several times a second',
  )
  eq(
    m.arrivals(one, new Set([m.fileRowId(APP, 'a.rs')]), m.allGroups(one)).files,
    [m.fileRowId(APP, 'src/b.rs')],
    'only the genuinely new file — "Claude edited a file, commit it" is one click',
  )
  eq(
    m.arrivals(one, new Set(), new Set()).files.some((id) => id.includes('notes.md')),
    false,
    'and never an unversioned one, however new it is',
  )

  // Ticks whose file is gone must not survive: a selection set that only ever grows ends up
  // naming paths from a repo that has been closed, and commit reads the selection.
  const stale = new Set([m.fileRowId(APP, 'a.rs'), m.fileRowId(APP, 'deleted.rs')])
  eq(
    [...m.pruneSelection(m.allFiles(one), stale)],
    [m.fileRowId(APP, 'a.rs')],
    'pruneSelection drops a tick whose file the refresh no longer reports, and keeps the rest',
  )

  // --- commit units --------------------------------------------------------------------

  // The bug this exists to prevent: `Changes` collapsed, its files still ticked.
  const collapsed = new Set([...openAll(one)].filter((id) => !id.includes('cl:default')))
  const rowsCollapsed = m.buildRows(one, collapsed)
  eq(
    rowsCollapsed.some((r) => r.kind === 'file' && r.entry.path === 'a.rs'),
    false,
    'a collapsed changelist renders no file rows',
  )
  const changesCollapsed = rowsCollapsed.find((r) => r.label === 'Changes')
  eq(
    changesCollapsed.files.length,
    2,
    'a collapsed changelist still owns its files, or its own checkbox reads unchecked over '
      + 'ticked files and a click on it does nothing',
  )
  eq(
    m.checkState(changesCollapsed, defaults, () => false),
    'checked',
    'and its tri-state is computed over them',
  )

  // The repo row has the same rule, and it is the row most likely to be expanded over
  // collapsed children.
  const reposOpenGroupsShut = new Set([m.repoRowId(APP), m.repoRowId(SUB)])
  const repoRow = m.buildRows(nested, reposOpenGroupsShut).find((r) => r.kind === 'repo')
  eq(
    repoRow.files.length,
    6,
    'an expanded repo whose changelists are shut still owns every file, submodule included',
  )

  const units = m.commitUnits(one, defaults)
  eq(units.length, 1, 'one repo, one commit')
  eq(units[0].repo, APP, 'named by RepoId — `repo_root` resolves an id, and a path is NoSuchRepo')
  eq(
    units[0].paths.sort(),
    ['a.rs', 'src/b.rs'],
    'commit reads the tree, not the rows: a collapsed group still commits its ticked files',
  )
  eq(units[0].changelist, 'default', 'the changelist is named when every tick came from one')

  eq(
    m.commitUnits(one, new Set(m.allFiles(one)))[0].changelist,
    null,
    'a selection spanning changelists names none — the backend commits the paths given',
  )
  eq(
    m.commitUnits(two, m.defaultSelection(two)).map((u) => u.repo),
    [APP, LIB],
    'two repos are two commits, split by repo id',
  )
  eq(
    m.commitUnits(nested, m.defaultSelection(nested)).map((u) => u.repo),
    [APP, SUB],
    'and a submodule is its own commit: git has no other kind',
  )

  eq(m.inRepo(m.fileRowId(APP, 'a.rs'), APP), true, 'a row belongs to its own repo')
  eq(
    m.inRepo(m.fileRowId(`${APP}x`, 'a.rs'), APP),
    false,
    'and not to a repo whose id is merely a prefix of it',
  )

  // --- normalisation is total ----------------------------------------------------------

  eq(m.normalizeStatus(undefined), { repos: [] }, 'no payload is an empty panel, not a throw')
  eq(m.normalizeStatus(null), { repos: [] }, 'null too')
  eq(m.normalizeStatus('nope'), { repos: [] }, 'a string too')
  eq(m.normalizeStatus({ repos: 'nope' }), { repos: [] }, 'a wrongly-typed field too')
  eq(
    m.normalizeStatus({
      repos: [
        {
          root: '/w/app',
          label: 'app',
          groups: [{ id: 'default', name: 'Changes', files: [{ path: 'a.rs', status: 'modified' }] }],
        },
      ],
    }),
    { repos: [] },
    'the shape the panel used to assume is NOT accepted — this assertion is the bug itself, '
      + 'written down: a fixture in that shape passed every check while the panel was empty '
      + 'against every real repository',
  )
  eq(
    m.normalizeStatus({ repos: [{ repo: { root: '/w/app' } }] }),
    { repos: [] },
    'a repo with no id is dropped: every git command resolves a RepoId, so its rows could '
      + 'draw but none of its buttons could work',
  )
  eq(
    m.normalizeStatus({ repos: [{ repo: { id: 'x', root: '/w/app' } }] }).repos[0],
    {
      id: 'x',
      root: '/w/app',
      name: 'app',
      branch: { head: '', detached: false, upstream: null, ahead: 0, behind: 0, operation: null, unborn: true },
      isSubmodule: false,
      indexChangedExternally: false,
      useStagingArea: false,
      groups: [],
      children: [],
    },
    'missing optionals are filled from what is there, and a branch we could not read is '
      + 'unborn — which disables Amend rather than enabling history rewriting on a guess',
  )
  eq(
    m.normalizeStatus({
      repos: [
        {
          repo: { id: 'x', root: '/w/app' },
          changelists: [{ id: 'd', name: 'C', changes: [{ path: 'a', index: 'exploded' }] }],
        },
      ],
    }).repos[0].groups[0].entries[0].index,
    'modified',
    'an unknown FileState shows as modified rather than hiding the file from the commit',
  )
  eq(
    m.normalizeStatus({
      repos: [
        {
          repo: { id: 'x', root: '/w/app' },
          changelists: [{ id: 'd', name: 'C', changes: [{ path: 'a', orig_path: 'b' }] }],
        },
      ],
    }).repos[0].groups[0].entries[0].origPath,
    null,
    'snake_case is NOT accepted: a missing rename_all_fields is a Rust bug to fix at source',
  )

  // --- the click rule, including the conditional one ------------------------------------

  /*
   * > *"in git files tree when i do one click on element - we should select it, but not open
   * > the diff. Only when diff is already opened one click should change current diff to
   * > selected file. Otherwise diff should be opened only via double click."*
   *
   * The third clause is a state dependency and it is the reason `clickSemantics.ts` exists as
   * a module rather than as three `onMouseDown` handlers. `diffOpen` comes from the Rust-owned
   * workspace (`openDiffTabs.ts`), so the rule below is the whole of what the tree decides.
   */
  const c = await import(`file://${join(out, 'sidebar', 'clickSemantics.js')}`)
  const click = (gesture, expandable, diffOpen) => c.gitTreeClick({ gesture, expandable, diffOpen })
  const act = (select, toggle, open) => ({ select, toggle, open })

  eq(
    click('single', false, false),
    act(true, false, false),
    'one click on a file with no diff on screen selects it and opens nothing — the report',
  )
  eq(
    click('single', false, true),
    act(true, false, true),
    'one click on a file WHILE a diff is open switches the diff to it, which is the whole of '
      + 'the conditional clause',
  )
  eq(
    click('double', false, false),
    act(true, false, true),
    'a double click opens the diff whether or not one was already there',
  )
  eq(click('double', false, true), act(true, false, true), 'and does the same when one was')
  eq(
    c.gestureOf(2),
    'double',
    'the second click of a double is told apart by `detail`, not by a timer that would make '
      + 'every single click in the tree feel late',
  )
  eq(c.gestureOf(1), 'single', 'and the first click of one is an ordinary single')
  eq(c.gestureOf(3), 'double', 'a triple click keeps opening rather than falling back to select')

  eq(
    click('single', true, false),
    act(true, true, false),
    'a changelist or repo row folds on a single click — it holds no diff, and there is nothing '
      + 'else a click on one could mean',
  )
  eq(
    click('double', true, true),
    act(false, false, false),
    'and the second half of a double-click on a group does NOTHING: `click` fires twice, so '
      + 'folding on both halves would open the group and shut it again in one gesture',
  )

  // --- and WHICH command an opening gesture routes to ------------------------------------

  /*
   * `gitTreeClick` above says *whether* a gesture opens. It returns `open` for two different
   * reasons, and until the retarget landed both went to `tab_open_diff`, whose reuse is keyed
   * on (repo, path) — so a single click down a 30-file changelist produced 30 tabs, which is
   * the bug the user reported. The second half of the decision is therefore load-bearing and
   * is checked here rather than left as an inline ternary in `ChangesTree`, which needs a DOM
   * and so cannot be executed by anything in this repo.
   */
  eq(
    m.diffOpenMode('single'),
    'retarget',
    'a single click that opens RETARGETS — one tab follows the pointer down the changelist',
  )
  eq(
    m.diffOpenMode('double'),
    'open',
    'and a double click opens a tab of its own, which is what marks it kept in Rust',
  )

  /*
   * The mapping being right is worth nothing if nothing calls it, and this project has shipped
   * a feature that was correct and unreachable more than once — see the note at the end of
   * `check-rows.mjs`. Everything from here down is a source assertion, and worth being exact
   * about what that buys: it proves the chain is spelled out, not that a click travels it.
   * The behaviour at the far end is covered by `cmd::file::tests` in Rust.
   */
  const tree = readFileSync(join(UI, 'src/sidebar/GitPanel/ChangesTree.tsx'), 'utf8')
  ok(
    /onOpenDiff\(row,\s*diffOpenMode\(gesture\)\)/.test(tree),
    'the mousedown handler routes through `diffOpenMode` — an inline ternary here is exactly '
      + 'the link no test can reach',
  )

  const hook = readFileSync(join(UI, 'src/sidebar/GitPanel/useGitPanel.ts'), 'utf8')
  ok(
    /mode === 'retarget'/.test(hook) && /gitDiffApi\.retargetTab\(/.test(hook),
    '`showDiff` branches on the mode and calls `retargetTab` — without the branch every '
      + 'gesture falls back to `openTab` and the thirty tabs come back',
  )

  const client = readFileSync(join(UI, 'src/ipc/client.ts'), 'utf8')
  ok(
    /retargetTab:[\s\S]{0,400}?invoke<TabId>\('tab_retarget_diff'/.test(client),
    '`gitDiff.retargetTab` invokes `tab_retarget_diff`',
  )
  // One `gitDiff` namespace, not two. Two agents last round both appended a `clipboard`
  // namespace to this file and the merge was a redeclaration; this is that rule, held.
  eq(
    (client.match(/^export const gitDiff = \{/gm) ?? []).length,
    1,
    'client.ts declares the `gitDiff` namespace exactly once',
  )

  // --- changelists: the group menu, the chooser, and the empty list ----------------------

  /*
   * A changelist the user just made is **empty**, and that is the case the whole create half
   * of the feature runs through: `git_changelist_create` answers with a tree, the new list is
   * in it with no files, and the chooser has to find it there. `normalizeRepo` used to drop
   * every empty group, so the list vanished between Rust and the panel — the row was never
   * drawn, the move chooser could not offer it, and `findChangelistId` returned `null`, which
   * made the inline *Create and move* fail with "was created but the files did not move" on
   * every single use. These assertions are that bug, pinned.
   */
  const withEmpty = {
    repos: [
      {
        ...repo(APP, '/w/app', 'app'),
        changelists: [
          ...repo(APP, '/w/app', 'app').changelists,
          { id: 'my-list', name: 'My list', comment: '', active: false, changes: [] },
        ],
        conflicts: [],
      },
    ],
  }
  const withEmptyView = m.normalizeStatus(withEmpty)
  eq(
    withEmptyView.repos[0].groups.map((g) => g.id),
    ['cl:default', 'cl:fixes', 'cl:my-list', 'unversioned', 'ignored'],
    'an empty CHANGELIST survives normalisation — it is a thing the user made, and a freshly '
      + 'created one has no files by definition — while the empty `conflicts` sibling list, '
      + 'which is derived from the walk, is still dropped',
  )
  eq(
    m.findChangelistId(withEmpty, APP, 'My list'),
    'my-list',
    'so the id of a just-created list can be read back out of the tree `git_changelist_create` '
      + 'answers with: the chooser mints no slug of its own, and this is the only step between '
      + '"create" and "move these files into it"',
  )
  eq(m.findChangelistId(withEmpty, APP, 'Nope'), null, 'and an absent name is null, not a guess')
  eq(
    m.changelistsOf(withEmptyView, APP).map((l) => [l.id, l.count]),
    [['default', 2], ['fixes', 1], ['my-list', 0]],
    'the chooser offers every changelist including the empty one — a list can only be moved '
      + 'back into if it is offered while it holds nothing — and never the sibling lists',
  )
  eq(
    m.buildRows(m.normalizeStatus({ repos: [{ ...repo(APP, '/w/app', 'app'), changelists: [{ id: 'default', name: 'Changes', comment: '', active: true, changes: [] }], unversioned: [], ignored: [] }] }), new Set()),
    [],
    'a clean repository still renders nothing, empty default changelist and all',
  )

  eq(m.changelistIdOf('cl:fixes'), 'fixes', 'the `cl:` prefix comes off before Rust sees the id')
  eq(
    m.changelistIdOf('unversioned'),
    null,
    'and a sibling list is not a changelist: sending its group id to git_changelist_* would be '
      + 'a NoSuchChangelist on a menu item that looked fine',
  )
  eq(m.changelistIdOf(undefined), null, 'a row with no group id is not a changelist either')
  eq(m.groupOf(withEmptyView, APP, 'cl:fixes').name, 'fixes', 'a group is found by its group id')
  eq(m.groupOf(withEmptyView, APP, 'nope'), undefined, 'and an unknown one is undefined')

  // --- directory rows -------------------------------------------------------------------

  /*
   * > *"i should be able to drag one or multiple selected elements (including whole
   * > directories) to another changelist"*
   *
   * There was nothing to grab: every file was one row carrying its whole path. A directory row
   * is the row that means "everything under here", and it is scoped to one group — the same
   * `src/` in two changelists is two rows, so a drag from one can never carry the other's files.
   */
  const rowsE = m.buildRows(withEmptyView, openAll(withEmptyView))
  const rowA = rowsE.find((r) => r.label === 'a.rs')
  const rowB = rowsE.find((r) => r.label === 'b.rs')
  const rowSrc = rowsE.find((r) => r.kind === 'dir' && r.path === 'src')
  const rowNotes = rowsE.find((r) => r.label === 'notes.md')
  eq(
    [rowSrc.kind, rowSrc.depth, rowSrc.label, rowSrc.groupKind],
    ['dir', 1, 'src', 'changelist'],
    'a file at `src/b.rs` puts a directory row between its changelist and itself',
  )
  eq(rowB.depth, 2, 'and the file sits one level inside it')
  eq(
    rowB.label,
    'b.rs',
    'showing its basename alone — the row above says `src`, and repeating it on every leaf is '
      + 'what the flat list did with the widest thing in a 420px panel',
  )
  eq(rowA.depth, 1, 'a file with no directory in its path stays where it always was')
  eq(
    rowSrc.files,
    [m.fileRowId(APP, 'src/b.rs')],
    'a directory row owns every file under it — this is what a drag from it carries, and what '
      + 'its tri-state checkbox folds over',
  )
  eq(
    m.dirRowId(APP, 'cl:default', 'src') === m.dirRowId(APP, 'cl:fixes', 'src'),
    false,
    'the same directory in two changelists is two rows: one id for both would make a drag from '
      + "one carry the other's files, and expanding one expand the other",
  )
  eq(
    m.defaultExpanded(withEmptyView).has(rowSrc.id),
    true,
    'directories start open, or the panel would open with changes hidden behind a twisty',
  )
  eq(
    m.buildRows(withEmptyView, new Set([m.groupRowId(APP, 'cl:default')])).map((r) => r.label),
    ['Changes', 'src', 'a.rs', 'fixes', 'My list', 'Unversioned Files', 'Ignored Files'],
    'a collapsed directory hides its files and keeps its own row; directories come before the '
      + "files beside them, which is what both other trees in this app do",
  )
  eq(
    m.buildRows(
      m.normalizeStatus({
        repos: [
          {
            repo: { id: APP, root: '/w/app' },
            changelists: [
              {
                id: 'default',
                name: 'C',
                changes: [
                  entry('ui/src/sidebar/GitPanel/model.ts', 'unmodified', 'modified', 'default'),
                ],
              },
            ],
          },
        ],
      }),
      new Set(['x']),
    ).flatMap((r) => (r.kind === 'dir' ? [[r.label, r.path]] : [])),
    [],
    'a collapsed changelist emits no directory rows either',
  )

  const deep = m.normalizeStatus({
    repos: [
      {
        repo: { id: APP, root: '/w/app' },
        changelists: [
          {
            id: 'default',
            name: 'C',
            changes: [
              entry('ui/src/sidebar/GitPanel/model.ts', 'unmodified', 'modified', 'default'),
              entry('ui/src/sidebar/GitPanel/types.ts', 'unmodified', 'modified', 'default'),
            ],
          },
        ],
      },
    ],
  })
  eq(
    m.buildRows(deep, m.allGroups(deep)).map((r) => [r.kind, r.label]),
    [
      ['group', 'C'],
      ['dir', 'ui/src/sidebar/GitPanel'],
      ['file', 'model.ts'],
      ['file', 'types.ts'],
    ],
    'a chain of directories with one child and no files of its own is compacted onto one row: '
      + 'four twisties to reach one file, at 14px of indent each, in a 420px panel',
  )
  eq(
    m.buildRows(deep, m.allGroups(deep))[1].path,
    'ui/src/sidebar/GitPanel',
    'and the compacted row keeps the deepest path, so its id and its files are unchanged by '
      + 'the collapsing',
  )

  // --- what a grab carries ---------------------------------------------------------------

  /*
   * `grab` is `actOn` with the rest of the rows added: it is the one answer to "which files is
   * this gesture about", read by the context menu *and* by the drag. Two functions answering
   * that separately is how a menu that moves four files ends up beside a drag that moves one.
   */
  const paths = (set) => set?.files.map((f) => f.path) ?? null
  eq(
    paths(d.grab(withEmptyView, new Set(), rowA)),
    ['a.rs'],
    'a gesture on an unticked row carries that row alone',
  )
  eq(
    paths(d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowA)),
    ['a.rs', 'src/b.rs'],
    "and the ticks when the row is one of them — IDEA's multi-file move, and the reason the "
      + 'count goes in the menu label and on the drag ghost',
  )
  eq(
    d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowA).widened,
    true,
    'a widened grab says so, because that is the fact the user has to see before they let go',
  )
  eq(
    d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowA).label,
    '2 files',
    'and the ghost says it in words',
  )
  eq(
    paths(d.grab(withEmptyView, new Set([rowA.id, rowNotes.id]), rowA)),
    ['a.rs'],
    'an unversioned tick is never swept into a changelist gesture: `status::repo_changes` '
      + 'builds `live` from paths that are neither Untracked nor Ignored, so filing one is a '
      + 'write the next status walk undoes',
  )
  eq(
    paths(d.grab(withEmptyView, new Set([rowNotes.id]), rowNotes)),
    ['notes.md'],
    'and the widening never returns an empty set, whatever is ticked',
  )
  eq(
    paths(d.grab(withEmptyView, new Set(), rowSrc)),
    ['src/b.rs'],
    'a directory row carries every file under it — the whole point of the row',
  )
  eq(
    paths(d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowSrc)),
    ['src/b.rs'],
    'and never widens to the ticks: the row already names a set, and sweeping in files from '
      + 'outside it would move what the user cannot see from the row they grabbed',
  )
  eq(
    d.grab(withEmptyView, new Set(), rowsE.find((r) => r.label === 'Changes')),
    null,
    'a changelist header is not a drag source: a press on one FOLDS it (see `gitTreeClick`), '
      + 'so a drag from it would begin by collapsing the thing being dragged. The group menu '
      + 'moves a whole changelist instead, with a count to read first',
  )
  eq(
    d.grab(m.normalizeStatus({ repos: [repo(APP, '/w/app', 'app'), repo(LIB, '/w/lib', 'lib')] }),
      new Set(),
      m.buildRows(two, openAll(two)).find((r) => r.kind === 'repo')),
    null,
    'and neither is a repository row: it is in no changelist, and its files span its submodules '
      + "— which have their own sidecar and cannot be filed into this repo's lists",
  )

  // --- which targets accept ---------------------------------------------------------------

  const carried = d.grab(withEmptyView, new Set(), rowA)
  const group = (label) => rowsE.find((r) => r.kind === 'group' && r.label === label)
  eq(
    d.dropOutcome(carried, group('fixes')),
    {
      kind: 'move',
      repo: APP,
      changelist: 'fixes',
      list: 'fixes',
      paths: ['a.rs'],
      hint: 'Move 1 file to “fixes”',
    },
    'dropping a file on another changelist moves it, and the id sent to Rust is the RAW one — '
      + 'the `cl:` prefix on a group id is a `NoSuchChangelist` on arrival',
  )
  eq(
    d.dropOutcome(carried, group('Changes')),
    { kind: 'noop', list: 'Changes', hint: 'Already in “Changes”' },
    'dropping it on the list it is already in is a no-op, not an error: nothing is sent, no '
      + 'busy line flashes, and the target still reads as accepting',
  )
  eq(
    d.dropOutcome(d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowA), group('fixes')).paths,
    ['a.rs', 'src/b.rs'],
    'a multi-file drop sends every file that changes list',
  )
  eq(
    d.dropOutcome(
      { ...carried, files: [carried.files[0], { id: 'x', path: 'c.rs', changelist: 'fixes' }] },
      group('fixes'),
    ).paths,
    ['a.rs'],
    'and only those: a file already in the target is left out of the command rather than sent '
      + 'as a move to where it is',
  )
  eq(
    d.dropOutcome(carried, group('Unversioned Files')),
    {
      kind: 'refuse',
      reason: '“Unversioned Files” is not a changelist — nothing can be filed there',
      hint: '“Unversioned Files” is not a changelist — nothing can be filed there',
    },
    'the sibling lists refuse, VISIBLY. This is the drop that must not silently succeed: '
      + '`Sidecar::move_paths` would happily record it and the next status walk would drop it, '
      + 'which looks exactly like a broken feature',
  )
  eq(
    d.dropOutcome(carried, group('Ignored Files')).kind,
    'refuse',
    'the ignored list too',
  )
  eq(
    d.dropOutcome(d.grab(withEmptyView, new Set(), rowNotes), group('fixes')).reason,
    'Only tracked changes belong to a changelist',
    'and an unversioned file refuses wherever it is dropped — the source is reported before '
      + 'the target, so the answer is the same over every row instead of a new complaint per '
      + 'changelist. Same sentence the context menu gives',
  )
  eq(
    d.dropOutcome(carried, null).kind,
    'refuse',
    'the empty space below the tree takes nothing',
  )
  eq(
    d.dropOutcome({ ...carried, repo: LIB }, group('fixes')).reason,
    'A changelist belongs to one repository — these files are in another',
    'and neither does another repository: a changelist lives in one repo\'s sidecar',
  )

  eq(
    [...d.bands(rowsE)].filter(([, g]) => g === group('Changes').id).map(([id]) =>
      rowsE.find((r) => r.id === id).label,
    ),
    ['Changes', 'src', 'b.rs', 'a.rs'],
    'the band of a changelist is the group row and everything under it, directories included — '
      + 'what gets tinted while a drop is being aimed, because the header itself is usually '
      + 'above the fold by the time the pointer is over the files',
  )
  eq(
    d.bands(m.buildRows(two, openAll(two))).get(m.repoRowId(LIB)),
    undefined,
    'a repository row is in no band: it sits above its own groups, and the group before it '
      + 'belongs to the other repository',
  )

  // Where a drop lands: any row inside a changelist resolves to that changelist, so the target
  // is the whole band rather than one 23px header that may have scrolled off the top.
  eq(d.dropTarget(rowsE, rowB.id)?.label, 'Changes', 'a file row resolves to its changelist')
  eq(d.dropTarget(rowsE, rowSrc.id)?.label, 'Changes', 'a directory row does too')
  eq(d.dropTarget(rowsE, group('fixes').id)?.label, 'fixes', 'and a group row is its own target')
  eq(d.dropTarget(rowsE, null), null, 'nothing under the pointer is no target')
  eq(
    d.dropTarget(m.buildRows(two, openAll(two)), m.repoRowId(LIB)),
    null,
    'a REPOSITORY row resolves to nothing at all: it sits above its own groups, so walking '
      + "back from it finds the previous repository's last changelist — the wrong sidecar",
  )
  eq(
    d.dropTarget(m.buildRows(nested, openAll(nested)), m.repoRowId(SUB)),
    null,
    'including a submodule row, whose parent’s changelist cannot hold its files',
  )

  /*
   * Reachability. The domain and the IPC for changelists shipped in M8 and were reachable from
   * nothing for three rounds because `items()` returned `[]` for every group row — the exact
   * failure `check-rows.mjs` ends with a note about. Source assertions, so they prove the
   * chain is spelled out rather than that a click travels it.
   */
  const host = readFileSync(join(UI, 'src/sidebar/GitPanel/GitPanelHost.tsx'), 'utf8')
  ok(
    /if \(entry === undefined\) return row\.kind === 'dir' \? dirMenu\(row, git\) : groupMenu\(row, git\)/
      .test(host),
    'a right-click on a group row opens the group menu rather than nothing — and a directory '
      + 'row opens its own, rather than the menu of the changelist it happens to sit in, which '
      + 'would offer to rename and delete that list',
  )
  /*
   * Drag and drop is unusable without a pointer, so every gesture it offers must also be on the
   * menu — which `useContextMenu` opens from Shift+F10 on the focused row. These are source
   * assertions: they prove the route is spelled out, not that a keystroke travels it.
   */
  ok(
    /const carried = grab\(git\.view, git\.selected, row\)/.test(host),
    'the file-row menu takes its files from `grab` — the same function a drag from that row '
      + 'uses, so the two routes cannot come to disagree about what the user is pointing at',
  )
  ok(
    /function dirMenu\([\s\S]{0,900}?git\.moveToChangelist\(row\.repo, paths\)/.test(host),
    'a directory row can be moved from the keyboard, not only dragged',
  )
  ok(
    /label: `Move \$\{plural\(count\)\} to Changelist…`,\s*\.\.\.\(empty/.test(host),
    'and so can a whole changelist — the one row a drag deliberately cannot start from',
  )
  const tree2 = readFileSync(join(UI, 'src/sidebar/GitPanel/ChangesTree.tsx'), 'utf8')
  ok(
    /onPointerDown=\{\(e\) => drag\.onPointerDown\(e, row\)\}/.test(tree2)
      && /useChangesDrag\(\{ rows, view, selected, container, onMove: onMovePaths \}\)/.test(tree2),
    'every row is a drag source candidate and the tree is wired to the pointer state machine — '
      + 'the rules being right is worth nothing if no gesture reaches them',
  )
  ok(
    /onMouseUp=\{\(e\) => \{\s*if \(e\.button !== 0 \|\| drag\.dragged\(\)\) return/.test(tree2),
    'and an expandable row folds on RELEASE, skipped when the press became a drag: a directory '
      + 'row is both a twisty and a drag handle, and folding on the press would take the rows '
      + 'being dragged off the screen at the moment the drag starts',
  )
  ok(
    /className=\{styles\.checkHit\}[\s\S]{0,600}?onPointerDown=\{\(e\) => e\.stopPropagation\(\)\}/
      .test(tree2),
    'the checkbox is a control and not a handle — a hand that shifts two pixels while ticking '
      + 'a box must not pick the row up',
  )
  ok(
    /className=\{styles\.checkHit\}[\s\S]{0,400}?onMouseUp=\{\(e\) => e\.stopPropagation\(\)\}/
      .test(tree2),
    'and the box swallows the release as well as the press, or ticking a changelist would fold '
      + 'it — the fold moved to mouseup, and the box only ever stopped mousedown',
  )
  ok(
    /git\.revertGroup\(/.test(host) && /git\.moveToChangelist\(/.test(host)
      && /git\.newChangelist\(/.test(host) && /git\.shelveGroup\(/.test(host),
    'and that menu reaches revert, move, create and shelve — every verb the brief asked for',
  )
  const panel = readFileSync(join(UI, 'src/sidebar/GitPanel/GitPanel.tsx'), 'utf8')
  ok(
    /<ChangelistDialog/.test(panel) && /<ConfirmDestructive/.test(panel),
    'both overlays are mounted by the panel itself — no line in App.tsx, which this feature '
      + 'does not own, stands between the gesture and the dialog',
  )
  const model = readFileSync(join(UI, 'src/sidebar/GitPanel/useGitPanel.ts'), 'utf8')
  ok(
    !/rollbackFile = useCallback\(\s*\(repo: RepoId, path: string\) => \{[\s\S]{0,200}gitApi\.rollback/.test(model)
      && /rollbackFile = useCallback\(\s*\(repo: RepoId, path: string\) => revertFiles/.test(model),
    'every route to `git_rollback` goes through the confirmation: the unguarded call is '
      + 'private to the hook, which is what stops the next caller skipping the dialog',
  )
  ok(
    /onMovePaths=\{git\.movePaths\}/.test(panel)
      && /gitApi\.changelist\.movePaths\(p, repo, changelist, paths\)/.test(model),
    'and a drop reaches `git_changelist_move_paths` through the panel itself — no line in '
      + 'App.tsx, which this feature does not own, stands between the gesture and the command',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('git panel model: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
