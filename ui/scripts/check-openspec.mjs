/**
 * Checks `src/sidebar/OpenSpecPanel/model.ts` — the pure core of M28's panel — and pins its
 * vocabularies against the Rust that defines them.
 *
 * `check-agents.mjs`'s shape, for its reasons: this project has no JS test runner, the module is
 * deliberately import-free so the TypeScript in `node_modules` can compile it standalone, and if
 * that compile ever needs a tsconfig then something has added an import and the node-testability
 * of the core has been lost.
 *
 * # The failure classes this makes unrepresentable
 *
 * **A vocabulary that drifts from Rust.** `DeltaOperation` is an enum in
 * `crates/cide-ipc/src/spec.rs` and a frozen list here. Neither `generated.ts` nor this module
 * imports the other, so nothing else in the build can see a fifth operation arriving with no
 * label, no tone and no icon.
 *
 * **A table lookup that misses.** `check-problems.mjs`'s `ROGUE` lesson: a miss returns
 * `undefined`, or a prototype key returns `Object.prototype.constructor` — a function, which
 * React refuses as a child and which `className` stringifies into the whole source of `Object`.
 * Every member of every vocabulary must yield a non-empty **string**, and so must
 * `'constructor'`.
 *
 * **A button greyed with nothing saying why.** `primaryAction` must return exactly one of a green
 * light and a non-empty sentence, over every combination of stage, progress, validation and
 * assignee. That is `Command::unavailable` one layer down and for the identical reason. The
 * comparison itself is self-tested against results with holes punched in them, the way
 * `check-commands.mjs` self-tests `missing()` — a gate nobody has seen fail is a gate nobody
 * knows works, and this one asserts the *absence* of something, which is the shape that passes
 * vacuously when its scan breaks.
 *
 * **An issue rendered nowhere.** `issuesFor` and `unattributedIssues` partition the validator's
 * output between the requirement cards and the banner. Anything that fell out of both would be
 * reported by the validator, carried across the wire, and drawn on no screen at all — so the two
 * are asserted to cover every issue exactly once.
 *
 * **`0/0` reading as finished.** A change whose task list exists and holds no checkboxes has not
 * been planned. Reading it as ready is what would offer to archive it, and the same rule guards
 * the Review hop in Rust — both sides state it because both would be wrong the same way without.
 *
 * # What this does NOT cover
 *
 *   - that the panel renders. `check-openspec-render.mjs` does that.
 *   - that `adapt.ts` converts the wire shapes. `tsc --noEmit` over the real `adapt.ts` pins it.
 *
 * Run: `pnpm --dir ui run check:openspec`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-openspec-'))

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
  if (cond !== true) fail(what)
}

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

function rustBody(source, opening, what) {
  const start = source.indexOf(opening)
  if (start < 0) throw new Error(`could not find ${what}`)
  const end = source.indexOf('\n}', start)
  return source.slice(start, end)
}

function variants(source, opening, what) {
  const body = stripComments(rustBody(source, opening, what))
  const inner = body.slice(body.indexOf('{') + 1)
  return [...inner.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*(\{|\(|,|$)/gm)].map(
    (m) => m[1].charAt(0).toLowerCase() + m[1].slice(1),
  )
}

const sorted = (list) => [...list].sort()

/** A change, with everything the model needs and nothing it does not. */
const change = (over = {}) => ({
  name: 'add-dark-mode',
  title: 'add-dark-mode',
  deltas: [],
  artifacts: [],
  tasks: [],
  completed: 0,
  total: 0,
  validation: { valid: true, issues: [] },
  // In flight. `primaryAction` reads this *first*, so leaving it undefined sends every case
  // down the archived arm and every assertion below it reports on a change that is not there.
  archivedAs: null,
  ...over,
})

const issue = (over = {}) => ({
  level: 'ERROR',
  path: 'requirement',
  message: 'needs a scenario',
  line: 4,
  ...over,
})

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/OpenSpecPanel/model.ts',
      'src/sidebar/OpenSpecPanel/editModel.ts',
      'src/sidebar/TasksPanel/specCard.ts',
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

  // `tsc` derives the common root from the inputs (`src/sidebar`), so each file lands under its
  // own directory in `out` — which is what keeps two modules called `model.ts` apart.
  const M = await import(`file://${join(out, 'OpenSpecPanel', 'model.js')}`)
  const E = await import(`file://${join(out, 'OpenSpecPanel', 'editModel.js')}`)
  const C = await import(`file://${join(out, 'TasksPanel', 'specCard.js')}`)

  /* ------------------------------------------------------- what the card says while it works */

  {
    /*
     * `Integrate & Archive` merges a branch, archives the change, re-validates and closes the
     * task, and it shipped drawing nothing at all while it did — a card indistinguishable from
     * one that had not taken the click. The label is a *present participle* and not the resting
     * label with a spinner beside it, because a button reading `Integrate & Archive` looks
     * pressable whatever is spinning next to it.
     */
    eq(C.busyLabel('accept'), 'Integrating…', 'the accept says what it is doing')
    ok(
      !C.busyLabel('accept').includes('Integrate & Archive'),
      'and not the resting label, which reads as a live control',
    )
    // Total over the three ids: `approve` opens a picker and never waits, but a function
    // answering for two of three arms is one somebody later calls with the third.
    for (const id of ['approve', 'accept', 'none']) {
      ok(C.busyLabel(id).trim() !== '', `${id}: a label, never an empty button`)
    }
  }

  /* --------------------------------------------------------- and what it says once it is done */

  {
    /*
     * The press used to say **nothing at all**, and two of the three answers it threw away were
     * failures: `SpecAccepted::Refused` and `::Conflicts` are ordinary `Ok` arms, so neither ever
     * reached `notifyFailure`. A conflicted merge — where nothing was archived and the user has
     * work to do — and a clean accept were the same event on screen: the spinner stopped.
     *
     * Every arm must therefore produce a non-empty sentence, and the two failures must be
     * `error`. That is the shape `check-commands.mjs` asserts of a refusal and `errorText.ts` of
     * a rejection: a failure path is only ever read by the person it is failing.
     */
    const arms = [
      { kind: 'accepted', commit: 'abcdef1234567890', files: 4, change: 'add-dark-mode' },
      /*
       * **`null`, not absent, and that distinction crashed the card.**
       *
       * This arm used to omit `commit` entirely, which is not what the wire sends:
       * `SpecAccepted::Accepted::commit` is `Option<String>` with `#[ts(optional)]` and no
       * `skip_serializing_if`, so a no-op merge sends `"commit": null`. The reader checked
       * `=== undefined`, fell through, and called `.slice(0, 8)` on `null` — so pressing
       * *Integrate & Archive* on a task whose work was in the user's own checkout archived the
       * change and then reported `null is not an object`. A fixture that spelled the field the
       * way the fixture's author would have is how a check passes over a live crash.
       */
      { kind: 'accepted', commit: null, files: 0, change: 'add-dark-mode' },
      // Absent as well, because ts-rs does render the field optional and both spellings arrive.
      { kind: 'accepted', files: 0, change: 'add-dark-mode' },
      { kind: 'refused', plan: { refusals: ['3 of 9 steps are still unticked.'] } },
      { kind: 'conflicts', paths: ['src/a.rs', 'src/b.rs'] },
    ]
    for (const arm of arms) {
      const said = C.acceptNotice(arm)
      ok(said.text.trim() !== '', `${arm.kind}: a sentence, never a silent press`)
      ok(
        said.kind === 'ok' || said.kind === 'error',
        `${arm.kind}: one of the two kinds notices have`,
      )
    }

    eq(C.acceptNotice(arms[3]).kind, 'error', 'a refusal is an error, not a quiet ok')
    eq(C.acceptNotice(arms[4]).kind, 'error', 'and so is a conflict')
    // Rust's own refusal sentences, carried verbatim — paraphrasing them here would be a second
    // refusal table to keep in step with `plan_change` by hand.
    ok(
      (C.acceptNotice(arms[3]).detail ?? '').includes('3 of 9 steps are still unticked.'),
      'the refusal carries the plan\u2019s own sentences',
    )
    /*
     * The conflict has to say that **nothing was archived**. `spec_accept` merges first exactly
     * so a refused merge leaves `openspec/specs/` untouched, and that ordering guarantee is the
     * thing the reader needs told — a message that only said "conflict" leaves them guessing
     * which half of the gesture landed.
     */
    const conflicted = C.acceptNotice(arms[4])
    ok(
      conflicted.text.includes('nothing was archived'),
      `a conflict says the archive did not happen: ${conflicted.text}`,
    )
    ok(
      (conflicted.detail ?? '').includes('src/a.rs'),
      'and names the files, because a count is not something anybody can act on',
    )

    /*
     * The merged and the not-merged answers must differ, and the not-merged one must not claim a
     * merge. `SpecAccepted::Accepted` carries `commit: null` both for a task whose work was done
     * in the user's own checkout and for a branch that was already up to date — one sentence
     * covers both, and it is the same lie as the button's if it names a merge that never was.
     */
    const merged = C.acceptNotice(arms[0])
    ok(merged.text.includes('abcdef12'), `the merge names its commit: ${merged.text}`)
    ok(merged.text.includes('4 files'), `and how much moved: ${merged.text}`)
    // Both spellings of "there was no merge", and they must read identically — the crash lived
    // in the gap between them.
    for (const nothing of [C.acceptNotice(arms[1]), C.acceptNotice(arms[2])]) {
      eq(nothing.kind, 'ok', 'nothing to merge is a success, not a failure')
      ok(
        !/merged/i.test(nothing.text),
        `an accept that merged nothing must not claim one: ${nothing.text}`,
      )
      ok(
        nothing.text.includes('add-dark-mode'),
        `but still names the change it archived: ${nothing.text}`,
      )
    }
    eq(
      C.acceptNotice(arms[1]).text,
      C.acceptNotice(arms[2]).text,
      '`null` and absent are one fact and must produce one sentence',
    )
  }

  /* ---------------------------------------------------------- the vocabulary, vs Rust */

  const specRs = read('../../crates/cide-ipc/src/spec.rs')
  const ops = variants(specRs, 'pub enum DeltaOperation', 'DeltaOperation')
  ok(ops.length >= 4, 'the DeltaOperation scan found variants')
  eq(
    sorted(M.DELTA_OPS),
    sorted(ops),
    'every delta operation Rust defines has a chip here, and no more',
  )

  /* ------------------------------------------------------------- no lookup may miss */

  for (const op of [...M.DELTA_OPS, 'constructor', 'toString', '', 'DEPRECATED']) {
    const label = M.opLabel(op)
    ok(
      typeof label === 'string' && label.length > 0,
      `opLabel(${JSON.stringify(op)}) is a non-empty string`,
    )
    ok(M.OP_TONES.includes(M.opTone(op)), `opTone(${JSON.stringify(op)}) is one of the tones`)
    const icon = M.opIcon(op)
    ok(
      typeof icon === 'string' && icon.length > 0 && !/[^a-z0-9-]/.test(icon),
      `opIcon(${JSON.stringify(op)}) is an icon name, never a character: ${JSON.stringify(icon)}`,
    )
  }
  ok(!M.isDeltaOp('constructor'), "'constructor' is not a delta operation")
  for (const stage of [...M.CHANGE_STAGES, 'constructor', '']) {
    const label = M.stageLabel(stage)
    ok(
      typeof label === 'string' && label.length > 0,
      `stageLabel(${JSON.stringify(stage)}) is a non-empty string`,
    )
  }
  ok(!M.isChangeStage('constructor'), "'constructor' is not a stage")

  /* --------------------------------------------------- unknown is not absent, ever */

  eq(M.BOARD_UNKNOWN.kind, 'unknown', 'the initial board is unknown')
  eq(M.metaFigure(M.BOARD_UNKNOWN), null, 'an unknown board counts nothing')
  eq(M.openChangeCount(M.BOARD_UNKNOWN), null, 'and badges nothing')
  eq(M.canWrite(M.BOARD_UNKNOWN), false, 'and offers no Set up button')
  eq(
    M.canWrite({ kind: 'unusable', reason: 'no cli' }),
    false,
    'a board cide could not read offers no write either',
  )
  eq(M.canWrite({ kind: 'absent', hint: 'h', path: '/repo/openspec' }), true, 'absent can be set up')

  const ready = {
    kind: 'ready',
    root: '/repo',
    specs: [{ id: 'dark-mode', requirements: 4 }],
    changes: [{ name: 'add-dark-mode', completed: 3, total: 9, status: 'in-progress' }],
  }
  eq(M.openChangeCount(ready), 1, 'a ready board badges what is in flight')
  ok(M.metaFigure(ready) !== null, 'and counts in the header')
  eq(
    M.metaFigure({ kind: 'ready', root: '/repo', specs: [], changes: [] }),
    null,
    'an empty ready board prints no figure — a header that says zero claims to have looked',
  )

  // The drop rule: a real board must not be replaced by "nobody has asked".
  eq(M.newerBoard(ready, M.BOARD_UNKNOWN), ready, 'unknown never replaces a ready board')
  eq(M.newerBoard(M.BOARD_UNKNOWN, ready), ready, 'and a ready board always lands')

  /* ------------------------------------------------------------------ 0/0 is not done */

  eq(M.stageOf(change({ completed: 0, total: 0 })), 'proposed', '0/0 is proposed, never ready')
  eq(M.stageOf(change({ completed: 3, total: 9 })), 'inProgress', '3/9 is in progress')
  eq(M.stageOf(change({ completed: 9, total: 9 })), 'ready', '9/9 is ready')
  eq(
    M.stageOf(change({ completed: 9, total: 9, validation: { valid: false, issues: [issue()] } })),
    'invalid',
    'a finished checklist on a broken change is still broken',
  )
  eq(
    M.summaryStage({ name: 'c', completed: 0, total: 0, status: 'no-tasks' }),
    'proposed',
    'and the same rule holds for a summary row, which has no verdict to consult',
  )

  eq(M.taskProgress(change({ completed: 0, total: 0 })).pct, 0, '0/0 does not divide by zero')
  eq(M.taskProgress(change({ completed: 9, total: 9 })).pct, 100, '9/9 is 100%')
  eq(
    M.taskProgress(change({ completed: 12, total: 9 })).done,
    9,
    'a count past the total is clamped rather than drawn past the end of the bar',
  )

  /* ------------------------------------------------- the proposal, and the design marker */

  /*
   * These key on the artifact **id**, and that distinction is what makes naming them safe.
   *
   * The artifact set is schema-driven — `openspec/config.yaml` picks a workflow schema whose
   * artifacts declare `generates` globs — so `proposal.md` is a default and not a guarantee,
   * which is why nothing in this feature writes that string anywhere. The *id* is the schema's
   * own vocabulary: it is what `status --json` keys `artifactPaths` by and what `applyRequires`
   * names.
   */
  const artifact = (over = {}) => ({ id: 'proposal', generates: '*.md', state: 'done', existing: [], ...over })
  const written = (id) => artifact({ id, existing: [`/p/openspec/changes/c/${id}.md`] })

  eq(
    M.proposalArtifact(change({ artifacts: [written('proposal')] }))?.id,
    'proposal',
    'a written proposal is found',
  )
  eq(
    M.proposalArtifact(change({ artifacts: [artifact({ id: 'proposal' })] })),
    null,
    'a declared-but-unwritten one is not — it belongs in Documents saying "not written yet"',
  )
  eq(
    M.proposalArtifact(change({ artifacts: [written('tasks')] })),
    null,
    'and a schema that declares no proposal at all costs nothing: the page draws as it did',
  )

  /*
   * The row leaves Documents **only** once the prose that replaces it is on screen.
   *
   * The proposal's text is a second read, so there is a render between the change arriving and
   * the file arriving — and dropping the row in that window would take the proposal off the page
   * entirely for a beat.
   */
  {
    const withProposal = change({ artifacts: [written('proposal'), written('tasks')] })
    eq(
      M.documentArtifacts(withProposal, true).map((a) => a.id),
      ['tasks'],
      'the proposal is lifted out of Documents when it is drawn as prose',
    )
    eq(
      M.documentArtifacts(withProposal, false).map((a) => a.id),
      ['proposal', 'tasks'],
      'and stays in the list until it is',
    )
    eq(
      M.documentArtifacts(change({ artifacts: [artifact({ id: 'proposal' })] }), true).map(
        (a) => a.id,
      ),
      ['proposal'],
      'an unwritten proposal is never removed, whatever the flag says',
    )
  }

  /*
   * The design marker. `design.md` is optional and `propose` writes none, so its presence
   * is somebody's decision that this change needed an argument settled before implementation —
   * the cheapest possible signal that it is not trivial. Declared-and-unwritten must draw
   * nothing, which is the majority of changes.
   */
  eq(M.hasDesign(change({ artifacts: [written('design')] })), true, 'a written design is a marker')
  eq(
    M.hasDesign(change({ artifacts: [artifact({ id: 'design' })] })),
    false,
    'a declared but unwritten one is not — most changes look like this and must draw nothing',
  )
  eq(M.hasDesign(change()), false, 'and a schema with no design artifact draws nothing either')

  /* --------------------------------------------------------- three validation states */

  eq(M.validateBadge(null).state, 'unchecked', 'no verdict is `unchecked`')
  eq(M.validateBadge({ valid: true, issues: [] }).state, 'ok', 'a clean verdict is `ok`')
  eq(
    M.validateBadge({ valid: false, issues: [issue()] }).state,
    'failed',
    'and a dirty one is `failed`',
  )
  ok(
    M.validateBadge(null).state !== M.validateBadge({ valid: true, issues: [] }).state,
    "`unchecked` must never render as `ok` — they are different claims",
  )
  eq(
    M.validateBadge({ valid: false, issues: [issue(), issue()] }).label,
    '2 issues',
    'the badge counts what is wrong',
  )
  eq(
    M.validateBadge({ valid: true, issues: [issue({ level: 'WARNING' })] }).state,
    'ok',
    'a warning does not make a change invalid',
  )
  ok(M.isBlocking(issue({ level: 'error' })), 'the level is matched whatever its case')
  ok(!M.isBlocking(issue({ level: 'INFO' })), 'and a note never blocks')

  /* ------------------------------------------------- every issue is drawn exactly once */

  {
    const validation = {
      valid: false,
      issues: [
        issue({ path: 'Theme switching', message: 'needs a scenario' }),
        issue({ path: 'Remembering', message: 'needs a SHALL' }),
        issue({ path: 'proposal', message: 'the Why section is too short' }),
      ]
    }
    const names = ['Theme switching', 'Remembering']
    const attributed = names.flatMap((name) => M.issuesFor(validation, name))
    const orphans = M.unattributedIssues(validation, names)
    eq(attributed.length, 2, 'two issues belong to requirement cards')
    eq(orphans.length, 1, 'and the third is drawn in the banner')
    eq(
      attributed.length + orphans.length,
      validation.issues.length,
      'every issue is drawn exactly once — one that fell out of both would be reported by the ' +
        'validator, sent across the wire, and rendered nowhere',
    )
    eq(M.issuesFor(validation, 'constructor'), [], "a prototype key claims no issues")
    eq(M.issuesFor(null, 'Theme switching'), [], 'and no verdict claims none')
  }

  /* --------------------------------- a button is enabled, or it says why. never both */

  const gate = (action, what) => {
    const enabled = action.gate.ok === true
    const reason = enabled ? '' : action.gate.reason
    if (enabled && typeof reason === 'string' && reason.length > 0) {
      fail(what, 'enabled *and* carrying a reason')
    }
    if (!enabled && (typeof reason !== 'string' || reason.trim().length === 0)) {
      fail(what, 'disabled with nothing saying why — a dead control in grey')
    }
    if (action.id !== 'none' && (typeof action.label !== 'string' || action.label.length === 0)) {
      fail(what, 'an action with no label on it')
    }
    if (typeof action.hint !== 'string' || action.hint.length === 0) {
      fail(what, 'no hint line — the hints are the whole of the onboarding')
    }
  }

  // Self-test, `check-commands.mjs`'s discipline: a gate nobody has seen fail is a gate nobody
  // knows works, and this one asserts an *absence*, which is the shape that passes vacuously.
  {
    const before = failed
    // Silenced while the holes are punched: these are *expected* failures and printing them
    // would put three FAIL lines above a passing run, which is how a check stops being read.
    const say = console.error
    console.error = () => {}
    gate({ id: 'accept', label: 'x', hint: 'h', gate: { ok: false, reason: '' } }, 'self-test')
    gate({ id: 'accept', label: '', hint: 'h', gate: { ok: true } }, 'self-test')
    gate({ id: 'accept', label: 'x', hint: '', gate: { ok: true } }, 'self-test')
    console.error = say
    if (failed === before + 3) {
      failed = before
    } else {
      failed = before
      fail('the gate self-test', 'a holed action passed the gate')
    }
  }

  for (const total of [0, 4, 9]) {
    for (const completed of [0, 3, 9]) {
      for (const valid of [true, false]) {
        for (const assignee of [null, '', 'developer']) {
          for (const status of [null, 'todo', 'doing', 'review', 'done']) {
            // The fifth axis: a conversation, none, and the blank that a wire field can be.
            for (const session of [null, '', 'sess-1']) {
              // And the sixth: whether the assigned role has a checkout of its own. `null` is
              // *no role, or no roster read yet* and is a third answer, not a shade of `false`.
              for (const worktree of [null, true, false]) {
                const c = change({
                  completed,
                  total,
                  validation: { valid, issues: valid ? [] : [issue()] },
                })
                const action = M.primaryAction(c, assignee, status, session, worktree)
                gate(
                  action,
                  `primaryAction(${total}/${completed}, ${valid}, ${assignee}, ${status}, ${session}, ${worktree})`,
                )
              }
            }
          }
        }
      }
    }
  }

  // The two that carry the vocabulary a reader is meant to learn.
  {
    /*
     * **Approve has no precondition, and that is the fix for a deadlock.**
     *
     * It used to refuse until an assignee had been picked — right while the assignee row was the
     * only way to start work, and exactly backwards once the button became the thing that picks
     * one. With the row hidden on an unapproved change, the two together produced a greyed
     * button reading "pick an assignee first" above no way to pick one.
     */
    const unassigned = M.primaryAction(change(), null, 'todo', null, null)
    eq(unassigned.id, 'approve', 'a proposed change offers approval')
    eq(
      unassigned.gate.ok,
      true,
      'with nothing assigned, because approving is what assigns it — the picker behind this ' +
        'button is where a role, an open conversation or a new one is chosen',
    )
    ok(
      unassigned.hint.includes('archived'),
      `and the hint still teaches what archiving is: ${unassigned.hint}`,
    )

    // Already assigned: the same action, worded as what it now does.
    const assigned = M.primaryAction(change(), 'developer', 'todo', null, true)
    eq(assigned.gate.ok, true)
    ok(
      assigned.label !== unassigned.label,
      `a change that already has a target says so rather than offering to approve it twice: ` +
        `${assigned.label}`,
    )

    const done = M.primaryAction(change({ completed: 9, total: 9 }), 'developer', 'review', null, true)
    eq(done.id, 'accept', 'a finished change offers the accept gesture')
    ok(
      done.hint.includes('openspec/specs/'),
      `the accept hint names what archiving writes: ${done.hint}`,
    )

    const broken = M.primaryAction(
      change({ completed: 9, total: 9, validation: { valid: false, issues: [issue()] } }),
      'developer',
      'review',
      null,
      true,
    )
    eq(broken.gate.ok, false, 'a change that does not validate cannot be accepted')

    /*
     * -------------------------------------------- the accept gesture names the step it takes
     *
     * The bug: the button read **Integrate & Archive** over a task whose work had been done in
     * the user's own checkout — a Claude conversation, or a `worktree: false` role — and there
     * was no branch to merge. `spec_accept` merges only when the task names a role *and*
     * `plan_accept` found a worktree for it; every other shape archives and nothing more. A
     * button that names a step it will not take is `Command::unavailable`'s failure wearing a
     * green button, and the user's reading of it was exact: "there is nothing to integrate".
     *
     * Both directions are asserted, because the label is only informative if it *varies*: a
     * function that always said `Archive` would pass a one-sided check and would then be lying
     * the other way, over a run whose branch really is waiting.
     */
    ok(
      /integrate/i.test(done.label),
      `a role with its own checkout has a branch to merge, and the label says so: ${done.label}`,
    )
    ok(
      done.hint.includes('branch'),
      `and so does its hint: ${done.hint}`,
    )

    for (const [what, action] of [
      // No role at all: `TaskEdit::SetSession` clears `Task::agent`, so a task handed to a
      // conversation reaches the accept arm with nothing that could name a branch.
      ['a conversation', M.primaryAction(change({ completed: 9, total: 9 }), null, 'review', 'sess-1', null)],
      // A role that opted out of worktree isolation: it committed into the checked-out tree.
      ['a worktree: false role', M.primaryAction(change({ completed: 9, total: 9 }), 'developer', 'review', null, false)],
    ]) {
      eq(action.id, 'accept', `${what} still reaches the accept gesture`)
      ok(
        !/integrate/i.test(action.label),
        `${what} has no branch, so the label must not offer to integrate one: ${action.label}`,
      )
      ok(
        !action.hint.includes("agent's branch"),
        `${what}: neither may the hint: ${action.hint}`,
      )
      ok(
        action.hint.includes('openspec/specs/'),
        `${what}: and it still names what archiving writes: ${action.hint}`,
      )
    }

    /*
     * -------------------------------------------- already handed to a conversation (M28)
     *
     * The branch the shipped build was missing, and the symptom was exact: choosing *New Claude
     * session* started the session, typed the task in, and left the card still offering **Approve
     * & dispatch** over a conversation that was already working. Pressing it again would open the
     * picker and invite a second dispatch of the same task.
     *
     * It was missed because the card only ever saw `assignee`, and `TaskStore::edit` **clears**
     * `Task::agent` when a session is set — so the one gesture that hands work to a conversation
     * left every field this function read exactly as it found them.
     */
    const handed = M.primaryAction(change(), null, 'doing', 'sess-1', null)
    eq(
      handed.id,
      'none',
      'a change whose work is with a conversation offers no approve button — the session row ' +
        'stands in its place, and pressing approve again would invite a second dispatch',
    )
    eq(handed.gate.ok, false, 'and the reason says where the work went')

    /*
     * **The role path is untouched**, which is the other half of the claim. Assigning a role is
     * `cide_agents::autodispatch`'s trigger edge and a session is read by no trigger at all;
     * Rust keeps them in different fields so a task cannot claim both. A rule that had keyed off
     * "anything assigned" would have taken the re-dispatch road away from every subagent task.
     */
    eq(
      M.primaryAction(change(), 'developer', 'doing', null, true).id,
      'approve',
      'a task assigned to a role can still be dispatched again — the session rule must not ' +
        'reach the role road',
    )

    /*
     * A blank session is not a session. The field crosses the wire as `Option<SessionId>` and
     * the mirror is `string | null`, but "" is what a hand-written fixture or a future adapter
     * bug produces, and reading it as a live conversation would silently remove the only button
     * on the card.
     */
    eq(M.primaryAction(change(), null, 'todo', '   ', null).id, 'approve', 'a blank session is none')

    /*
     * And accepting outranks it: a finished change is accepted whoever did the work. This is the
     * ordering inside the function — the `finished` branch comes first — and stating it here is
     * what stops a later edit from moving the session check above it and hiding the one gesture
     * that lands the change.
     */
    eq(
      M.primaryAction(change({ completed: 9, total: 9 }), null, 'review', 'sess-1', null).id,
      'accept',
      'a finished change is accepted whoever did the work',
    )
  }

  /* ------------------------------------------------------------------ the archive preview */

  {
    const deltas = [
      { spec: 'dark-mode', op: 'added', description: '', requirements: [{}, {}], renamedFrom: null, renamedTo: null },
      { spec: 'editor', op: 'modified', description: '', requirements: [{}], renamedFrom: null, renamedTo: null },
    ]
    const preview = M.archivePreview(deltas)
    ok(preview.includes('dark-mode'), `the preview names the capability, not just a count: ${preview}`)
    ok(preview.includes('editor'), `and every capability: ${preview}`)
    ok(preview.includes('2 requirements'), `with what happens to each: ${preview}`)
    ok(
      preview.includes('1 requirement '),
      `singular where it is one, plural where it is not: ${preview}`,
    )
    ok(M.archivePreview([]).length > 0, 'and a change that edits nothing still says so')
  }

  /* ------------------------------------------------------------------------ the tree */

  {
    const board = {
      kind: 'ready',
      root: '/repo',
      specs: [
        { id: 'z-last', requirements: 1 },
        { id: 'a-first', requirements: 2 },
      ],
      changes: [
        { name: 'far', completed: 0, total: 9, status: 'in-progress' },
        { name: 'close', completed: 8, total: 9, status: 'in-progress' },
      ],
    }
    const rows = M.specRows(board, {})
    eq(rows[0].kind, 'section', 'the tree opens with a section')
    eq(rows[0].id, 'changes', 'and changes come first — that is what is being worked on')
    eq(rows[1].id, 'close', 'the change closest to acceptable is at the top')
    eq(rows[2].id, 'far', 'and the one with most left to do is below it')
    const specRows = rows.filter((row) => row.kind === 'spec').map((row) => row.id)
    eq(specRows, ['a-first', 'z-last'], 'specs are alphabetical — an order somebody can learn')

    const collapsed = M.specRows(board, { changes: false, specs: false })
    eq(
      collapsed.filter((row) => row.kind !== 'section').length,
      0,
      'a collapsed section draws none of its rows',
    )
    eq(collapsed.length, 2, 'and both headings stay')
    eq(M.specRows(M.BOARD_UNKNOWN, {}), [], 'an unknown board draws no tree at all')
    for (const row of rows) {
      if (row.kind === 'section') {
        ok(row.hint.length > 0, `the ${row.id} section carries its one-line explanation`)
        ok(
          row.hint.includes('openspec/'),
          `and names the real directory, because the brand is shown: ${row.hint}`,
        )
      }
    }
  }

  /* ------------------------------------------------------- the strings a screen prints */

  for (const [name, value] of Object.entries(M)) {
    if (typeof value !== 'string') continue
    ok(value.trim().length > 0, `${name} is not an empty string`)
  }
  ok(M.CLI_INSTALL.includes('@fission-ai/openspec'), 'the install command names the package')
  ok(
    M.SETUP_TITLE.includes('openspec init'),
    'the Set up button says what it runs — it writes a tracked directory',
  )
  ok(
    !M.SETUP_TITLE.includes('/opsx'),
    'and does not name a surface OpenSpec stopped installing — see `invocation`',
  )
  /*
   * The post-init notices. Set up restarts the project console itself now (`consoleReload.ts`),
   * so only the arm that could not reach a pane may tell the user to restart anything — the
   * other two claim the opposite, and an arm that drifted into asking for a restart it already
   * performed would read as a restart that did not work.
   */
  {
    const arms = ['resumed', 'restarted', 'unreachable'].map((arm) => M.setUpNotice(arm))
    for (const sentence of arms) ok(sentence.trim().length > 0, 'every reload arm has words')
    ok(new Set(arms).size === 3, 'and the three arms are told apart')
    ok(
      M.setUpNotice('unreachable').includes('Restart'),
      'the unreachable arm still asks for the manual restart — it is the fallback, not the feature',
    )
    ok(
      M.setUpNotice('resumed').includes('resuming'),
      'the resumed arm says the conversation was kept — "restarted" alone reads as "gone"',
    )
    ok(
      !M.setUpNotice('resumed').includes('Restart the'),
      'and neither completed arm asks the user to restart anything',
    )
    ok(
      M.SETUP_RELOAD_FAILED.includes('restart it yourself'),
      'a failed respawn hands the gesture back with its name, not a bare apology',
    )
  }
  /*
   * Every command the panel offers has to be one OpenSpec's default profile installs.
   *
   * The panel typed `/opsx:onboard` for one iteration. It is in the CLI's *templates* and is not
   * installed by the profile `openspec init` uses, so Claude answered `Unknown command` and
   * nothing in cide could explain it. Rust checks the project's own directory before typing now;
   * this is the cheap half — a name in the frontend that upstream is known not to ship fails
   * here rather than at a user's keyboard.
   */
  {
    const INSTALLED_BY_DEFAULT = ['apply', 'archive', 'explore', 'propose', 'sync', 'update']
    for (const [name, command] of [
      ['PROPOSE_COMMAND', M.PROPOSE_COMMAND],
      ['EXPLORE_COMMAND', M.EXPLORE_COMMAND],
    ]) {
      ok(
        INSTALLED_BY_DEFAULT.includes(command),
        `${name} is \`${command}\`, which openspec's default profile does not install — the ` +
          `button would make Claude answer "Unknown command"`,
      )
      ok(
        typeof command === 'string' && /^[a-z-]+$/.test(command),
        `${name} is a kebab command name: ${JSON.stringify(command)}`,
      )
    }
    ok(
      !Object.values(M).some((value) => typeof value === 'string' && value.includes('onboard')),
      '`onboard` is gone from the panel’s strings — it is not installed by default, and the ' +
        'Rust test pins it as absent so a future release adding it fails loudly',
    )
  }

  /*
   * **No invocation is spelled in this panel except the one fallback.**
   *
   * The bug this closes ran for a year. OpenSpec installed its workflow as slash commands, so
   * `propose` was `/opsx:propose`, and cide wrote that prefix into nine strings. The CLI then
   * moved the same workflow to Claude Code skills — `/openspec-propose` — and every button in
   * this panel began refusing on every correctly set up project, with a sentence recommending
   * `openspec update`, which on such a project writes nothing. A prefix in a constant here is
   * what made that possible, so a prefix in a constant here fails.
   *
   * `CURRENT_PREFIX` is the sole exception and is checked against Rust's newest surface below.
   */
  {
    for (const [key, value] of Object.entries(M)) {
      if (typeof value !== 'string' || key === 'CURRENT_PREFIX') continue
      ok(
        !/\/opsx:|\/openspec-/.test(value),
        `${key} spells an invocation (${JSON.stringify(value)}) — it belongs on the board, ` +
          `read from the project by cide_spec::claude`,
      )
    }
    eq(M.CURRENT_PREFIX, '/openspec-', 'the fallback is the spelling a current init writes')
  }

  /* --------------------------------------------------------------------- finding text */

  {
    // The page marks its own text rather than calling `window.find()`, which searches the whole
    // document — a hit in the sidebar or the tab strip would count, and the number on screen
    // would not be a number about this page.
    eq(
      M.splitMatches('Theme switching', 'theme'),
      [
        { text: 'Theme', hit: true },
        { text: ' switching', hit: false },
      ],
      'case-insensitive: a reader looking for `acl` means `ACL` too',
    )
    eq(M.countMatches('a-b-a-b-a', 'a'), 3, 'every occurrence counts')
    eq(
      M.splitMatches('abc', ''),
      [{ text: 'abc', hit: false }],
      'an empty query matches nothing — opening the bar must not light up the page',
    )
    eq(M.splitMatches('abc', '   '), [{ text: 'abc', hit: false }], 'nor a whitespace one')
    eq(M.countMatches('', 'a'), 0, 'and an empty string holds nothing')
    // The pieces always reassemble into the original, whatever the query — a marker that dropped
    // or duplicated a character would rewrite the document on screen.
    for (const [text, query] of [
      ['Theme switching', 'e'],
      ['aaaa', 'aa'],
      ['abc', 'abc'],
      ['abc', 'x'],
      ['', 'x'],
    ]) {
      eq(
        M.splitMatches(text, query).map((piece) => piece.text).join(''),
        text,
        `the pieces of ${JSON.stringify(text)} reassemble exactly`,
      )
    }

    // Next/prev wraps at both ends, and does nothing at all when there is nothing found.
    eq(M.stepMatch(0, 1, 3), 1, 'next')
    eq(M.stepMatch(2, 1, 3), 0, 'wraps forward')
    eq(M.stepMatch(0, -1, 3), 2, 'and backward')
    eq(M.stepMatch(0, 1, 0), 0, 'pressing next with nothing found is a no-op, not an index of -1')
    eq(M.stepMatch(5, 1, 3), 0, 'an index past the end comes back inside')
  }

  /* ------------------------------------------------- an archived change, and a missing one */

  {
    /*
     * `archivedAs` decides four answers, and it arrives over the wire — so the three shapes it
     * can have all have to be pinned, including the one the type says is impossible.
     *
     * **The bug this is written about.** The archived branch was spelled `!== null`. A webview
     * newer than the binary it talks to gets a `SpecChange` with no `origin` at all, `adapt.ts`
     * then produced `undefined`, and `undefined !== null` is true — so *every live change in the
     * project* answered `none` with the sentence "this change has been archived". Approve &
     * dispatch disappeared from every card and every change page at once, which is exactly how
     * it was reported. Vite hot-reloads the frontend and the Rust binary only changes on a
     * relaunch, so that mismatch is not an edge case here, it is every dev loop.
     *
     * `adapt.ts` now answers `null` for an absent origin *and* every reader takes `!= null`. Two
     * guards for one fact, deliberately: the adapter is where the wire is read, and the readers
     * are what a second adapter — or a fixture written by hand — would bypass.
     */
    const live = {
      name: 'add-dark-mode',
      title: 'add-dark-mode',
      deltas: [],
      artifacts: [],
      tasks: [],
      completed: 0,
      total: 9,
      validation: { valid: true, issues: [] },
      archivedAs: null,
    }
    for (const [what, value] of [
      ['null', null],
      ['undefined', undefined],
    ]) {
      const change = { ...live, archivedAs: value }
      eq(
        M.primaryAction(change, null, 'todo', null, null).id,
        'approve',
        `${what}: a change nobody archived still offers Approve & dispatch`,
      )
      eq(M.stageOf(change), 'proposed', `${what}: and reads as proposed, not archived`)
      eq(M.validateBadge(live.validation, change.archivedAs != null).state, 'ok', `${what}: valid`)
    }

    const gone = { ...live, archivedAs: '2026-08-27-add-dark-mode' }
    const action = M.primaryAction(gone, 'developer', 'review', null, true)
    eq(action.id, 'none', 'and an archived change offers nothing, whatever the task says')
    ok(
      action.hint.includes('2026-08-27-add-dark-mode'),
      `naming where it went: ${action.hint}`,
    )
    eq(M.stageOf(gone), 'archived', 'the stage says so too')
    eq(M.stageLabel('archived'), 'Archived', 'and it has a word')
    eq(
      M.validateBadge({ valid: true, issues: [] }, true).state,
      'unchecked',
      'never `ok`: an archive is vacuously valid because nothing validated it',
    )
  }

  /* ------------------------------------------------------------ the change row's action */

  {
    // Two states of one gesture. Naming them differently matters: an *Open task* on a change
    // with no task would be a button that creates something while claiming to open it.
    const start = M.rowAction(null)
    eq(start.id, 'start', 'a change nobody is working offers to start it')
    ok(start.label.length > 0 && start.title.length > 0, 'with a label and a sentence')
    ok(
      start.title.includes('Assigning'),
      `and the sentence says what actually starts the agent: ${start.title}`,
    )
    const open = M.rowAction('t-14')
    eq(open.id, 'open', 'a change with a task opens it')
    ok(open.title.includes('t-14'), `naming the task: ${open.title}`)
    eq(start.disabledReason, undefined, 'and neither is drawn inert against a ready tracker')
    eq(open.disabledReason, undefined, 'nor is the open one')

    /*
     * The third state, which is the bug this argument exists for.
     *
     * `task === null` means *this change has no task* only when the **task** board is `ready`.
     * On `unknown` and `unreadable` it means nobody has looked, and the row was drawing that as
     * a confident `Start work` — on a change whose task was finished. Pressing it did nothing
     * whatever: `tasksStore.create` gates on `canWrite`, which is `false` for exactly those two
     * arms, and returns without a word. So they are inert here, each with the sentence saying
     * what would un-grey it.
     *
     * `absent` is *not* one of them, matching `canWrite`: there is no `.cide/tasks.json` and
     * creating the first task is what writes one.
     */
    for (const arm of ['unknown', 'unreadable']) {
      const blocked = M.rowAction(null, arm)
      eq(blocked.id, 'start', `${arm}: still the start gesture`)
      ok(
        typeof blocked.disabledReason === 'string' && blocked.disabledReason.length > 20,
        `${arm}: drawn inert with a sentence, not a bare grey: ${blocked.disabledReason}`,
      )
      // And an id in hand does not override it — a task read off a board nobody has read is not
      // evidence that the task is there.
      eq(
        M.rowAction('t-14', arm).disabledReason,
        blocked.disabledReason,
        `${arm}: a task id from an unread board does not unlock the row`,
      )
    }
    ok(
      M.rowAction(null, 'unreadable').disabledReason.includes('.cide/tasks.json'),
      'and the unreadable one names the file the user has to go and look at',
    )
    eq(
      M.rowAction(null, 'absent').disabledReason,
      undefined,
      'no tracker file yet is not the same as no answer: creating the first task writes one',
    )
    eq(M.rowAction(null).disabledReason, undefined, 'the tracker defaults to ready')

    /*
     * `splitAction` — the change page's second button. (M31)
     *
     * It takes no task id, because the page draws it only while there is none: a change gets
     * exactly one task carrying `change`, `spec_triggers::consider_one` finds a change's task by
     * that field and on more than one match moves neither, so fanning a change out is a thing you
     * do to one nobody has started. What it does still ask is the tracker's arm — a control that
     * writes must not be live while nobody knows what is already on the board, which is
     * `rowAction`'s whole argument one function along.
     */
    ok(M.splitAction().label.length > 0, 'split work has a label')
    ok(
      M.splitAction().title.length > 20,
      'and a tooltip saying what pressing it asks for, not a restatement of the label',
    )
    eq(M.splitAction().disabledReason, undefined, 'the tracker defaults to ready')
    eq(
      M.splitAction('absent').disabledReason,
      undefined,
      'no tracker file yet is not the same as no answer — `canWrite` has it the same way',
    )
    for (const arm of ['unknown', 'unreadable']) {
      const blocked = M.splitAction(arm)
      ok(
        typeof blocked.disabledReason === 'string' && blocked.disabledReason.length > 20,
        `${arm}: split is inert with a sentence, not a bare grey: ${blocked.disabledReason}`,
      )
      eq(blocked.label, M.splitAction().label, `${arm}: an inert control keeps its own name`)
    }
    ok(
      M.splitAction('unreadable').disabledReason.includes('.cide/tasks.json'),
      'and the unreadable one names the file the user has to go and look at',
    )

    // The row carries the link, and a prototype key is not a task.
    const board = {
      kind: 'ready',
      root: '/repo',
      specs: [],
      changes: [{ name: 'constructor', completed: 0, total: 3, status: 'in-progress' }],
    }
    const rows = M.specRows(board, {}, {})
    const row = rows.find((r) => r.kind === 'change')
    eq(row.task, null, "a change named `constructor` finds no task on an empty map")
    eq(M.specRows(board, {}, { constructor: 't-1' })[1].task, 't-1', 'and a real entry is found')
  }

  /* ----------------------------------------------------------------- the composer */

  {
    /*
     * The line is *typed* into a PTY and ended with Enter, so a newline in the middle submits the
     * first half as a turn and feeds the rest in as further turns — the failure that looks like a
     * model answering nonsense. Rust flattens it again with `opening_prompt`'s own helper, which
     * is the guard that counts; this one is what the panel previews, and a preview that differed
     * from what is sent would be its own small lie.
     */
    for (const rogue of [
      'a dark theme\nthat follows the system',
      'a dark theme\r\nthat follows the system',
      'a dark theme\tthat follows   the system',
      '  a dark theme that follows the system  ',
    ]) {
      const line = M.commandLine('/openspec-propose', rogue)
      ok(!/[\n\r]/.test(line), `no newline survives into the line: ${JSON.stringify(line)}`)
      ok(!/\s{2,}/.test(line), `and no run of whitespace: ${JSON.stringify(line)}`)
      ok(line.startsWith('/openspec-propose '), line)
    }
    eq(M.commandLine('/openspec-propose', ''), '/openspec-propose', 'empty text sends the bare command')
    eq(M.commandLine('/opsx:explore', '   '), '/opsx:explore', 'and so does whitespace')

    /*
     * The invocation comes off the board, and both surfaces reach the composer intact.
     *
     * A project set up by a current `openspec` types `/openspec-propose`; one set up by an older
     * CLI still has `.claude/commands/opsx/` on disk and types `/opsx:propose`. cide reads which
     * from the directory (`cide_spec::claude`) and the panel previews what it is about to send —
     * so a panel that resolved either spelling itself would be right for half the world.
     */
    const skills = {
      kind: 'ready',
      root: '/repo',
      specs: [],
      changes: [],
      commands: [
        { name: 'explore', line: '/openspec-explore' },
        { name: 'propose', line: '/openspec-propose' },
      ],
    }
    const legacy = {
      ...skills,
      commands: [
        { name: 'explore', line: '/opsx:explore' },
        { name: 'propose', line: '/opsx:propose' },
      ],
    }
    eq(M.invocation(skills, 'propose'), '/openspec-propose')
    eq(M.invocation(legacy, 'propose'), '/opsx:propose', 'an older project keeps its own spelling')
    eq(
      M.commandLine(M.invocation(legacy, 'propose'), 'dark mode'),
      '/opsx:propose dark mode',
      'and the composed line is that spelling, not the current one',
    )
    // A command the project does not have still has to render *something* above the box. The
    // guess is the current spelling, and it is one Rust refuses to type — `spec_run_command`
    // resolves against the directory and answers with what the project does have.
    eq(
      M.invocation({ ...skills, commands: [] }, 'propose'),
      '/openspec-propose',
      'a board listing nothing falls back to the current spelling',
    )
    eq(M.invocation(M.BOARD_UNKNOWN, 'propose'), '/openspec-propose', 'and so does an unread board')

    // The tooltip names the line *this* project would type — it said `/opsx:propose` on projects
    // where no such command existed, which is the bug.
    ok(M.commandTitle(legacy, 'propose').includes('/opsx:propose'), M.commandTitle(legacy, 'propose'))
    ok(M.commandTitle(skills, 'propose').includes('/openspec-propose'))
    ok(M.commandTitle(skills, 'explore').includes('/openspec-explore'))
    ok(
      M.commandTitle(skills, 'propose') !== M.commandTitle(skills, 'explore'),
      'the two buttons describe different actions',
    )

    // `propose` needs a subject; `explore` is a mode and may be entered with nothing said.
    eq(M.canAsk('propose', ''), false, 'a bare propose is refused — Claude would just ask')
    eq(M.canAsk('propose', '  '), false)
    eq(M.canAsk('propose', 'dark mode'), true)
    eq(M.canAsk('explore', ''), true, 'explore is a mode, not a request')

    for (const command of [M.PROPOSE_COMMAND, M.EXPLORE_COMMAND, 'constructor', '']) {
      const placeholder = M.askPlaceholder(command)
      ok(
        typeof placeholder === 'string' && placeholder.length > 0,
        `askPlaceholder(${JSON.stringify(command)}) is a non-empty string`,
      )
    }
    ok(
      M.askPlaceholder(M.PROPOSE_COMMAND) !== M.askPlaceholder(M.EXPLORE_COMMAND),
      'the two commands ask different questions, because they are different questions',
    )

  }

  /* ------------------------------------------------------------------- the editor */

  {
    const requirement = {
      name: 'Theme switching',
      text: 'The app SHALL switch themes.',
      scenarios: [{ title: 'Picks dark', body: '- **WHEN** a\n- **THEN** b' }],
    }

    // A target is the one string a `data-target` can carry, and it comes back off an attribute —
    // so `'constructor'` and `'d1.rX'` are real inputs, not hypotheticals.
    eq(E.targetId({ delta: 0, requirement: 2 }), 'd0.r2', 'a target round-trips as a string')
    eq(E.parseTarget('d0.r2'), { delta: 0, requirement: 2 }, 'and back')
    for (const rogue of ['constructor', '', 'd1.rX', 'd-1.r0', 'nonsense', 'd0r2']) {
      eq(E.parseTarget(rogue), null, `${JSON.stringify(rogue)} addresses no requirement`)
    }

    // Opening an editor and closing it must not report a change — otherwise Save would rewrite a
    // file because somebody looked at it.
    const draft = E.draftOf(requirement)
    eq(E.dirty(draft, requirement), false, 'an untouched draft is not dirty')
    ok(E.dirty({ ...draft, name: 'Other' }, requirement), 'a renamed one is')
    ok(
      E.dirty({ ...draft, scenarios: [] }, requirement),
      'and one that dropped a scenario is — the count is compared, not just the contents',
    )

    // Save is enabled, or it says why. Never both, never neither.
    eq(E.saveRefusal(draft), null, 'a complete requirement saves')
    ok(
      E.saveRefusal({ ...draft, name: '  ' }).includes('name'),
      'a nameless one is refused, naming the field',
    )
    ok(
      E.saveRefusal({ ...draft, scenarios: [] }).includes('scenario'),
      'and one with no scenarios',
    )
    ok(
      E.saveRefusal({ ...draft, text: 'The app switches themes.' }).includes('SHALL'),
      'and one that states no obligation — which is what openspec validate refuses it for',
    )
    ok(
      E.saveRefusal({
        ...draft,
        scenarios: [{ title: '   ', body: '- **WHEN** a' }],
      }).includes('title'),
      'and a scenario with no title, which is what says which case it is',
    )

    // The composer writes the canonical shape, and Rust refuses anything it could get wrong.
    const composed = E.compose(draft)
    ok(
      composed.startsWith('### Requirement: Theme switching\n'),
      `the block opens with its header: ${JSON.stringify(composed)}`,
    )
    ok(composed.includes('#### Scenario: Picks dark'), `and names each scenario: ${composed}`)
    ok(composed.endsWith('\n'), 'and ends with exactly one newline')
    ok(!composed.includes('\n\n\n'), `and runs no blank lines together: ${JSON.stringify(composed)}`)
    // The name is trimmed, because it is what the archive matches on and a trailing space is
    // invisible in the editor and load-bearing in the file.
    ok(E.compose({ ...draft, name: '  Theme switching  ' }).startsWith('### Requirement: Theme switching\n'))

    // The clause helper inserts a line; it never replaces the body, so a user who writes their
    // own bullets or a third clause is not fought.
    {
      const start = E.insertClause('', 0, 'WHEN')
      eq(start.text, '- **WHEN** ', 'into an empty body, no leading newline')
      eq(start.selStart, start.text.length, 'and the caret lands after the marker')

      const after = E.insertClause('- **WHEN** a', 12, 'THEN')
      eq(after.text, '- **WHEN** a\n- **THEN** ', 'onto a line with text, on a new line')

      const mid = E.insertClause('- **WHEN** a\n- **THEN** b', 3, 'AND')
      ok(
        mid.text.startsWith('- **WHEN** a\n- **AND** '),
        `a caret mid-line inserts after that line, never inside a word: ${JSON.stringify(mid.text)}`,
      )
      // A selection index can outlive the text it was taken from.
      // A selection index can outlive the text it was taken from — a chip pressed after the
      // draft was replaced. Both ends clamp, and both then insert after the caret's *line*,
      // which for a single-line body is the end. Neither slices out of range.
      const past = E.insertClause('ab', 9999, 'WHEN')
      eq(past.text, 'ab\n- **WHEN** ', 'a caret past the end clamps to it')
      const before = E.insertClause('ab', -5, 'WHEN')
      eq(before.text, 'ab\n- **WHEN** ', 'and a negative one clamps to zero')
    }

    // Scenario list edits are no-ops out of range rather than holes in the array.
    eq(E.removeScenario(draft, 99), draft, 'removing a scenario that is not there changes nothing')
    eq(E.setScenario(draft, -1, { title: 'x' }), draft, 'and so does setting one')
    eq(E.addScenario(draft).scenarios.length, 2, 'adding one seeds a second')
    ok(
      E.addScenario(draft).scenarios[1].body.includes('**WHEN**'),
      'and seeds it with the template, which is how the shape is taught',
    )
  }

  /* ------------------------------------------------------------- the row context menu */

  {
    // Archive is offered from the panel at all because the accept gesture was built on the task
    // card, which left a change proposed straight from the pinned session with no route to
    // finish anywhere in the UI.
    const ids = (row) => M.changeRowActions(row).map((action) => action.id)
    eq(
      ids({ done: 3, total: 9 }),
      ['open', 'validate', 'archive'],
      'a change row offers the same three whatever state it is in — an action that vanished ' +
        'when it did not apply would be a menu whose shape changed under the pointer',
    )
    eq(M.specRowActions().map((a) => a.id), ['open'], 'a capability row offers one')

    const archive = (row) => M.changeRowActions(row).find((action) => action.id === 'archive')
    // Enabled, or disabled *with a sentence*. Never disabled in silence — `MenuItem` has no
    // `disabled` for exactly this reason.
    for (const row of [
      { done: 0, total: 0 },
      { done: 0, total: 31 },
      { done: 30, total: 31 },
      { done: 31, total: 31 },
    ]) {
      const item = archive(row)
      const reason = item.disabledReason
      const enabled = reason === undefined
      ok(
        enabled || (typeof reason === 'string' && reason.trim().length > 0),
        `archive at ${row.done}/${row.total} is disabled with nothing saying why`,
      )
      ok(
        typeof item.label === 'string' && item.label.length > 0,
        'and it always has a label',
      )
    }

    eq(
      archive({ done: 31, total: 31 }).disabledReason,
      undefined,
      'a finished checklist can be archived',
    )
    ok(
      archive({ done: 30, total: 31 }).disabledReason.includes('1 of 31'),
      `an unfinished one says how far off it is: ${archive({ done: 30, total: 31 }).disabledReason}`,
    )
    // `0/0` is a change nobody has planned, not a finished one. The same rule guards the Review
    // hop in Rust, and getting it wrong here would offer to archive an empty proposal.
    ok(
      archive({ done: 0, total: 0 }).disabledReason !== undefined,
      'a change with no task list cannot be archived — 0/0 is unplanned, not done',
    )
    ok(
      archive({ done: 0, total: 0 }).disabledReason.includes('no task list'),
      'and says so in those terms rather than reporting 0 of 0 steps',
    )
  }

  /* ------------------------------------------------------- the wire loop, both ways */

  {
    const client = read('../src/ipc/client.ts')
    const contract = JSON.parse(read('../../contract/commands.json'))
    const list = Array.isArray(contract) ? contract : Object.keys(contract)
    const mine = list.filter((name) => name.startsWith('spec_'))
    ok(mine.length >= 7, `the contract carries the spec commands (${mine.length})`)
    for (const name of mine) {
      ok(
        client.includes(`'${name}'`),
        `${name} is in the contract and no client call names it — the panel cannot reach it`,
      )
    }
    for (const [, name] of client.matchAll(/invoke<[^>]*>\('(spec_[a-z_]+)'/g)) {
      ok(
        list.includes(name),
        `client.ts calls ${name}, which no command answers — an unregistered command rejects, ` +
          `and an unhandled rejection out of a React 19 effect unmounts the window`,
      )
    }
    const events = JSON.parse(read('../../contract/events.json'))
    const eventList = Array.isArray(events) ? events : Object.keys(events)
    ok(
      eventList.includes('cide://spec-changed'),
      'the spec-changed event is in the contract',
    )
    ok(
      client.includes('cide://spec-changed'),
      'and the client subscribes to it',
    )
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log('openspec: ok (vocabulary vs Rust, gates, the tree, the wire loop)')
