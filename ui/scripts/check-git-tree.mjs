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
        // Which rows a gesture selects — the tree's third concept, and the one `grab` widens
        // by. Same argument as `dragDrop.ts`: the rules are plain functions precisely so this
        // script can execute them, because the gesture that drives them needs a pointer.
        join(UI, 'src', 'sidebar', 'GitPanel', 'rowSelection.ts'),
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
  /*
   * No specifier rewrite for this one, and that is an assertion in itself: `rowSelection.ts`
   * takes only `type Row` from `model.ts`, so `verbatimModuleSyntax` erases the import and the
   * emitted file has none at all. If someone adds a value import, this line throws
   * `ERR_MODULE_NOT_FOUND` rather than quietly pulling half the panel into a check that is
   * supposed to be running rules.
   */
  const s = await import(`file://${join(out, 'sidebar', 'GitPanel', 'rowSelection.js')}`)

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
          // Both sides dirty: partially staged. Kept because that used to drag its own box
          // and every ancestor to `–`; see the tri-state section.
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

  /*
   * There are two different questions here and they must not be answered by one number.
   *
   * A **group** row counts its own files, ignored group included: that row exists to say how
   * many ignored files there are.
   *
   * A **repo** row counts *changes*, which excludes them — and this assertion used to say `5`
   * against a fixture whose fifth file is `ignored: [target]`. That was the bug being pinned
   * rather than the behaviour: the repo row's number moved when the user opened the Ignored
   * twisty, because opening it flips `includeIgnored` on the next `git_status` and the panel
   * summed every group. A count that changes because a twisty was opened is wrong, and once
   * the same number is on the activity rail it is wrong somewhere nothing explains it.
   *
   * So the repo row and the rail are both `countRepoFiles`, and the group rows are their own
   * `entries.length`. A repo row therefore need not equal the sum of its group rows when the
   * ignored group is on screen — the two are labelled differently and mean different things.
   */
  eq(rows1.find((r) => r.label === 'Changes').count, 2, 'a changelist counts its own files')
  eq(
    rows1.find((r) => r.label === 'Ignored Files').count,
    1,
    'and so does the ignored group — that row is *about* the ignored files',
  )
  eq(rowsN[0].count, 5, "a repo's count includes its submodule's files")
  eq(rows2[0].count, 4, 'and a plain repo counts every changed file under it, ignored excluded')

  eq(
    m.counts('ignored'),
    false,
    'the one rule, stated once: ignored files are not changes',
  )
  for (const kind of ['changelist', 'conflicts', 'unversioned']) {
    eq(
      m.counts(kind),
      true,
      `${kind} files are — a conflict blocks a commit, so a badge that dropped it would read `
        + 'low during exactly the merge it should be shouting about',
    )
  }

  // --- the activity rail's number, which must be the panel's ------------------------------

  eq(m.countChangedFiles({ repos: [] }), 0, 'no repositories, nothing changed')
  eq(
    m.countChangedFiles(two),
    rows2.filter((r) => r.kind === 'repo').reduce((n, r) => n + r.count, 0),
    'the rail total IS the sum of the repo rows — not a second walk that agrees, the same '
      + 'function summed. One badge and a panel disagreeing about how much work is '
      + 'uncommitted would make both untrustworthy.',
  )
  eq(m.countChangedFiles(two), 8, 'two repos of four changed files each')
  eq(
    m.countChangedFiles(nested),
    5,
    'a submodule is counted through `children`, exactly once',
  )

  /*
   * The invariance that motivates all of it: opening the Ignored twisty is what makes the
   * next payload carry ignored entries, and the badge must not move when it does.
   */
  {
    const clean = { repos: [{ ...repo(APP, '/w/app', 'app'), ignored: [] }] }
    const loud = {
      repos: [
        {
          ...repo(APP, '/w/app', 'app'),
          ignored: Array.from({ length: 11000 }, (_, i) => entry(`target/${i}`, 'unmodified', 'ignored', 'default')),
        },
      ],
    }
    eq(
      m.countChangedFiles(m.normalizeStatus(loud)),
      m.countChangedFiles(m.normalizeStatus(clean)),
      'eleven thousand ignored files do not move the badge by one — the user opened a '
        + 'twisty, they did not do eleven thousand units of work',
    )
  }

  /*
   * And a repository holding *only* ignored files still draws. `walkRepo` elides a repo with
   * nothing to show, and "nothing" there has to mean "no rows" rather than "no changes" —
   * otherwise asking to see a repo's ignored files makes the repo disappear.
   */
  {
    const onlyIgnored = m.normalizeStatus({
      repos: [
        { ...repo(APP, '/w/app', 'app'), changelists: [], unversioned: [], conflicts: [] },
        repo(LIB, '/w/lib', 'lib'),
      ],
    })
    const drawn = m.buildRows(onlyIgnored, openAll(onlyIgnored))
    ok(
      drawn.some((r) => r.label === 'Ignored Files' && r.repo === APP),
      'a repository whose only content is ignored files keeps its rows',
    )
    eq(
      drawn.find((r) => r.kind === 'repo' && r.repo === APP).count,
      0,
      'and its repo row honestly says zero changes, which is what it has',
    )
  }

  // --- what the badge draws ----------------------------------------------------------------

  /*
   * `null` and `0` are different facts with the same rendering, and both renderings are
   * decisions rather than omissions.
   *
   * `null` is "no walk has landed yet". Drawing `0` there would be a claim — "your tree is
   * clean" — made before anything had been looked at, and it would flash on every launch.
   * `0` is a clean tree, and a zero badge is noise: it is the state the badge exists to
   * distinguish *from*. `Explorer`'s header withholds its count at zero for the same reason,
   * and its comment is the one this follows.
   */
  eq(m.badgeText(null), null, 'nothing is drawn before the first walk lands')
  eq(m.badgeText(0), null, 'and nothing is drawn for a clean tree')
  eq(m.badgeText(1), '1', 'one changed file')
  eq(m.badgeText(99), '99', 'two digits still fit the 28px button inside a 42px rail')
  eq(m.badgeText(100), '99+', 'and past that it caps — `1,203` is five glyphs and does not fit')
  eq(m.badgeText(11803), '99+', 'however far past')
  eq(
    m.badgeText(-1),
    null,
    'a negative count is not a thing, and rendering `-1` would be worse than rendering nothing',
  )

  eq(m.badgeLabel(null, ''), undefined, 'a badge that is not drawn adds nothing to the button name')
  eq(m.badgeLabel(0, '0'), undefined, 'nor does a clean tree')
  eq(m.badgeLabel(1, '1'), '1 changed file', 'singular')
  eq(m.badgeLabel(14, '14'), '14 changed files', 'plural')
  eq(
    m.badgeLabel(1203, '1,203'),
    '1,203 changed files',
    'the exact number reaches the tooltip and the accessible name even though the badge says '
      + '`99+` — a number nobody can act on precisely belongs where there is room for it',
  )

  // --- tri-state -----------------------------------------------------------------------

  const changes = rows1.find((r) => r.label === 'Changes')
  const none = new Set()
  eq(m.checkState(changes, none), 'unchecked', 'nothing ticked')

  const all = m.toggleRow(changes, none)
  eq(all.size, 2, 'ticking a changelist reaches every file in it')
  eq(
    m.checkState(changes, all),
    'checked',
    'a changelist ticked whole reads `✓`. One of these two files is staged only in part — '
      + 'index AND worktree both dirty — and that used to force the group to `–` for ever: a '
      + 'box reporting a property of `.git/index` that no click on it could change, about an '
      + 'index `cide_git::commit` resets to HEAD before it writes anything anyway',
  )
  eq(m.toggleRow(changes, all).size, 0, 'a second click clears rather than completing')

  const half = new Set([...all].slice(0, 1))
  eq(
    m.checkState(changes, half),
    'partial',
    'and `partial` still means the one thing a click can act on: some of these files are '
      + 'ticked and some are not',
  )

  const fixes = rows1.find((r) => r.label === 'fixes')
  eq(m.checkState(fixes, m.toggleRow(fixes, none)), 'checked', 'every file ticked is checked')

  // --- a tick follows its file into the changelist it was filed into -----------------------
  //
  // The defect: a file row's id is repo + path, and filing a change into another changelist
  // changes neither — so the tick stayed exactly where it was. Filing a file *out* of the
  // list about to be committed left it ticked, and the commit took it anyway.

  const aId = m.fileRowId(APP, 'a.rs')
  const cId = m.fileRowId(APP, 'c.rs')

  eq(
    m.ticksAfterMove(one, all, APP, 'fixes', ['a.rs']).has(aId),
    false,
    'filing a ticked file into a list with nothing ticked takes it out of the commit, which '
      + 'is what moving it there was for',
  )
  eq(
    m.ticksAfterMove(one, new Set([cId]), APP, 'fixes', ['a.rs']).has(aId),
    true,
    'and a list the user has already ticked is one they are building a commit out of, so a '
      + 'file joining it joins the commit — the reason this reads the destination rather than '
      + 'the active flag',
  )
  eq(
    m.ticksAfterMove(one, all, APP, 'not-a-list-yet', ['a.rs']).has(aId),
    false,
    '`New changelist…` mints its list in the same gesture, so the destination does not exist '
      + 'here yet. It holds nothing, so it takes the same answer as an untouched list',
  )
  eq(
    [...m.ticksAfterMove(one, all, APP, 'fixes', [])].sort(),
    [...all].sort(),
    'moving nothing changes nothing',
  )
  eq(
    m.ticksAfterMove(one, all, APP, 'fixes', ['a.rs']).has(m.fileRowId(APP, 'src/b.rs')),
    true,
    'and only the files that moved are touched',
  )

  // --- Space over a multi-row selection --------------------------------------------------
  //
  // This is here because the defect it pins was invisible to all 29 gates: the rule lived in
  // `useGitPanel.ts`, the one file in this panel a check script cannot compile, and it was the
  // only selection rule not placed where this script could run it.
  //
  // The shape of the bug: `Row.files` is a whole subtree, and a selection spanning a folder
  // ALWAYS holds the folder row *and* the rows beneath it — `between` keeps every selectable
  // row in a shift band, `selectAll` keeps every row on screen. Folding `toggleRow` over that
  // visits each file twice, so it lands back where it started, and what survives is the
  // inverse of the documented rule: any tick anywhere leaves the subtree fully ticked. The
  // next thing this panel does is `git commit`, so that silently re-ticked files the user had
  // excluded.
  // A fixture of its own rather than one of the shared ones: this needs a directory holding
  // *two* files, and pinning that shape onto `nested` would make an unrelated edit there fail
  // here for a reason nobody reading it would guess. `walkDir` compacts single-child chains,
  // so `src/` must hold two entries to survive as a row at all.
  const dirFixture = m.normalizeStatus({
    repos: [
      {
        ...repo(APP, '/w/app', 'app'),
        changelists: [
          {
            id: 'default',
            name: 'Changes',
            comment: '',
            active: true,
            changes: [
              entry('src/keep.rs', 'modified', 'unmodified', 'default'),
              entry('src/excluded.rs', 'modified', 'unmodified', 'default'),
            ],
          },
        ],
        unversioned: [],
        ignored: [],
      },
    ],
  })
  const rowsD = m.buildRows(dirFixture, openAll(dirFixture))
  const dirRow = rowsD.find((r) => r.kind === 'dir' && r.files.length > 1)
  ok(dirRow !== undefined, 'the fixture yields a directory row with several files under it')
  const kidIds = new Set(dirRow.files)
  const band = new Set([
    dirRow.id,
    ...rowsD.filter((r) => r.kind === 'file' && kidIds.has(r.id)).map((r) => r.id),
  ])
  eq(band.size, 3, 'and the band holds the folder as well as both rows under it')

  eq(
    [...m.toggleRows(rowsD, band, none)].sort(),
    [...dirRow.files].sort(),
    'Space over a folder and its children ticks the subtree exactly once, not twice back to nothing',
  )

  // The commit-safety case, stated as itself rather than as a corollary.
  const [keep, excluded] = dirRow.files
  const afterExcluding = m.toggleRows(rowsD, band, new Set([keep]))
  ok(
    !afterExcluding.has(excluded),
    'a file the user deliberately unticked is NOT re-ticked by Space over its folder — the '
      + 'failure here puts a file into a commit that the user removed from it',
  )
  eq(afterExcluding.size, 0, 'any tick in the selection means Space clears, rather than completing')

  // Order independence. A fold's answer depended on whether `walkDir` emitted a directory
  // before its children, which is a property of the tree builder and not of anything the user
  // did — so the same selection could mean two things.
  eq(
    [...m.toggleRows([...rowsD].reverse(), band, none)].sort(),
    [...m.toggleRows(rowsD, band, none)].sort(),
    'the answer does not depend on the order rows happen to be walked in',
  )

  // One rule, not two: the single-row entry point must be the multi-row one with one row.
  for (const row of [dirRow, rowsD.find((r) => r.kind === 'file'), changes]) {
    eq(
      [...m.toggleRows(row === changes ? rows1 : rowsD, new Set([row.id]), none)].sort(),
      [...m.toggleRow(row, none)].sort(),
      `\`toggleRows\` with one ${row.kind} row agrees with \`toggleRow\` — they are one rule`,
    )
  }

  // A selection made before a group was folded away must not act on what is no longer shown.
  eq(
    m.toggleRows([], band, none).size,
    0,
    'a selected id with no row on screen contributes nothing — Space acts on what is visible',
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
    m.checkState(changesCollapsed, defaults),
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
  const rowIgnored = rowsE.find((r) => r.label === 'target')
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
      + 'four twisties to reach one file, at 19px of indent each, in a 420px panel',
  )
  eq(
    m.buildRows(deep, m.allGroups(deep))[1].path,
    'ui/src/sidebar/GitPanel',
    'and the compacted row keeps the deepest path, so its id and its files are unchanged by '
      + 'the collapsing',
  )

  // --- which rows a gesture selects --------------------------------------------------------

  /*
   * The row selection: the tree's third concept, beside the ticks and the cursor.
   *
   * It exists because `grab` used to widen by **ticks**, and the active changelist opens fully
   * ticked — so dragging one file out of it moved every file in it. Every rule below is one
   * the pointer and the keyboard now share, which is the other half of the point: shift+↓↓ and
   * shift-clicking two rows down go through the same function and cannot drift apart.
   */
  const sel = (ids, anchor = null) => ({ ids: new Set(ids), anchor })
  const idsOf = (s) => [...s.ids].sort()

  eq(
    idsOf(s.pressSelect(rowsE, s.NO_ROWS, rowA.id, { ctrl: false, shift: false }).next),
    [rowA.id],
    'a plain press selects the row it landed on and nothing else',
  )
  eq(
    s.pressSelect(rowsE, s.NO_ROWS, rowA.id, { ctrl: false, shift: false }).next.anchor,
    rowA.id,
    'and puts the anchor there, so a shift-click after it has somewhere to extend from',
  )
  eq(
    s.pressSelect(rowsE, sel([rowB.id], rowB.id), rowA.id, { ctrl: false, shift: false }).deferred,
    false,
    'a plain press on an UNSELECTED row takes effect immediately — this is what makes "a drag '
      + 'starting on an unselected row selects it first" true without the drag knowing anything '
      + 'about selection',
  )
  eq(
    s.pressSelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowB.id, { ctrl: false, shift: false }),
    { next: sel([rowA.id, rowB.id], rowA.id), deferred: true },
    'a plain press on a row that is ALREADY one of several selected changes nothing yet and '
      + 'says so: the press is how a drag of the whole selection begins, and collapsing here '
      + 'would destroy the set before the pointer had moved a pixel',
  )
  eq(
    s.pressSelect(rowsE, sel([rowA.id], rowA.id), rowA.id, { ctrl: false, shift: false }).deferred,
    false,
    'and it is not deferred when that row is the only one selected — there is no set to '
      + 'preserve, and deferring would leave the anchor stale',
  )
  eq(
    idsOf(s.releaseSelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowB.id)),
    [rowB.id],
    'the release of a deferred press is what finally collapses it. The caller only gets here '
      + 'when the gesture did NOT become a drag',
  )
  eq(
    idsOf(s.pressSelect(rowsE, sel([rowA.id], rowA.id), rowB.id, { ctrl: true, shift: false }).next),
    [rowA.id, rowB.id].sort(),
    'ctrl adds a row to the selection',
  )
  eq(
    idsOf(
      s.pressSelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowA.id, { ctrl: true, shift: false })
        .next,
    ),
    [rowB.id],
    'and ctrl on a row that is in it takes that row back out',
  )
  eq(
    s.pressSelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowA.id, { ctrl: true, shift: false })
      .deferred,
    false,
    'a ctrl press is never deferred: it is not the start of a drag (`useChangesDrag` refuses a '
      + 'modified press outright), so there is nothing to wait for',
  )

  /*
   * The range. `rowsE` is `Changes / src / b.rs / a.rs / fixes / notes.md / …` with everything
   * open, so a shift from `b.rs` to `notes.md` crosses two changelist headers and a directory.
   */
  const shiftTo = (from, to) =>
    idsOf(s.pressSelect(rowsE, sel([from], from), to, { ctrl: false, shift: true }).next)
  eq(
    shiftTo(rowB.id, rowA.id),
    [rowA.id, rowB.id].sort(),
    'shift takes the inclusive band between the anchor and the press',
  )
  eq(
    shiftTo(rowSrc.id, rowA.id).includes(rowSrc.id),
    true,
    'a directory row is a member of a range like any other selectable row — it is a drag source '
      + 'and it names a set of files, which is what every gesture downstream wants',
  )
  eq(
    shiftTo(rowB.id, rowNotes.id).some((id) =>
      rowsE.some((r) => r.id === id && (r.kind === 'group' || r.kind === 'repo')),
    ),
    false,
    'and a range that crosses a changelist header does NOT pull the header in: a group row is '
      + 'not a drag source, and carrying it would sweep in its whole list — the exact surprise '
      + 'the tick-widening used to spring',
  )
  eq(
    s.pressSelect(rowsE, sel([rowB.id], rowB.id), rowA.id, { ctrl: false, shift: true }).next
      .anchor,
    rowB.id,
    'shift leaves the anchor where it was, so a second shift-click re-extends from the same '
      + 'end rather than from the last one',
  )
  eq(
    idsOf(s.pressSelect(rowsE, s.NO_ROWS, rowA.id, { ctrl: false, shift: true }).next),
    [rowA.id],
    'shift with no anchor yet behaves as a plain press rather than selecting nothing',
  )
  eq(
    idsOf(s.pressSelect(rowsE, sel([rowA.id], 'gone'), rowB.id, { ctrl: false, shift: true }).next),
    [rowB.id],
    'and so does shift from an anchor whose row is no longer in the tree — a background refresh '
      + 'can fold a group away mid-gesture, and extending from a row that is off screen would '
      + 'select a band whose top the user cannot see',
  )

  const groupRow = (label) => rowsE.find((r) => r.kind === 'group' && r.label === label)
  eq(
    idsOf(s.pressSelect(rowsE, sel([rowA.id], rowA.id), groupRow('Changes').id, { ctrl: false, shift: false }).next),
    [],
    'a plain press on a changelist header clears the selection — that row folds, it is not a '
      + 'drag source, and it is the only way left to clear a selection with the pointer',
  )
  eq(
    s.pressSelect(rowsE, sel([rowA.id], rowA.id), groupRow('Changes').id, { ctrl: false, shift: false })
      .next.anchor,
    groupRow('Changes').id,
    'and the anchor still moves there: an anchor is a position, not a member, which is what '
      + 'lets shift+↓ walk out of a header into the files below it',
  )
  eq(
    idsOf(s.pressSelect(rowsE, sel([rowA.id], rowA.id), groupRow('Changes').id, { ctrl: true, shift: false }).next),
    [rowA.id],
    'and a ctrl press on one adds nothing rather than putting an unselectable id in the set',
  )

  eq(
    s.isSelectable(rowSrc) && s.isSelectable(rowA) && !s.isSelectable(groupRow('Changes')),
    true,
    'files and directories are selectable; repositories and changelists are positions',
  )

  // --- the keyboard reaches the same rules --------------------------------------------------

  /*
   * Anchored on the DIRECTORY row, two rows above `a.rs`, so the band has a row in the middle
   * of it. Anchored on `b.rs` — its immediate neighbour — a "union the endpoints" mistake and a
   * real range produce the same two ids, and this assertion would hold for both.
   */
  eq(
    idsOf(s.keySelect(rowsE, sel([rowSrc.id], rowSrc.id), rowA.id, { ctrl: false, shift: true })),
    idsOf(
      s.pressSelect(rowsE, sel([rowSrc.id], rowSrc.id), rowA.id, { ctrl: false, shift: true }).next,
    ),
    'shift+arrow and shift+click produce the SAME set from the same anchor, the rows in the '
      + 'middle included. Two implementations of "extend the selection" is how a tree ends up '
      + 'with a keyboard that disagrees with its mouse about what is selected',
  )
  eq(
    idsOf(s.keySelect(rowsE, sel([rowSrc.id], rowSrc.id), rowA.id, { ctrl: false, shift: true })),
    [rowSrc.id, rowB.id, rowA.id].sort(),
    'and that set really is the band and not the two ends of it',
  )
  eq(
    idsOf(s.keySelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowNotes.id, { ctrl: true, shift: false })),
    [rowA.id, rowB.id].sort(),
    'ctrl+arrow moves the cursor and leaves the selection completely alone — how a keyboard '
      + 'user reaches a row to ctrl-Space without losing what they have',
  )
  eq(
    idsOf(s.keySelect(rowsE, sel([rowA.id, rowB.id], rowA.id), rowNotes.id, { ctrl: false, shift: false })),
    [rowNotes.id],
    'and a bare arrow takes the selection along, which is what every tree does',
  )
  eq(
    idsOf(s.selectAll(rowsE)),
    rowsE.flatMap((r) => (s.isSelectable(r) ? [r.id] : [])).sort(),
    'Ctrl+A takes every selectable row that is on screen, and no header',
  )
  eq(
    idsOf(s.collapseTo(rowA.id)),
    [rowA.id],
    'Escape collapses to the row the cursor is on',
  )
  eq(idsOf(s.collapseTo(null)), [], 'and clears outright when there is no cursor')

  // --- pruning, and what survives a refresh -------------------------------------------------

  eq(
    idsOf(s.pruneRowSelection(new Set([rowA.id]), sel([rowA.id, rowB.id], rowA.id))),
    [rowA.id],
    'a selected row whose file has been committed away drops out on the next refresh',
  )
  eq(
    s.pruneRowSelection(new Set([rowA.id]), sel([rowA.id, rowB.id], rowB.id)).anchor,
    rowB.id,
    'and the anchor is kept even when its own row went: it is a position, a stale one just '
      + 'makes the next shift-range fall back to the plain rule, and clearing it would lose the '
      + "range's origin every time an agent touched a file elsewhere in the tree",
  )
  eq(
    s.pruneRowSelection(new Set([rowA.id, rowB.id]), sel([rowA.id, rowB.id], rowA.id)).ids.size,
    2,
    'a prune that drops nothing returns the same object, so a refresh that changed nothing does '
      + 'not re-render the tree',
  )
  eq(
    m.allGroups(withEmptyView).has(rowSrc.id),
    true,
    '`allGroups` already enumerates directory rows, which is what lets a selected FOLDER be '
      + 'pruned against the same set as a selected file with no second walk of the view',
  )

  // --- the selection, resolved for a drag ---------------------------------------------------

  eq(
    [...s.carriedIds(rowsE, sel([rowSrc.id], rowSrc.id))].sort(),
    [rowSrc.id, rowB.id].sort(),
    'a selected directory resolves to the files under it, and keeps its own id: `grab` asks '
      + 'both "is the row I started on selected" and "is this file one of ours" of the answer',
  )
  eq(
    [...s.carriedIds(rowsE, sel([rowA.id], rowA.id))],
    [rowA.id],
    'a selected file passes straight through',
  )

  // --- what a grab carries ---------------------------------------------------------------

  /*
   * `grab` is the one answer to "which files is this gesture about", read by the context menu
   * *and* by the drag. Two functions answering that separately is how a menu that moves four
   * files ends up beside a drag that moves one.
   *
   * It widens by the **row selection** now. It used to widen by the ticks, and the assertions
   * below used to pin that; they are rewritten rather than relaxed, and the case that changed
   * meaning — a directory row — is pinned in both directions.
   */
  const paths = (set) => set?.files.map((f) => f.path) ?? null
  const carry = (ids, row) => d.grab(withEmptyView, s.carriedIds(rowsE, sel(ids)), row)
  eq(
    paths(carry([], rowA)),
    ['a.rs'],
    'a gesture on an unselected row carries that row alone',
  )
  eq(
    paths(carry([rowA.id, rowB.id], rowA)),
    ['a.rs', 'src/b.rs'],
    "and the whole selection when the row is part of it — IDEA's multi-file move, and the "
      + 'reason the count goes in the menu label and on the drag ghost',
  )
  eq(
    carry([rowA.id, rowB.id], rowA).widened,
    true,
    'a widened grab says so, because that is the fact the user has to see before they let go',
  )
  eq(
    carry([rowA.id, rowB.id], rowA).label,
    '2 files',
    'and the ghost says it in words',
  )
  eq(
    paths(d.grab(withEmptyView, new Set([rowA.id, rowB.id]), rowA)),
    ['a.rs', 'src/b.rs'],
    'the widening reads the row SELECTION, and the ticks are a different set entirely. This '
      + 'call passes ids that happen to look alike; the two are told apart by which set the '
      + 'panel threads in — `git.carried`, never `git.selected`',
  )
  eq(
    paths(carry([rowA.id, rowNotes.id], rowA)),
    ['a.rs'],
    'an unversioned row is never swept into a TRACKED drag even when it is selected: the '
      + 'widening filters on the group kind, so one grab carries one kind. That is what lets '
      + '`dropOutcome` answer with a single verb — filing a tracked change is a re-filing, '
      + 'filing an unversioned one adds it to git, and a ghost cannot honestly describe both '
      + 'at once over a set whose composition the user cannot see',
  )
  eq(
    paths(carry([rowNotes.id], rowNotes)),
    ['notes.md'],
    'and the widening never returns an empty set, whatever is selected',
  )
  eq(
    paths(carry([], rowSrc)),
    ['src/b.rs'],
    'a directory row carries every file under it — the whole point of the row',
  )
  eq(
    paths(carry([rowA.id, rowB.id], rowSrc)),
    ['src/b.rs'],
    'a directory that is NOT in the selection still carries its own files alone, and changes '
      + 'neither the selection nor the ticks',
  )
  eq(
    paths(carry([rowSrc.id, rowA.id], rowSrc)),
    ['a.rs', 'src/b.rs'],
    'but a directory that IS in the selection carries the rest of it. This is the one rule '
      + 'that reversed: under the ticks a directory never widened, because a tick is set by a '
      + 'checkbox somewhere else and may name half the tree. A selection is rows the user just '
      + 'clicked, drawn with the band — refusing to carry them would make a folder the one row '
      + 'kind that silently drops the rest of a multi-row drag',
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
    d.dropOutcome(d.grab(withEmptyView, new Set(), rowIgnored), group('fixes')).reason,
    'Only tracked changes belong to a changelist',
    'a drag OUT of the ignored list keeps the old refusal, word for word. `target/` dropped '
      + 'on `Changes` would be a very surprising way to un-ignore something, and unlike an '
      + 'unversioned file nobody asked for it — only the unversioned list gained a verb',
  )
  /*
   * The reported bug, and the shape of its fix.
   *
   * > *"i should be able to move files from Unversioned Files to any of change list via drag
   * > and drop and via context, currently it doesn't allow"*
   *
   * An unversioned drag used to hit the same refusal as an ignored one — a true sentence about
   * the state of the world and no answer at all to what was asked, since changing that state
   * is the request. It is a second outcome kind rather than a `move` with a flag precisely so
   * that every surface drawing a verdict has to say what it does: the ghost, the target ring
   * and the menu label all switch on `kind`.
   */
  const unversioned = d.grab(withEmptyView, new Set(), rowNotes)
  eq(
    d.dropOutcome(unversioned, group('fixes')),
    {
      kind: 'track',
      repo: APP,
      changelist: 'fixes',
      list: 'fixes',
      paths: ['notes.md'],
      hint: 'Add 1 file to git and move to “fixes”',
    },
    'an unversioned file dropped on a changelist is TRACKED: added to git, then filed. The '
      + 'hint says the `git add` out loud, because that is a change to the repository and not '
      + 'a re-filing — silent staging is the thing this outcome exists to prevent',
  )
  eq(
    d.dropOutcome(unversioned, group('Changes')),
    {
      kind: 'track',
      repo: APP,
      changelist: 'default',
      list: 'Changes',
      paths: ['notes.md'],
      hint: 'Add 1 file to git and move to “Changes”',
    },
    'and dropping it on the ACTIVE changelist is a track too, not `Already in “Changes”`. '
      + '`ChangeEntry.changelist` comes from `Sidecar::owner_of`, which answers the active '
      + "list for every unfiled path — untracked ones included — so the `move` branch's "
      + 'same-list filter would compute an empty set and report the file as already being in '
      + 'a list it is not in and in a git that has never seen it',
  )
  eq(
    d.dropOutcome(unversioned, group('Ignored Files')).kind,
    'refuse',
    'the sibling lists still refuse it: `Ignored Files` is not a changelist, so there is '
      + 'nothing to file into and nothing to add to git for',
  )
  ok(
    d.trackHint(3, 'fixes') === 'Add 3 files to git and move to “fixes”'
      && d.trackMenuLabel(3) === 'Add 3 files to Git and Move to Changelist…',
    'and the claim is written once, for both routes: the ghost and the context-menu item read '
      + 'the same two helpers, so a drag cannot promise a `git add` that the menu calls a move',
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
  // is the whole band rather than one 24px header that may have scrolled off the top.
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
    /const carried = grab\(git\.view, git\.carried, row\)/.test(host)
      && !/grab\(git\.view, git\.selected/.test(host),
    'the file-row menu takes its files from `grab`, and from the row SELECTION rather than the '
      + 'ticks — the same function and the same set a drag from that row uses, so the two '
      + 'routes cannot come to disagree about what the user is pointing at. `git.selected` '
      + 'here would put the menu back on the ticks while the drag had moved on, which is worse '
      + 'than either of them being wrong on its own',
  )
  ok(
    /function dirMenu\([\s\S]{0,1600}?git\.moveToChangelist\(row\.repo, paths, untracked\)/.test(host),
    'a directory row can be moved from the keyboard, not only dragged — and it carries the '
      + '`untracked` fact to the chooser, so a folder of unversioned files opens the dialog '
      + 'that adds them to git rather than the one that only re-files',
  )
  ok(
    /label: `Move \$\{plural\(count\)\} to Changelist…`,\s*\.\.\.\(empty/.test(host),
    'and so can a whole changelist — the one row a drag deliberately cannot start from',
  )
  /*
   * The directory row's *destructive* verb, which the row model made much bigger than it was.
   *
   * `stage::rollback` restores a tracked path from HEAD and **deletes** an untracked one from
   * disk (`crates/cide-git/src/stage.rs`), and a directory row names every file under it at
   * once. So the label has to split on the kind exactly as the group row's does — `Roll Back
   * 214 files` over a directory of unversioned work promises a restore and performs a delete —
   * and the ignored list is refused outright, because the group row's `Revert Group` is
   * disabled there and a directory row inside the same list must not be the way around it.
   */
  ok(
    /function dirMenu\([\s\S]{0,3400}?label: untracked \? `Delete \$\{plural\(count\)\}` : `Roll Back \$\{plural\(count\)\}`/
      .test(host),
    'a directory of UNVERSIONED files says Delete, not Roll Back: git has nothing to restore '
      + 'them from, so the command removes them from disk, and the dialog must not promise a '
      + 'restore before running a delete',
  )
  ok(
    /function dirMenu\([\s\S]{0,3700}?\.\.\.\(ignored\s*\?\s*\{ disabledReason:/.test(host),
    'and a directory inside IGNORED FILES refuses to roll back at all — `target/` and '
      + '`node_modules/` are not part of any change, and the group row above it has refused '
      + 'the same verb since it shipped',
  )
  ok(
    /git\.revertFiles\(row\.repo, paths, untracked\)/.test(host)
      && /\(repo: RepoId, paths: string\[\], untracked = false\)/.test(
        readFileSync(join(UI, 'src/sidebar/GitPanel/useGitPanel.ts'), 'utf8'),
      ),
    'and the fact travels to the confirmation rather than stopping at the label: one dialog, '
      + 'worded by what the command will actually do',
  )
  const tree2 = readFileSync(join(UI, 'src/sidebar/GitPanel/ChangesTree.tsx'), 'utf8')
  ok(
    /onPointerDown=\{\(e\) => drag\.onPointerDown\(e, row\)\}/.test(tree2)
      && /onMove: onMovePaths,\s*onTrack: onTrackPaths,/.test(tree2),
    'every row is a drag source candidate and the tree is wired to the pointer state machine — '
      + 'the rules being right is worth nothing if no gesture reaches them. BOTH verbs are '
      + 'wired: a tree given only the move would drag tracked changes and drop unversioned '
      + 'ones into nothing at all',
  )
  /*
   * And it does not start when there is nothing to land on. Without this the drag ran in full
   * against a tree with no `onMovePaths` — rows dimmed, the ghost read `Move 4 files to
   * “fixes”`, the target ringed in the accent — and the drop did nothing at all, because
   * `finish` ends at an optional call. Both doc comments already promised the opposite.
   */
  const gesture = readFileSync(join(UI, 'src/sidebar/GitPanel/useChangesDrag.ts'), 'utf8')
  ok(
    /if \(handlerFor\(live\.current, load\) === undefined\) return/.test(gesture)
      && /load\.kind === 'unversioned' \? handlers\.onTrack : handlers\.onMove/.test(gesture),
    'a tree with no handler for what this row would do is inert rather than decorative: a '
      + 'press never becomes a drag, instead of a full gesture whose drop silently does '
      + 'nothing. Per KIND, because there are two verbs — requiring both would make a tree '
      + 'that wired up only the move inert for every row',
  )
  ok(
    /onMouseUp=\{\(e\) => \{\s*if \(e\.button !== 0\) return\s*if \(drag\.dragged\(\)\) \{/.test(tree2),
    'and an expandable row folds on RELEASE, skipped when the press became a drag: a directory '
      + 'row is both a twisty and a drag handle, and folding on the press would take the rows '
      + 'being dragged off the screen at the moment the drag starts',
  )
  /*
   * The deferred collapse. `rowSelection.ts` decides *that* a plain press on an already-selected
   * row waits; only the component can carry the answer from the press to the release, and if it
   * drops it on the floor the rule is inert and "drag the four files I selected" is unreachable
   * — the press would collapse the selection to one row before the pointer had moved.
   */
  ok(
    /if \(action\.select && onPress\(row\.id, mods\)\) deferred\.current = row\.id/.test(tree2)
      && /if \(deferred\.current === row\.id\) \{\s*deferred\.current = null\s*onRelease\(row\.id\)/
        .test(tree2),
    'a press that `rowSelection` deferred is remembered and answered on the mouseup, so a drag '
      + 'that starts on one of several selected rows still has the whole selection to carry',
  )
  /*
   * And the press refuses the native text selection at the engine as well as in CSS. This is
   * the half that no stylesheet can do: a *shift*-click extends a selection that began
   * somewhere else on the page — the commit box, a diff pane — and `user-select` on this
   * subtree cannot reach it. `preventDefault` costs the automatic focus, which the roving
   * tabindex depends on, so the focus must be taken by hand in the same breath.
   */
  ok(
    /onMouseDown=\{\(e\) => \{\s*if \(e\.button !== 0\) return[\s\S]{0,1400}?e\.preventDefault\(\)\s*e\.currentTarget\.focus\(\)/
      .test(tree2),
    'a row press refuses the default action and takes the focus itself — text selection on a '
      + 'drag across rows was the report, and CSS alone cannot answer a shift-click that '
      + 'extends a selection made outside this tree',
  )
  /*
   * `aria-selected` says the row SELECTION, not the cursor.
   *
   * It said the cursor for as long as there was no selection, on a tree that has carried
   * `aria-multiselectable="true"` since it shipped — a promise to a screen reader that exactly
   * one row could ever be selected, made by markup claiming the opposite two lines up. Pinned
   * from both ends because the wrong version type-checks, renders, and looks right: a
   * multi-row selection would simply be invisible to anything that is not a pair of eyes.
   */
  ok(
    /aria-selected=\{isSelected\}/.test(tree2) && !/aria-selected=\{isCurrent\}/.test(tree2),
    'a row reports the selection to assistive tech, not the cursor — the tree claims '
      + '`aria-multiselectable` and must be able to honour it',
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
  /*
   * The unversioned drop, traced from the gesture to the two commands it runs.
   *
   * This is the half the panel has got wrong before: a rule that decides correctly, a ghost
   * that says so, and nothing on the other end. Every link is asserted separately, because
   * any one of them missing leaves a gesture that looks alive and does nothing.
   */
  ok(
    /onTrackPaths=\{git\.trackPaths\}/.test(panel),
    'the panel passes the track handler as well as the move handler — without it a drop out '
      + 'of `Unversioned Files` computes its outcome, draws its ghost, and lands on an '
      + 'optional call that is not there',
  )
  ok(
    /await gitApi\.stage\(project, repo, wholeFiles\(paths\)\)/.test(model)
      && /await gitApi\.changelist\.movePaths\(project, repo, changelist, paths\)/.test(model),
    'and `trackPaths` runs both halves in order: the `git add` FIRST, then the filing. The '
      + 'other order is a write that undoes itself — `status::repo_changes` builds `live` from '
      + 'paths that are neither Untracked nor Ignored, so an assignment recorded while the '
      + 'path is still untracked is dropped by `Sidecar::reconcile` on the next status walk',
  )
  ok(
    /added\s*\?\s*`\$\{files\(paths\.length\)\} were added to git, but the move failed/.test(model)
      && /: `nothing was added to git/.test(model),
    'and a half-applied gesture SAYS SO, naming which half ran. A failed add leaves the move '
      + 'impossible and must not be silent; a failed move after a successful add leaves the '
      + 'files in git and in the active changelist, which the user cannot be left to discover',
  )
  ok(
    /label: untracked\s*\n?\s*\? trackMenuLabel\(scope\.paths\.length\)/.test(host)
      && /\.\.\.\(row\.groupKind === 'changelist' \|\| untracked/.test(host),
    'the CONTEXT MENU takes the same route — the item is enabled on an unversioned row and '
      + 'labelled from the same helper the drag ghost reads. It was disabled there with '
      + '"Only tracked changes belong to a changelist", which is the sentence the user '
      + 'reported: a statement about the state of the world where an answer was wanted',
  )
  const dialog = readFileSync(join(UI, 'src/sidebar/GitPanel/ChangelistDialog.tsx'), 'utf8')
  ok(
    /const TRACK_TITLE = 'Add to git and move to changelist'/.test(dialog)
      && /Git is not tracking th/.test(dialog),
    'and the chooser the menu opens says it too, in its title and in a sentence, before '
      + 'anything runs. A dialog that looked identical for the two operations would be the '
      + 'silent staging this whole path exists to avoid, reached through the keyboard',
  )
  ok(
    /if \(track\) \{\s*trackPaths\(repo, id, \[\.\.\.paths\]\)/.test(model),
    'and answering it runs the same operation the drop runs, not a plain move: one '
      + 'implementation, so the two routes cannot come to do different things to the repository',
  )
  ok(
    /const here = track\s*\?\s*new Set<string>\(\)/.test(model),
    'and the chooser does not grey out the row most of these files are headed for. '
      + '`ChangeEntry.changelist` comes from `Sidecar::owner_of`, which answers the ACTIVE '
      + 'list for every unfiled path — untracked ones included — so the "already here" '
      + 'suppression, read literally, disables the active changelist on a set of files that '
      + 'is in no changelist at all. Same fact `dropOutcome`\'s track branch is built around, '
      + 'asserted here because the menu route reaches it through different code',
  )

  // --- when Commit can be pressed -----------------------------------------------------------
  //
  // One rule with one exception, and the exception is the whole reason it is a function.

  eq(m.canCommit({ picked: 2, reword: false, busy: null }), true, 'ticks commit')
  eq(
    m.canCommit({ picked: 0, reword: false, busy: null }),
    false,
    'nothing ticked and not a reword is nothing to commit — the ticks are the commit',
  )
  eq(
    m.canCommit({ picked: 0, reword: true, busy: null }),
    true,
    'nothing ticked *while amending one named repository* is a reword, which is the commonest '
      + 'amend there is. It was unreachable from the panel until the button and '
      + '`cide_git::commit`\'s `NothingToCommit` guard were widened together',
  )
  eq(
    m.canCommit({ picked: 3, reword: false, busy: 'committing' }),
    false,
    'and never while one is already in flight — the second click is the one that commits twice',
  )
  eq(
    m.canCommit({ picked: 0, reword: true, busy: 'committing' }),
    false,
    '…including on the reword path, which must not be an exception to the busy check as well',
  )

  // --- a reword mints a unit from no files ----------------------------------------------------
  //
  // The other half of the same fact, and the reason `canCommit` takes the conclusion rather than
  // the parts: `commit` returns early on an empty unit list, so a lit button over zero units is a
  // click that silently does nothing.

  eq(
    m.commitUnits(one, new Set(), null),
    [],
    'nothing ticked and no repository named yields no units — which is what makes the '
      + 'multi-root checkbox case a no-op rather than a reword of whichever root sorted first',
  )
  eq(
    m.commitUnits(one, new Set(), APP),
    [{ repo: APP, paths: [], changelist: null }],
    'a named repository with nothing ticked is one file-less unit: the reword',
  )
  eq(
    m.commitUnits(one, defaults, APP),
    m.commitUnits(one, defaults, null),
    'and an amend that *does* tick something is an ordinary amend — the file-less unit must not '
      + 'be added alongside real ones, or a monorepo amend would commit in a repo nobody '
      + 'selected anything in',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('git panel model: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
