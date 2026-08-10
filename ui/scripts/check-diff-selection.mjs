/**
 * Checks `src/sidebar/GitPanel/diffSelection.ts` — the map from what the diff pane
 * highlights to what `git_stage`, `git_unstage` and `git_commit` are sent.
 *
 * # Why this one is not optional
 *
 * Everything else in the git panel gets a number wrong when it breaks. This gets *lines*
 * wrong: `cide_git::patch::choose` turns the `Selection` into a set of positions and
 * `synthesize` builds a patch containing exactly those, which is then applied to the index.
 * A mapping that drifts by one stages a line the user did not tick and leaves one they did —
 * silently, with the panel still painting the ticks they made. There is no later step that
 * would notice.
 *
 * So the claim is checked directly and exhaustively, not sampled:
 *
 *     resolve(diff, toSelection(marks, diff)) === highlightedKeys(marks, diff)
 *
 * over **every subset** of the six change lines of a fixture with three hunks — 64 subsets,
 * each run four ways with different junk marks mixed in, 256 cases — plus the shapes that
 * make the encoding branch: hunk-aligned selections must come out as `hunks`,
 * everything-selected as `whole`, a ragged selection as `lines`.
 *
 * `resolve` is a port of `patch::choose`, so the two sides of the equation are the pane's
 * arithmetic and Rust's, written independently. What it cannot prove is that the port is
 * still faithful — that is pinned on the Rust side by `cide-git`'s own tests over
 * `choose`/`synthesize`, and the port names the function it mirrors so a change there is
 * greppable here.
 *
 * Same harness as `check-git-tree.mjs`: compile the one module with tsc, import the output
 * under node. It has no runtime imports (its wire types are type-only), so nothing has to be
 * resolved at run time.
 *
 * Run: `pnpm --dir ui run check:diff`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve as resolvePath } from 'node:path'

const UI = resolvePath(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-diffsel-'))
try {
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
        noEmitOnError: true,
        outDir: out,
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      files: [
        join(UI, 'src', 'sidebar', 'GitPanel', 'diffSelection.ts'),
        join(UI, 'src', 'sidebar', 'GitPanel', 'partialStore.ts'),
        join(UI, 'src', 'sidebar', 'GitPanel', 'repoRoots.ts'),
        join(UI, 'src', 'panes', 'diffTabs.ts'),
      ],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const s = await import(`file://${join(out, 'sidebar', 'GitPanel', 'diffSelection.js')}`)
  const store = await import(`file://${join(out, 'sidebar', 'GitPanel', 'partialStore.js')}`)
  const roots = await import(`file://${join(out, 'sidebar', 'GitPanel', 'repoRoots.js')}`)
  const tabs = await import(`file://${join(out, 'panes', 'diffTabs.js')}`)

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
  const sorted = (set) => [...set].sort()

  // --- the wire shape this module encodes ------------------------------------------------

  /*
   * `Selection` is the type being produced, so its variants are pinned the way
   * `check-git-tree.mjs` pins `ChangeEntry`'s fields: a rename in `cide-ipc::git` that
   * `cargo xtask codegen` propagates would otherwise leave this module emitting a tag Rust
   * no longer accepts, and serde would reject it at run time with the panel still green.
   */
  const generated = readFileSync(join(UI, 'src', 'ipc', 'generated.ts'), 'utf8')
  const selectionDecl = /export type Selection = [^;]+;/.exec(generated)?.[0] ?? ''
  for (const tag of ['"whole"', '"hunks"', '"lines"', 'hunks:', 'lines:']) {
    ok(selectionDecl.includes(tag), `Selection still carries ${tag}`)
  }
  const lineRefDecl = /export type LineRef = \{[\s\S]*?\};/.exec(generated)?.[0] ?? ''
  for (const field of ['hunk', 'line']) {
    ok(new RegExp(`(^|[\\s{,])${field}:`, 'm').test(lineRefDecl), `LineRef.${field} is still sent`)
  }

  // --- a fixture -------------------------------------------------------------------------

  const line = (origin, content, oldLineno, newLineno) => ({
    origin,
    content,
    oldLineno,
    newLineno,
    noNewline: false,
  })

  /*
   * Three hunks, deliberately unlike each other: one with a lone addition, one with a
   * deletion/addition pair surrounded by context, one with a run of three. Context lines sit
   * at the start, middle and end of a hunk so that "the index of a change line" and "the
   * index of a line" never coincide — an off-by-one that only shows up when they differ.
   */
  const diff = {
    path: 'src/main.rs',
    oldPath: null,
    side: 'unstaged',
    status: 'modified',
    binary: false,
    oldMode: 33188,
    newMode: 33188,
    rev: 'cafef00dcafef00d',
    partialOk: true,
    hunks: [
      {
        index: 0,
        header: '@@ -1,2 +1,3 @@ fn main() {',
        oldStart: 1,
        oldLines: 2,
        newStart: 1,
        newLines: 3,
        lines: [
          line('context', 'fn main() {', 1, 1),
          line('addition', '    let x = 1;', null, 2),
          line('context', '}', 2, 3),
        ],
      },
      {
        index: 1,
        header: '@@ -10,3 +11,3 @@',
        oldStart: 10,
        oldLines: 3,
        newStart: 11,
        newLines: 3,
        lines: [
          line('context', 'let a = 0;', 10, 11),
          line('deletion', 'let b = 1;', 11, null),
          line('addition', 'let b = 2;', null, 12),
          line('context', 'let c = 3;', 12, 13),
        ],
      },
      {
        index: 2,
        header: '@@ -20,1 +21,4 @@',
        oldStart: 20,
        oldLines: 1,
        newStart: 21,
        newLines: 4,
        lines: [
          line('addition', 'one', null, 21),
          line('addition', 'two', null, 22),
          line('addition', 'three', null, 23),
          line('context', 'end', 20, 24),
        ],
      },
    ],
  }

  const every = s.changeMarks(diff)
  eq(every, ['0:1', '1:1', '1:2', '2:0', '2:1', '2:2'], 'the selectable positions are the changes')
  eq(s.hunkMarks(diff, 1), ['1:1', '1:2'], 'a hunk contributes its changes and no context')

  // --- the claim, over every subset ------------------------------------------------------

  /*
   * 2^6 subsets of the change lines, plus the same subsets polluted with a context mark and
   * with an out-of-range one. Both pollutions must be dropped: a context row cannot be
   * staged (`choose` skips it) and a position that no longer exists is not a position.
   */
  let cases = 0
  for (let bits = 0; bits < 1 << every.length; bits++) {
    const chosen = every.filter((_, i) => (bits & (1 << i)) !== 0)
    for (const extra of [[], ['0:0'], ['9:9'], ['0:2', '5:0']]) {
      const marks = new Set([...chosen, ...extra])
      const painted = s.highlightedKeys(marks, diff)
      eq(sorted(painted), chosen, `highlighting drops context and stale marks (bits ${bits})`)

      const selection = s.toSelection(marks, diff)
      const sent = s.resolve(diff, selection)
      eq(
        sorted(sent),
        sorted(painted),
        `what is sent is what is highlighted (bits ${bits}, extra ${extra.length})`,
      )
      cases++
    }
  }
  ok(cases === 256, `every subset was checked, not a sample (${cases})`)

  // --- the encoding, shape by shape ------------------------------------------------------

  eq(s.toSelection(new Set(), diff), null, 'nothing selected sends nothing at all')
  eq(
    s.toSelection(new Set(every), diff).kind,
    'whole',
    'everything selected is `whole` — the index API, not a patch we wrote',
  )
  eq(
    s.toSelection(new Set(s.hunkMarks(diff, 2)), diff),
    { kind: 'hunks', hunks: [2] },
    'a whole hunk is `hunks`, which is what the user expressed',
  )
  eq(
    s.toSelection(new Set([...s.hunkMarks(diff, 0), ...s.hunkMarks(diff, 1)]), diff),
    { kind: 'hunks', hunks: [0, 1] },
    'two whole hunks are one `hunks` selection',
  )
  eq(
    s.toSelection(new Set(['2:0', '2:2']), diff),
    { kind: 'lines', lines: [{ hunk: 2, line: 0 }, { hunk: 2, line: 2 }] },
    'a ragged selection is `lines`, in reading order',
  )
  eq(
    s.toSelection(new Set([...s.hunkMarks(diff, 0), '2:0']), diff),
    {
      kind: 'lines',
      lines: [{ hunk: 0, line: 1 }, { hunk: 2, line: 0 }],
    },
    'a whole hunk plus one stray line is `lines` — `hunks` would over-select the rest of hunk 2',
  )

  // --- the staleness check travels with the positions ------------------------------------

  eq(
    s.pathSelection(new Set(every), diff),
    { path: 'src/main.rs', selection: { kind: 'whole' }, rev: 'cafef00dcafef00d' },
    'every row ticked still carries the rev — `whole` is only how the encoding spells "all of '
      + 'these positions", and without it `git add` would stage lines added after the tick',
  )
  eq(
    s.pathSelection(new Set(['2:0']), diff).rev,
    'cafef00dcafef00d',
    'a positional selection carries the rev of the diff it was made against',
  )
  /*
   * The panel's own checkbox is the other case and keeps `rev: null` — it means "this file",
   * not "these rows". `wholeFiles` lives in `useGitPanel`, so what is pinned here is that this
   * function is not it: no selection it builds may skip the staleness check.
   */
  for (const marks of [new Set(every), new Set(['2:0']), new Set(s.hunkMarks(diff, 1))]) {
    ok(s.pathSelection(marks, diff).rev === diff.rev, 'no selection from the pane skips the check')
  }
  eq(s.pathSelection(new Set(), diff), null, 'nothing selected produces no PathSelection')

  // --- the toggles ------------------------------------------------------------------------

  eq(
    sorted(s.toggleLine(new Set(), diff, 0, 0)),
    [],
    'a context row cannot be ticked, however it is clicked',
  )
  eq(sorted(s.toggleLine(new Set(), diff, 0, 1)), ['0:1'], 'a change row ticks')
  eq(sorted(s.toggleLine(new Set(['0:1']), diff, 0, 1)), [], 'and unticks')
  eq(sorted(s.toggleHunk(new Set(), diff, 1)), ['1:1', '1:2'], 'a hunk ticks its changes')
  eq(
    sorted(s.toggleHunk(new Set(['1:1', '1:2']), diff, 1)),
    [],
    'a full hunk clears rather than re-ticking',
  )
  eq(
    sorted(s.toggleHunk(new Set(['1:1']), diff, 1)),
    ['1:1', '1:2'],
    'a partial hunk fills, which is what the mixed box promises',
  )
  eq(s.hunkState(new Set(), diff, 1), 'none', 'tri-state: none')
  eq(s.hunkState(new Set(['1:1']), diff, 1), 'some', 'tri-state: some')
  eq(s.hunkState(new Set(['1:1', '1:2']), diff, 1), 'all', 'tri-state: all')
  eq(
    s.hunkState(new Set(['0:0']), diff, 0),
    'none',
    'a context mark never lights the hunk box — it is not a selection',
  )

  // --- a diff with nothing selectable ------------------------------------------------------

  const modeOnly = { ...diff, hunks: [], partialOk: false }
  eq(s.changeMarks(modeOnly), [], 'a file with no hunks has nothing to select')
  eq(s.toSelection(new Set(['0:0']), modeOnly), null, 'and cannot produce a selection')

  /*
   * `partialOk: false` is `cide_git`'s refusal — binary, submodule, symlink, deletion,
   * rename. Hunks may still be present (a rename carries content hunks), so the file *looks*
   * selectable; every one of those selections would come back `PartialRefused`.
   */
  const refused = { ...diff, partialOk: false }
  eq(
    sorted(s.highlightedKeys(new Set(['2:0']), refused)),
    [],
    'a file that cannot be staged in parts highlights nothing, hunks or no hunks',
  )
  eq(
    s.toSelection(new Set(['2:0', '2:1']), refused),
    null,
    'and produces no selection, so no button can send one that would be refused',
  )

  // --- what the commit is actually sent ------------------------------------------------------

  /*
   * `commitSelections` is the last function between a held selection and `git commit`, and it
   * was the only one on this path with no coverage at all. Everything it can get wrong is
   * silent: honour a selection made against the wrong side and the commit contains lines from
   * a different diff; drop the rev and a moved file is committed at stale positions; fail to
   * fall back and a file the user ticked is left out of the commit entirely.
   */
  const REPO = 'repo-a'
  const OTHER = 'repo-b'
  const entry = (over) => ({
    repo: REPO,
    path: 'src/main.rs',
    side: 'combined',
    rev: 'cafef00dcafef00d',
    selection: { kind: 'lines', lines: [{ hunk: 2, line: 0 }] },
    lines: 1,
    ...over,
  })
  const whole = (path) => ({ path, selection: { kind: 'whole' }, rev: null })

  store.clearAllPartials()
  eq(
    store.commitSelections(REPO, ['src/main.rs', 'README.md']),
    [whole('src/main.rs'), whole('README.md')],
    'with nothing held, every ticked path is the whole file — what the panel always did',
  )

  store.setPartial(entry())
  eq(
    store.commitSelections(REPO, ['src/main.rs', 'README.md']),
    [
      {
        path: 'src/main.rs',
        selection: { kind: 'lines', lines: [{ hunk: 2, line: 0 }] },
        rev: 'cafef00dcafef00d',
      },
      whole('README.md'),
    ],
    'a held selection travels with its rev, and only for its own path',
  )
  eq(
    store.commitSelections(OTHER, ['src/main.rs']),
    [whole('src/main.rs')],
    'another repository’s file of the same name is not the same file',
  )

  /*
   * The side is the rule that keeps this honest. `commit` and `shelve` re-derive against
   * `Combined`; a selection made on `unstaged` or `staged` names positions in a different
   * diff, so it must be ignored rather than sent — the fallback is coarser, never wrong.
   */
  for (const side of ['unstaged', 'staged']) {
    store.clearAllPartials()
    store.setPartial(entry({ side }))
    eq(
      store.commitSelections(REPO, ['src/main.rs']),
      [whole('src/main.rs')],
      `a selection made on \`${side}\` is not honoured by a commit, which resolves \`combined\``,
    )
  }

  store.clearAllPartials()
  store.setPartial(entry())
  ok(store.getSnapshot().length === 1, 'the snapshot follows the store')
  store.pruneTo(new Set([store.liveKey(REPO, 'other.rs')]))
  eq(
    store.commitSelections(REPO, ['src/main.rs']),
    [whole('src/main.rs')],
    'a file that left the tree takes its held positions with it',
  )
  ok(store.getSnapshot().length === 0, 'and the snapshot with them')

  store.setPartial(entry())
  store.setPartial(entry({ path: 'README.md' }))
  ok(store.getSnapshot().length === 2, 'two files can be held at once')
  store.clearPartial(REPO, 'src/main.rs')
  eq(
    store.getSnapshot().map((e) => e.path),
    ['README.md'],
    'clearing one leaves the other',
  )
  store.clearAllPartials()
  ok(store.getSnapshot().length === 0, '“Use whole files” drops every one of them')

  /*
   * The snapshot must be reference-stable between changes: `useSyncExternalStore` compares
   * with `Object.is` on every render, so a fresh array per call is an infinite render loop and
   * not a small waste. Cheap to state, and impossible to notice by reading.
   */
  store.setPartial(entry())
  ok(store.getSnapshot() === store.getSnapshot(), 'the snapshot is stable between changes')
  ok(store.getServerSnapshot() === store.getSnapshot(), 'and SSR sees the same identity')
  store.clearAllPartials()

  /* --- which tool calls a diff tab is allowed to care about -------------------------------
   *
   * `cide://session-tool` reaches every window and names a session, not a project, so an
   * unfiltered diff tab refetched on every tool call any Claude in the app made. `TabContent`
   * keeps every tab mounted, so that was N `git_diff_file` calls per tool call, nearly all of
   * them for tabs nobody was looking at. The filter below is the fix, and the case that makes
   * it worth pinning is the *second* project: two checkouts with the same `src/main.rs`.
   */
  const ROOT = '/home/dev/work/cide'
  const ELSEWHERE = '/home/dev/work/other'
  const touches = (paths, root, path) => roots.touchesFile(paths, root, path)

  ok(
    touches([`${ROOT}/src/main.rs`], ROOT, 'src/main.rs'),
    'a tool call on this tab’s own file is this tab’s business',
  )
  eq(
    touches([`${ELSEWHERE}/src/main.rs`], ROOT, 'src/main.rs'),
    false,
    'the same relative path in another project is a different file and must not refetch',
  )
  eq(
    touches([`${ROOT}/src/other.rs`, `${ROOT}/README.md`], ROOT, 'src/main.rs'),
    false,
    'a tool call on a neighbouring file in the same repo does not move this diff',
  )
  ok(
    touches([`${ROOT}/README.md`, `${ROOT}/src/main.rs`], ROOT, 'src/main.rs'),
    'one edit reports several paths and any of them may be the one',
  )
  eq(touches([], ROOT, 'src/main.rs'), false, 'no paths, no fetch')
  ok(
    touches([`${ROOT}/src/main.rs`], `${ROOT}/`, 'src/main.rs'),
    'a trailing separator on the root does not produce a double slash that matches nothing',
  )
  eq(
    touches([`${ROOT}/src/main.rs.bak`], ROOT, 'src/main.rs'),
    false,
    'a path this one is a prefix of is not this one',
  )

  // The fallback, for a window that has not seen a `ChangesTree` yet — the sidebar on Files,
  // no mutation yet, so no root is known. Loose on purpose: the cost of a false positive is
  // one wasted fetch, the cost of a false negative is a tab that stops following its file.
  ok(
    touches([`${ROOT}/src/main.rs`], null, 'src/main.rs'),
    'without a root the tail still matches, so the tab keeps following its file',
  )
  ok(
    touches([`${ELSEWHERE}/src/main.rs`], null, 'src/main.rs'),
    'and matches another project too — the admitted cost of not knowing the root',
  )
  eq(
    touches([`${ROOT}/src/other.rs`], null, 'src/main.rs'),
    false,
    'the fallback is still a path match, not “refetch on everything”',
  )
  eq(
    touches([`${ROOT}/vendor/src/main.rs`], ROOT, 'src/main.rs'),
    false,
    'with a root known, a deeper file with the same tail is rejected',
  )

  // The registry that supplies the root. `RepoChanges` is flat — submodules arrive as their
  // own entry — so this is a walk, not a recursion.
  eq(roots.repoRoot('r1'), null, 'a repo no tree has named yet has no root')
  roots.noteRepoRoots({
    repos: [
      { repo: { id: 'r1', root: ROOT } },
      { repo: { id: 'r2', root: `${ROOT}/vendor/sub` } },
    ],
  })
  eq(roots.repoRoot('r1'), ROOT, 'a tree teaches the root of every repo in it')
  eq(roots.repoRoot('r2'), `${ROOT}/vendor/sub`, 'including submodules, which arrive flat')
  ok(
    touches([`${ROOT}/vendor/sub/src/main.rs`], roots.repoRoot('r2'), 'src/main.rs'),
    'a submodule’s src/main.rs is its own, not the parent repo’s',
  )
  eq(
    touches([`${ROOT}/vendor/sub/src/main.rs`], roots.repoRoot('r1'), 'src/main.rs'),
    false,
    'and the parent repo does not claim it',
  )

  /* --- and when it is allowed to act on them ----------------------------------------------
   *
   * The filter above says *which* events are this tab's; this says *when* it spends one. Both
   * halves have to work off something the pane can get for itself: the shell renders
   * `<GitDiffPane project spec />` with no visibility flag, so a deferral that only reads a
   * prop is a deferral that never runs, and a hidden diff tab goes on refetching exactly as it
   * did before the change. `diffTabOnScreen` is that fact, derived from the workspace mirror.
   */
  const spec = (repo, path, side = 'unstaged') => ({
    kind: 'diff',
    spec: {
      title: `${path} — diff`,
      oldPath: path,
      newPath: path,
      origin: { kind: 'git', repo, path, side },
    },
  })
  const project = {
    activeTab: 't-main',
    tabs: [
      { id: 't-home', kind: { kind: 'claudeHome' } },
      { id: 't-main', kind: spec('r1', 'src/main.rs') },
      { id: 't-lib', kind: spec('r1', 'src/lib.rs') },
      { id: 't-sub', kind: spec('r2', 'src/main.rs') },
      { id: 't-file', kind: { kind: 'file', path: 'README.md', dirty: false } },
    ],
  }

  ok(tabs.diffTabOnScreen(project, 'r1', 'src/main.rs'), 'the active diff tab is on screen')
  eq(
    tabs.diffTabOnScreen(project, 'r1', 'src/lib.rs'),
    false,
    'a diff tab behind the active one is not, which is the whole point of deferring',
  )
  eq(
    tabs.diffTabOnScreen(project, 'r2', 'src/main.rs'),
    false,
    'and the same relative path in a submodule is its own tab, not this one',
  )
  ok(
    tabs.diffTabOnScreen(project, 'r1', 'src/gone.rs'),
    'a file with no tab of its own answers “on screen”: not knowing must never defer forever',
  )
  ok(
    tabs.diffTabOnScreen(undefined, 'r1', 'src/main.rs'),
    'nor must a window that has not received a snapshot yet',
  )
  eq(
    tabs.showsGitDiff({ kind: 'file', path: 'src/main.rs', dirty: false }, 'r1', 'src/main.rs'),
    false,
    'an *editor* on the same file is a different tab — the pair alone does not identify one',
  )
  // A tab opened on `unstaged` and switched to `staged` is still the tab for this file — the
  // pane switches sides in place and `shows_git_diff` leaves the side out for that reason. A
  // match that included it would report the tab in front as hidden and defer its fetches
  // forever, which is the failure mode worth an assertion.
  ok(
    tabs.diffTabOnScreen(
      { activeTab: 't-main', tabs: [{ id: 't-main', kind: spec('r1', 'src/main.rs', 'staged') }] },
      'r1',
      'src/main.rs',
    ),
    'the side a tab was opened on is not part of the match',
  )
  ok(
    tabs.diffTabOnScreen(
      {
        activeTab: 't-second',
        tabs: [
          { id: 't-first', kind: spec('r1', 'src/main.rs') },
          { id: 't-second', kind: spec('r1', 'src/main.rs') },
        ],
      },
      'r1',
      'src/main.rs',
    ),
    'two tabs over one file cannot be opened, but if a workspace.json held them the one in ' +
      'front still answers — a `find` on the first would defer the visible tab forever',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`diff selection: ok (${cases} subset cases)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
