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
    ['FetchOutcome', ['remote', 'shelledOut', 'output', 'advanced']],
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

  // --- every failure becomes a sentence --------------------------------------------------------

  const said = (error) => m.explain(error)
  for (const [error, needle, what] of [
    [refusal, 'src/a.rs', 'a refusal names the files even outside the panel'],
    [{ kind: 'branchExists', detail: { name: 'main' } }, 'main', 'branchExists names the branch'],
    [{ kind: 'invalidBranchName', detail: { name: 'a b' } }, 'valid branch name', 'invalidBranchName explains'],
    [{ kind: 'branchNotMerged', detail: { name: 'old' } }, 'loses them', 'branchNotMerged says what is lost'],
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
  ]) {
    ok(said(error).includes(needle), `${what} — got ${JSON.stringify(said(error))}`)
  }

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

  eq(
    m.fetchNote({ remote: 'origin', advanced: 0, output: '' }),
    'Already up to date with origin',
    'a pull with nothing to take still answers — the user asked for network and waited',
  )
  eq(
    m.fetchNote({ remote: 'origin', advanced: 1, output: '' }),
    'Fast-forwarded 1 commit from origin',
    'singular',
  )
  eq(
    m.fetchNote({ remote: 'origin', advanced: 4, output: '' }),
    'Fast-forwarded 4 commits from origin',
    'plural',
  )
  eq(
    m.fetchNote({ remote: 'origin', advanced: 0, output: ' From /tmp/x\n' }),
    'From /tmp/x',
    "git's own text is shown when there is any",
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-branches: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-branches: ok')
