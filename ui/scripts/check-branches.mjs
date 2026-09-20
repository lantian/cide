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

/** Source, read relative to this script. */
const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

/**
 * Source with comments removed.
 *
 * A grep over raw source matches the *explanation* of a rule as happily as the rule, so an
 * assertion written that way stays green after the code is deleted and only the prose is left.
 * That is a lesson this repository has paid for; `check-scratch.mjs` and `check-paths.mjs` strip
 * for the same reason. String-aware, so a `'//'` inside a literal does not eat the rest of the
 * line.
 */
const stripComments = (source) => {
  let out = ''
  let i = 0
  while (i < source.length) {
    const ch = source[i]
    if (ch === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++
      continue
    }
    if (ch === '/' && source[i + 1] === '*') {
      i += 2
      while (i < source.length && !(source[i] === '*' && source[i + 1] === '/')) i++
      i += 2
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch
      out += ch
      i++
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') {
          out += source[i]
          i++
        }
        if (i < source.length) {
          out += source[i]
          i++
        }
      }
      out += quote
      i++
      continue
    }
    out += ch
    i++
  }
  return out
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
        // The M37 half: the walk behind `commits` as a revspec, for *View commits* to hand the log.
        'received',
        // The M20 half. `strategy` is what stops the notice saying "Fast-forwarded" after a
        // rebase, and `conflicts` is what makes a stopped merge a success with work attached
        // rather than a failure.
        'strategy',
        'rewritten',
        'skipped',
        'conflicts',
      ],
    ],
    // The merge's own report (M24). Same join, same reason: `branchModel::MergeReport` declares
    // this shape structurally, so only this line notices a rename in `cide-ipc`.
    [
      'MergeOutcome',
      [
        'source',
        'branch',
        'fastForward',
        'oldOid',
        'newOid',
        'advanced',
        'filesChanged',
        'insertions',
        'deletions',
        'commits',
        'moreCommits',
        'conflicts',
      ],
    ],
    [
      'PushOutcome',
      ['remote', 'refspec', 'shelledOut', 'output', 'branch', 'pushed', 'oldOid', 'newOid'],
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
  // The two staging refusals the Git panel's drop-to-add raises. Without these the panel's
  // `explain` arms are dead code and the gesture is back to printing whichever tag Rust
  // happened to send — which is how it came to say `[object Object]`.
  for (const tag of ['pathIgnored', 'nestedRepository']) {
    ok(
      new RegExp(`"kind": "${tag}", "detail": \{ path: string, \}`).test(generated),
      `GitError still carries ${tag} {path} — the refusal the Git panel explains`,
    )
  }

  /*
   * And the surface. `explain` having an arm is half the fix; the panel calling it is the
   * other half, and the half that was missing.
   *
   * `useGitPanel::trackPaths` is the drop-to-add gesture. Its failure line read
   * `e instanceof Error ? e.message : String(e)`, and a `GitError` is neither an `Error` nor a
   * string — so the message a user saw was `nothing was added to git — [object Object]`, for a
   * refusal Rust had a perfectly good reason for. Grepped rather than executed because the hook
   * is a React module this script cannot import.
   */
  // Comments stripped: the fixed line carries a comment that *quotes* `String(e)` as the thing
  // it is not, and a raw grep would match the warning and call the bug present.
  const panel = stripComments(
    readFileSync(join(UI, 'src', 'sidebar', 'GitPanel', 'useGitPanel.ts'), 'utf8'),
  )
  const track = panel.slice(panel.indexOf('const trackPaths'))
  ok(track.includes('const detail = explain(e)'), 'the drop-to-add failure goes through explain')
  ok(
    !/String\(e\)/.test(track.slice(0, track.indexOf('const dismissDialog'))),
    'and nothing in that gesture stringifies a rejection by hand',
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
    m.actionsFor(ref('feature/login'), head()),
    ['checkout', 'newFrom', 'merge', 'rename', 'delete'],
    'an ordinary local branch offers all five',
  )
  eq(
    m.actionsFor(ref('main', { current: true }), head()),
    ['newFrom', 'rename'],
    'the branch you are on cannot be checked out again, merged into itself, or deleted — git ' +
      'refuses the last and the first two would do nothing',
  )
  eq(
    m.actionsFor(ref('origin/main', { remote: true }), head()),
    ['checkout', 'newFrom', 'merge'],
    'a remote-tracking ref cannot be renamed or deleted (it is a cache of the remote’s names ' +
      'and the next fetch would put it back) — but it CAN be merged, which is `git merge ' +
      'origin/x`: taking a fetched branch without creating a local copy first',
  )
  /*
   * Merge needs somewhere to merge *into*. A detached or unborn HEAD has no current branch, so
   * the item vanishes rather than opening a panel whose only outcome is a refusal — the same
   * gate `canPull` applies, minus the upstream half a merge does not need.
   */
  eq(
    m.actionsFor(ref('feature/login'), head({ detached: true })),
    ['checkout', 'newFrom', 'rename', 'delete'],
    'a detached HEAD offers no merge — there is no current branch to merge into',
  )
  eq(
    m.actionsFor(ref('feature/login'), head({ unborn: true })),
    ['checkout', 'newFrom', 'rename', 'delete'],
    'an unborn branch offers no merge — there is no commit to merge onto',
  )
  eq(
    m.actionsFor(ref('feature/login'), null),
    ['checkout', 'newFrom', 'rename', 'delete'],
    'and no head at all offers no merge rather than throwing',
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
    [
      { kind: 'pathIgnored', detail: { path: '.cide/worktrees/' } },
      '.gitignore',
      // Raised by the Git panel's drop-to-add. Rust answered `noSuchChange` for this until M30,
      // which is *"…has no changes to apply"* about a directory full of files the user is
      // looking at, and the panel then printed the whole tagged object as `[object Object]`.
      // The sentence has to name the thing that is refusing — a rule, in a file they can open.
      'an ignored path is told which mechanism refuses it',
    ],
    [
      { kind: 'pathIgnored', detail: { path: '.cide/worktrees/' } },
      'git add -f',
      'and how to overrule it, since cide has no gesture that can',
    ],
    [
      { kind: 'nestedRepository', detail: { path: '.cide/worktrees/agent-1/' } },
      'submodule',
      // An agent worktree, or a vendored clone. libgit2's own refusal is `invalid path:
      // 'x/'` — a complaint about a trailing slash, which reads as a bug in cide.
      'a nested repository is told the one arrangement that would make it work here',
    ],
    [
      { kind: 'noSuchChange', detail: { path: 'src/a.rs' } },
      'src/a.rs',
      'noSuchChange names the path — it is a race, and the user needs to know which file lost it',
    ],
    [
      { kind: 'staleSelection', detail: { path: 'src/a.rs' } },
      'pick again',
      'staleSelection says the ticks are what went stale, not the file',
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
    said(refusal, 'merge').startsWith('Merging feature'),
    'a merge names the merge — and the branch in the sentence is the SOURCE, because Rust puts ' +
      'the ref being merged in `branch` for this op',
  )
  ok(
    said(refusal, 'pull').includes('src/a.rs') && said(refusal, 'merge').includes('src/a.rs'),
    'and whichever gesture it was, the files that block are still named — that is the whole variant',
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
    // The M20 half: which strategy ran, what a rebase did to your own commits, and what is left
    // to resolve. A plain fetch fills in none of it, which is what these defaults are.
    rewritten: 0,
    skipped: 0,
    conflicts: [],
    filesChanged: 0,
    insertions: 0,
    deletions: 0,
    commits: [],
    moreCommits: 0,
    received: '',
    ...over,
  })

  eq(
    m.fetchNote(fetched()),
    'Already up to date with origin',
    'a fetch with nothing to take still answers — the user asked for network and waited',
  )
  eq(m.fetchNote(fetched({ advanced: 1 })), 'Fast-forwarded 1 commit from origin', 'singular')
  eq(m.fetchNote(fetched({ advanced: 4 })), 'Fast-forwarded 4 commits from origin', 'plural')

  /*
   * The verb follows the strategy. (M20)
   *
   * This function predated merge and rebase and said *"Fast-forwarded"* whatever had happened,
   * which is not a wording quibble: a user who chose **Rebase** in the dialog and was then told
   * their branch had been fast-forwarded has been told their answer was ignored, and the history
   * they are about to push is not the shape the sentence describes.
   */
  eq(
    m.fetchNote(fetched({ advanced: 4, branch: 'main', strategy: 'merge' })),
    'Merged 4 commits from origin into main',
    'a merge says it merged',
  )
  eq(
    m.fetchNote(
      fetched({ advanced: 4, branch: 'main', strategy: 'rebase', rewritten: 2 }),
    ),
    'Rebased main onto 4 commits from origin, replaying 2 commits of yours',
    'a rebase says both numbers, because it does two things and one is to YOUR commits',
  )
  eq(
    m.fetchNote(
      fetched({ advanced: 4, branch: 'main', strategy: 'rebase', rewritten: 1, skipped: 2 }),
    ),
    'Rebased main onto 4 commits from origin, replaying 1 commit of yours (2 commits already upstream, dropped)',
    'and names the dropped ones — git drops them silently and the user goes looking for them',
  )
  eq(
    m.fetchNote(
      fetched({ advanced: 4, branch: 'main', strategy: 'fastForward' }),
    ),
    'Fast-forwarded main 4 commits from origin',
    'and a fast-forward still says so',
  )

  /*
   * Conflicts lead, whatever else is true.
   *
   * A merge that stopped still *took* every one of its commits, so the counts are all true —
   * and leading with them would bury the one fact the user has to act on under a sentence that
   * reads like success.
   */
  eq(
    m.fetchNote(
      fetched({
        advanced: 4,
        branch: 'main',
        strategy: 'merge',
        conflicts: ['src/a.rs', 'src/b.rs'],
      }),
    ),
    'Merging main — 2 files to resolve: src/a.rs and src/b.rs',
    'a conflicted merge says what is left to do, not how many commits it took',
  )
  eq(
    m.fetchNote(
      fetched({ advanced: 4, branch: 'main', strategy: 'rebase', conflicts: ['src/a.rs'] }),
    ),
    'Rebasing main — 1 file to resolve: src/a.rs',
    'and a conflicted rebase names its own verb',
  )
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

  // The id is derived from the name only so the fixture is short; the point of carrying it is
  // that a real project can have two roots with one basename.
  const repoFetch = (name, over = {}) => ({ repo: `id-${name}`, name, outcome: pulled(over) })

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
    'Updated 2 of 3 repositories · 7 commits · 10 files +49 −3',
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
    'Updated all 2 repositories · 2 commits',
    '"all N" rather than "N of N", which reads as though something had been left out',
  )
  eq(
    m.pullReport([]),
    { text: '', detail: '' },
    'no results is not a sentence — `dispatch.ts` never notifies for it, and this pins that '
      + 'calling it anyway cannot produce an empty toast with a bullet in it',
  )

  // --- *View commits*, one per repository that received any (M37) -----------------------------

  {
    const RANGE = `${'a'.repeat(40)}..${'b'.repeat(40)}`
    eq(
      m.receivedLinks([repoFetch('app', { advanced: 2, received: RANGE })]),
      [{ repo: 'id-app', label: 'View commits', spec: RANGE }],
      'one repository: the bare label, the id and not the name, and the range passed through '
        + 'untouched — Rust spelled it and Rust will parse it',
    )
    eq(
      m
        .receivedLinks([
          repoFetch('app', { advanced: 2, received: RANGE }),
          repoFetch('lib', { advanced: 1, received: RANGE }),
          repoFetch('docs'),
        ])
        .map((l) => l.label),
      ['View commits in app', 'View commits in lib'],
      'several: one link per repository that received any, each named — "Updated 2 of 4 '
        + 'repositories" says nothing about which two — and none for the one that took nothing',
    )
    eq(m.receivedLinks([repoFetch('app')]), [], 'nothing came down, nothing to view')

    const dispatch = stripComments(read('../src/keys/dispatch.ts'))
    ok(
      /done\.push\(\{ repo: repo\.id, name: repo\.name/.test(dispatch),
      'dispatch labels each answer with the repository’s id',
    )
    ok(
      /role\.kind === 'shell' \? receivedLinks\(done\) : \[\]/.test(dispatch),
      'and offers the links in the shell window only — the tool window is drawn there, and the '
        + 'request the link parks lives in that window’s realm',
    )
    ok(
      /actions: actions\.length > 0 \? actions : undefined/.test(dispatch),
      'and passes no list rather than an empty one, so a detached window’s notice is not "news" '
        + 'to `notices.admit`',
    )
  }

  // --- what a *merge* asks, and what it says afterwards (M24) ----------------------------------

  {
    const ask = m.mergeConfirm('feature/login', 'main')
    eq(ask.title, 'Merge feature/login into main?', 'the question names both branches')
    ok(
      ask.body.includes('merge commit') && ask.body.includes('Nothing is pushed'),
      'the body says what will happen and what will NOT — "merge" one button from Push must ' +
        'not read as publishing anything',
    )
  }

  const merged = (over = {}) => ({
    source: 'feature/login',
    branch: 'main',
    fastForward: false,
    oldOid: 'a1b2c3d4',
    newOid: 'e5f6a7b8',
    advanced: 0,
    filesChanged: 0,
    insertions: 0,
    deletions: 0,
    commits: [],
    moreCommits: 0,
    conflicts: [],
    ...over,
  })

  eq(
    m.mergeNote(merged({ advanced: 3, filesChanged: 5, insertions: 40, deletions: 2 })),
    'Merged 3 commits from feature/login into main · 5 files +40 −2',
    'a merge commit reports what it took — the counts answer "is my build stale", '
      + '`fetchNote`\'s argument',
  )
  eq(
    m.mergeNote(merged({ fastForward: true, advanced: 1, filesChanged: 1, insertions: 3 })),
    'Fast-forwarded main 1 commit to feature/login · 1 file +3',
    'a fast-forward says so — the verb is the shape of the history about to be pushed, '
      + 'singular throughout, and a side with nothing on it is left out',
  )
  eq(
    m.mergeNote(merged()),
    'main already contains feature/login',
    'nothing to take names both branches — with several repositories it is the only thing '
      + 'that says which one answered',
  )
  eq(
    m.mergeNote(
      merged({ advanced: 4, oldOid: 'a1b2c3d4', newOid: 'a1b2c3d4', conflicts: ['src/a.rs', 'src/b.rs'] }),
    ),
    'Merging feature/login into main — 2 files to resolve: src/a.rs and src/b.rs',
    'conflicts lead, whatever else is true — the counts would bury the one fact that '
      + 'needs acting on',
  )
  eq(
    m.mergeNote(merged({ advanced: 1, conflicts: ['src/a.rs'] })),
    'Merging feature/login into main — 1 file to resolve: src/a.rs',
    'and the conflict count is singular when it should be',
  )

  // --- the keyboard, which is the half a browserless harness can still own -------------------

  /*
   * > *"when i pressing UP/DOWN buttons - i should be able to travers through branches and back
   * > to filter input if top of the list of branches, enter (keyboard button) on a branch should
   * > switch to that branch with popup close"*
   *
   * Every transition below is driven directly, because the alternative is a component test this
   * project has no runner for. The two that matter most, and would both look fine in a casual
   * read of the component: Up from the FIRST row landing in the *filter field* rather than
   * wrapping to the bottom, and a highlight that survives the list shrinking under it.
   */
  const row = (index) => ({ kind: 'row', index })

  eq(m.FILTER, { kind: 'filter' }, 'the resting place is the field the popup opens focused on')

  eq(m.navigate('ArrowDown', m.FILTER, 5), row(0), 'Down out of the filter enters the list')
  eq(m.navigate('ArrowDown', row(0), 5), row(1), 'Down walks the list')
  eq(
    m.navigate('ArrowDown', row(4), 5),
    null,
    'Down on the last row does not wrap to the top — the key is handed back to the field ' +
      'rather than silently moving the highlight somewhere the user was not looking',
  )
  eq(
    m.navigate('ArrowUp', row(0), 5),
    m.FILTER,
    'Up from the FIRST branch returns to the filter input — the transition the report spells ' +
      'out, and the one a wrap-around would have eaten',
  )
  eq(m.navigate('ArrowUp', row(3), 5), row(2), 'Up walks the list')
  eq(
    m.navigate('ArrowUp', m.FILTER, 5),
    null,
    'and Up in the field is the field’s own key — there is nothing above it',
  )

  /*
   * Down to the bottom and back up again. A loop rather than three more literals, because what
   * is being pinned is that the two directions are inverses over the whole list: an asymmetry
   * anywhere in the middle is exactly the bug a user reports as "it skips one".
   */
  {
    const count = 6
    let focus = m.FILTER
    for (let i = 0; i < count; i += 1) {
      focus = m.navigate('ArrowDown', focus, count) ?? focus
      eq(focus, row(i), `Down ${i + 1} times from the filter is row ${i}`)
    }
    eq(m.navigate('ArrowDown', focus, count), null, 'and the bottom is the bottom')
    for (let i = count - 1; i > 0; i -= 1) {
      focus = m.navigate('ArrowUp', focus, count) ?? focus
      eq(focus, row(i - 1), `Up from row ${i} is row ${i - 1}`)
    }
    eq(m.navigate('ArrowUp', focus, count), m.FILTER, 'and the top is the filter again')
  }

  for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End', 'PageUp', 'PageDown']) {
    eq(
      m.navigate(key, m.FILTER, 0),
      null,
      `${key} with nothing to walk — a filter that matched no branch — moves nothing`,
    )
  }
  for (const key of ['Enter', 'Escape', 'Tab', 'a', ' ']) {
    eq(m.navigate(key, row(1), 5), null, `${key} is not the list’s key`)
  }
  /*
   * Escape in particular. It is absent from `navigate` on purpose: the popup's scrim owns it
   * and it means *back out one panel* — a question about `Mode`, which the reducer cannot see.
   * Claiming it here would have taken `preventDefault` on the one key that has to keep bubbling.
   */
  eq(m.navigate('Escape', m.FILTER, 5), null, 'Escape still belongs to the scrim, from anywhere')

  eq(m.navigate('Home', m.FILTER, 5), null, 'Home in the field is the caret’s — it edits text')
  eq(m.navigate('End', m.FILTER, 5), null, 'and so is End')
  eq(m.navigate('Home', row(3), 5), row(0), 'Home in the list is the first row')
  eq(m.navigate('End', row(0), 5), row(4), 'End in the list is the last')

  eq(m.navigate('PageDown', m.FILTER, 40), row(m.PAGE_ROWS - 1), 'a page down from the field')
  eq(m.navigate('PageDown', m.FILTER, 5), row(4), 'a page longer than the list stops at the end')
  eq(m.navigate('PageUp', row(20), 40), row(20 - m.PAGE_ROWS), 'a page up walks back')
  eq(
    m.navigate('PageUp', row(5), 40),
    row(0),
    'a page up from near the top lands ON the top row, not in the field: a jump that ends in a ' +
      'text box swallows the next thing typed',
  )
  eq(m.navigate('PageUp', row(0), 40), m.FILTER, 'and from the top row it continues into the field')
  eq(m.navigate('PageUp', m.FILTER, 40), null, 'a page up in the field moves nothing')

  // --- the highlight cannot point past the list ----------------------------------------------

  /*
   * The off-by-one this list has two live routes to, neither of them exotic: typing into the
   * filter shortens the list under the highlight, and `cide://git-status` re-renders the popup
   * whenever anything touches the refs — a `git branch -d` in a terminal pane, a fetch that
   * prunes, another window's delete. An index left past the end is ⏎ checking out `undefined`.
   */
  eq(m.clampFocus(row(4), 2), row(1), 'a highlight past the end lands on the last surviving row')
  eq(m.clampFocus(row(0), 0), m.FILTER, 'a list that emptied puts the highlight back in the field')
  eq(m.clampFocus(m.FILTER, 0), m.FILTER, 'the field is always a valid place to be')
  eq(m.clampFocus(m.FILTER, 5), m.FILTER, 'and is never dragged into the list')
  eq(m.clampFocus(row(1), 5), row(1), 'an index inside the list is left alone')
  eq(m.clampFocus(row(-1), 5), m.FILTER, 'a negative index is not an index')

  /*
   * The whole gesture, end to end: highlight the last of five rows, then type. The filtered
   * list is two rows long, and what ⏎ acts on has to be a branch that is still on screen.
   */
  {
    const all = m.visibleBranches(list, '')
    const narrowed = m.visibleBranches(list, 'login')
    eq(all.length, 5, 'the fixture is five rows unfiltered')
    const kept = m.clampFocus(row(all.length - 1), narrowed.length)
    eq(kept, row(1), 'the highlight follows the shrinking list rather than pointing off its end')
    eq(
      m.enterAction(narrowed, kept),
      { kind: 'checkout', name: 'origin/feature/login' },
      '⏎ checks out a row that is actually visible — the classic off-by-one is a checkout of ' +
        'the branch that USED to be at that index',
    )
  }

  // --- what ⏎ does ---------------------------------------------------------------------------

  const rows = m.visibleBranches(list, '')
  eq(
    m.enterAction(rows, row(1)),
    { kind: 'checkout', name: 'feature/login' },
    '⏎ on a highlighted branch checks it out',
  )
  eq(
    m.enterAction(rows, row(0)),
    { kind: 'already', name: 'main' },
    'the current branch is pinned first, so it is the row the first Down lands on — and ⏎ ' +
      'there has to SAY something rather than do nothing, which reads as a dead keyboard',
  )
  ok(m.alreadyOn('main').includes('main'), 'and the sentence names the branch')
  eq(
    m.enterAction(m.visibleBranches(list, 'maint'), m.FILTER),
    { kind: 'checkout', name: 'maintenance' },
    '⏎ straight from the filter checks out the only remaining row — the older behaviour, kept',
  )
  eq(
    m.enterAction(rows, m.FILTER),
    { kind: 'none' },
    'with more than one row it does nothing rather than guessing: a checkout is not a gesture ' +
      'to resolve an ambiguity with',
  )
  eq(m.enterAction([], m.FILTER), { kind: 'none' }, 'nothing matched, nothing to check out')
  eq(m.enterAction(rows, row(99)), { kind: 'none' }, 'and an impossible index is not a checkout')

  // --- and the popup actually uses all of it ---------------------------------------------------

  /*
   * The defect this repository keeps producing is a model that is designed, documented and
   * never called. Everything above passes just as happily against a `BranchSelector.tsx` that
   * imports none of it, so the wiring is asserted here — over comment-stripped source, because
   * a grep matches the *explanation* of a rule as happily as the rule itself.
   */
  const popup = stripComments(read('../src/chrome/BranchSelector.tsx'))
  const css = read('../src/chrome/BranchSelector.module.css')

  for (const call of ['navigate(', 'clampFocus(', 'enterAction(', 'alreadyOn(', 'setFocus(FILTER)']) {
    ok(popup.includes(call), `BranchSelector.tsx actually calls ${call} — the model is wired up`)
  }
  ok(
    /scrollIntoView\(\{\s*block: 'nearest'\s*\}\)/.test(popup),
    'the highlight is scrolled into view: `.rows` scrolls, and arrows that walk the highlight ' +
      'out of sight look like a list that is not moving',
  )
  ok(
    popup.includes('aria-activedescendant'),
    'the field names the highlighted row, so focus can stay in the filter and a screen reader ' +
      'still follows the arrows',
  )
  ok(popup.includes('styles.rowOn'), 'the highlight has a class…')
  ok(css.includes('.rowOn'), '…and the class exists in the stylesheet — one without the other ' +
    'is a highlight nobody can see')

  /*
   * Where the dismiss is, and where it is not.
   *
   * "It should close after selection" is answered *after the checkout resolves*, never on the
   * click: `CheckoutWouldOverwrite` is a real refusal that this popup answers with a panel
   * naming the files, and a popup that had already closed would have thrown that away — the
   * user would be left standing on the branch they started from with no explanation.
   */
  const success = popup.slice(
    popup.indexOf('await branchApi.checkout('),
    popup.indexOf('} catch (error) {', popup.indexOf('await branchApi.checkout(')),
  )
  ok(success !== '' && success.includes('onDismiss()'), 'a checkout that succeeded closes the popup')
  const failure = popup.slice(
    popup.indexOf('} catch (error) {', popup.indexOf('await branchApi.checkout(')),
    popup.indexOf('} finally {', popup.indexOf('await branchApi.checkout(')),
  )
  ok(
    failure !== '' && !failure.includes('onDismiss'),
    'a checkout that was REFUSED leaves the popup up — closing on a refusal would hide the ' +
      'panel and the sentence that `explain` exists to produce',
  )
  /*
   * Focus has to come back to the field when a panel closes, and go *somewhere inside the
   * scrim* while one is open.
   *
   * Every key this popup answers — the arrows and ⏎ on the search field, Escape on the scrim —
   * is a React handler on an element inside the scrim. A panel that unmounts the field leaves
   * focus on `document.body`, which is outside that subtree, and all three keys go dead at
   * once. ⏎ is what put that on the ordinary route: it can raise the refusal panel, and Escape
   * then failed to back out of the panel the keystroke had just produced.
   *
   * Grepped rather than driven, because it is DOM: `jsdom` is not a dependency here and adding
   * one to assert `document.activeElement` would be a larger commitment than the four lines it
   * covers. What is pinned is that the effect exists and is keyed on the panel, which is the
   * part a later edit would drop.
   */
  /*
   * And `act` has to take focus back itself, because the effect above cannot do it for the one
   * route that needs it most.
   *
   * Fetch, Pull and Push are reachable only *from* the list, so `act`'s `setMode({kind:'list'})`
   * writes the value `mode.kind` already held. The effect is keyed on `mode.kind` — deliberately,
   * so a panel re-render does not yank focus mid-read — so it does not re-run. Focus stays on the
   * button the pointer pressed, and every arrow key is a handler on the search field. One click
   * on Fetch therefore killed the keyboard navigation this popup exists for, with nothing on
   * screen to say so and no panel involved for the effect above to notice.
   */
  const actBody = popup.slice(popup.indexOf('const act = ('), popup.indexOf('void useBranches.getState().run(', popup.indexOf('const act = (')))
  ok(
    actBody !== '' && actBody.includes('search.current?.focus()'),
    'Fetch/Pull/Push return focus to the search field — they do not change `mode.kind`, so the '
      + 'effect above never fires for them and the arrows would go dead after one click',
  )
  ok(
    /useEffect\(\(\) => \{\s*if \(mode\.kind === 'list'\) search\.current\?\.focus\(\)/.test(popup),
    'returning to the list refocuses the search field — otherwise the arrows and ⏎ are inert '
      + 'after any panel, because focus is left on `document.body` outside the scrim',
  )
  ok(
    /\}, \[mode\.kind\]\)/.test(popup),
    'and that effect is keyed on the panel, not on mount alone: a mount-only focus call is '
      + 'exactly the version that leaves the keyboard dead after Cancel',
  )
  ok(
    popup.includes('popupEl.current?.focus()') && popup.includes('tabIndex={-1}'),
    'the delete and refusal panels own no field, so the dialog itself takes focus — Escape has '
      + 'to keep working on the panel ⏎ produced',
  )
  ok(
    css.includes('.popup:focus'),
    '…and that programmatic focus is not allowed to paint a ring around the whole popup',
  )

  ok(
    success.includes('notify('),
    'and a success that had something to say — a stash taken, a tracking branch created, a ' +
      'restore that conflicted — hands it to the notice stack, which outlives the popup',
  )

  /*
   * The merge gesture is wired end to end (M24). A conflicted merge must reach
   * `showConflicts` — the one surface that can finish it, the same route `keys/dispatch.ts`
   * takes after a conflicted pull — and never the note bar alone, which would say "2 files to
   * resolve" and offer nothing that resolves them. Every other failure goes through `explain`
   * with the merge wording, so the sentence names the merge that was asked for rather than a
   * switch that was not.
   */
  for (const call of [
    'branchApi.merge(',
    'showConflicts(',
    "explain(error, 'merge')",
    'mergeConfirm(',
    'mergeNote(',
  ]) {
    ok(popup.includes(call), `BranchSelector.tsx actually calls ${call} — the merge action is wired up`)
  }

  /*
   * Every long operation in this popup reports itself to the status bar. (M65)
   *
   * Fetch, Pull and Merge are each a network round trip on a blocking Tauri command, and until
   * M65 the only thing that said so was `disabled` on the buttons of a popup that is shut by
   * then. An untracked road is invisible in exactly the way the feature exists to fix, and
   * nothing else in the suite can see one — so each is pinned to the *call it wraps*.
   *
   * `trackGitOp` goes round `branchApi.*` and deliberately not round `store.run`: `run`'s
   * promise resolves only after its `finally` re-runs `load()`, so wrapping it would leave the
   * bar saying `Pulling…` through a status walk of every repository in the project.
   */
  for (const [call, verb] of [
    ["trackGitOp(p, 'fetch', branchApi.fetch(p, r))", 'Fetch'],
    ["trackGitOp(p, 'pull', branchApi.pull(p, r, request))", 'the pull the divergence dialog re-issues'],
    ["trackGitOp(p, 'pull', branchApi.pull(p, r, { skipFetch: false, remember: false }))", 'the opening pull'],
    ["trackGitOp(p, 'merge', branchApi.merge(p, r, name))", 'Merge'],
  ]) {
    ok(
      popup.includes(call),
      `${verb} tells the status bar it is running. The second pull road is the one that hides: ` +
        '`one()` is re-issued from the strategy dialog long after tryPull’s finally has run, ' +
        'and it is the SLOWER road, because it is the one that actually merges or rebases',
    )
  }
  ok(
    !/GitOp\s*=\s*'checkout' \| 'pull' \| 'merge' \| 'push' \| 'fetch'/.test(
      stripComments(read('../src/chrome/branchModel.ts')),
    ),
    "and branchModel's GitOp gained no `fetch` arm — that type exists for a refusal shared by " +
      'the operations that move the working tree onto another commit, and widening it to label ' +
      'a spinner would put which-button-was-pressed into a type that exists for a sentence',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-branches: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-branches: ok')
