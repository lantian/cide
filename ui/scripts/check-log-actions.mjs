/**
 * Checks `src/chrome/logActions.ts` — every decision and every sentence behind the commit log's
 * actions — plus the `explain` arms in `src/chrome/branchModel.ts` that the same round added,
 * plus the three rules `src/chrome/ConfirmDestructive.tsx` has to keep while growing a mode
 * picker.
 *
 * # Why these three things are one check
 *
 * `crates/cide-git` already proves that a revert reverts and a `--hard` discards, differentially
 * against the real `git`. What no Rust test can see is the half the user meets: **whether the
 * dialog asked the right question before any of that ran.** A reset dialog that names the
 * `--hard` casualties while `--soft` is selected is a correct backend behind a lying front end,
 * and it fails in the one direction that cannot be undone.
 *
 * So the properties pinned here are the ones that are wrong *silently*:
 *
 *   - the three reset modes exist, in safest-first order, and each names **its own** casualties.
 *     `ConfirmDestructive` falls back to `choices[0]`, so an order change would silently make
 *     `--hard` the default answer;
 *   - Mixed says something different in changelist mode and in staging-area mode. The two costs
 *     are "nothing" and "an hour of `git add -p`", and one sentence covering both has to be
 *     vague enough to be true of the second — at which point every changelist user learns to
 *     skim it;
 *   - `amend` is offered on the `HEAD` row and nowhere else. Amend is the one action that
 *     rewrites a commit, and a `head` that failed to match would put it on all of them;
 *   - every `GitError` the commit actions can raise becomes a sentence. A `GitError` is a tagged
 *     object, so a UI that prints `String(error)` shows `[object Object]` — the failure
 *     `check-branches.mjs` exists for, extended here to the fifteen new tags. The list is
 *     enumerated so a *missing* arm fails here rather than in front of a user;
 *   - and the dialog itself still focuses Cancel, still accents Cancel rather than the
 *     destructive button, and draws the radio group only for a caller that asked for one.
 *
 * Same harness as `check-branches.mjs` and `check-sidebar.mjs`: there is no JS test runner in
 * this project, `logActions.ts` and `branchModel.ts` are import-free on purpose, and the
 * TypeScript in `node_modules` compiles them standalone. The component is checked by reading its
 * source, which `check-sidebar.mjs` does for `tokens.css` and for `crates/cide-ipc/src/settings.rs`
 * — a regex over source is a weak assertion in general and a strong one for "this className is
 * not on that button", which is the whole of rule 3 in the DOM.
 *
 * The **fourth** section compiles a probe against the real component types. That is the one
 * assertion that proves `logActions.ts` extends `ConfirmDestructive` rather than forking it: the
 * choices it writes have to be assignable to `ConfirmChoice`, its wording half has to spread
 * into a `ConfirmState`, and a legacy zero-argument `run` has to still assign *and* still be
 * callable with no arguments after the widening.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a reset resets. `crates/cide-git/tests/commit_actions.rs` owns that, against real
 *     repositories, differentially against `git`.
 *   - that the radio group paints, that clicking it changes the body, or that Escape reaches the
 *     handler. There is no DOM in this process.
 *   - that the sentences are *good*. They are pinned to be present, distinct, non-empty, and to
 *     name the specific things a user has to check — a count, a path, an oid. Prose is reviewed
 *     by people.
 *
 * Run: `node scripts/check-log-actions.mjs` from `ui/` (or `pnpm --dir ui run check:log-actions`).
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-log-actions-'))

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

/**
 * Source with comments removed.
 *
 * A grep over raw source matches the *explanation* of a rule as happily as the rule, so an
 * assertion written that way stays green after the code is deleted and only the prose is left.
 * That is not hypothetical here: `ConfirmDestructive.tsx` carries a comment that says the words
 * "Never `buttonPrimary`" directly above the button this file checks does not carry it. Copied
 * from `check-branches.mjs`, which copied it from `check-scratch.mjs`, for the same reason.
 * String-aware, so a `'//'` inside a literal does not eat the rest of the line.
 */
const stripComments = (source) => {
  let result = ''
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
      result += ch
      i++
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') {
          result += source[i]
          i++
        }
        if (i < source.length) {
          result += source[i]
          i++
        }
      }
      result += quote
      i++
      continue
    }
    result += ch
    i++
  }
  return result
}

try {
  // --- 1. the wire shape, pinned against the generated types --------------------------------
  //
  // `logActions.ts` declares every wire type structurally — it may not import them and stay
  // standalone-compilable — so a rename in `crates/cide-ipc/src/git.rs` that `cargo xtask
  // codegen` propagates would typecheck on both sides and produce `undefined` in a dialog. This
  // is the join that catches it, and it is the arrangement `check-branches.mjs` settled on for
  // `FetchOutcome`.

  const generated = readFileSync(join(UI, 'src', 'ipc', 'generated.ts'), 'utf8')
  const shape = (name) => {
    const at = generated.indexOf(`export type ${name} = `)
    if (at < 0) return ''
    const end = generated.indexOf('};', at)
    return generated.slice(at, end < 0 ? generated.length : end)
  }
  for (const [type, fields] of [
    ['CommitRow', ['repo', 'oid', 'shortOid', 'summary', 'author', 'parents']],
    [
      'ResetPreview',
      [
        'head',
        'detached',
        'headOid',
        'targetOid',
        'targetSummary',
        'commitsDropped',
        'commitsGained',
        'staged',
        'dirty',
        'untrackedKept',
        'useStagingArea',
      ],
    ],
    ['ResetOutcome', ['kind', 'headBefore', 'headAfter', 'commitsDropped', 'filesDiscarded', 'shelved']],
    ['ReplayOutcome', ['op', 'source', 'created', 'summary', 'files']],
    ['DetachOutcome', ['head', 'summary', 'previous', 'stashed', 'restoreFailed']],
    ['TagOutcome', ['name', 'oid', 'annotated', 'moved']],
    ['ShelfEntry', ['id', 'name', 'files']],
    ['PulledCommit', ['shortOid', 'summary', 'author']],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still on the wire`)
    }
  }
  // The two string enums whose *values* this module hard-codes. `resetChoices` uses them as
  // choice ids and the caller passes the selected id straight to `git_reset`, so a Rust rename
  // here would send a kind the backend rejects.
  ok(
    /export type ResetKind = "soft" \| "mixed" \| "hard";/.test(generated),
    'ResetKind is still soft/mixed/hard — those strings are the radio ids and the wire value',
  )
  ok(
    /export type ReplayOp = "revert" \| "cherryPick";/.test(generated),
    'ReplayOp is still revert/cherryPick — `replayNote` and `explain` both switch on it',
  )

  // --- 2. the fifteen new GitError tags exist -----------------------------------------------
  //
  // Enumerated rather than derived, deliberately. Deriving the list from `generated.ts` would
  // make the check pass the moment a variant is deleted in Rust, which is the opposite of what
  // it is for: this list is the contract the log's error handling was written against, and a
  // variant leaving it should be a decision somebody makes here.

  const NEW_TAGS = [
    'notHead',
    'replayWouldConflict',
    'mergeNeedsMainline',
    'notAMerge',
    'noSuchCommit',
    'emptyReplay',
    'tagExists',
    'invalidTagName',
    'notTracked',
    'fileTooLarge',
    'badRevspec',
    'notACommit',
    'ambiguousRev',
    'staleLogCursor',
    'noSuchRevision',
  ]
  const gitError = shape('GitError')
  for (const tag of NEW_TAGS) {
    ok(new RegExp(`"kind": "${tag}"`).test(generated), `GitError carries ${tag}`)
  }
  void gitError

  // --- 3. compile the two models on their own -----------------------------------------------

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
        // `src`, not `src/chrome`: `branchModel.ts` reaches `@/ipc/generated` through the alias,
        // and tsc requires every source file to sit under `rootDir`.
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        // No ambient types at all. This is what "import-free" is worth: if either module ever
        // grows a value import, or reaches for `document`, this compile is the thing that says
        // so — with a resolution error, before any assertion below runs.
        types: [],
      },
      files: [
        join(UI, 'src', 'chrome', 'logActions.ts'),
        // Compiled beside it because half of this round's wording is `explain`'s new arms, and
        // a refusal that renders `[object Object]` is the same defect as a dialog that names the
        // wrong files. Two modules, one gesture, one check.
        join(UI, 'src', 'chrome', 'branchModel.ts'),
      ],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const A = await import(`file://${join(out, 'chrome', 'logActions.js')}`)
  const B = await import(`file://${join(out, 'chrome', 'branchModel.js')}`)

  // --- fixtures ------------------------------------------------------------------------------

  /** A full forty-hex oid and the eight characters a sentence is allowed to show of it. */
  const FULL = 'a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0'
  const SHORT = 'a1b2c3d4'
  const OTHER = '0f1e2d3c4b5a69788796a5b4c3d2e1f009182736'

  const preview = (over = {}) => ({
    head: 'main',
    detached: false,
    headOid: OTHER,
    targetOid: 'a1b2c3d',
    targetSummary: 'fix the parser',
    commitsDropped: 3,
    commitsGained: 0,
    staged: ['src/a.rs', 'src/b.rs'],
    dirty: ['src/a.rs', 'src/b.rs', 'src/c.rs'],
    untrackedKept: 0,
    useStagingArea: false,
    ...over,
  })

  const row = (over = {}) => ({
    oid: FULL,
    shortOid: SHORT,
    summary: 'fix the parser',
    parents: [OTHER],
    ...over,
  })

  // --- which actions a row offers --------------------------------------------------------------

  const EVERYTHING = ['revert', 'cherryPick', 'reset', 'amend', 'tag', 'branch', 'detach']
  const WITHOUT_AMEND = EVERYTHING.filter((a) => a !== 'amend')

  eq(A.actionsFor(row(), FULL), EVERYTHING, 'the HEAD row offers all seven actions, amend included')
  eq(
    A.actionsFor(row({ oid: OTHER, shortOid: '0f1e2d3c' }), FULL),
    WITHOUT_AMEND,
    'every other row offers the same six — amend is the only line that is ever missing',
  )
  eq(
    A.actionsFor(row(), FULL).filter((a) => !WITHOUT_AMEND.includes(a)),
    ['amend'],
    'and the difference between the two lists is exactly `amend`, nothing else moved',
  )
  ok(
    A.actionsFor(row(), SHORT).includes('amend'),
    'a HEAD given as a short oid still matches its row — `BranchInfo.head` is abbreviated when ' +
      'HEAD is detached, which is exactly the state you are in after checking a commit out',
  )
  ok(
    A.actionsFor(row(), FULL.slice(0, 7)).includes('amend'),
    'so does a seven-character abbreviation, which is git\'s own default width',
  )
  ok(
    !A.actionsFor(row(), '').includes('amend'),
    'an empty HEAD matches nothing — otherwise `startsWith("")` would put Amend on every row, ' +
      'and amend is the one action that rewrites a commit',
  )
  ok(
    !A.actionsFor(row(), 'a').includes('amend'),
    'and neither does a one-character prefix, for the same reason — MIN_ABBREV is the floor',
  )
  eq(A.MIN_ABBREV, 7, 'the prefix floor is git\'s default abbreviation width')
  ok(A.isMerge(row({ parents: [OTHER, FULL] })), 'two parents is a merge')
  ok(!A.isMerge(row()), 'and one is not')

  // --- the reset dialog: title and body ----------------------------------------------------------

  eq(
    A.resetTitle(preview()),
    'Reset main to a1b2c3d?',
    'the title names the branch that moves and where it lands',
  )
  eq(
    A.resetBody(preview()),
    '“fix the parser” — 3 commits would be undone.',
    'the body leads with the target’s message, because an oid is not something anyone can check',
  )
  eq(
    A.resetBody(preview({ commitsDropped: 1 })),
    '“fix the parser” — 1 commit would be undone.',
    'and the singular is right — "1 commits" in a destructive dialog survives for years',
  )

  // The forward move. `ResetPreview.commits_gained` is non-zero whenever the target is not an
  // ancestor, which is what a reset *onto* a tip you are behind looks like — and the dropping
  // sentence would report "0 commits would be undone" for it.
  const forward = preview({ commitsDropped: 0, commitsGained: 2 })
  eq(
    A.resetBody(forward),
    '“fix the parser” — main moves forward 2 commits.',
    'a forward reset says so rather than reporting nothing undone',
  )
  ok(
    A.resetBody(forward) !== A.resetBody(preview()),
    'and it is a different sentence from the dropping case — the whole point of having two',
  )
  ok(
    !A.resetBody(forward).includes('undone'),
    'the forward sentence does not claim anything was undone',
  )
  const sideways = preview({ commitsDropped: 3, commitsGained: 2 })
  ok(
    A.resetBody(sideways).includes('3 commits') && A.resetBody(sideways).includes('2 commits'),
    'a sideways move names both numbers — either one alone describes half of what happens',
  )
  ok(
    A.resetBody(sideways) !== A.resetBody(preview()) && A.resetBody(sideways) !== A.resetBody(forward),
    'and is a third sentence, distinct from both',
  )
  ok(
    A.resetBody(preview({ commitsDropped: 0, commitsGained: 0 })).length > 0,
    'a reset that moves no commits still says something — it is how you discard everything',
  )
  eq(
    A.resetTitle(preview({ detached: true, head: '0f1e2d3c' })),
    'Reset detached HEAD to a1b2c3d?',
    'with no branch to move the title says so, rather than reading as though the commit changes',
  )
  ok(
    A.resetTitle(preview({ targetOid: FULL })).includes(SHORT)
      && !A.resetTitle(preview({ targetOid: FULL })).includes(FULL),
    'a full forty-hex target is abbreviated in the sentence, never printed whole',
  )
  eq(
    A.resetBody(preview({ targetSummary: '' })),
    '“(no summary)” — 3 commits would be undone.',
    'an empty commit message gets a stand-in — `“”` mid-sentence reads as a rendering bug',
  )

  // --- the three modes ------------------------------------------------------------------------------

  const choices = A.resetChoices(preview())
  eq(choices.length, 3, 'a reset has exactly three modes')
  eq(
    choices.map((c) => c.id),
    ['soft', 'mixed', 'hard'],
    'in safest-first order, and the ids are the wire’s ResetKind values — `ConfirmDestructive` ' +
      'falls back to choices[0], so a reordering silently makes --hard the default answer',
  )
  eq(choices.map((c) => c.label), ['Soft', 'Mixed', 'Hard'], 'labelled as git names them')

  eq(
    choices.map((c) => c.files),
    [[], ['src/a.rs', 'src/b.rs'], ['src/a.rs', 'src/b.rs', 'src/c.rs']],
    'rule 1 holds per mode: --soft risks nothing, --mixed the staged paths, --hard the dirty ones',
  )
  eq(
    choices.map((c) => c.danger),
    [false, false, true],
    'and only --hard is red. `--red` means "this destroys work" in exactly one place in this ' +
      'app, and spending it on a --soft reset is how it stops meaning anything on a --hard one',
  )
  ok(
    choices.every((c) => c.body.length > 0 && c.confirmLabel.length > 0),
    'every mode has a sentence and a button label',
  )
  ok(
    new Set(choices.map((c) => c.body)).size === 3,
    'and the three sentences are three different sentences',
  )
  ok(
    new Set(choices.map((c) => c.confirmLabel)).size === 3,
    'as are the three buttons — a dialog whose button says the same thing in all three modes is ' +
      'one where the radio looks decorative',
  )

  const byId = (list, id) => list.find((c) => c.id === id)

  eq(
    byId(choices, 'soft').body,
    'Keep everything. main moves back 3 commits and their changes stay staged.',
    'Soft: nothing is lost and the sentence says which direction the branch goes',
  )
  eq(
    byId(A.resetChoices(forward), 'soft').body,
    'Keep everything. main moves forward 2 commits and the difference stays staged.',
    'and it follows the direction of travel rather than always saying "back"',
  )

  // --- Mixed, which is the mode whose cost depends on a setting --------------------------------------
  //
  // The gap is not cosmetic. In staging-area mode the index is the user's own work — they ran
  // `git add -p` and picked hunks, and git keeps no record of what was staged, so a mixed reset
  // destroys the selection with no undo. In changelist mode the index is derived: `cide_git`
  // rebuilds it from the changelists before every commit, so the same act costs a recomputation
  // the panel performs anyway. One sentence for both would have to be vague enough to be true of
  // the destructive case, and every changelist user would learn to skim it.

  const changelist = byId(A.resetChoices(preview({ useStagingArea: false })), 'mixed')
  const stagingArea = byId(A.resetChoices(preview({ useStagingArea: true })), 'mixed')
  eq(
    changelist.body,
    'Keep your files. main moves back 3 commits; nothing you have selected in the panel is affected.',
    'changelist mode: the selection survives, and the sentence says so',
  )
  eq(
    stagingArea.body,
    'Keep your files, unstage everything. 2 staged files go back to unstaged and the selection is lost.',
    'staging-area mode: the selection does not survive, and the sentence names how much of it',
  )
  ok(
    changelist.body !== stagingArea.body,
    'the two Mixed bodies differ — this is the assertion `ResetPreview.use_staging_area` exists for',
  )
  ok(
    changelist.body.length > 0 && stagingArea.body.length > 0,
    'and neither of them is empty',
  )
  ok(
    changelist.confirmLabel !== stagingArea.confirmLabel,
    'the button differs too: "keep my files" and "unstage" are not the same promise',
  )
  eq(
    byId(A.resetChoices(preview({ useStagingArea: true, staged: [] })), 'mixed').body,
    'Keep your files, unstage everything. Nothing is staged right now, so this costs nothing beyond moving main.',
    'nothing staged says so, rather than "0 staged files go back to unstaged"',
  )
  eq(
    byId(A.resetChoices(preview({ useStagingArea: true, staged: ['src/a.rs'] })), 'mixed').body,
    'Keep your files, unstage everything. 1 staged file goes back to unstaged and the selection is lost.',
    'and the singular agrees with its verb',
  )
  ok(
    !byId(A.resetChoices(preview({ useStagingArea: true })), 'mixed').danger,
    'Mixed is not red even in staging-area mode: it loses a selection, not a file',
  )

  // --- Hard, the only one with no route back --------------------------------------------------------

  eq(
    byId(choices, 'hard').body,
    'DISCARD 3 changed files. There is no undo for this in git.',
    'Hard names the count and says plainly that git cannot bring the contents back',
  )
  ok(
    byId(choices, 'hard').body.includes('3'),
    'the count is in the sentence as well as in the list — a user who skims reads one of the two',
  )
  ok(
    !byId(choices, 'hard').body.toLowerCase().includes('untracked'),
    'and untracked files are not mentioned when there are none to reassure anyone about',
  )
  eq(
    byId(A.resetChoices(preview({ untrackedKept: 2 })), 'hard').body,
    'DISCARD 3 changed files. There is no undo for this in git. 2 untracked files are left alone.',
    'with untracked files present the fear is answered, positively and with a number: the most ' +
      'common worry at this dialog is that new files are about to vanish, and they are not',
  )
  eq(
    byId(A.resetChoices(preview({ untrackedKept: 1 })), 'hard').body,
    'DISCARD 3 changed files. There is no undo for this in git. 1 untracked file is left alone.',
    'singular again',
  )
  eq(
    byId(A.resetChoices(preview({ dirty: ['only.rs'] })), 'hard').confirmLabel,
    'Discard 1 file',
    'the button counts what it is about to destroy, in the register of `Revert 4 files`',
  )
  ok(
    byId(A.resetChoices(preview({ dirty: [] })), 'hard').danger,
    'Hard stays red with a clean tree: the colour belongs to the mode, which overwrites the ' +
      'working tree with no undo, not to whatever happened to be dirty when the dialog opened',
  )
  ok(
    !byId(A.resetChoices(preview({ dirty: [] })), 'hard').body.includes('DISCARD 0'),
    'and it does not shout about discarding nothing',
  )

  // --- the shelve-first option -------------------------------------------------------------------

  eq(A.SHELVE_FIRST_DEFAULT, true, 'the recoverable answer is the one a reflexive click produces')
  ok(A.SHELVE_FIRST_LABEL.length > 0, 'and it has a label')
  ok(
    A.shelveFirstOffered(preview(), 'hard'),
    'the option is offered for --hard with a dirty tree, which is the only case it can rescue',
  )
  ok(!A.shelveFirstOffered(preview(), 'soft'), 'not for --soft, which leaves the tree alone')
  ok(!A.shelveFirstOffered(preview(), 'mixed'), 'nor for --mixed, for the same reason')
  ok(
    !A.shelveFirstOffered(preview({ dirty: [] }), 'hard'),
    'nor with a clean tree — a control that never does anything is one the user stops reading ' +
      'before the mode where it matters',
  )
  ok(
    !A.shelveFirstOffered(preview(), null),
    'and not before a mode is chosen, since the fallback selection is the safe one',
  )

  // --- checking a commit out -----------------------------------------------------------------------

  const clean = A.detachConfirm(row(), [])
  const blocked = A.detachConfirm(row(), ['src/a.rs', 'src/b.rs'])
  eq(clean.title, `Check out ${SHORT}?`, 'the title names the commit')
  eq(clean.files, [], 'with nothing in the way, nothing is listed')
  eq(blocked.files, ['src/a.rs', 'src/b.rs'], 'and with blockers, every one of them is')
  ok(
    clean.body !== blocked.body,
    'the two cases ask different questions: "did you mean to leave your branch" and "these get stashed"',
  )
  ok(
    clean.body.toLowerCase().includes('detached'),
    'the no-blockers body names the state you are about to be in — the one users reach by ' +
      'accident and cannot leave without knowing where they came from',
  )
  eq(
    clean.mark,
    'arrow-up-right',
    'the mark is not the removal dash: nothing here is deleted, and a minus over files that ' +
      'are about to be stashed and put back is a sentence the dialog contradicts',
  )
  eq(
    blocked.mark,
    'arrow-up-right',
    'both cases, or the two halves of one gesture would look like two gestures',
  )
  ok(
    clean.confirmLabel !== blocked.confirmLabel && blocked.confirmLabel.toLowerCase().includes('stash'),
    'and the button says when a stash is part of the deal',
  )

  // --- moving a tag --------------------------------------------------------------------------------

  const moved = A.forceTagConfirm('v1.2.0', OTHER, FULL)
  ok(moved.title.includes('v1.2.0'), 'the tag is named in the title')
  ok(
    moved.body.includes('0f1e2d3c') && moved.body.includes(SHORT),
    'and the body names both ends — where it points now and where it would point',
  )
  ok(
    !moved.body.includes(FULL) && !moved.body.includes(OTHER),
    'abbreviated, never the full forty',
  )
  eq(moved.files, [], 'no files are at stake: what is at stake is a ref other people may have')
  eq(moved.mark, 'arrow-right', 'a move, not a removal and not a departure')
  ok(moved.confirmLabel.includes('v1.2.0'), 'the button names the tag it is about to move')

  // --- which side of a merge ------------------------------------------------------------------------

  const mainlineError = {
    kind: 'mergeNeedsMainline',
    detail: {
      oid: 'a1b2c3d',
      parents: [
        { shortOid: '11111111', summary: 'release 1.2', author: 'Ada Lovelace' },
        { shortOid: '22222222', summary: 'add the parser', author: 'Grace Hopper' },
      ],
    },
  }
  const parents = A.mainlineChoices(mainlineError)
  eq(parents.length, 2, 'one choice per parent')
  eq(parents.map((c) => c.id), ['1', '2'], 'numbered the way `git revert -m` numbers them: 1-based')
  ok(
    parents[0].label.includes('Parent 1') && parents[0].label.includes('11111111'),
    'each is labelled by its position and its oid',
  )
  ok(
    parents[1].label.includes('Parent 2') && parents[1].label.includes('22222222'),
    'including the second',
  )
  ok(
    parents[0].body.includes('release 1.2') && parents[0].body.includes('Ada Lovelace'),
    'the body is the parent’s own summary and author — an oid pair does not tell two sides apart',
  )
  ok(
    parents[1].body.includes('add the parser') && parents[1].body.includes('Grace Hopper'),
    'for the second as well',
  )
  eq(parents.map((c) => c.files), [[], []], 'picking a parent puts no file at risk')
  eq(parents.map((c) => c.danger), [false, false], 'so neither choice is red')
  ok(
    new Set(parents.map((c) => c.body)).size === 2 && new Set(parents.map((c) => c.confirmLabel)).size === 2,
    'and the two are distinguishable, which is the entire purpose of the picker',
  )
  ok(A.MAINLINE_TITLE.length > 0, 'the picker has a title')
  ok(
    A.mainlineBody('a1b2c3d').includes('a1b2c3d'),
    'and a body that names the merge and explains why it is asking — "mainline" is git jargon',
  )

  for (const [what, value] of [
    ['a different GitError', { kind: 'notAMerge', detail: { oid: 'a1b2c3d' } }],
    ['null', null],
    ['a string', 'boom'],
    ['an Error', new Error('nope')],
    ['an object with no kind', {}],
    ['a mergeNeedsMainline with no parents', { kind: 'mergeNeedsMainline', detail: { oid: 'x', parents: [] } }],
    ['a mergeNeedsMainline with no detail', { kind: 'mergeNeedsMainline' }],
  ]) {
    eq(A.mainlineChoices(value), null, `mainlineChoices is null for ${what}`)
  }

  // --- what a completed action says ---------------------------------------------------------------
  //
  // Never empty, unlike a checkout's note: the log looks identical after every one of these, so
  // silence is indistinguishable from a menu item that did nothing. And every one names a short
  // oid, because the oid is the only thing a user can look up afterwards.

  const replayCommit = A.replayNote({ op: 'revert', source: FULL, created: OTHER, summary: 'x', files: 4 })
  const replayTree = A.replayNote({ op: 'revert', source: FULL, created: '', summary: 'x', files: 4 })
  const cherry = A.replayNote({ op: 'cherryPick', source: FULL, created: OTHER, summary: 'x', files: 4 })
  ok(replayCommit.includes(SHORT) && replayCommit.includes('0f1e2d3c'), 'a replay names both oids')
  ok(!replayCommit.includes(FULL), 'abbreviated, never the full forty')
  ok(replayCommit.includes('4 files'), 'and how much moved')
  ok(
    replayTree !== replayCommit && replayTree.toLowerCase().includes('nothing committed'),
    'working-tree mode says out loud that there is no commit — a user who assumes there is one ' +
      'and pushes will push nothing',
  )
  ok(
    cherry !== replayCommit && cherry.startsWith('Cherry-picked') && replayCommit.startsWith('Reverted'),
    'a revert and a cherry-pick are told apart by their first word',
  )

  const resetHard = A.resetNote({
    kind: 'hard',
    headBefore: OTHER,
    headAfter: FULL,
    commitsDropped: 3,
    filesDiscarded: 5,
    shelved: { name: 'wip' },
  })
  ok(resetHard.includes(SHORT), 'a reset names where HEAD landed')
  ok(
    resetHard.includes('0f1e2d3c') && resetHard.toLowerCase().includes('reflog'),
    'and — always — where it came from, plus the word "reflog": `git reset` has no porcelain ' +
      'undo, but HEAD@{1} is the old tip and it is the thing a user needs after the toast is gone',
  )
  ok(resetHard.includes('3 commits') && resetHard.includes('5 files'), 'with both counts')
  ok(resetHard.includes('wip'), 'and the shelf entry, when one was made')
  const resetSoft = A.resetNote({
    kind: 'soft',
    headBefore: OTHER,
    headAfter: FULL,
    commitsDropped: 3,
    filesDiscarded: 0,
    shelved: null,
  })
  ok(resetSoft !== resetHard, 'a soft reset reads differently from a hard one')
  ok(
    !resetSoft.includes('discarded') && !resetSoft.includes('shelved'),
    'and claims neither a discard nor a shelf that did not happen',
  )
  ok(resetSoft.toLowerCase().includes('reflog'), 'but still says where it came from')

  const detached = A.detachNote({
    head: SHORT,
    summary: 'fix the parser',
    previous: 'main',
    stashed: null,
    restoreFailed: null,
  })
  ok(detached.includes(SHORT) && detached.includes('fix the parser'), 'a detach names where you are')
  ok(
    detached.includes('main'),
    'and where you came from — by the time you want to leave, the branch selector says ' +
      '"a1b2c3d4 detached" and nothing on screen remembers the branch',
  )
  const stashedNote = A.detachNote({
    head: SHORT,
    summary: 'fix the parser',
    previous: 'main',
    stashed: 'WIP on main',
    restoreFailed: null,
  })
  ok(
    stashedNote !== detached && stashedNote.includes('WIP on main') && stashedNote.includes('main'),
    'a stash is named, and the way back is still there',
  )
  eq(
    A.detachNote({
      head: SHORT,
      summary: 's',
      previous: 'main',
      stashed: 'WIP',
      restoreFailed: 'could not restore: conflict in src/a.rs',
    }),
    'could not restore: conflict in src/a.rs',
    'a failed restore is git’s own message, alone — anything wrapped round it pushes the part ' +
      'that matters off the end of a one-line note',
  )

  const tagged = A.tagNote({ name: 'v1.2.0', oid: FULL, annotated: true, moved: false })
  const removed = A.tagNote({ name: 'v1.2.0', oid: FULL, annotated: false, moved: true })
  ok(tagged.includes(SHORT) && !tagged.includes(FULL), 'a tag note names the short oid')
  ok(tagged.includes('v1.2.0') && removed.includes('v1.2.0'), 'and the tag')
  ok(
    tagged !== removed,
    'created and moved are different sentences: after the fact the note is the only record that ' +
      'a force happened at all',
  )
  ok(
    tagged.includes('annotated') && removed.includes('lightweight'),
    'and the kind is stated, because it is invisible afterwards and decides whether ' +
      '`git describe` can see the tag',
  )

  const notes = [replayCommit, resetHard, detached, tagged]
  ok(notes.every((n) => n.length > 0), 'no note is empty')
  ok(new Set(notes).size === notes.length, 'and no two of the four say the same thing')
  ok(notes.every((n) => n.includes(SHORT)), 'every one of them names the short oid')

  // --- every new GitError becomes a sentence ---------------------------------------------------------
  //
  // The defect this whole family of assertions exists for: a `GitError` on the wire is
  // `{kind, detail}`, so `String(error)` is `[object Object]` and the control appears to have
  // done nothing. Driven with real `{kind, detail}` objects — not with tags — because that is
  // the shape a rejection actually has, and an arm that reads the wrong detail key is invisible
  // to a test that passes it a bare string.

  const DETAIL = {
    notHead: { oid: 'a1b2c3d', head: '9f8e7d6c' },
    replayWouldConflict: { op: 'cherryPick', oid: 'a1b2c3d', paths: ['src/main.rs', 'src/lib.rs'] },
    mergeNeedsMainline: {
      oid: 'a1b2c3d',
      parents: [{ shortOid: '11111111', summary: 's', author: 'a' }],
    },
    notAMerge: { oid: 'a1b2c3d' },
    noSuchCommit: { rev: 'a1b2c3d' },
    emptyReplay: { op: 'revert', oid: 'a1b2c3d' },
    tagExists: { name: 'v1.2.0', oid: '9f8e7d6c' },
    invalidTagName: { name: 'v 1.2' },
    notTracked: { path: 'src/new.rs' },
    fileTooLarge: { path: 'src/big.bin', bytes: 12582912, limit: 5242880 },
    badRevspec: { spec: 'main@{yesterday}', detail: 'unknown revision' },
    notACommit: { spec: 'v1.2.0', kind: 'tree' },
    ambiguousRev: { spec: 'a1b2' },
    staleLogCursor: undefined,
    noSuchRevision: { rev: 'HEAD~99' },
  }

  const sentences = new Map()
  for (const tag of NEW_TAGS) {
    const detail = DETAIL[tag]
    const error = detail === undefined ? { kind: tag } : { kind: tag, detail }
    const said = B.explain(error)
    sentences.set(tag, said)

    ok(typeof said === 'string' && said.length > 0, `explain(${tag}) says something`)
    ok(
      said !== `git: ${tag}`,
      `explain(${tag}) has an arm of its own rather than falling through to the bare tag — a ` +
        `listed-and-unexplained variant is the state this assertion makes unrepresentable`,
    )
    ok(!said.includes('[object Object]'), `explain(${tag}) never renders [object Object]`)
    ok(!said.includes('undefined'), `explain(${tag}) never renders "undefined"`)
    ok(said !== tag, `explain(${tag}) is a sentence, not the tag`)

    /*
     * `explain`'s missing-field fallback is the literal `?`, so a stray one is an arm asking for
     * a key the wire does not carry under that name — a rename in `crates/cide-ipc/src/git.rs`
     * arriving as a shrug in the middle of a sentence.
     *
     * `invalidTagName` is exempt and cannot not be: `?` is one of the characters git forbids in
     * a refname, and the sentence lists them. Its detail is pinned by `says()` below instead.
     */
    if (tag !== 'invalidTagName') {
      ok(!said.includes('?'), `explain(${tag}) reads every detail key it was given: ${said}`)
    }

    /*
     * The same property from the other side, and the one that has no exemptions: strip the
     * detail and the sentence must change. An arm that ignores its payload — pasted from a
     * neighbour and never re-read — passes every assertion above and tells the user nothing
     * they can act on.
     *
     * `staleLogCursor` carries no detail on the wire at all, so there is nothing to strip.
     */
    if (detail !== undefined) {
      ok(
        B.explain({ kind: tag }) !== said,
        `explain(${tag}) actually reads its detail rather than printing a fixed sentence`,
      )
    }
  }
  ok(
    new Set(sentences.values()).size === NEW_TAGS.length,
    'and all fifteen sentences are different from each other',
  )

  // The details that have to survive the trip, variant by variant. These are the values the user
  // acts on: a path they go and look at, an oid they type, a limit they plan around.
  const says = (tag, ...fragments) => {
    const said = sentences.get(tag) ?? ''
    for (const fragment of fragments) {
      ok(said.includes(fragment), `explain(${tag}) names ${JSON.stringify(fragment)}: ${said}`)
    }
  }
  says('notHead', 'a1b2c3d', '9f8e7d6c', 'rebase')
  // `terminal` is gone from this sentence, and its absence is the assertion. (M20) The variant
  // is unreachable now — `cide_git::replay` lands the conflict and the resolver finishes it —
  // but the sentence has to survive for an old payload, and it must not go on telling people
  // to leave the app for something the app now does. The paths still matter: they are what the
  // user acts on either way.
  says('replayWouldConflict', 'Cherry-picking', 'a1b2c3d', 'src/main.rs', 'src/lib.rs')
  ok(
    !(sentences.get('replayWouldConflict') ?? '').includes('terminal'),
    'explain(replayWouldConflict) no longer sends the user to a terminal',
  )
  says('mergeNeedsMainline', 'a1b2c3d', 'merge')
  says('notAMerge', 'a1b2c3d')
  says('noSuchCommit', 'a1b2c3d')
  says('emptyReplay', 'Reverting', 'a1b2c3d')
  says('tagExists', 'v1.2.0', '9f8e7d6c')
  says('invalidTagName', 'v 1.2')
  says('notTracked', 'src/new.rs')
  says('fileTooLarge', 'src/big.bin', '12.0 MB', '5.0 MB')
  says('badRevspec', 'main@{yesterday}', 'unknown revision')
  says('notACommit', 'v1.2.0', 'tree')
  says('ambiguousRev', 'a1b2')
  says('noSuchRevision', 'HEAD~99')
  ok(
    (sentences.get('staleLogCursor') ?? '').toLowerCase().includes('refresh'),
    'staleLogCursor carries no detail, so its whole content is the instruction — say it',
  )

  // The op is read, not assumed. Reverting and cherry-picking fail identically in Rust — one
  // enum, one code path — and the sentence is the only place the two are told apart.
  ok(
    B.explain({ kind: 'replayWouldConflict', detail: { op: 'revert', oid: 'a1b2c3d', paths: ['a'] } })
      .startsWith('Reverting'),
    'a conflicting revert says "Reverting", not "Cherry-picking"',
  )
  ok(
    B.explain({ kind: 'emptyReplay', detail: { op: 'cherryPick', oid: 'a1b2c3d' } })
      .startsWith('Cherry-picking'),
    'and an empty cherry-pick says "Cherry-picking"',
  )
  ok(
    B.explain({ kind: 'replayWouldConflict', detail: { op: 'revert', oid: 'x', paths: [] } }).length > 0,
    'a conflict with no paths on it still produces a sentence rather than a dangling " in "',
  )
  ok(
    !B.explain({ kind: 'replayWouldConflict', detail: { op: 'revert', oid: 'x', paths: [] } })
      .includes(' in .'),
    '…and specifically not a dangling one',
  )
  ok(
    B.explain({
      kind: 'replayWouldConflict',
      detail: { op: 'revert', oid: 'x', paths: ['a', 'b', 'c', 'd', 'e'] },
    }).includes('2 more'),
    'a forty-file conflict is capped with a count rather than an ellipsis — "and 37 more" is a ' +
      'fact you can act on and "a, b, c…" is not',
  )
  ok(
    B.explain({ kind: 'fileTooLarge', detail: { path: 'p', bytes: 12582912n, limit: 5242880n } })
      .includes('12.0 MB'),
    'a bigint size is read too: ts-rs maps u64 to bigint while serde_json writes a plain number, ' +
      'so which one arrives depends on the transport having no reviver',
  )

  // The fallback is still there for a tag this file has never heard of, which is what makes the
  // assertion above ("not the bare tag") mean something.
  eq(
    B.explain({ kind: 'somethingNewInRust', detail: {} }),
    'git: somethingNewInRust',
    'an unrecognised variant is still shown, with its tag, rather than swallowed',
  )

  // --- 4. the dialog kept its three rules -----------------------------------------------------------
  //
  // Read from source, because there is no DOM here. Weak in general; strong for exactly this —
  // "the radio group is behind a guard", "this className is not on that button" — which is what
  // rules 1 and 3 reduce to once they reach the JSX. `check-sidebar.mjs` reads `tokens.css` and
  // `crates/cide-ipc/src/settings.rs` the same way.

  const rawDialog = readFileSync(join(UI, 'src', 'chrome', 'ConfirmDestructive.tsx'), 'utf8')
  const dialog = stripComments(rawDialog)

  // Rule 3, both halves — the focus, and the colour — plus its one declared exception: the
  // default is Cancel's unless the caller set `defaultButton: 'confirm'`, which only the file
  // tree's trash move (reversible by construction) may do. Focus and accent must travel
  // together whichever way the default points.
  ok(
    /\(confirmDefault \? confirmBtn : cancel\)\.current\?\.focus\(\)/.test(dialog),
    'the initial focus follows the declared default — Cancel unless the caller flipped it, so ' +
      'the keystroke already in flight when a *destructive* dialog appeared backs out',
  )
  ok(
    /const confirmDefault = state\.defaultButton === 'confirm'/.test(dialog),
    'and a caller that says nothing gets Cancel as the default',
  )
  ok(
    !stripComments(readFileSync(join(UI, 'src', 'chrome', 'logActions.ts'), 'utf8')).includes(
      'defaultButton',
    ),
    'and no log action flips it — reset, checkout and a moved tag have no undo, so for all of ' +
      'them the reflexive Enter must back out, not go ahead',
  )
  ok(
    /useEffect\(/.test(dialog) && /ref=\{cancel\}/.test(dialog) && /ref=\{confirmBtn\}/.test(dialog),
    'and both refs are on real buttons',
  )
  ok(
    /ev\.key === 'Escape'/.test(dialog) && /onCancel\(\)/.test(dialog),
    'Escape still cancels',
  )

  const before = (marker, span) => {
    const at = dialog.indexOf(marker)
    return at < 0 ? '' : dialog.slice(Math.max(0, at - span), at)
  }
  const runButton = before('data-audit="confirmDestructiveRun"', 260)
  const cancelButton = before('data-audit="confirmDestructiveCancel"', 260)
  ok(runButton !== '', 'the destructive button is still identifiable')
  ok(
    runButton.includes("variant={confirmDefault ? 'primary' : danger ? 'danger' : 'secondary'}"),
    'the accent reaches the confirm button ONLY behind `confirmDefault`. This one variant is ' +
      'rule 3 in the DOM: for every destructive caller the prominent button has to be Cancel, ' +
      'or a reflexive Enter performs the destruction — and where a caller did declare the act ' +
      'reversible, the accent displaces the red, because an act safe enough to confirm by ' +
      'reflex has no claim on the colour that means "this destroys work"',
  )
  ok(
    cancelButton.includes("variant={confirmDefault ? 'secondary' : 'primary'}"),
    'Cancel is the accent-filled one exactly when it is the default — the accent and the focus ' +
      'travel together, or the dialog looks like Enter will do one thing and it does the other',
  )
  ok(
    runButton.includes("'danger'"),
    'the destructive button can still wear the danger fill…',
  )
  ok(
    /danger \? 'danger'/.test(dialog),
    '…and the red is now conditional, so a --soft reset does not borrow the colour that means ' +
      '"this destroys work"',
  )
  ok(
    /const danger = active === undefined \|\| active\.danger === true/.test(dialog),
    'with the older single-mode callers still red by default — every one of them is destroying ' +
      'something, and only a mode that explicitly risks nothing gives the colour up',
  )

  // The radio group, and the guard in front of it.
  // The kit's `RadioGroup` since the redesign (2026-09-24); it is `role="radiogroup"` itself.
  const groupAt = dialog.indexOf('<RadioGroup')
  const guardAt = dialog.indexOf('{choices !== undefined && (')
  ok(groupAt > 0, 'the radio group is rendered')
  ok(guardAt > 0, 'behind an explicit `choices !== undefined` guard')
  ok(
    guardAt > 0 && groupAt > guardAt && groupAt - guardAt < 400,
    'and the guard is immediately in front of it — a radio group with one radio in it is a ' +
      'control that cannot be operated, so every caller that predates the log must draw none',
  )
  ok(
    /option !== undefined && \(/.test(dialog),
    'the option row is guarded the same way',
  )
  ok(
    // The kit draws them now, so the kit's source is where the native inputs are.
    (() => {
      const kit = stripComments(readFileSync(join(UI, 'src', 'kit', 'components', 'Choice.tsx'), 'utf8'))
      return /<RadioGroup/.test(dialog) && /<Checkbox/.test(dialog) &&
        /type="radio"/.test(kit) && /type="checkbox"/.test(kit) && /role="radiogroup"/.test(kit)
    })(),
    'both are real native inputs — the browser brings arrow-key navigation, roving tab order ' +
      'and the right screen-reader semantics, and a hand-rolled group that got any of the three ' +
      'wrong would be wrong in a dialog whose whole job is being read carefully',
  )
  ok(
    /name=\{group\}/.test(dialog) && /useId\(\)/.test(dialog),
    'and the radios group by a generated name, so two dialogs mounted at once cannot steal each ' +
      'other’s selection',
  )

  // Rule 1, per choice: the list, the sentence and the button all follow the radio together.
  for (const [what, pattern] of [
    ['body', /const body = active\?\.body \?\? state\.body/],
    ['files', /const files = active\?\.files \?\? state\.files/],
    ['confirmLabel', /const confirmLabel = active\?\.confirmLabel \?\? state\.confirmLabel/],
  ]) {
    ok(
      pattern.test(dialog),
      `the ${what} comes from the selected choice when there is one — rule 1 holds per mode, or ` +
        `the dialog names the --hard casualties while --soft is selected`,
    )
  }
  ok(
    /\{files\.map\(\(path\)/.test(dialog),
    'and the list draws that `files`, not `state.files`',
  )
  ok(
    /choices\.find\(\(c\) => c\.id === state\.chosen\) \?\? choices\[0\]/.test(dialog),
    'an unrecognised selection falls back to the first choice, which is why `resetChoices` is ' +
      'ordered least-destructive-first',
  )

  // The widening, spelled out. Required parameters here would break two existing consumers —
  // `useGitPanel.ts`'s `pending?.run()` and `FileTree.tsx`'s `closeDelete(pendingDelete.run)`,
  // which wants a bare `() => void`. Optional parameters satisfy producers and consumers at once.
  ok(
    /run: \(chosen\?: string \| null, option\?: boolean\) => void/.test(dialog),
    'both `run` parameters are optional, which is what keeps the widening assignable in BOTH ' +
      'directions — a two-required-parameter signature is not assignable to `() => void` and ' +
      'cannot be called with none',
  )

  // --- 5. and it really does extend the one dialog ----------------------------------------------------
  //
  // The assertions above are text. This one is the compiler: a probe that assigns what
  // `logActions.ts` produces into the component's own exported types. If the two ever drift —
  // a field renamed on one side, `ConfirmChoice.files` narrowed, the `run` widening reverted —
  // this fails with a type error naming the field, which no regex could.

  /*
   * The probe lives in the temp directory, not under `src/`.
   *
   * It only needs to *resolve* `@/chrome/…`, and `paths` is resolved against `baseUrl` rather
   * than against the importing file, so it works from anywhere. Writing it into `src/` would
   * have been simpler and is exactly the kind of simpler that goes wrong during a parallel run:
   * a stray file there shows up in `git status`, is a candidate for a blind `git add`, and is
   * counted by `check:casing` — all of which happen at whichever moment this check is failing
   * and the file has not been cleaned up.
   */
  const probe = join(out, 'logActionsProbe.ts')
  writeFileSync(
    probe,
    [
      "import type { ConfirmChoice, ConfirmOption, ConfirmState } from '@/chrome/ConfirmDestructive'",
      "import type { ConfirmChoiceLike, ConfirmStateLike } from '@/chrome/logActions'",
      'import {',
      '  detachConfirm, forceTagConfirm, mainlineChoices, resetBody, resetChoices, resetTitle,',
      '  SHELVE_FIRST_DEFAULT, SHELVE_FIRST_LABEL,',
      "} from '@/chrome/logActions'",
      '',
      'declare const p: Parameters<typeof resetChoices>[0]',
      'declare const r: Parameters<typeof detachConfirm>[0]',
      '',
      '// What this module writes is what the dialog draws.',
      'const modes: readonly ConfirmChoice[] = resetChoices(p)',
      'const ask: ConfirmStateLike = detachConfirm(r, [])',
      'const tag: ConfirmStateLike = forceTagConfirm("v1", "a", "b")',
      'const picked: ConfirmChoiceLike[] | null = mainlineChoices({})',
      '',
      '// The wording half spreads into a full state; the caller adds the callbacks.',
      'const plain: ConfirmState = { ...ask, run: () => {} }',
      'const withModes: ConfirmState = {',
      '  title: resetTitle(p), body: resetBody(p), files: [], confirmLabel: "",',
      '  choices: modes, chosen: "hard", onChoose: (id: string) => { void id },',
      '  option: { label: SHELVE_FIRST_LABEL, checked: SHELVE_FIRST_DEFAULT, onToggle: () => {} },',
      '  run: (chosen, option) => { void chosen; void option },',
      '}',
      'const opt: ConfirmOption = { label: "x", checked: true, onToggle: () => {} }',
      '',
      '// The widening, from both sides. Every existing producer writes a zero-argument `run`;',
      "// `useGitPanel.ts` calls it with none and `FileTree.tsx` hands it to a `() => void`.",
      'const legacy: ConfirmState = { title: "", body: "", files: [], confirmLabel: "", run: () => {} }',
      'legacy.run()',
      'const bare: (() => void) | null = legacy.run',
      '',
      'void modes; void tag; void picked; void plain; void withModes; void opt; void bare',
      '',
    ].join('\n'),
  )
  {
    const probeConfig = join(out, 'tsconfig.probe.json')
    writeFileSync(
      probeConfig,
      JSON.stringify({
        // The project's own options, minus `noEmit`, plus an `outDir` nobody reads. Copied rather
        // than `extends`-ed: `extends` would resolve `include: ["src"]` too and drag the whole
        // app — including whatever else is mid-edit — into an assertion about two files.
        compilerOptions: {
          target: 'ES2022',
          lib: ['ES2023', 'DOM', 'DOM.Iterable'],
          module: 'ESNext',
          moduleResolution: 'bundler',
          jsx: 'react-jsx',
          strict: true,
          noUncheckedIndexedAccess: true,
          noImplicitOverride: true,
          noUnusedLocals: true,
          noUnusedParameters: true,
          noFallthroughCasesInSwitch: true,
          exactOptionalPropertyTypes: true,
          verbatimModuleSyntax: true,
          skipLibCheck: true,
          isolatedModules: true,
          resolveJsonModule: true,
          allowImportingTsExtensions: true,
          emitDeclarationOnly: false,
          noEmit: true,
          baseUrl: UI,
          paths: { '@/*': ['src/*'] },
        },
        files: [join(UI, 'src', 'vite-env.d.ts'), probe],
      }),
    )
    /*
     * Caught, unlike the compile in section 3.
     *
     * That one is a prerequisite — nothing below it can run if it fails — so a stack trace is
     * the honest outcome. This one *is* an assertion, and the thing it asserts has a sentence
     * worth printing: the tsc errors are already on stderr thanks to `stdio: 'inherit'`, and
     * what a reader needs after them is which property of the design just broke.
     */
    try {
      execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', probeConfig], {
        stdio: 'inherit',
        cwd: UI,
      })
    } catch {
      fail(
        'logActions.ts still extends ConfirmDestructive rather than forking it — the probe above ' +
          'assigns what this module writes into the component’s own exported types, and one of ' +
          'those assignments no longer holds',
      )
    }
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `log-actions: ok (${EVERYTHING.length} actions, ${choices.length} reset modes, ` +
      `${NEW_TAGS.length} new GitError arms, 4 notes, ${parents.length}-parent picker)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
