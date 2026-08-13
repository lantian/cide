/**
 * Checks `src/chrome/branchModel.ts` — every decision the branch selector makes.
 *
 * Same shape as `check-menu-model.mjs` and `check-git-tree.mjs`, and for the same reason:
 * this project has no JS test runner, and adding one for a dozen pure functions would be a
 * larger commitment than the code it tests. So the decisions live in a DOM-free module and
 * this script compiles that one file and drives it.
 *
 * # What is worth pinning here
 *
 * The popup is a list and some buttons. What can go silently wrong is *which rows appear*,
 * *which actions a row offers*, and *what a failure says* — and the third one is the reason
 * this feature exists at all. A `GitError` is a tagged object on the wire, so a UI that prints
 * `String(error)` shows `[object Object]`: the control looks like it did nothing, which is
 * precisely the complaint this round is answering. `explain` is therefore asserted variant by
 * variant, including the fallback for a tag this file has never heard of.
 *
 * The wire field names are read out of `src/ipc/generated.ts` first, so a rename in
 * `crates/cide-ipc/src/git.rs` that `cargo xtask codegen` propagates fails here rather than
 * quietly emptying the popup — the failure mode that kept the git panel blank for a milestone.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that the popup opens, or that clicking a row checks anything out. That is JSX and a
 *     Tauri round trip; `crates/cide-git/tests/branches.rs` owns the behaviour at the other
 *     end, against real repositories.
 *   - that the refusal is *correct*. Which files block a checkout is git's rule and it is
 *     tested in Rust; this only checks that the list survives the trip to a sentence.
 *
 * Run: `pnpm --dir ui run check:branches`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-branches-'))

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

try {
  // --- the wire shape, pinned against the generated types ---------------------------------

  const generated = readFileSync(join(UI, 'src', 'ipc', 'generated.ts'), 'utf8')
  const shape = (name) => {
    const at = generated.indexOf(`export type ${name} = `)
    if (at < 0) return ''
    const end = generated.indexOf('};', at)
    return generated.slice(at, end < 0 ? generated.length : end)
  }
  for (const [type, fields] of [
    ['BranchList', ['repo', 'head', 'local', 'remote']],
    ['BranchRef', ['name', 'remote', 'current', 'upstream', 'ahead', 'behind', 'tip', 'subject']],
    ['BranchInfo', ['head', 'detached', 'upstream', 'ahead', 'behind', 'operation', 'unborn']],
    ['CheckoutOutcome', ['branch', 'createdFromRemote', 'stashed', 'restoreFailed']],
    // The second half of `FetchOutcome` is the pull's own report, and it is pinned field by
    // field for the reason the header gives: `branchModel.ts` declares the shape structurally
    // (it may not import the generated types and stay standalone-compilable), so a rename in
    // `crates/cide-ipc/src/git.rs` would typecheck on both sides and produce `undefined` in a
    // toast. This is the join that catches it.
    [
      'FetchOutcome',
      [
        'remote',
        'shelledOut',
        'output',
        'advanced',
        'branch',
        'oldOid',
        'newOid',
        'filesChanged',
        'insertions',
        'deletions',
        'commits',
        'moreCommits',
      ],
    ],
    ['PulledCommit', ['shortOid', 'summary', 'author']],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
    }
  }
  // The refusal's tag and its payload, spelled exactly as `refusalOf` looks for them.
  ok(
    /"kind": "checkoutWouldOverwrite", "detail": \{ branch: string, paths: Array<string>, \}/.test(
      generated,
    ),
    'GitError still carries checkoutWouldOverwrite {branch, paths} — the refusal the popup unpacks',
  )

  // --- compile the model on its own -------------------------------------------------------

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
        noEmitOnError: true,
        outDir: out,
        // `src`, not `src/chrome`: the model reaches `@/ipc/generated` through the alias, and
        // tsc requires every source file to sit under `rootDir`.
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      files: [join(UI, 'src', 'chrome', 'branchModel.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'chrome', 'branchModel.js')}`)

  // --- fixtures ---------------------------------------------------------------------------

  const head = (over = {}) => ({
    head: 'main',
    detached: false,
    upstream: 'origin/main',
    ahead: 0,
    behind: 0,
    operation: null,
    unborn: false,
    ...over,
  })

  const ref = (name, over = {}) => ({
    name,
    remote: false,
    current: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: 'a1b2c3d4',
    subject: 'a commit',
    committed: 1700000000,
    ...over,
  })

  const list = {
    repo: { id: 'repo-1', root: '/w', name: 'w', parent: null, isSubmodule: false },
    head: head(),
    // Rust hands these back in recency order; `main` is older than `feature` here, which is
    // what makes "current first" a visible re-ordering rather than a coincidence.
    local: [ref('feature/login'), ref('maintenance'), ref('main', { current: true })],
    remote: [ref('origin/main', { remote: true }), ref('origin/feature/login', { remote: true })],
  }

  // --- the status bar's label --------------------------------------------------------------

  eq(m.headLabel(null), m.NO_REPO, 'no repository says so rather than showing an empty slot')
  eq(m.headLabel(head()), 'main', 'a branch level with its upstream is just the name')
  eq(m.headLabel(head({ ahead: 2 })), 'main ↑2', 'ahead only')
  eq(m.headLabel(head({ ahead: 2, behind: 1 })), 'main ↑2 ↓1', 'both counts')
  eq(m.headLabel(head({ behind: 3 })), 'main ↓3', 'behind only')
  eq(
    m.headLabel(head({ detached: true, head: 'a1b2c3d4' })),
    'a1b2c3d4 detached',
    'a detached HEAD says so — the counts would be meaningless',
  )

  ok(m.headTitle(null, 0).includes('No git repository'), 'the tooltip explains an empty slot')
  ok(
    m.headTitle(head({ upstream: null }), 1).includes('no upstream'),
    'the tooltip says when there is nothing to push or pull against',
  )
  ok(
    m.headTitle(head({ unborn: true }), 1).includes('no commits yet'),
    'an unborn branch is called out — half the popup needs a commit to point at',
  )
  ok(
    m.headTitle(head({ operation: 'rebase' }), 1).includes('rebase in progress'),
    'a half-finished operation reaches the tooltip',
  )
  ok(
    m.headTitle(head(), 3).includes('3 repositories'),
    'a superproject says how many repositories the one slot is speaking for',
  )

  // --- which rows appear ---------------------------------------------------------------------

  eq(
    m.visibleBranches(list, '').map((b) => b.name),
    ['main', 'feature/login', 'maintenance', 'origin/main', 'origin/feature/login'],
    'the current branch is pinned first; everything else keeps the order Rust sent (recency), ' +
      'locals before remotes',
  )
  eq(m.visibleBranches(null, ''), [], 'no repository, no rows')
  eq(
    m.visibleBranches(list, 'LOGIN').map((b) => b.name),
    ['feature/login', 'origin/feature/login'],
    'the filter is case-insensitive and drops the current branch when it does not match',
  )
  eq(
    m.visibleBranches(list, '   ').map((b) => b.name).length,
    5,
    'a whitespace-only query is not a filter',
  )
  eq(
    m.visibleBranches(list, 'mn').map((b) => b.name),
    [],
    'substring, NOT fuzzy: `mn` must not find `main`, `maintenance` and `origin/main` at once',
  )

  eq(m.matches(ref('main'), 'MAI'), true, 'matches is case-insensitive')
  eq(m.matches(ref('main'), ''), true, 'an empty query keeps everything')
  eq(m.matches(ref('main'), 'x'), false, 'a miss is a miss')

  eq(m.remoteOf(ref('origin/feature/login', { remote: true })), 'origin', 'the remote is the first segment')
  eq(m.remoteOf(ref('main')), null, 'a local branch has no remote')

  // --- what a row may do ---------------------------------------------------------------------

  eq(
    m.actionsFor(ref('feature/login')),
    ['checkout', 'newFrom', 'rename', 'delete'],
    'an ordinary local branch offers all four',
  )
  eq(
    m.actionsFor(ref('main', { current: true })),
    ['newFrom', 'rename'],
    'the branch you are on cannot be checked out again or deleted — git refuses the second ' +
      'and the first would do nothing',
  )
  eq(
    m.actionsFor(ref('origin/main', { remote: true })),
    ['checkout', 'newFrom'],
    'a remote-tracking ref cannot be renamed or deleted: it is a cache of the remote’s names ' +
      'and the next fetch would put it back',
  )

  eq(m.canPull(head()), true, 'pull is offered with an upstream')
  eq(m.canPull(head({ upstream: null })), false, 'pull with no upstream would only ever fail')
  eq(m.canPull(head({ detached: true })), false, 'a detached HEAD has nothing to fast-forward')
  eq(m.canPull(head({ unborn: true })), false, 'an unborn branch has no commit to move')
  eq(m.canPull(null), false, 'no repository, no pull')
  eq(m.canPush(head({ upstream: null })), true, 'push IS offered with no upstream — that is how one is set')
  eq(m.canPush(head({ unborn: true })), false, 'nothing to push before the first commit')

  // --- refusals ------------------------------------------------------------------------------

  const refusal = {
    kind: 'checkoutWouldOverwrite',
    detail: { branch: 'feature', paths: ['src/a.rs', 'README.md'] },
  }
  eq(
    m.refusalOf(refusal),
    { branch: 'feature', paths: ['src/a.rs', 'README.md'] },
    'the refusal is unpacked — `paths` is the whole feature',
  )
  eq(m.refusalOf({ kind: 'noUpstream', detail: { branch: 'x' } }), null, 'other errors are not refusals')
  eq(m.refusalOf('boom'), null, 'a non-wire error is not a refusal')
  eq(m.refusalOf(null), null, 'null is not a refusal')

  eq(m.kindOf(refusal), 'checkoutWouldOverwrite', 'kindOf reads the tag')
  // The delete panel branches on this inside a `catch`, so it has to survive every shape a
  // rejection can take. A `(error as {kind?: string}).kind` would throw on the first two.
  for (const odd of [null, undefined, 'boom', 42, {}, { kind: 7 }]) {
    eq(m.kindOf(odd), null, `kindOf survives ${JSON.stringify(odd)}`)
  }
  eq(
    m.refusalOf({ kind: 'checkoutWouldOverwrite', detail: { branch: 'f', paths: ['a', 7] } }),
    { branch: 'f', paths: ['a'] },
    'a malformed payload is filtered rather than rendered as `7`',
  )
  /*
   * A refusal with no path list at all is not a refusal. Returning `{branch, paths: []}` would
   * put the popup into its stash panel over an empty `<ul>` — a question about no files, whose
   * two answers both stash the working tree. `null` sends it down `explain` instead.
   */
  eq(
    m.refusalOf({ kind: 'checkoutWouldOverwrite', detail: { branch: 'f' } }),
    null,
    'a refusal missing its paths is not turned into an empty one',
  )

  // --- every failure becomes a sentence --------------------------------------------------------

  const said = (error, op) => (op === undefined ? m.explain(error) : m.explain(error, op))
  for (const [error, needle, what] of [
    [refusal, 'src/a.rs', 'a refusal names the files even outside the panel'],
    [{ kind: 'branchExists', detail: { name: 'main' } }, 'main', 'branchExists names the branch'],
    [{ kind: 'invalidBranchName', detail: { name: 'a b' } }, 'valid branch name', 'invalidBranchName explains'],
    [
      { kind: 'branchNotMerged', detail: { name: 'old' } },
      'not fully merged into the branch you are on',
      // Rust's test is `git branch -d`'s — is the tip an ancestor of HEAD — so this variant
      // arrives for a branch already merged into some *other* branch, with nothing at stake.
      // The sentence may therefore not claim the commits exist nowhere else.
      'branchNotMerged says only what was measured, not "the commits are on no other branch"',
    ],
    [{ kind: 'branchIsCurrent', detail: { name: 'main' } }, 'Switch somewhere else', 'branchIsCurrent suggests the fix'],
    [{ kind: 'noSuchBranch', detail: { name: 'gone' } }, 'gone', 'noSuchBranch names it'],
    [
      { kind: 'notFastForward', detail: { branch: 'main', ahead: 2, behind: 3 } },
      '2 ahead and 3 behind',
      'notFastForward carries both counts, which is what a user chooses merge-or-rebase with',
    ],
    [{ kind: 'noUpstream', detail: { branch: 'solo' } }, 'no upstream', 'noUpstream names the branch'],
    [{ kind: 'operationInProgress', detail: { operation: 'rebase' } }, 'rebase', 'the operation is named'],
    [{ kind: 'fetch', detail: { output: 'could not read Username' } }, 'Username', "the transport's own text survives"],
    [{ kind: 'unborn' }, 'no commits', 'a payload-free variant still says something'],
    [
      { kind: 'detachedHead', detail: { head: 'a1b2c3d4' } },
      'check out a branch',
      // Folded into `noUpstream` until now, which told a detached HEAD that "a1b2c3d4 has no
      // upstream branch to pull from" — a sentence that calls a commit a branch and points at
      // `--set-upstream`, which cannot help. The sentence has to name the fix that can.
      'a detached HEAD is told what is actually wrong',
    ],
    [
      { kind: 'noRemote', detail: { name: 'origin' } },
      'git remote add',
      // This used to arrive as libgit2's own `Config: remote 'origin' does not exist`, via
      // the catch-all `git` variant. True, and useless to someone who has not added a remote.
      'a repository with no remote is told how to get one',
    ],
  ]) {
    ok(said(error).includes(needle), `${what} — got ${JSON.stringify(said(error))}`)
  }

  /*
   * The same refusal, two gestures.
   *
   * `CheckoutWouldOverwrite` is raised by a pull as well as by a checkout — both move the
   * working tree onto another commit and both use `blockers()` to decide. Reported to someone
   * who pressed Ctrl+T with the checkout wording it read "Switching to main would overwrite
   * local changes", which names a switch nobody asked for and names the branch they are
   * standing on as the destination.
   */
  ok(
    said(refusal, 'checkout').startsWith('Switching to feature'),
    'a checkout still says "switching"',
  )
  ok(
    said(refusal, 'pull').startsWith('Fast-forwarding feature'),
    'a pull says what a pull does, not what a checkout does',
  )
  ok(
    said(refusal, 'pull').includes('src/a.rs'),
    'and either way the files that block are still named — that is the whole variant',
  )
  eq(
    said(refusal),
    said(refusal, 'checkout'),
    'the operation defaults to checkout, so every existing caller is unmoved',
  )
  eq(
    said({ kind: 'noUpstream', detail: { branch: 'solo' } }, 'pull'),
    said({ kind: 'noUpstream', detail: { branch: 'solo' } }),
    'and no other variant is operation-sensitive',
  )

  eq(
    said({ kind: 'somethingNew', detail: {} }),
    'git: somethingNew',
    'an unrecognised tag is still shown, with its tag — never swallowed and never [object Object]',
  )
  eq(said(new Error('boom')), 'boom', 'a plain Error keeps its message')
  for (const error of [refusal, { kind: 'unborn' }, { kind: 'zzz' }, 'x', null, undefined, 42]) {
    ok(
      !said(error).includes('[object Object]'),
      `explain never renders an object literally: ${JSON.stringify(error)}`,
    )
  }

  // --- what a success says ---------------------------------------------------------------------

  const outcome = (over = {}) => ({
    branch: 'feature',
    createdFromRemote: null,
    stashed: null,
    restoreFailed: null,
    ...over,
  })
  eq(
    m.checkoutNote(outcome()),
    '',
    'a clean switch says nothing — the branch name in the status bar is the feedback',
  )
  ok(
    m.checkoutNote(outcome({ stashed: 'cide: switching to feature' })).includes('in the stash'),
    'a switch that stashed says where the work went',
  )
  ok(
    m.checkoutNote(outcome({ createdFromRemote: 'origin/feature' })).includes('tracking origin/feature'),
    'a local branch created from a remote one says so',
  )
  eq(
    m.checkoutNote(outcome({ stashed: 'cide: x', restoreFailed: 'conflicts in a.rs' })),
    'conflicts in a.rs',
    'a failed restore wins over every other note — it is the only one that needs acting on',
  )

  /*
   * A whole `FetchOutcome`. The fetch half is what the *Fetch* button gets; the pull half is
   * empty here and each pull case fills in what it is about, so the difference between a
   * fetch's sentence and a pull's is visible in the fixture rather than buried in a flag.
   */
  const fetched = (over = {}) => ({
    remote: 'origin',
    advanced: 0,
    output: '',
    branch: '',
    oldOid: '',
    newOid: '',
    filesChanged: 0,
    insertions: 0,
    deletions: 0,
    commits: [],
    moreCommits: 0,
    ...over,
  })

  eq(
    m.fetchNote(fetched()),
    'Already up to date with origin',
    'a fetch with nothing to take still answers — the user asked for network and waited',
  )
  eq(m.fetchNote(fetched({ advanced: 1 })), 'Fast-forwarded 1 commit from origin', 'singular')
  eq(m.fetchNote(fetched({ advanced: 4 })), 'Fast-forwarded 4 commits from origin', 'plural')
  eq(
    m.fetchNote(fetched({ output: ' From /tmp/x\n' })),
    'From /tmp/x',
    "git's own text is shown when there is any",
  )
  /*
   * The note bar is one line. `git fetch` writes several, and libgit2's sideband stream — which
   * `branch::fetch_with` no longer forwards, precisely because of this — is a progress meter
   * full of carriage returns. Both used to be printed verbatim as the one sentence a user got
   * for asking to fetch.
   */
  eq(
    m.fetchNote(
      fetched({ output: 'From /srv/thing\n   abc1234..def5678  main       -> origin/main\n' }),
    ),
    'From /srv/thing · abc1234..def5678  main       -> origin/main',
    'multi-line transport output is joined, not printed with newlines in a one-line bar',
  )
  eq(
    m.fetchNote(
      fetched({
        output: 'Counting objects 1\rCounting objects 3\r\nCompressing objects: 100% (3/3), done\n',
      }),
    ),
    'Counting objects 1 · Counting objects 3 · Compressing objects: 100% (3/3), done',
    'carriage returns are line breaks here too — a progress meter never reaches the bar intact',
  )
  ok(
    m.fetchNote(fetched({ output: 'x'.repeat(400) })).length <= 160,
    'a fetch of forty branches is truncated rather than pushing the list off the popup',
  )

  // --- what a *pull* says, which is the half that had nowhere to be said -----------------------

  /*
   * The three answers a pull can give have to be distinguishable, because they call for three
   * different next actions: rebuild, do nothing, or go and merge in a terminal. The third is a
   * `GitError` and is covered above; these are the first two.
   *
   * Before this, `FetchOutcome` carried `{remote, shelledOut, output, advanced}` and the best
   * sentence obtainable was "Fast-forwarded 7 commits from origin" — which cannot tell a
   * person whether their build is stale.
   */
  const pulled = (over = {}) =>
    fetched({ branch: 'main', oldOid: 'a1b2c3d4', newOid: 'e5f6a7b8', ...over })

  eq(
    m.fetchNote(pulled()),
    'main is already up to date with origin',
    'a pull that took nothing names its branch — with four repositories in a project that is '
      + 'the only thing that says which one just answered',
  )
  eq(
    m.fetchNote(
      pulled({ advanced: 7, filesChanged: 12, insertions: 230, deletions: 41 }),
    ),
    'Fast-forwarded main 7 commits from origin · 12 files +230 −41',
    'a pull that took something says what it took — the counts are the feature',
  )
  eq(
    m.fetchNote(pulled({ advanced: 1, filesChanged: 1, insertions: 3, deletions: 0 })),
    'Fast-forwarded main 1 commit from origin · 1 file +3',
    'singular throughout, and a side with nothing on it is left out rather than printed as −0',
  )
  eq(
    m.fetchNote(pulled({ advanced: 2 })),
    'Fast-forwarded main 2 commits from origin',
    'a fast-forward of pure merges changes no files, and says so by saying nothing',
  )
  eq(
    m.fetchNote(pulled({ advanced: 3, output: 'From /srv/thing' })),
    'Fast-forwarded main 3 commits from origin',
    "the transport's chatter never displaces the answer in the headline — it goes in the body",
  )

  const commit = (oid, summary, author = 'Ada') => ({ shortOid: oid, summary, author })

  eq(
    m.fetchDetail(pulled({ advanced: 2, commits: [commit('e5f6a7b8', 'Add the thing'), commit('c3d4e5f6', 'Fix the other', 'Grace')] })),
    'e5f6a7b8  Add the thing — Ada\nc3d4e5f6  Fix the other — Grace',
    'the body is one line per commit: the oid to paste into `git show`, the subject to '
      + 'recognise it by, the author to tell whether it is yours',
  )
  eq(
    m.fetchDetail(pulled({ advanced: 14, commits: [commit('e5f6a7b8', 'Newest')], moreCommits: 13 })),
    'e5f6a7b8  Newest — Ada\n… and 13 commits more',
    'the overflow is reported rather than dropped — Rust caps the list, so 14 must not read as 1',
  )
  eq(
    m.fetchDetail(pulled({ commits: [], output: 'Fetched 5 objects from origin' })),
    'Fetched 5 objects from origin',
    'with no commits the transport gets the space: "your branch did not move but other refs did"',
  )
  eq(m.fetchDetail(pulled()), '', 'and with nothing at all to add there is no disclosure to draw')
  eq(
    m.fetchDetail(pulled({ advanced: 1, commits: [commit('e5f6a7b8', '', '')] })),
    'e5f6a7b8  (no summary)',
    'a commit whose message is not UTF-8 arrives with empty strings — the row still has to '
      + 'render as something a person can read',
  )

  // --- one gesture, one answer, however many repositories ---------------------------------------

  const repoFetch = (name, over = {}) => ({ name, outcome: pulled(over) })

  eq(
    m.pullReport([repoFetch('app', { advanced: 2, filesChanged: 3, insertions: 9, deletions: 1 })]),
    {
      text: 'Fast-forwarded main 2 commits from origin · 3 files +9 −1',
      detail: '',
    },
    'with one repository the report is exactly the single-repo sentence — the multi-root '
      + 'machinery is invisible in the common case',
  )
  eq(
    m.pullReport([repoFetch('app'), repoFetch('vendor/zlib'), repoFetch('docs')]),
    {
      text: 'All 3 repositories are already up to date',
      detail:
        'app: main is already up to date with origin\n'
        + 'vendor/zlib: main is already up to date with origin\n'
        + 'docs: main is already up to date with origin',
    },
    /*
     * The dedupe hazard, pinned. `notices.admit` collapses by identical text, so three
     * submodules each notifying "main is already up to date with origin" would have shown ONE
     * toast and silently spoken for three. One aggregated notice is why that cannot happen,
     * and every repository is named in the body even though none of them moved.
     */
    'several repositories that all did nothing produce one notice that names all of them',
  )
  eq(
    m.pullReport([
      repoFetch('app', { advanced: 2, filesChanged: 3, insertions: 9, deletions: 1 }),
      repoFetch('vendor/zlib'),
      repoFetch('docs', { advanced: 5, filesChanged: 7, insertions: 40, deletions: 2 }),
    ]).text,
    'Fast-forwarded 2 of 3 repositories · 7 commits · 10 files +49 −3',
    'the totals are summed across the repositories that moved, and the ones that did not are '
      + 'counted in the denominator rather than hidden',
  )
  ok(
    m.pullReport([
      repoFetch('app', { advanced: 2 }),
      repoFetch('vendor/zlib'),
    ]).detail.includes('vendor/zlib: main is already up to date'),
    'a repository that took nothing is still listed — leaving it out would read as skipped',
  )
  eq(
    m.pullReport([repoFetch('app', { advanced: 1 }), repoFetch('lib', { advanced: 1 })]).text,
    'Fast-forwarded all 2 repositories · 2 commits',
    '"all N" rather than "N of N", which reads as though something had been left out',
  )
  eq(
    m.pullReport([]),
    { text: '', detail: '' },
    'no results is not a sentence — `dispatch.ts` never notifies for it, and this pins that '
      + 'calling it anyway cannot produce an empty toast with a bullet in it',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-branches: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-branches: ok')
