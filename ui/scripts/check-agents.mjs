/**
 * Checks `src/sidebar/AgentsPanel/model.ts` and `src/sidebar/TasksPanel/model.ts` — the pure
 * cores of the two M18 panels — and pins their vocabularies against the Rust that defines them.
 *
 * Same shape as `check-problems.mjs` and `check-theme.mjs`, and for the same reason: this
 * project has no JS test runner, adding one for a handful of pure functions would be a larger
 * commitment than the code it tests, and both modules are deliberately import-free so the
 * TypeScript already in `node_modules` can compile them standalone and node can import the
 * result. If either compile ever needs a tsconfig, something has added an import and the
 * node-testability of the core has been lost.
 *
 * # The three failure classes this makes unrepresentable
 *
 * **A vocabulary that drifts from Rust.** `TaskStatus`, `RunState` and `Harness` are enums in
 * `crates/cide-ipc/src/{tasks,agents}.rs` and frozen lists in TypeScript. `xtask codegen`
 * regenerates `generated.ts`, but neither model imports it — they restate the shapes
 * structurally so they can be compiled alone — so nothing else in the build can see a fifth
 * `TaskStatus` arriving with no glyph, no label, no tone and no group to be drawn in. Adding a
 * status in Rust fails this check until the panel's tables know it.
 *
 * **A table lookup that misses.** `check-problems.mjs`'s `ROGUE` lesson, made structural: a
 * miss returns `undefined`, or worse a prototype key returns `Object.prototype.constructor` —
 * a function, which React refuses as a child and which `className` stringifies into the whole
 * source text of `Object`. So every member of every vocabulary is asserted to yield a non-empty
 * *string*, and `'constructor'` is pinned as not being a member of either.
 *
 * **A control that is greyed with nothing saying why.** `canDispatch` must return exactly one
 * of a green light and a non-empty sentence — never both, never neither. That is
 * `Command::unavailable` one layer down and for the identical reason: this project has paid
 * twenty-four times for a row that is listed and silently inert. The comparison itself
 * ([`gate`]) is pure and is self-tested below against results with holes punched in them, the
 * way `check-commands.mjs` self-tests `missing()` — a gate nobody has seen fail is a gate
 * nobody knows works, and this one asserts the *absence* of something, which is the shape that
 * passes vacuously when its scan breaks.
 *
 * **A subagent with no row.** The panel is now one list of roles, so a run whose role the roster
 * does not define — somebody deleted `.cide/agents/<id>.md` while it was working — has nowhere
 * to go unless `sections()` invents a row for it, and a run with no row anywhere is a `claude`
 * spending the user's quota that they cannot see, open or stop. The same hole opens from the
 * other side for a phase this build cannot read, which is why `isActivePhase` is the negation of
 * `isDonePhase` rather than membership of a list. Both are asserted below, in both directions.
 *
 * # What this does NOT cover, and nothing here should be read as claiming
 *
 *   - that either panel renders. That is `check-agents-render.mjs`'s job, through Vite's SSR
 *     bundle, and a model can pass every assertion below while the component paints nothing.
 *   - that `adapt.ts` converts the wire's `bigint` millisecond fields to the `number`s both
 *     models take. Nothing here imports `generated.ts`; the models are structural restatements
 *     and `tsc --noEmit` over the real `adapt.ts` is what pins them to it.
 *   - that the two `cide://` events reach the stores at all.
 *
 * Run: `pnpm --dir ui run check:agents`   (or `node ui/scripts/check-agents.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-agents-'))

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

/** Line comments out, block comments out. Every scan below wants code, not prose. */
const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

/**
 * The body of a Rust item, found by its opening line and its `\n}`.
 *
 * Lifted from `check-commands.mjs`, which reads `fn build()` the same way. It works on an enum
 * for the same reason it works on that function: every one of the three enums below has its
 * closing brace in column zero and every variant payload on one line, so the first `\n}` after
 * the opening is the end of the item and not the end of a variant.
 */
function rustBody(source, opening, what) {
  const start = source.indexOf(opening)
  if (start < 0) throw new Error(`could not find ${what}`)
  const end = source.indexOf('\n}', start)
  return source.slice(start, end)
}

/**
 * The camelCase wire names of a Rust enum's variants.
 *
 * Every one of the three enums carries `#[serde(rename_all = "camelCase")]`, and camelCasing a
 * PascalCase identifier is lowercasing its first letter — `AwaitingPermission` →
 * `awaitingPermission`, `Todo` → `todo`. A regex rather than a parser, on `check-commands.mjs`'s
 * argument; the "at least this many" assertion at each call site is what stops a change of shape
 * from silently matching nothing and passing.
 */
function variants(source, opening, what) {
  const body = stripComments(rustBody(source, opening, what))
  const inner = body.slice(body.indexOf('{') + 1)
  return [...inner.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*(\{|\(|,|$)/gm)].map(
    (m) => m[1].charAt(0).toLowerCase() + m[1].slice(1),
  )
}

const sorted = (list) => [...list].sort()

try {
  /*
   * Both models in one invocation. `tsc` puts each under its own directory in `out` because it
   * derives the common root from the inputs (`src/sidebar`), which is what keeps two files
   * called `model.ts` from overwriting each other.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/AgentsPanel/model.ts',
      'src/sidebar/TasksPanel/model.ts',
      'src/sidebar/TasksPanel/mentionModel.ts',
      'src/sidebar/TasksPanel/markdownTools.ts',
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

  const agents = await import(`file://${join(out, 'AgentsPanel', 'model.js')}`)
  const tasks = await import(`file://${join(out, 'TasksPanel', 'model.js')}`)
  const mentions = await import(`file://${join(out, 'TasksPanel', 'mentionModel.js')}`)
  const mdTools = await import(`file://${join(out, 'TasksPanel', 'markdownTools.js')}`)

  const {
    HARNESSES,
    SCOPES,
    scopeBadge,
    RUN_PHASES,
    WORKING_PHASES: AGENT_WORKING_PHASES,
    ACTIVE_PHASES,
    TONES: RUN_TONES,
    ROSTER_UNKNOWN,
    OFF_FOR_THIS_PROJECT,
    rosterRoles,
    RECENT_CAP,
    RESTING_GLYPH,
    RESTING_LABEL,
    ROLE_UNDEFINED,
    glyphSpins,
    isActivePhase,
    isRunPhase,
    phaseGlyph,
    SPINNING_GLYPH,
    phaseHint,
    phaseLabel,
    phaseTone,
    canDispatch,
    canOpen,
    canPause,
    occupiedSlots,
    liveCount,
    queuedCount,
    metaFigure: rosterFigure,
    sections,
    elapsed,
    staleTurnLine,
  } = agents

  const {
    TASK_STATUSES,
    GROUP_ORDER,
    TONES: TASK_TONES,
    WORKING_PHASES: TASK_WORKING_PHASES,
    BOARD_UNKNOWN,
    isTaskStatus,
    statusGlyph,
    statusLabel,
    statusTone,
    agentChip,
    groupOf,
    groups,
    matchesFilter,
    matchesQuery,
    listEmpty,
    armedDelete,
    openCount,
    metaFigure: boardFigure,
    newerBoard,
    canWrite,
    commentOrder,
    authorLabel,
    EMPTY_DRAFT,
    draftReady,
    draftDirty,
    closeCompose,
    filterAfterCreate,
    queryAfterCreate,
    TASK_FIELDS,
    EDITABLE_FIELDS,
    UNASSIGNED,
    NO_TITLE,
    NO_BODY,
    isTaskField,
    isEditableField,
    fieldLabel,
    fieldValue,
    isFieldEmpty,
    restText,
    assigneeLabel,
    assigneeFromDraft,
    assignableRoles,
    assigneeHint,
    startEdit,
    isDirty,
    beginEdit,
    commitEdit,
    cancelEdit,
    closeCard,
    openTask,
    activeEdit,
    LINK_KINDS,
    isLinkKind,
    linkLabel,
    taskLinks,
    linkGroups,
    draftLinkChips,
    linkTargetStatus,
    linkTargetText,
    linkableTargets,
    linkTargetOptions,
    LINK_TARGET_CAP,
    ATTACHMENT_KINDS,
    formatBytes,
    basename,
    imageAttachmentsOf,
    dropTargetKey,
    parseDropTarget,
    cssPoint,
    dropZoneAt,
  } = tasks

  /* == 0 ============================ an optional wire field is `null`, not missing (M28) == */

  /*
   * `Task`'s optional fields carry **`#[serde(default)]` and no `skip_serializing_if`**, so a
   * value that is `None` in Rust reaches the webview as `"field": null` — the key is present.
   * An adapter testing `=== undefined` therefore falls straight through to `String(null)`, which
   * is the seven-character string `"null"`: a truthy id on every row.
   *
   * That is not hypothetical. `session` shipped that way for an afternoon: every task on the
   * board grew a session row reading *Conversation null*, and `primaryAction` — which refuses the
   * approve road once a task has a session — took **Approve & dispatch off every OpenSpec task
   * there was**. `tsc` cannot see it (both branches type), and no render check can, because the
   * fixtures are written by hand and never carry a wire value.
   *
   * So the rule is a grep, and it is a grep over the one file that touches wire values: in
   * `adapt.ts`, an absent optional is tested with `== null`, which catches both.
   */
  {
    const adapt = read('../src/sidebar/TasksPanel/adapt.ts')
    const code = adapt
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/\/\/[^\n]*/g, '')
    ok(
      !/===\s*undefined/.test(code),
      'TasksPanel/adapt.ts tests an absent wire field with `=== undefined` — `Task`\'s optionals ' +
        'have no `skip_serializing_if`, so they arrive as an explicit `null` and that branch ' +
        'reads it as a value. Use `== null`, which catches both',
    )
    ok(
      /session:\s*from\.session\s*==\s*null/.test(code),
      'and `session` in particular, which is the field that proved it',
    )
  }

  /* == 1 ================================================== the vocabularies match Rust == */

  const tasksRs = read('../../crates/cide-ipc/src/tasks.rs')
  const agentsRs = read('../../crates/cide-ipc/src/agents.rs')

  const rustStatuses = variants(tasksRs, 'pub enum TaskStatus {', 'TaskStatus')
  const rustPhases = variants(agentsRs, 'pub enum RunState {', 'RunState')
  const rustHarnesses = variants(agentsRs, 'pub enum Harness {', 'Harness')

  ok(rustStatuses.length === 4, `read ${rustStatuses.length} TaskStatus variants — the scan still matches`)
  ok(rustPhases.length === 9, `read ${rustPhases.length} RunState variants — the scan still matches`)
  ok(rustHarnesses.length === 4, `read ${rustHarnesses.length} Harness variants — the scan still matches`)

  eq(
    sorted(TASK_STATUSES),
    sorted(rustStatuses),
    'TASK_STATUSES is exactly `pub enum TaskStatus` — a status Rust gained and the panel has ' +
      'not is a task the tracker can hold and never draw',
  )
  eq(
    sorted(RUN_PHASES),
    sorted(rustPhases),
    'RUN_PHASES is exactly the tags of `pub enum RunState`',
  )
  eq(sorted(HARNESSES), sorted(rustHarnesses), 'HARNESSES is exactly `pub enum Harness`')

  /*
   * The scopes, pinned the way the harnesses are. (M30)
   *
   * A variant Rust gained and the panel has not is a row whose badge silently reads `null` — and
   * for a source cide does not own, that is precisely the fact the badge exists to state.
   */
  const rustScopes = variants(agentsRs, 'pub enum AgentScope {', 'AgentScope')
  eq(sorted(SCOPES), sorted(rustScopes), 'SCOPES is exactly `pub enum AgentScope`')
  /*
   * And the badge itself is total in the only sense that matters: **a mark or nothing, never an
   * empty string.** An empty badge renders as a bordered box with no text in it, which reads as a
   * rendering fault rather than as "no badge" — the same argument `canDispatch` makes about
   * returning exactly one of a green light and a sentence.
   */
  for (const scope of SCOPES) {
    const badge = scopeBadge(scope)
    ok(
      badge === null || (typeof badge === 'string' && badge.trim() !== ''),
      `scopeBadge(${scope}) is a real mark or null, never blank`,
    )
  }
  eq(
    SCOPES.filter((scope) => scopeBadge(scope) !== null),
    ['claudeProject', 'claudeGlobal'],
    'exactly the two Claude Code scopes are badged — badging cide’s own two would put a chip ' +
      'on every row and say nothing',
  )
  eq(
    scopeBadge('claudeProject'),
    scopeBadge('claudeGlobal'),
    'project and user share one mark: which of the two a subagent is in matters when you go to ' +
      'edit it, not when you are reading a roster',
  )

  /* The link kinds, pinned the way the statuses are. (M30) */
  const rustLinkKinds = variants(tasksRs, 'pub enum LinkType {', 'LinkType')
  ok(
    rustLinkKinds.length === 3,
    `read ${rustLinkKinds.length} LinkType variants — the scan still matches`,
  )
  eq(
    sorted(LINK_KINDS),
    sorted(rustLinkKinds),
    'LINK_KINDS is exactly `pub enum LinkType` — a kind Rust gained and the panel has not is ' +
      'an edge the tracker can hold and never draw, which is the task-leaves-the-tracker ' +
      'failure applied to an edge',
  )

  eq(
    sorted(GROUP_ORDER),
    sorted(TASK_STATUSES),
    'GROUP_ORDER is a permutation of TASK_STATUSES — a status the panel never groups is a ' +
      'task the user cannot see',
  )
  eq(
    GROUP_ORDER.length,
    TASK_STATUSES.length,
    'GROUP_ORDER lists each status once (a permutation, not a multiset)',
  )
  ok(
    JSON.stringify(GROUP_ORDER) !== JSON.stringify(TASK_STATUSES),
    'GROUP_ORDER is a display order and not a copy of the wire order — active groups first',
  )
  eq(GROUP_ORDER[0], 'doing', 'the panel opens on what is happening, not on the backlog')

  /*
   * `TasksPanel` restates `WORKING_PHASES` rather than importing it, because both modules must
   * stay import-free. This is the seam that keeps the copy honest — and it is why the rename
   * out of `LIVE_PHASES` had to move all three files at once: this destructures both copies
   * *by name*, so renaming either one alone fails here rather than passing.
   */
  eq(
    TASK_WORKING_PHASES.filter((p) => !RUN_PHASES.includes(p)),
    [],
    "TasksPanel's WORKING_PHASES copy names only phases RunState still has",
  )
  eq(
    sorted(TASK_WORKING_PHASES),
    sorted(AGENT_WORKING_PHASES),
    'the two WORKING_PHASES copies agree — otherwise a chip lights for a run the Agents panel ' +
      'counts as finished, or the reverse',
  )
  ok(
    !AGENT_WORKING_PHASES.includes('queued'),
    'a queued run holds no slot, so it cannot block the dispatch that would start it',
  )
  ok(
    !AGENT_WORKING_PHASES.includes('idle'),
    'an idle run handed its turn back and released its slot — it is alive, which is a different ' +
      'question, and the one this list stopped being named for',
  )
  ok(
    AGENT_WORKING_PHASES.includes('paused'),
    'a SIGSTOPped run is frozen mid-turn and still holds its worktree and its slot',
  )

  /* == 2 ============================== every member of every vocabulary has a table entry == */

  let tableEntries = 0
  const nonEmptyString = (value, what) => {
    if (typeof value !== 'string' || value.length === 0) {
      fail(what, `actual: ${JSON.stringify(value)} (${typeof value})`)
    } else {
      tableEntries += 1
    }
  }

  for (const status of TASK_STATUSES) {
    nonEmptyString(statusGlyph(status), `statusGlyph(${status}) is a non-empty string`)
    nonEmptyString(statusLabel(status), `statusLabel(${status}) is a non-empty string`)
    nonEmptyString(statusTone(status), `statusTone(${status}) is a non-empty string`)
    ok(TASK_TONES.includes(statusTone(status)), `statusTone(${status}) is a declared tone`)
  }
  for (const phase of RUN_PHASES) {
    nonEmptyString(phaseGlyph(phase), `phaseGlyph(${phase}) is a non-empty string`)
    nonEmptyString(phaseLabel(phase), `phaseLabel(${phase}) is a non-empty string`)
    nonEmptyString(phaseTone(phase), `phaseTone(${phase}) is a non-empty string`)
    ok(RUN_TONES.includes(phaseTone(phase)), `phaseTone(${phase}) is a declared tone`)
  }

  /*
   * Exactly one mark turns, and it is `running`'s.
   *
   * `PHASE_GLYPH`'s own comment says the arc is chosen *because* it reads as turning, and that
   * `idle`'s ringed dot is only distinguishable from it on that reading. It did not turn — not on
   * the task list's chip, not on the card's run strip, not on the Agents panel's rows — so for a
   * milestone and a half the single signal that an assigned agent was working was a static shape,
   * indistinguishable at a glance from the state it was chosen to contrast with.
   *
   * Keyed off the *glyph* and not the phase, so the animation and the mark cannot drift apart:
   * give `running` a different mark and it stops spinning, give another phase the arc and it
   * starts. The three stylesheets each carry their own `@keyframes` — a CSS module cannot share
   * one — and `check:agents-render` is what pins the class onto the markup.
   */
  {
    const spinning = RUN_PHASES.filter((phase) => glyphSpins(phaseGlyph(phase)))
    eq(spinning, ['running'], 'one phase turns, and it is the one whose mark is a spinner')
    eq(phaseGlyph('running'), SPINNING_GLYPH, 'and the table and the rule name the same mark')
    ok(!glyphSpins(phaseGlyph('idle')), 'idle sits still — the contrast the arc was chosen for')
    ok(!glyphSpins(''), 'and an unknown phase\u2019s fallback mark does not spin')
  }

  /*
   * The paused hint. "Paused" alone read as a lie to a user watching their local model keep
   * inferring after the freeze — the child is stopped, but a model call already in flight
   * finishes on the provider's side and is read on resume. The row's title must carry that
   * truth, and only there: every other phase's one-word label is the whole truth and gets no
   * hint, so a view falling back to the label stays correct.
   */
  {
    const hint = phaseHint('paused')
    nonEmptyString(hint, `phaseHint('paused') carries the in-flight truth`)
    ok(
      hint.includes('in flight') && hint.includes('resumed'),
      `the paused hint names the in-flight model call and when its answer lands`
    )
    for (const phase of RUN_PHASES) {
      if (phase === 'paused') continue
      eq(phaseHint(phase), null, `phaseHint(${phase}) === null — the label is the whole truth`)
    }
  }

  /*
   * The rogue keys. `'constructor'` is the one that is not merely absent but *present* on every
   * object literal's prototype, so a `value in TABLE` membership test would admit it and the
   * lookup would return a function.
   */
  for (const rogue of ['constructor', 'toString', '__proto__', 'hasOwnProperty', '']) {
    eq(isTaskStatus(rogue), false, `isTaskStatus(${JSON.stringify(rogue)}) === false`)
    eq(isRunPhase(rogue), false, `isRunPhase(${JSON.stringify(rogue)}) === false`)
    nonEmptyString(statusGlyph(rogue), `statusGlyph(${JSON.stringify(rogue)}) still renders`)
    nonEmptyString(statusLabel(rogue), `statusLabel(${JSON.stringify(rogue)}) still renders`)
    nonEmptyString(statusTone(rogue), `statusTone(${JSON.stringify(rogue)}) still renders`)
    nonEmptyString(phaseGlyph(rogue), `phaseGlyph(${JSON.stringify(rogue)}) still renders`)
    nonEmptyString(phaseLabel(rogue), `phaseLabel(${JSON.stringify(rogue)}) still renders`)
    nonEmptyString(phaseTone(rogue), `phaseTone(${JSON.stringify(rogue)}) still renders`)
  }

  /* == 3 ========================================================= the dispatchability gate == */

  /**
   * The comparison, kept pure so it can be tested with a hole punched in its input.
   *
   * `exactlyOne` is the whole claim: a role either has a button or has a sentence. Both is a
   * row that offers a control it will then refuse; neither is a control greyed out with nothing
   * on screen saying why, which is a dead control wearing grey.
   */
  const gate = (result) => {
    const green = result !== null && result !== undefined && result.ok === true
    const reason = result === null || result === undefined ? undefined : result.reason
    const sentence = typeof reason === 'string' && reason.trim() !== ''
    return { green, sentence, exactlyOne: green !== sentence }
  }

  {
    // The check's own test, before it is trusted with the real roster.
    eq(gate({ ok: true }).exactlyOne, true, 'the gate accepts a green light (self-test)')
    eq(
      gate({ ok: false, reason: 'The dispatch queue is paused.' }).exactlyOne,
      true,
      'the gate accepts a refusal with a sentence (self-test)',
    )
    eq(
      gate({ ok: false, reason: '' }).exactlyOne,
      false,
      'the gate reports a refusal with no sentence — the dead-control state (self-test)',
    )
    eq(
      gate({ ok: false }).exactlyOne,
      false,
      'the gate reports a refusal with no reason field at all (self-test)',
    )
    eq(
      gate({ ok: true, reason: 'but also' }).exactlyOne,
      false,
      'the gate reports a result that is both (self-test)',
    )
  }

  const def = (over) => ({
    id: 'developer',
    label: 'Developer',
    harness: 'claude',
    description: 'Implements one task end to end.',
    systemPrompt: 'You are the developer agent.',
    model: null,
    unavailable: null,
    maxConcurrent: 1,
    ...over,
  })

  const run = (over) => ({
    run: 'r1',
    agent: 'developer',
    agentLabel: 'Developer',
    harness: 'claude',
    session: 's1',
    phase: 'running',
    task: 't-14',
    startedMs: 1_700_000_000_000,
    pausedSinceMs: null,
    exitCode: null,
    failure: null,
    staleTurn: false,
    note: null,
    // Rust's rule, restated for the fixture: a row with a session is openable. Stated
    // before `over` so a story can say otherwise — a finished opencode run has no session
    // and an openable conversation.
    openable: (over.session === undefined ? 's1' : over.session) !== null,
    ...over,
  })

  /* The fixture, with a hole punched for each refusal the gate is supposed to produce. */
  const ROLES = [
    def({ id: 'developer', label: 'Developer', maxConcurrent: 2 }),
    def({ id: 'qa', label: 'QA', unavailable: 'opencode is not on PATH.', harness: 'opencode' }),
    def({ id: 'artist', label: 'Artist', maxConcurrent: 0 }),
    def({ id: 'writer', label: 'Writer', maxConcurrent: 1 }),
    def({ id: 'blank', label: '', unavailable: '   ' }),
  ]
  const RUNS = [
    run({ run: 'r1', agent: 'writer', phase: 'running', task: 't-14' }),
    run({ run: 'r2', agent: 'developer', phase: 'queued', session: null, task: 't-15' }),
    run({ run: 'r3', agent: 'developer', phase: 'awaitingPermission', task: 't-16' }),
    run({ run: 'r4', agent: 'qa', phase: 'finished', exitCode: 0, task: null, staleTurn: true }),
    run({ run: 'r5', agent: 'artist', phase: 'failed', failure: 'no worktree', task: 't-99' }),
  ]

  const READY = { kind: 'ready', agents: ROLES, runs: RUNS, dispatching: true }
  const PAUSED_QUEUE = { ...READY, dispatching: false }
  const DISABLED = {
    kind: 'disabled',
    hint: 'Subagents are off for this project.',
    configPath: '/repo/.cide/config.json',
  }
  const EMPTY = { kind: 'empty', configPath: '/repo/.cide/config.json' }

  const ROSTERS = [
    ['ready', READY],
    ['queue paused', PAUSED_QUEUE],
    ['disabled', DISABLED],
    ['empty', EMPTY],
    ['unknown', ROSTER_UNKNOWN],
  ]

  let gated = 0
  for (const [name, roster] of ROSTERS) {
    for (const role of ROLES) {
      const result = canDispatch(role, roster)
      const verdict = gate(result)
      gated += 1
      ok(
        verdict.exactlyOne,
        `canDispatch(${role.id}, ${name}) returns exactly one of a green light and a ` +
          `sentence — got ${JSON.stringify(result)}`,
      )
      if (result.ok === false) {
        nonEmptyString(
          result.reason,
          `canDispatch(${role.id}, ${name}) refuses with a non-empty reason`,
        )
      }
      if (typeof role.unavailable === 'string' && role.unavailable.trim() !== '') {
        eq(
          result.reason,
          role.unavailable,
          `canDispatch(${role.id}, ${name}) shows the role's own sentence, verbatim and first`,
        )
      }
      ok(
        result.ok === false || role.unavailable === null,
        `canDispatch never green-lights a role carrying an unavailable reason (${role.id})`,
      )
    }
  }

  // ...and the individual refusals are the ones the table says they are.
  eq(canDispatch(ROLES[0], READY).ok, true, 'a healthy role under a ready roster dispatches')
  eq(
    canDispatch(ROLES[1], READY).reason,
    'opencode is not on PATH.',
    "the role's own unavailable sentence wins over every other test",
  )
  ok(
    /no concurrency/.test(canDispatch(ROLES[2], READY).reason),
    'maxConcurrent: 0 refuses with a sentence naming the configuration',
  )
  eq(
    canDispatch(ROLES[3], READY).reason,
    'Writer already has 1 running.',
    'a role at its ceiling names the count',
  )
  eq(
    canDispatch(ROLES[0], PAUSED_QUEUE).reason,
    'The dispatch queue is paused.',
    'a shut queue refuses every role, including healthy ones',
  )
  ok(
    canDispatch(ROLES[0], ROSTER_UNKNOWN).reason !==
      canDispatch(ROLES[0], DISABLED).reason,
    '"nobody has looked" and "off for this project" are different sentences — telling a user a ' +
      'feature is off when the truth is that cide has not read the file is the confident ' +
      'empty list in sentence form',
  )
  ok(
    canDispatch(ROLES[4], READY).reason.trim() !== '',
    'a role whose unavailable marker is blank is still refused, with a sentence of our own',
  )

  /* == 4 ============================================================ newerBoard is the drop == */

  const task = (over) => ({
    id: 't-14',
    title: 'Add the retry bar',
    body: '',
    status: 'doing',
    agent: 'developer',
    comments: [],
    links: [],
    createdBy: { kind: 'user' },
    createdMs: 1_699_999_000_000,
    updatedMs: 1_700_000_000_000,
    ...over,
  })

  const ready = (rev, list = [task({})]) => ({ kind: 'ready', tasks: list, rev })
  const UNREADABLE = {
    kind: 'unreadable',
    path: '/repo/.cide/tasks.json',
    error: 'expected value at line 12 column 3',
  }
  const ABSENT = { kind: 'absent', hint: 'No tracker here yet.', path: '/repo/.cide/tasks.json' }

  {
    const current = ready(7)
    ok(newerBoard(current, ready(8)).rev === 8, 'a higher rev replaces')
    ok(
      newerBoard(current, ready(6)) === current,
      'a lower rev is dropped, returning the identical object so no reader re-renders',
    )
    ok(
      newerBoard(current, ready(7)) === current,
      'an equal rev is dropped too, and by identity — two windows re-reading one file produce ' +
        'two equal-rev snapshots and admitting the second repaints a list mid-scroll',
    )
    ok(
      newerBoard(current, UNREADABLE) === UNREADABLE,
      'a tracker that became unreadable always replaces, whatever the predecessor rev was',
    )
    ok(newerBoard(current, ABSENT) === ABSENT, 'so does one that is gone')
    ok(
      newerBoard(current, BOARD_UNKNOWN) === BOARD_UNKNOWN,
      'and so does the boot state, which is not a claim about the file',
    )
    ok(
      newerBoard(UNREADABLE, ready(1)).rev === 1,
      'a ready board replaces a non-ready one whatever its rev',
    )
    ok(
      newerBoard(current, ready(Number.NaN)) === current,
      'a rev that cannot be ordered keeps the known-good board rather than replacing it',
    )
  }

  eq(canWrite(UNREADABLE), false, 'an unparseable tracker offers nothing that writes')
  eq(canWrite(BOARD_UNKNOWN), false, 'and neither does a board nobody has read yet')
  eq(canWrite(ABSENT), true, 'an absent tracker offers New task, which is what creates the file')
  eq(canWrite(ready(1)), true, 'a ready tracker is writable')

  /* == 5 ==================================================== agentChip survives its inputs == */

  const ref = (over) => ({
    run: 'r1',
    task: 't-14',
    agentLabel: 'Developer',
    phase: 'running',
    session: 's1',
    openable: true,
    ...over,
  })
  const ROLE_LABELS = { developer: 'Developer', qa: 'QA' }

  let chips = 0
  const chip = (t, runs, roles, what) => {
    let result
    try {
      result = agentChip(t, runs, roles)
    } catch (error) {
      fail(`agentChip does not throw: ${what}`, String(error))
      return null
    }
    chips += 1
    if (result !== null) {
      nonEmptyString(result.label, `agentChip label is a non-empty string: ${what}`)
      // The four legal (lit, tone) pairs and nothing else: lit is live/attention, unlit is
      // assigned/attention — the unlit-attention pair being the stalled reading. The two
      // combinations outside the square are each a lie: a lit-assigned chip claims work with
      // no working run behind the tone, and an unlit-live one claims the reverse.
      ok(
        !(result.lit && result.tone === 'assigned') && !(!result.lit && result.tone === 'live'),
        `agentChip's two discriminators stay inside the legal square: ${what}`,
      )
    }
    return result
  }

  {
    const live = chip(task({}), [ref({})], ROLE_LABELS, 'a live run on this task')
    ok(live !== null && live.lit === true, 'a live run wins and is lit')
    ok(live !== null && live.tone === 'live', 'and carries the live tone')

    // The fixture's default is `doing` + assigned, so no runs at all is the STALLED exemplar —
    // the orchestrator's orphaned-t-62 state, previously indistinguishable from just-started.
    const stalled = chip(task({}), [], ROLE_LABELS, 'doing + assigned + no runs = stalled')
    ok(stalled !== null && stalled.lit === false, 'stalled is unlit — no run is working')
    ok(
      stalled !== null && stalled.tone === 'attention',
      'and carries attention — the task claims work is under way and nobody is coming',
    )

    const dim = chip(task({ status: 'todo' }), [], ROLE_LABELS, 'todo + assigned + no runs')
    ok(dim !== null && dim.lit === false, 'the assigned role alone is dim')
    ok(
      dim !== null && dim.tone === 'assigned',
      'a todo task is not started, not stalled — assignment claims nothing about progress',
    )
    ok(
      live !== null && dim !== null && live.tone !== dim.tone && live.lit !== dim.lit,
      'the live and assigned renderings differ on both discriminators — a chip that looked the ' +
        'same either way would claim an exited agent is still working',
    )
    ok(
      stalled !== null && dim !== null && stalled.tone !== dim.tone,
      'and stalled differs from plain assigned — "nobody is coming" rendered identically to ' +
        '"assigned, quietly" is the invisibility the debug notes reported',
    )

    // The suppression boundary, edge by edge — each why is in `agentChip`'s doc.
    const queued = chip(
      task({}),
      [ref({ phase: 'queued', session: null })],
      ROLE_LABELS,
      'doing + a queued run',
    )
    ok(
      queued !== null && queued.tone === 'assigned',
      'a queued run suppresses stalled — the system is on the task and the run is literally next',
    )
    const idle = chip(task({}), [ref({ phase: 'idle' })], ROLE_LABELS, 'doing + an idle run')
    ok(
      idle !== null && idle.tone === 'assigned',
      'an idle run suppresses stalled — a live child one follow-up from moving, already nudged',
    )
    const interrupted = chip(
      task({}),
      [ref({ phase: 'interrupted' })],
      ROLE_LABELS,
      'doing + an interrupted run',
    )
    ok(
      interrupted !== null && interrupted.lit === false && interrupted.tone === 'attention',
      'an interrupted run does NOT suppress stalled — its child died with a cide restart and ' +
        'nothing moves until a human presses Resume, which is exactly what attention asks for',
    )
    const reviewed = chip(task({ status: 'review' }), [], ROLE_LABELS, 'review + assigned')
    ok(
      reviewed !== null && reviewed.tone === 'assigned',
      'review is past the point where a missing run means anything',
    )

    eq(chip(task({ agent: null }), [], ROLE_LABELS, 'neither'), null, 'neither fact yields no chip')

    // The three shapes that cross a boundary and are routinely a few hundred ms out of date.
    const orphan = chip(
      task({ id: 't-77' }),
      [ref({ task: 't-does-not-exist' })],
      ROLE_LABELS,
      'a run naming a task that does not exist',
    )
    ok(orphan !== null && orphan.lit === false, 'a run on another task does not light this row')

    chip(
      task({ agent: 'nobody-defined-this' }),
      [],
      ROLE_LABELS,
      'a task naming a role the roster does not define',
    )
    chip(task({}), [ref({ phase: 'teleporting' })], ROLE_LABELS, 'a phase outside RUN_PHASES')
    chip(task({}), [ref({ phase: 'constructor' })], ROLE_LABELS, 'a prototype key as a phase')
    chip(task({ agent: 'constructor' }), [], ROLE_LABELS, 'a prototype key as an agent id')
    chip(task({}), [ref({ agentLabel: '' })], ROLE_LABELS, 'a run with a blank label')
    chip(task({ agent: null }), [ref({ agentLabel: '' })], {}, 'a blank label and no roles at all')

    const unknownPhase = chip(
      task({}),
      [ref({ phase: 'teleporting' })],
      ROLE_LABELS,
      'unknown phase, again',
    )
    ok(
      unknownPhase !== null && unknownPhase.lit === false,
      'a phase this build has never heard of is treated as not-live — the conservative ' +
        'direction, because the alternative is claiming work is under way on a guess',
    )

    const awaiting = chip(
      task({}),
      [ref({ run: 'r1', phase: 'running' }), ref({ run: 'r2', phase: 'awaitingPermission' })],
      ROLE_LABELS,
      'two live runs, one awaiting permission',
    )
    ok(
      awaiting !== null && awaiting.run === 'r2' && awaiting.tone === 'attention',
      'the run that is a call to action wins over the one that merely came first',
    )
  }

  /* == 6 ===================================================== null is not 0, in both panels == */

  eq(occupiedSlots(ROSTER_UNKNOWN), null, 'occupiedSlots is null when nobody has looked')
  eq(queuedCount(ROSTER_UNKNOWN), null, 'queuedCount is null when nobody has looked')
  eq(rosterFigure(ROSTER_UNKNOWN), null, 'and the header figure draws nothing at all')
  eq(occupiedSlots(DISABLED), null, 'a disabled roster carries no runs array, so it counts nothing')

  /*
   * `liveCount` is the pre-rename spelling, surviving only because `App.tsx` imports it for the
   * activity rail and that file was not part of the rename. Pinned as the *same function
   * object*, not merely as an equal answer: an alias that turned into a second implementation
   * would be two numbers for one question again, which is the failure the rename removed.
   */
  ok(liveCount === occupiedSlots, 'liveCount is an alias of occupiedSlots and not a second count')

  const IDLE = { kind: 'ready', agents: ROLES, runs: [], dispatching: true }
  eq(occupiedSlots(IDLE), 0, 'a ready-but-idle roster is a real, reportable zero')
  eq(queuedCount(IDLE), 0, 'and so is its queue')
  eq(rosterFigure(IDLE), '0', 'which the header prints, because something looked')
  eq(occupiedSlots(READY), 2, 'the fixture has two runs holding a slot')
  eq(queuedCount(READY), 1, 'and one waiting for one')
  eq(rosterFigure(READY), '2+1', 'the queue depth rides the figure — a header reading 0 while ' +
    'runs wait to start is the same quiet lie as an unchecked zero')

  eq(openCount(BOARD_UNKNOWN), null, 'openCount is null when nobody has looked')
  eq(openCount(UNREADABLE), null, 'and when the file will not parse')
  eq(openCount(ABSENT), null, 'a project with no tracker has no count, it has no tracker')
  eq(openCount(ready(1, [])), 0, 'a tracker that exists and is empty is a real zero')
  eq(boardFigure(BOARD_UNKNOWN), null, 'the header draws nothing until something has looked')
  eq(boardFigure(ready(1, [])), '0', 'and prints the zero once it has')
  eq(
    boardFigure(ready(1, [task({ id: 't-1' }), task({ id: 't-2', status: 'done' })])),
    '1/2',
    'open over total, because "how much is left" and "how big is this" are two questions',
  )
  /*
   * And it takes the board and nothing else.
   *
   * The header names the **tracker**, not the slice of it the user happens to be reading. A
   * filtered figure would print `0/1` under the `done` filter over a board with three open
   * tasks — the "my tasks are gone" conclusion the filter's own empty screen exists to prevent,
   * relocated into the one line of the panel a user trusts as a fact about the file. The arity
   * is what makes that structural rather than a convention: there is nowhere to pass a filter.
   */
  eq(
    boardFigure.length,
    1,
    'metaFigure takes only the board — the header figure cannot be made to follow the filter',
  )

  /* == the role rows and the figures, which nothing else can see ============================= */

  {
    const TITLES = { 't-14': 'Add the retry bar', 't-15': 'Sweep the phase table' }
    const list = sections(READY, TITLES)
    eq(
      list.map((s) => s.kind),
      ['agents', 'recent'],
      'the panel is one list of subagents, then what has finished — the five run-centric ' +
        'groups are gone, and their order was the thing the user could not read',
    )
    eq(sections(DISABLED, TITLES), [], 'a non-ready roster draws its designed screen, not a list')
    eq(sections(ROSTER_UNKNOWN, TITLES), [], 'and so does the boot state')

    const agentsSection = list.find((s) => s.kind === 'agents')
    const byId = Object.fromEntries(agentsSection.rows.map((row) => [row.def.id, row]))

    /*
     * **The user's rule, and the reason this file exists at all.**
     *
     * A role with nothing active offers no Open — not a disabled one, none: the view derives its
     * buttons from `runs`, which is empty, so there is no element to disable. `blank` has no run
     * of any kind in the fixture and `artist` has only a `failed` one, so the two cover both
     * ways of arriving at "doing nothing": never started, and over.
     */
    /*
     * The two constants are asserted non-empty **before** they are used as expected values.
     * Comparing a row's glyph against `RESTING_GLYPH` alone proves only that the row reads the
     * constant: emptying the constant would satisfy the comparison and leave the row's fixed
     * width dot column collapsed, which reads as a rendering fault rather than as "nothing is
     * happening". Same trap `phaseGlyph`'s table entries are checked against.
     */
    nonEmptyString(RESTING_GLYPH, 'the resting mark is a real glyph, not an empty cell')
    nonEmptyString(RESTING_LABEL, 'and the resting status is a real word')
    for (const id of ['blank', 'artist']) {
      eq(byId[id].runs.length, 0, `${id} has no active run`)
      eq(byId[id].canOpen, false, `${id} offers no Open — there is nothing of its to look at`)
      eq(byId[id].glyph, RESTING_GLYPH, `${id} draws the resting mark rather than an empty cell`)
      eq(byId[id].status, RESTING_LABEL, `${id} says so in words as well as in a glyph`)
    }
    ok(
      byId.artist.runs.length === 0 && READY.runs.some((r) => r.agent === 'artist'),
      'and `artist` really does have a run in the fixture — a finished one. A role whose runs ' +
        'have all ended is doing nothing, which is the half of the rule a story with no runs ' +
        'at all could not have shown',
    )

    /* The other half: a role that *is* doing something offers it. */
    eq(byId.writer.runs.length, 1, 'writer has its running run')
    eq(byId.writer.canOpen, true, 'and offers Open, because that run has a session')
    eq(byId.writer.status, 'Running', 'with the phase in words on the role row itself')
    ok(byId.writer.glyph.length > 0, 'and a glyph')

    /*
     * A role with a run and still no Open, for the *other* reason. `developer`'s queued run has
     * no session — nothing to mirror — so the row exists, the line is drawn, and the control is
     * still withheld. Two different causes, one behaviour, and neither is a greyed button.
     */
    const dev = byId.developer
    ok(
      dev.runs.some((row) => row.run.phase === 'queued' && row.canOpen === false),
      'a queued run is drawn under its role and offers no Open',
    )

    /*
     * **The multi-run summary.** `developer` has a queued run and an `awaitingPermission` one,
     * so the order is a real choice: the run that is blocked on the user leads, whatever came
     * first, and the role's one-word summary is that run's.
     */
    eq(dev.runs.length, 2, 'both of developer’s active runs are drawn — a run on screen nowhere ' +
      'is a claude spending the user’s quota that they cannot see, open or stop')
    eq(
      dev.runs.map((row) => row.run.run),
      ['r3', 'r2'],
      'most demanding first: awaitingPermission is blocked on the user and outranks a queue',
    )
    eq(dev.status, 'Awaiting permission', 'and the role summarises itself with that run')
    eq(dev.canOpen, true, 'the awaiting run has a session, so the role does offer an Open')

    /* Every role is listed, refused ones included, and each carries exactly one of the two. */
    eq(agentsSection.rows.length, ROLES.length, 'every role is listed, unavailable ones included')
    ok(
      agentsSection.rows.every((row) => gate(row.dispatch).exactlyOne),
      'and every listed role carries exactly one of a button and a sentence',
    )

    const recent = list.find((s) => s.kind === 'recent')
    ok(recent?.rows.length === 2, 'the finished and the failed run land in Recent — the only ' +
      'place a finished transcript is reachable from now the four run sections are gone')
    ok(
      (recent?.rows ?? []).every((row) => !dev.runs.includes(row)),
      'and nothing is in two places at once, which the five-group layout could not promise: a ' +
        'role with a run appeared under both Running and Roles',
    )

    /*
     * A run whose title the board knows shows it; one it does not shows `no task` in dim. The
     * lookup runs over activity lines now rather than over a Running section.
     */
    const withTitle = agentsSection.rows.flatMap((row) => row.runs)
    ok(
      withTitle.some((row) => row.taskTitle === 'Add the retry bar'),
      'a run whose task the board knows shows the title',
    )
    ok(
      withTitle.every((row) => typeof row.glyph === 'string' && row.glyph.length > 0),
      'every activity line carries a glyph',
    )
    const orphaned = sections(READY, {})
    ok(
      orphaned
        .find((s) => s.kind === 'agents')
        .rows.flatMap((row) => row.runs)
        .every((row) => row.taskTitle === null),
      'a title the board does not have is null, and the row draws `no task` in dim',
    )
    // ...and a rogue key in the title map must not become a title.
    const rogueTitles = sections(READY, { 't-14': 'ok' })
    ok(
      rogueTitles
        .find((s) => s.kind === 'recent')
        .rows.every((row) => row.taskTitle === null || typeof row.taskTitle === 'string'),
      'the title lookup never hands back a prototype member',
    )

    /*
     * ===== A live run of a role nothing defines. ==========================================
     *
     * The roster's `agents` come from `.cide/agents/`, the runs come from the registry, and the
     * two disagree the moment somebody deletes a role file while a run of it is in flight. A
     * list built only from `agents` would drop that run off the panel while its `claude` kept
     * working — the failure this repository names most often, reached from a new direction.
     */
    const ghost = run({ run: 'g1', agent: 'ghost', agentLabel: 'Ghost', phase: 'running' })
    const withGhost = sections({ ...READY, runs: [...RUNS, ghost] }, TITLES)
    const ghostRow = withGhost
      .find((s) => s.kind === 'agents')
      .rows.find((row) => row.def.id === 'ghost')
    /*
     * `?.` throughout, and that is not defensive style for its own sake: the failure this block
     * is about *is* the row being missing, so an unguarded read would throw on exactly the input
     * it exists to catch and take every assertion after it down with it. One `FAIL` line per
     * claim beats a stack trace that stops the file.
     */
    ok(ghostRow !== undefined, 'a run of an undefined role still gets a row, invented from itself')
    eq(ghostRow?.runs.length, 1, 'carrying the run, so it can be watched')
    eq(ghostRow?.canOpen, true, 'and opened')
    eq(ghostRow?.def.label, 'Ghost', 'named by the label the run carried at dispatch')
    eq(
      ghostRow?.dispatch.reason,
      ROLE_UNDEFINED,
      'and refused a second run with a sentence of its own — there is no definition to run',
    )
    eq(
      withGhost.find((s) => s.kind === 'agents').rows.at(-1)?.def.id,
      'ghost',
      'after the defined roles, so an anomaly does not reorder the list a user recognises',
    )

    /*
     * A run of an undefined role that has **ended** gets no row: it is in Recent, which draws
     * the label the run carried and needs no definition at all. A row for it would be a
     * subagent on screen that does not exist and is not doing anything.
     */
    const goneRuns = [...RUNS, run({ run: 'g2', agent: 'ghost', phase: 'finished', exitCode: 0 })]
    const gone = sections({ ...READY, runs: goneRuns }, TITLES)
    eq(
      gone.find((s) => s.kind === 'agents').rows.filter((row) => row.def.id === 'ghost').length,
      0,
      'a finished run of a deleted role invents no subagent',
    )
    ok(
      gone.find((s) => s.kind === 'recent').rows.some((row) => row.run.run === 'g2'),
      'it is in Recent instead, where a run needs no role to be drawn',
    )

    /*
     * ===== The phase nobody recognises. ===================================================
     *
     * `isActivePhase` is the **negation of `isDonePhase`** and not membership of
     * `ACTIVE_PHASES`, so a phase from a newer cide is drawn rather than falling between the
     * two lists and disappearing. It sorts last, so it cannot take over a role's summary from a
     * run that is genuinely awaiting permission.
     */
    eq(isActivePhase('teleporting'), true, 'a phase this build cannot read is still active')
    eq(isActivePhase('constructor'), true, '...even a prototype key, which is not a phase at all')
    eq(isActivePhase('finished'), false, 'and the two that really are over are not')
    eq(isActivePhase('failed'), false, 'neither of them')
    ok(
      !ACTIVE_PHASES.includes('finished') && !ACTIVE_PHASES.includes('failed'),
      'the order list names neither terminal phase — a role whose runs have ended is resting',
    )
    for (const phase of ACTIVE_PHASES) {
      eq(isActivePhase(phase), true, `${phase} is active`)
      ok(RUN_PHASES.includes(phase), `${phase} is a real phase`)
    }
    eq(ACTIVE_PHASES[0], 'awaitingPermission', 'the run blocked on the user leads the order')

    const rogueFirst = sections(
      {
        ...READY,
        agents: [def({ id: 'writer', label: 'Writer' })],
        runs: [
          run({ run: 'x1', agent: 'writer', phase: 'teleporting', startedMs: 1 }),
          run({ run: 'x2', agent: 'writer', phase: 'awaitingPermission', startedMs: 2 }),
        ],
      },
      {},
    ).find((s) => s.kind === 'agents').rows[0]
    eq(rogueFirst.runs.length, 2, 'a rogue phase is drawn rather than dropped from the panel')
    eq(
      rogueFirst.runs.map((row) => row.run.run),
      ['x2', 'x1'],
      'and sorts LAST: a value nothing could parse must not outrank a run that is really ' +
        'awaiting permission when the role’s one-word summary is decided',
    )
    eq(rogueFirst.status, 'Awaiting permission', 'so the summary is the readable run’s')
    nonEmptyString(rogueFirst.runs[1].glyph, 'the unreadable one still gets a glyph')

    /* The order within one phase is total, so two rows cannot swap places on a re-render. */
    const tied = sections(
      {
        ...READY,
        agents: [def({ id: 'writer', label: 'Writer' })],
        runs: [
          run({ run: 'b', agent: 'writer', phase: 'running', startedMs: 5 }),
          run({ run: 'a', agent: 'writer', phase: 'running', startedMs: 5 }),
        ],
      },
      {},
    ).find((s) => s.kind === 'agents').rows[0]
    eq(
      tied.runs.map((row) => row.run.run),
      ['a', 'b'],
      'two runs sharing a millisecond fall back to the run id — a partial order leaves rows ' +
        'free to swap places under the pointer',
    )

    ok(RECENT_CAP === 20, 'RECENT_CAP is 20')
    const many = {
      ...READY,
      runs: Array.from({ length: 40 }, (_, i) =>
        run({ run: `f${i}`, phase: 'finished', exitCode: 0, startedMs: 1_700_000_000_000 + i }),
      ),
    }
    const capped = sections(many, {}).find((s) => s.kind === 'recent')
    eq(capped.rows.length, RECENT_CAP, 'Recent is capped')
    eq(capped.rows[0].run.run, 'f39', 'newest first, so the cap keeps what just happened')
  }

  eq(elapsed(1000, 1000), '0s', 'elapsed counts from zero')
  eq(elapsed(1000, 5000), '0s', 'a clock that went backwards yields 0s, never a negative figure')
  eq(elapsed(59_999, 0), '59s', 'seconds up to the minute')
  eq(elapsed(60_000, 0), '1m', 'then minutes')
  eq(elapsed(3_599_000, 0), '59m', 'up to the hour')
  eq(elapsed(3_600_000, 0), '1h 00m', 'then hours, zero-padded so a column does not jitter')
  eq(elapsed(3_840_000, 0), '1h 04m', 'and the mock’s own figure')
  eq(elapsed(Number.NaN, 0), '—', 'a non-finite input prints an em dash, never `NaNs`')

  eq(
    staleTurnLine(run({ staleTurn: false })),
    null,
    'a healthy run says nothing about stale turns',
  )
  nonEmptyString(
    staleTurnLine(run({ staleTurn: true })),
    'a stale turn carries a sentence the bar can print',
  )
  ok(
    /may/.test(staleTurnLine(run({ staleTurn: true }))),
    'and it is hedged — cide cannot see the model request, only a process it froze',
  )

  eq(canOpen(run({ phase: 'queued', session: null })), false, 'a queued run has nothing to open')
  eq(canOpen(run({ phase: 'queued', session: 's1' })), false, 'nor one whose session is premature')
  eq(canOpen(run({ phase: 'finished', session: 's1' })), true, 'a finished run still has its screen')
  eq(
    canOpen(run({ phase: 'interrupted', session: 's1' })),
    true,
    'an interrupted run opens too: its child died with the old cide, its conversation did ' +
      'not, and Open now re-opens the real harness on it rather than mirroring a dead session',
  )
  eq(
    canOpen(run({ phase: 'finished', session: null, openable: true })),
    true,
    'a finished opencode run has no session and an openable conversation — the wire decides',
  )
  eq(
    canOpen(run({ phase: 'finished', session: null })),
    false,
    'and one with neither is withheld, not disabled',
  )
  eq(canPause(run({ phase: 'running' })), true, 'a running child can be frozen')
  eq(canPause(run({ phase: 'paused' })), false, 'an already-frozen one draws Resume instead')
  eq(canPause(run({ phase: 'queued' })), false, 'and a queued one has no child to freeze')

  /* == the grouping and the log ============================================================== */

  {
    const board = ready(4, [
      task({ id: 't-1', status: 'todo' }),
      task({ id: 't-2', status: 'doing' }),
      task({ id: 't-3', status: 'todo', updatedMs: 1_700_000_600_000 }),
      task({ id: 't-4', status: 'review' }),
    ])
    const list = groups(board)
    eq(
      list.map((g) => g.status),
      ['doing', 'review', 'todo'],
      'groups come in GROUP_ORDER, and an empty group is not drawn',
    )
    eq(
      list.find((g) => g.status === 'todo').tasks.map((t) => t.id),
      ['t-3', 't-1'],
      'order within a group is recency — t-3 was touched later and comes first however the ' +
        'file interleaves them; the dedicated block below carries the tie-breaks',
    )
    eq(groups(UNREADABLE), [], 'a non-ready board groups nothing')

    const rogue = groups(ready(1, [task({ id: 't-9', status: 'constructor' })]))
    eq(
      rogue.flatMap((g) => g.tasks.map((t) => t.id)),
      ['t-9'],
      'a task whose status is unreadable is still drawn — dropping it would delete a row from ' +
        'a tracker somebody is relying on',
    )
    eq(rogue[0].status, 'todo', 'and it lands in the group that claims the least')
  }

  /* == the recency order ==================================================================== */

  {
    const order = (list) => list.flatMap((g) => g.tasks.map((t) => t.id))

    eq(
      order(
        groups(
          ready(1, [
            task({ id: 't-1', status: 'todo', updatedMs: 100, createdMs: 100 }),
            task({ id: 't-2', status: 'todo', updatedMs: 300, createdMs: 50 }),
            task({ id: 't-3', status: 'todo', updatedMs: 200, createdMs: 200 }),
          ]),
        ),
      ),
      ['t-2', 't-3', 't-1'],
      'last touched first, off `updatedMs` alone — which Rust stamps on EVERY TaskEdit in one ' +
        'place, so a status change, a new comment, an assignment and a retitle all count as ' +
        'touching. t-2 wins on its update stamp despite being the oldest task on the board',
    )
    eq(
      order(
        groups(
          ready(1, [
            task({ id: 't-1', status: 'todo', updatedMs: 100, createdMs: 10 }),
            task({ id: 't-2', status: 'todo', updatedMs: 100, createdMs: 20 }),
          ]),
        ),
      ),
      ['t-2', 't-1'],
      'equal update stamps fall back to creation, newest first',
    )
    eq(
      order(
        groups(
          ready(1, [
            task({ id: 't-1', status: 'todo', updatedMs: 100, createdMs: 10 }),
            task({ id: 't-2', status: 'todo', updatedMs: 100, createdMs: 10 }),
          ]),
        ),
      ),
      ['t-1', 't-2'],
      'and full ties keep the file’s own order — the sort is stable, so two tasks stamped in ' +
        'the same millisecond cannot swap places between two paints of the same board',
    )
    eq(
      order(
        groups(
          ready(1, [
            task({ id: 't-1', status: 'todo', updatedMs: 100, createdMs: 100 }),
            task({ id: 't-2', status: 'todo', updatedMs: Number.NaN, createdMs: 100 }),
          ]),
        ),
      ),
      ['t-1', 't-2'],
      'a non-finite stamp compares as the epoch and sinks, rather than floating on NaN ' +
        'comparisons — which are not an ordering at all, and hand Array.sort licence to leave ' +
        'the rows in any arrangement it likes. This file is hand-editable JSON two layers down',
    )
  }

  {
    const comment = (atMs, text) => ({ author: { kind: 'user' }, text, atMs })
    const inOrder = task({ comments: [comment(1, 'a'), comment(2, 'b')] })
    ok(
      commentOrder(inOrder) === inOrder.comments,
      'an already-ordered log is returned by identity, so a memoising reader does not rebuild it',
    )
    const jumbled = task({ comments: [comment(3, 'c'), comment(1, 'a'), comment(2, 'b')] })
    eq(
      commentOrder(jumbled).map((c) => c.text),
      ['a', 'b', 'c'],
      'a log a merge interleaved is put back in time order',
    )
    eq(commentOrder(task({ comments: [] })).length, 0, 'an empty log is an empty log')
  }

  /* == who wrote it, and who asked for it =================================================== */

  {
    eq(
      authorLabel({ kind: 'user' }),
      'You',
      'the user’s own lines and the user’s own tasks say You — the second person, because the ' +
        'panel is talking to the person who wrote them',
    )
    eq(authorLabel({ kind: 'orchestrator' }), 'Orchestrator', 'the project’s main session')
    eq(
      authorLabel({ kind: 'agent', agent: 'developer', label: 'Developer' }),
      'Developer',
      'a subagent is named by its label, which is the name the user chose in `.cide/`',
    )
    eq(
      authorLabel({ kind: 'agent', agent: 'developer', label: '   ' }),
      'developer',
      'a role whose label is blank falls back to its id rather than to a blank line — an ' +
        'unattributed line in a log is one no reader can weigh',
    )
    eq(
      authorLabel({ kind: 'agent', agent: '', label: '' }),
      'Agent',
      'and a role that has lost both still gets a word. The ladder is `agentChip`’s, for its ' +
        'reason: a role renamed or deleted out of `.cide/` must not blank a record of what it did',
    )
    eq(
      authorLabel({ kind: 'agent', agent: 'you', label: 'You' }),
      'You',
      'a role *called* You still renders its own label — the point of asserting this is the ' +
        'other direction: `authorLabel` never string-matches a name to decide whose line it is, ' +
        'so this cannot make an agent’s task read as the user’s anywhere it matters. The kind ' +
        'is what the card branches on, and `data-creator` is what the render check reads',
    )
    eq(
      task({}).createdBy.kind,
      'user',
      'and a task carries a creator at all — the field the panel reads to say any of the above',
    )
  }

  /* == the status filter ==================================================================== */

  {
    const board = ready(4, [
      task({ id: 't-1', status: 'todo' }),
      task({ id: 't-2', status: 'doing' }),
      task({ id: 't-3', status: 'review' }),
      task({ id: 't-4', status: 'done' }),
    ])
    const ids = (list) => list.flatMap((g) => g.tasks.map((t) => t.id))

    eq(
      ids(groups(board, null)),
      ['t-2', 't-3', 't-1', 't-4'],
      'no filter is every task, still in GROUP_ORDER',
    )
    eq(
      ids(groups(board)),
      ids(groups(board, null)),
      'and the argument defaults to no filter, so every caller that predates it is unchanged',
    )
    eq(
      groups(board, 'doing').map((g) => g.status),
      ['doing'],
      'a filter narrows the tasks and therefore the headings — no empty Review heading over ' +
        'nothing, because `groups` already omits an empty group',
    )
    eq(ids(groups(board, 'doing')), ['t-2'], 'and only the matching task survives')
    eq(ids(groups(board, 'done')), ['t-4'], 'each of the four is reachable')

    ok(
      TASK_STATUSES.every((status) => matchesFilter(task({ status }), null)),
      'a null filter matches every status there is — "all" is the absence of a filter',
    )
    eq(matchesFilter(task({ status: 'doing' }), 'todo'), false, 'and a set one excludes')
    eq(matchesFilter(task({ status: 'todo' }), 'todo'), true, 'as well as includes')

    /*
     * The rogue status, from the filter's side — and the reason `groupOf` exists as a function
     * rather than as two copies of the same ternary.
     */
    const rogue = task({ id: 't-9', status: 'constructor' })
    eq(groupOf(rogue), 'todo', 'a status this build cannot read is drawn in Todo')
    eq(
      matchesFilter(rogue, 'todo'),
      true,
      'and the Todo filter therefore matches it. A filter reading the raw value would hide a ' +
        'task the unfiltered list had just shown under Todo — the tracker losing a row, ' +
        'arrived at from the filter’s side',
    )
    eq(matchesFilter(rogue, 'doing'), false, 'while every other filter passes over it')
    eq(ids(groups(ready(1, [rogue]), 'todo')), ['t-9'], 'end to end, through `groups`')

    /*
     * The two empties, which are two different sentences.
     */
    eq(listEmpty(BOARD_UNKNOWN, null), null, 'nobody has looked, so the list is not "empty"')
    eq(listEmpty(UNREADABLE, 'doing'), null, 'an unparseable file has its own screen')
    eq(listEmpty(ABSENT, null), null, 'and so does a project with no tracker')
    eq(listEmpty(ready(1, []), null), 'tracker', 'a read, empty tracker is the screen that existed')
    eq(
      listEmpty(ready(1, []), 'doing'),
      'tracker',
      'and it stays that screen under a filter — there is nothing there to have been filtered out',
    )
    eq(listEmpty(board, null), null, 'a populated board with no filter is not empty at all')
    eq(listEmpty(board, 'doing'), null, 'nor one whose filter matches something')
    eq(
      listEmpty(ready(1, [task({ id: 't-1', status: 'todo' })]), 'done'),
      'filter',
      'a filter that matches nothing is its OWN state. Collapsing it into "tracker" would print ' +
        '"No tasks yet" over a board with tasks in it, which is how a user concludes theirs are gone',
    )
    eq(
      listEmpty(ready(1, [rogue]), 'doing'),
      'filter',
      'and the rogue task counts as a task for that purpose, so a board holding only one is ' +
        'never reported as an empty tracker',
    )
  }

  /* == the search =========================================================================== */

  {
    const board = ready(4, [
      task({ id: 't-1', status: 'todo', title: 'Add the retry bar', body: '' }),
      task({ id: 't-2', status: 'doing', title: 'Sweep the phase table', body: 'The retry path is the hard half.' }),
      task({ id: 't-3', status: 'review', title: 'Write the check', body: '' }),
    ])
    const ids = (list) => list.flatMap((g) => g.tasks.map((t) => t.id))

    /*
     * The text rule moved to Rust in M68, and so did its tests: the board no longer carries a
     * body, so `cide_tasks::search` is the one definition of a hit and
     * `search_reads_the_id_the_title_and_the_body_but_never_the_comments` in `cide-tasks` is
     * where *id, title, body, never the comments* is pinned. Restating it here would be the
     * second copy that whole arrangement exists to avoid.
     *
     * What is left in this module is the **seam**, and it has its own failure to prevent.
     */
    const matched = (...ids) => new Set(ids)

    ok(
      matchesQuery(task({ id: 't-1' }), null),
      'null is "no query at all" and matches everything, the way a null StatusFilter does — and ' +
        'it is also what a caller holds while an answer is outstanding',
    )
    ok(matchesQuery(task({ id: 't-2' }), matched('t-1', 't-2')), 'a named id survives')
    eq(
      matchesQuery(task({ id: 't-3' }), matched('t-1', 't-2')),
      false,
      'and one the search did not name does not',
    )
    eq(
      matchesQuery(task({ id: 't-1' }), matched()),
      false,
      'an EMPTY set is a real answer — "the search ran and nothing matched" — and must not be ' +
        'read as "no search": that conflation is what would draw the whole board under a query ' +
        'that genuinely matches none of it',
    )

    eq(
      ids(groups(board, null, matched('t-1', 't-2'))),
      ['t-2', 't-1'],
      'a query narrows the list — still in GROUP_ORDER, still recency inside a group, because ' +
        'a search FILTERS and never re-ranks: the order is the same one the unnarrowed list ' +
        'draws, so a row keeps its place as the query grows and shrinks',
    )
    eq(
      ids(groups(board, 'todo', matched('t-1', 't-2'))),
      ['t-1'],
      'and it composes with the status filter as the intersection — each control is its own ' +
        'claim about the row',
    )
    eq(
      ids(groups(board, null)),
      ids(groups(board, null, null)),
      'the argument defaults to no query, so every caller that predates it is unchanged',
    )

    eq(listEmpty(board, null, 'retry', matched('t-1')), null, 'a query with matches is not empty')
    eq(
      listEmpty(board, null, 'quaternion', matched()),
      'search',
      'one with none is its OWN state, a third sentence rather than a reuse of the filter’s: ' +
        'the way out of this one is clearing what was typed, so the screen has to name the query',
    )
    /*
     * The new silent failure, and the reason `matched` is nullable rather than defaulting to an
     * empty set. Between a keystroke and `task_search`'s answer there is no id set, and a panel
     * that read that state as "nothing matched" would print `No tasks match "retry"` — about a
     * search it had not run yet — over a board that may well contain it. One frame is enough: it
     * is the frame the user is looking at while they type.
     */
    eq(
      listEmpty(board, null, 'retry', null),
      null,
      'a typed query whose answer has NOT landed draws no sentence at all: nothing is claimed ' +
        'about a search that has not run',
    )
    eq(
      listEmpty(ready(1, []), null, 'retry', matched()),
      'tracker',
      'an empty tracker stays the tracker screen under a query — there was nothing to have hidden',
    )
    eq(
      listEmpty(board, 'review', 'retry', matched('t-1')),
      'search',
      'when both narrowings are on and nothing survives, the search takes the blame — the ' +
        'sentence can name the query AND the status it ran inside, where blaming the filter ' +
        'would claim a status hid tasks that a different control is hiding',
    )
    eq(
      listEmpty(board, 'done', '', null),
      'filter',
      'and with no query at all the filter keeps its own screen exactly as before',
    )

    /*
     * `queryAfterCreate` — `filterAfterCreate`'s rule applied to the box: a create the user
     * cannot see reads as a create that did not happen.
     */
    const draft = (over) => ({ ...EMPTY_DRAFT, ...over })
    eq(queryAfterCreate('', draft({ title: 'New' })), '', 'no query, nothing to protect')
    eq(
      queryAfterCreate('retry', draft({ title: 'Fix the retry bar' })),
      'retry',
      'a query the new task matches survives — the row will appear under it, and a user ' +
        'working through a themed backlog keeps their narrowing',
    )
    eq(
      queryAfterCreate('Retry', draft({ title: 'x', body: 'the retry path' })),
      'Retry',
      'matched case-insensitively against the body too — the same hit rule as matchesQuery, ' +
        'shared so the two cannot disagree about what will be visible',
    )
    eq(
      queryAfterCreate('retry', draft({ title: 'Something else' })),
      '',
      'one it does not match is cleared: the dialog closes, and the row must be on the screen ' +
        'behind it rather than hidden by a box the user stopped thinking about',
    )
  }

  /* == the armed delete ===================================================================== */

  {
    const board = ready(9, [task({ id: 't-1' }), task({ id: 't-2' })])

    eq(armedDelete(board, null), null, 'nothing armed, nothing to confirm')
    eq(armedDelete(board, { task: 't-1', rev: 9 }), 't-1', 'an arming on its own board stands')
    eq(
      armedDelete(board, { task: 't-1', rev: 8 }),
      null,
      'a board that has moved on by one rev disarms it. `.cide/tasks.json` has several writers ' +
        '— two windows and every dispatched agent — so the screen really can be replaced ' +
        'between the two clicks, and the second must not land on one the user never saw',
    )
    eq(
      armedDelete(board, { task: 't-1', rev: 10 }),
      null,
      'and so does a rev from the future, which an out-of-order snapshot can produce',
    )
    eq(
      armedDelete(board, { task: 't-9', rev: 9 }),
      null,
      'a task the board does not hold cannot be confirmed against — somebody else deleted it, ' +
        'or it came from the project the user just left',
    )
    eq(
      armedDelete(board, { task: 'constructor', rev: 9 }),
      null,
      'a prototype key is a task id like any other here: `some()` compares values and never ' +
        'consults the prototype chain, which is the trap `in` would have walked into',
    )
    eq(
      armedDelete(UNREADABLE, { task: 't-1', rev: 9 }),
      null,
      'nothing is armable on a board that is not `ready` — which is `canWrite`’s answer too: a ' +
        'confirm button over an unparseable tracker offers to write a file the panel has ' +
        'promised not to touch',
    )
    eq(armedDelete(ABSENT, { task: 't-1', rev: 9 }), null, 'or on one with no file behind it')
    eq(armedDelete(BOARD_UNKNOWN, { task: 't-1', rev: 9 }), null, 'or before anything looked')
  }

  /* == typed links (M30) ==================================================================== */
  //
  // The pure half of the Links section: the direction labels, the derived inverse reading, the
  // related dedupe, and the gone flag. The section's markup claims are `check-agents-render`'s;
  // what belongs here is everything that must hold for ANY board, which no fixture can say.

  {
    for (const kind of [...LINK_KINDS, 'constructor', '']) {
      nonEmptyString(
        linkLabel(kind, 'out'),
        `linkLabel(${JSON.stringify(kind)}, out) is a real label`,
      )
      nonEmptyString(
        linkLabel(kind, 'in'),
        `linkLabel(${JSON.stringify(kind)}, in) is a real label`,
      )
    }
    eq(linkLabel('blockedBy', 'out'), 'Blocked by', 'the stored direction reads as waiting')
    eq(
      linkLabel('blockedBy', 'in'),
      'Blocks',
      'and the derived one as holding up — one stored edge, two readings, and the difference ' +
        'is the difference between being held up and holding work up',
    )
    eq(
      linkLabel('related', 'out'),
      linkLabel('related', 'in'),
      'related reads identically from both ends — it has no direction that matters',
    )
    eq(isLinkKind('blockedBy'), true, 'the wire spelling is the vocabulary')
    eq(isLinkKind('blocked_by'), false, 'snake_case is not — serde writes camelCase')
    eq(isLinkKind('constructor'), false, 'a prototype key is not a kind')

    const linked = task({
      id: 't-1',
      links: [
        { kind: 'blockedBy', target: 't-2' },
        { kind: 'related', target: 't-3' },
        { kind: 'blockedBy', target: 't-99' },
      ],
    })
    const blocker = task({ id: 't-2', title: 'The loader', status: 'doing', links: [] })
    // Both sides hold the related pair — the legal post-merge state the dedupe exists for.
    const relatedBack = task({ id: 't-3', links: [{ kind: 'related', target: 't-1' }] })
    const child = task({ id: 't-4', links: [{ kind: 'subtaskOf', target: 't-1' }] })
    // A rogue kind naming t-1: the derived scan must skip what it cannot read rather than
    // invent a direction label for it.
    const rogue = task({ id: 't-5', links: [{ kind: 'constructor', target: 't-1' }] })
    const board = [linked, blocker, relatedBack, child, rogue]

    eq(
      taskLinks(linked, board).map((c) => `${c.kind}|${c.direction}|${c.target}|${c.gone}`),
      [
        'blockedBy|out|t-2|false',
        'related|out|t-3|false',
        'blockedBy|out|t-99|true',
        'subtaskOf|in|t-4|false',
      ],
      'the whole derivation at once: outgoing edges in stored order, then the derived incoming ' +
        'readings; the related pair drawn ONCE although both sides store it (two chips saying ' +
        'one fact would read as two facts); the dangling t-99 marked gone rather than hidden; ' +
        'and the rogue-kind edge on t-5 invisible to the derived scan',
    )
    eq(
      taskLinks(linked, board).find((c) => c.target === 't-2')?.targetTitle,
      'The loader',
      'a chip resolves its target from the board — the reader decides what to do next from it',
    )
    eq(
      taskLinks(linked, board).find((c) => c.target === 't-99')?.targetTitle ?? null,
      null,
      'and a gone target resolves to nothing, with the flag carried beside it — title-is-null ' +
        'alone could not tell deleted from untitled',
    )
    eq(
      taskLinks(rogue, board).map((c) => `${c.kind}|${c.direction}`),
      ['constructor|out'],
      'a rogue-kind edge still draws on its OWN task — labelled as its raw text, never ' +
        'vanished: the file is hand-editable and a future cide may have written it',
    )

    /* ---------------------------------------------------- the rows those chips become (M40) */

    /*
     * The grouping, which is where the reading is decided.
     *
     * By the direction-resolved LABEL and not by the kind, and the `blockedBy` pair is the whole
     * argument: one stored kind reads as *Blocked by* on the task that waits and *Blocks* on the
     * task waited for, and a heading that collapsed the two would say "this is holding me up"
     * and "I am holding this up" with one word. `related`, whose two readings are the same
     * words, correctly stays one group.
     */
    {
      const chips = taskLinks(linked, board)
      eq(
        linkGroups(chips).map((g) => `${g.label}|${g.chips.map((c) => c.target).join(',')}`),
        ['Blocked by|t-2,t-99', 'Related to|t-3', 'Subtask|t-4'],
        'grouped by the reading, in first-appearance order — so the list’s order is still ' +
          'taskLinks’ (stored edges in file order, then the derived readings) rather than a ' +
          'second ordering nobody asked for, with t-99 pulled up beside the row it shares a ' +
          'heading with',
      )
      const bothWays = taskLinks(
        task({ id: 't-1', links: [{ kind: 'blockedBy', target: 't-2' }] }),
        [task({ id: 't-1', links: [] }), task({ id: 't-2', links: [{ kind: 'blockedBy', target: 't-1' }] })],
      )
      eq(
        linkGroups(bothWays).map((g) => g.label),
        ['Blocked by', 'Blocks'],
        'the two readings of ONE kind are two headings — the assertion this function exists for',
      )
      eq(
        linkGroups(taskLinks(rogue, board)).map((g) => g.label),
        ['constructor'],
        'and a rogue kind groups under its own raw text without touching a prototype: the ' +
          'label is the key, and `constructor` is a label a hand-edited file can produce',
      )
      eq(linkGroups([]).length, 0, 'no edges, no headings')
    }

    /*
     * What a row draws in its marker and its title cell. Both total, both never empty — a blank
     * title cell would read as an untitled task rather than as a missing one, which is the
     * distinction the whole `gone` flag exists to keep.
     */
    {
      const chips = taskLinks(linked, board)
      const at = (target) => chips.find((c) => c.target === target)
      eq(linkTargetStatus(at('t-2')), 'doing', 'a row’s marker is the TARGET’s status')
      eq(
        linkTargetStatus(at('t-99')),
        '',
        'and a target off the board resolves to the empty string, which every `status*` helper ' +
          'already reads as unknown — `circle-slash`, not a status the task does not have',
      )
      eq(statusGlyph(linkTargetStatus(at('t-99'))), 'circle-slash', 'stated as the glyph too')
      eq(linkTargetText(at('t-2')), 'The loader', 'the title, where there is one')
      eq(linkTargetText(at('t-99')), 'Not on the board', 'the plain sentence, where there is not')
      eq(
        linkTargetText(
          draftLinkChips([{ kind: 'related', target: 't-6' }], [
            { id: 't-6', title: '   ', status: 'todo' },
          ])[0],
        ),
        'Untitled',
        'and a real task with a blank title is UNTITLED and not the missing sentence — the two ' +
          'are different facts and the row must not merge them',
      )
      for (const chip of chips) {
        ok(linkTargetText(chip) !== '', `linkTargetText is never empty (${chip.target})`)
      }
    }

    /*
     * The compose dialog's half. Outgoing only, and that is not a simplification: a task that
     * does not exist yet cannot be the target of anything, so there is no incoming reading to
     * derive. Through the same `linkChipFor` as the card, so a draft link and a stored one
     * resolve to the same title and the same marker.
     */
    {
      const options = [
        { id: 't-2', title: 'The loader', status: 'doing' },
        { id: 't-4', title: 'The child', status: 'todo' },
      ]
      const draft = draftLinkChips(
        [
          { kind: 'blockedBy', target: 't-2' },
          { kind: 'subtaskOf', target: 't-4' },
          { kind: 'related', target: 't-99' },
        ],
        options,
      )
      eq(
        draft.map((c) => `${c.label}|${c.target}|${linkTargetText(c)}|${linkTargetStatus(c)}`),
        [
          'Blocked by|t-2|The loader|doing',
          'Subtask of|t-4|The child|todo',
          'Related to|t-99|Not on the board|',
        ],
        'a draft link says what it will MEAN before the task exists — the target’s state and ' +
          'title, not an id to go and look up — and a target that left the board between the ' +
          'pick and the render is marked exactly as the card marks one',
      )
      eq(
        draft.every((c) => c.direction === 'out'),
        true,
        'every draft chip is outgoing: there is no other end yet to read one from',
      )
      eq(draftLinkChips([], options).length, 0, 'no picks, no rows')
    }

    const targets = linkableTargets('t-1', board)
    ok(
      !targets.some((t) => t.id === 't-1'),
      'the picker never offers the task itself — a self-link would gate a task on itself',
    )
    eq(targets.length, board.length - 1, 'and offers everything else')
    eq(
      targets[0]?.id,
      't-2',
      'in the panel’s own reading order — doing before todo, the list condensed into a picker; ' +
        'a second ordering would put the same task in two places on one screen',
    )
    eq(
      linkableTargets(null, board).length,
      board.length,
      'and the compose dialog (no task yet, nothing to exclude) is offered the whole board',
    )

    /* -- the target search: find a task by its key or its summary -------------------------- */

    const T = (id, title, status = 'todo') => ({ id, title, status })
    const list = [
      T('t-2', 'Fix the loader', 'doing'),
      T('t-10', 'Ship the retry bar'),
      T('t-14', 'Teach the parser about tabs'),
      T('t-1', 'Sweep the phase table', 'done'),
    ]

    eq(
      linkTargetOptions('', list).map((o) => o.id),
      ['t-2', 't-10', 't-14', 't-1'],
      'the empty query offers everything IN THE ORDER IT ARRIVED — the input order is the ' +
        'panel order (`linkableTargets`), and the picker is the list condensed, so re-sorting ' +
        'would put the same task in two places on one screen',
    )
    eq(
      linkTargetOptions('t-1', list).map((o) => o.id),
      ['t-10', 't-14', 't-1'],
      'an id query matches by PREFIX first and keeps arrival order within the tier — typing ' +
        'the key is how a person who knows it reaches for it, and t-2 (no match) is gone',
    )
    eq(
      linkTargetOptions('retry', list).map((o) => o.id),
      ['t-10'],
      'a summary word reaches the task whose title says it',
    )
    eq(
      linkTargetOptions('RETRY', list).map((o) => o.id),
      ['t-10'],
      'case-insensitively — a query is typed, not quoted',
    )
    eq(
      linkTargetOptions('loader', list).map((o) => o.id),
      ['t-2'],
      'a word-prefix on the title outranks nothing here, but the tier exists so `t` in a ' +
        'title cannot drown the ids: the row order above is the pinned claim',
    )
    eq(linkTargetOptions('quaternion', list), [], 'and no match is no row, never a guess')
    eq(
      linkTargetOptions('constructor', list),
      [],
      'a prototype key is a query like any other — nothing here does a bare table lookup',
    )
    {
      const many = Array.from({ length: LINK_TARGET_CAP + 5 }, (_, i) =>
        T(`t-${i + 100}`, `Task number ${i + 100}`),
      )
      eq(
        linkTargetOptions('', many).length,
        LINK_TARGET_CAP,
        'the popup is capped: a hundred-row list under a one-line input is a list nobody ' +
          'scans, and every keystroke narrows — the cap is only ever felt on queries too ' +
          'short to mean anything yet',
      )
    }
  }

  /* == the card's read/edit posture ========================================================= */
  //
  // The card is a modal now, and read-only until a field is put into edit. These are the rules
  // that make that a behaviour rather than three event handlers that each remember part of it.
  //
  // The class of bug they exist to prevent is not a crash. It is a user typing a new title,
  // reaching for the assignee, and finding the title back the way it was — silent loss of the
  // one thing on this card the user authored themselves, in a file the whole team commits.

  {
    let intents = 0
    /** Every intent goes through here, so the shape is asserted once for all of them. */
    const drive = (intent, what) => {
      intents += 1
      ok(intent !== null && typeof intent === 'object', `${what}: an intent came back`)
      ok(
        intent.commit === null ||
          (isEditableField(intent.commit.field) && typeof intent.commit.value === 'string'),
        `${what}: the commit names an editable field and carries a string`,
      )
      ok(
        intent.editing === null || isEditableField(intent.editing.field),
        `${what}: the field left in edit is one that can be edited`,
      )
      ok(typeof intent.close === 'boolean', `${what}: says whether the card closes`)
      return intent
    }

    /* -- which fields, and the one that is deliberately left out -------------------------- */

    eq(
      EDITABLE_FIELDS.filter((f) => !TASK_FIELDS.includes(f)),
      [],
      'every editable field is one the card draws',
    )
    eq(
      TASK_FIELDS.filter((f) => !EDITABLE_FIELDS.includes(f)),
      ['status'],
      'status is the ONE field with no edit affordance, and that is a decision rather than an ' +
        'omission: its four buttons cannot be changed by a gesture that was not aimed at one of ' +
        'them, the lit one already IS the read-only rendering, and moving a task along is the ' +
        'gesture the tracker exists for. Every other field rests as text',
    )
    eq(
      sorted(TASK_FIELDS),
      sorted(['title', 'status', 'assignee', 'body']),
      'the card draws four fields — comments are not one of them (the log is append-only and ' +
        'has no affordance here), and neither are links (M30): a field is one value with one ' +
        'pencil, an edge set is add-and-remove with no draft and no single commit, so links ' +
        'are a SECTION like the spec block and this vocabulary stands exactly as it was',
    )
    eq(isEditableField('status'), false, 'and the exclusion is what the predicate says too')
    eq(isEditableField('constructor'), false, 'a prototype key is not a field')
    eq(isTaskField('constructor'), false, 'in either vocabulary')
    eq(EDITABLE_FIELDS.length, new Set(EDITABLE_FIELDS).size, 'no field is listed twice')

    for (const field of [...TASK_FIELDS, 'constructor', '']) {
      nonEmptyString(fieldLabel(field), `fieldLabel(${JSON.stringify(field)}) is a real heading`)
    }

    /* -- what a field reads as at rest --------------------------------------------------- */

    const T = task({ title: 'Add the retry bar', body: 'A frozen run may have lost its turn.' })
    const BARE = task({ title: '', body: '', agent: null })
    const ROLES = { developer: 'Developer', qa: 'QA' }

    for (const [t, label] of [[T, 'a full task'], [BARE, 'an empty one']]) {
      for (const field of [...TASK_FIELDS, 'constructor']) {
        nonEmptyString(
          restText(t, field, ROLES),
          `restText(${field}) is never empty — ${label}. A row that collapsed to its heading is ` +
            'indistinguishable on screen from one the card failed to draw',
        )
      }
    }

    eq(restText(T, 'title', ROLES), 'Add the retry bar', 'a title reads as itself')
    eq(restText(BARE, 'title', ROLES), NO_TITLE, 'and an absent one as a placeholder')
    eq(restText(BARE, 'body', ROLES), NO_BODY, 'as does an absent body')
    eq(restText(T, 'assignee', ROLES), 'Developer', 'the assignee reads as the role LABEL')
    eq(restText(BARE, 'assignee', ROLES), UNASSIGNED, 'and nobody reads as Unassigned')
    eq(
      restText(task({ agent: 'ghost' }), 'assignee', ROLES),
      'ghost',
      'a role the roster no longer defines still reads as the id it was assigned to. Drawing it ' +
        'as Unassigned would be the card telling the user their assignment is gone while the ' +
        'file says otherwise — the roster and the board are two reads of two files',
    )
    eq(
      restText(task({ agent: 'constructor' }), 'assignee', ROLES),
      'constructor',
      'and a prototype key is an agent id like any other: the lookup is `Object.hasOwn`, so it ' +
        'yields the id rather than `Object.prototype.constructor`, which React refuses as a child',
    )
    eq(restText(T, 'status', ROLES), statusLabel(T.status), 'status is answered too, in one place')
    eq(assigneeLabel(null, ROLES), UNASSIGNED, 'the assignee label agrees on nobody')
    eq(assigneeLabel('   ', ROLES), UNASSIGNED, 'and a blank id is not an assignment')

    eq(isFieldEmpty(BARE, 'title'), true, 'an absent title is empty')
    eq(isFieldEmpty(T, 'title'), false, 'and a present one is not')
    eq(
      isFieldEmpty(task({ title: NO_TITLE }), 'title'),
      false,
      'a task whose title IS the placeholder word is not empty — which is why this asks the ' +
        'value rather than comparing `restText` against the constant it would have returned',
    )
    eq(isFieldEmpty(BARE, 'assignee'), true, 'nobody assigned is empty')

    /* -- the editor's value, and the round trip through it -------------------------------- */

    eq(fieldValue(T, 'title'), 'Add the retry bar', 'the editor opens on the value on screen')
    eq(fieldValue(BARE, 'title'), '', 'verbatim, placeholder or not — the editor edits the value')
    eq(fieldValue(BARE, 'assignee'), '', 'an unassigned task edits as the empty option')
    eq(fieldValue(T, 'constructor'), '', 'and an unknown field has no value rather than a function')
    eq(
      assigneeFromDraft(fieldValue(BARE, 'assignee')),
      BARE.agent,
      'the two halves of the assignee mapping round-trip on an unassigned task. If they did ' +
        'not, opening the assignee editor and closing it again would write an agent id of the ' +
        'empty string into a committed file that no roster will ever match',
    )
    eq(
      assigneeFromDraft(fieldValue(T, 'assignee')),
      T.agent,
      '...and on an assigned one',
    )

    eq(
      assignableRoles(T.agent, ROLES),
      ['developer', 'qa'],
      'the assignee editor offers the roster, sorted',
    )
    eq(
      assignableRoles('ghost', ROLES),
      ['developer', 'qa', 'ghost'],
      'with the task’s own role folded in even when the roster has dropped it. A `<select>` ' +
        'whose value is not among its options silently shows the first one — so this task would ' +
        'render as assigned to `developer` and reassign itself the moment it was touched',
    )
    eq(assignableRoles(BARE.agent, ROLES), ['developer', 'qa'], 'and nothing is folded in for nobody')

    /* -- dirtiness, which every rule below is gated on ----------------------------------- */

    eq(isDirty(T, null), false, 'no edit is not dirty')
    eq(isDirty(T, startEdit(T, 'title')), false, 'a freshly opened field is not dirty')
    eq(isDirty(T, { field: 'title', draft: 'Other' }), true, 'a changed draft is')
    eq(
      isDirty(BARE, { field: 'assignee', draft: '' }),
      false,
      'an unassigned task with the empty option chosen is CLEAN. `null` against `""` is the ' +
        'comparison that would otherwise write an `Assign(null)` over a `null` on the way out — ' +
        'bumping `rev` and repainting every window for a change that is not one',
    )
    eq(
      isDirty(T, { field: 'title', draft: `${T.title} ` }),
      true,
      'a trailing space is a change. A card that silently declined to save it would be a second, ' +
        'invisible rule about what the user’s text is',
    )
    eq(startEdit(T, 'body').draft, T.body, 'the draft starts at the value, not at empty')

    /* -- the pencil, and the field that is already in edit -------------------------------- */

    {
      const clean = startEdit(T, 'title')
      const dirty = { field: 'title', draft: 'Add the retry bar, with a reason' }

      const first = drive(beginEdit(T, null, 'title'), 'opening the first field')
      eq(first.commit, null, 'opening a field with nothing else in edit writes nothing')
      eq(first.editing, clean, 'and the editor starts on the value that was on screen')
      eq(first.close, false, 'the card stays up')

      const swapClean = drive(beginEdit(T, clean, 'body'), 'a second field over a clean one')
      eq(swapClean.commit, null, 'leaving an UNCHANGED field writes nothing')
      eq(swapClean.editing.field, 'body', 'and the second field opens')

      const swapDirty = drive(beginEdit(T, dirty, 'body'), 'a second field over a dirty one')
      eq(
        swapDirty.commit,
        { field: 'title', value: dirty.draft },
        'a second activation COMMITS the field being left. Discarding it silently is the wrong ' +
          'answer — it is text the user typed, lost to a click on an unrelated row — and asking ' +
          'would be a confirmation over a dialog. The wire is one variant per field, so there is ' +
          'no half-done form to hold open',
      )
      eq(swapDirty.editing.field, 'body', 'and the second field opens all the same')
      eq(swapDirty.editing.draft, T.body, 'on its own value, not on the one just committed')

      const again = drive(beginEdit(T, dirty, 'title'), 'the pencil on the field already open')
      eq(again.commit, null, 'pressing the pencil twice does not commit-and-reopen')
      ok(again.editing === dirty, '...and keeps the identical draft rather than resetting it')

      const rogue = drive(beginEdit(T, dirty, 'status'), 'the pencil on a field with none')
      eq(rogue.commit, null, 'a field with no editor changes nothing')
      ok(
        rogue.editing === dirty,
        '...and above all does not close the one that is open. A gesture nobody can name must ' +
          'not be able to discard a draft',
      )
      ok(
        beginEdit(T, dirty, 'constructor').editing === dirty,
        'which goes for a prototype key too',
      )
    }

    /* -- Save, Cancel, and the two ways of closing ---------------------------------------- */

    {
      const clean = startEdit(T, 'title')
      const dirty = { field: 'title', draft: 'A different title' }

      const saved = drive(commitEdit(T, dirty), 'Save on a changed field')
      eq(saved.commit, { field: 'title', value: 'A different title' }, 'Save writes the draft')
      eq(saved.editing, null, 'and the field goes back to reading')
      eq(saved.close, false, 'the card stays up — Save is not a way out of the card')

      const savedClean = drive(commitEdit(T, clean), 'Save on an unchanged field')
      eq(
        savedClean.commit,
        null,
        'Save on an UNCHANGED field writes nothing. A `SetTitle` carrying the title the task ' +
          'already has is not a no-op: it bumps `rev`, broadcasts to every window, and puts a ' +
          'line in the diff of a committed file saying nothing happened',
      )
      eq(savedClean.editing, null, 'and it still closes the editor — the user asked it to')

      const cancelled = drive(cancelEdit(), 'Cancel')
      eq(cancelled.commit, null, 'Cancel writes nothing, by construction')
      eq(cancelled.editing, null, 'the field goes back to reading')
      eq(cancelled.close, false, 'and the card stays up')

      const escField = drive(closeCard(T, dirty, 'escape'), 'Escape with a field in edit')
      eq(escField.close, false, 'Escape is scoped to the INNERMOST thing that is open: with a ' +
        'field in edit it cancels the field and the card stays up')
      eq(escField.commit, null, '...writing nothing, which is what makes Escape the safe way out')
      eq(escField.editing, null, '...and leaving nothing in edit')

      const escCard = drive(closeCard(T, null, 'escape'), 'Escape with nothing in edit')
      eq(escCard.close, true, 'with no field open, the same key closes the card')
      eq(escCard.commit, null, 'and still writes nothing')

      const dismissDirty = drive(closeCard(T, dirty, 'dismiss'), 'the scrim over a dirty field')
      eq(
        dismissDirty.commit,
        { field: 'title', value: 'A different title' },
        'a DELIBERATE close — the scrim, the ✕ — commits the field. Clicking away is leaving, ' +
          'and leaving a field has meant "keep what I typed" in this panel since the first ' +
          'version of the card. Escape above is the way to leave without writing',
      )
      eq(dismissDirty.close, true, 'and the card closes')

      const dismissClean = drive(closeCard(T, clean, 'dismiss'), 'the scrim over a clean field')
      eq(dismissClean.commit, null, 'an unchanged field still writes nothing on the way out')
      eq(dismissClean.close, true, 'and the card still closes')

      const dismissNone = drive(closeCard(T, null, 'dismiss'), 'the ✕ with nothing in edit')
      eq(dismissNone.commit, null, 'nothing in edit, nothing to write')
      eq(dismissNone.close, true, 'and the card closes')
    }

    /* -- composing a task that does not exist yet (M21) ------------------------------------ */

    /*
     * The report: *"i'm expecting that all fields are editable and task isn't created while not
     * press Create"*. *New task* used to create the row on the click, so a mis-click put a row
     * titled `New task` into a file the whole team commits — one that had to be deleted rather
     * than abandoned, and that a dispatched agent could read in between.
     *
     * The dialog's rules are here, in the module the check can drive, rather than in the
     * component: which drafts may be created, which are worth protecting, and which ways out
     * are honoured.
     */
    {
      const draft = (over) => ({ ...EMPTY_DRAFT, ...over })

      eq(EMPTY_DRAFT.status, 'todo', 'the dialog opens on `todo`, not on the filter the list ' +
        'happens to be on — a status the user did not choose is one they will not notice, and ' +
        'this one is written into a file their repository tracks')
      eq(EMPTY_DRAFT.assignee, '', 'and on nobody. The empty string IS unassigned, because a ' +
        '`<select>` has no null — `assigneeFromDraft` is the other half of that convention')

      /* -- Create's gate. The title is the one required field. -- */

      eq(draftReady(EMPTY_DRAFT), false, 'an empty draft cannot be created')
      eq(
        draftReady(draft({ title: '   ' })),
        false,
        'nor can a whitespace title — `cide_tasks::validate` refuses it, and a failure notice ' +
          'is a worse way to learn that than a button that is visibly waiting',
      )
      eq(draftReady(draft({ title: 'x' })), true, 'one character is enough')
      eq(
        draftReady(draft({ body: 'a paragraph', assignee: 'qa', status: 'doing' })),
        false,
        'and the other three cannot stand in for it: a task with no title is a row nobody can ' +
          'act on, whatever else is filled in',
      )

      /* -- Dirtiness, which is what the scrim is refused over. -- */

      eq(draftDirty(EMPTY_DRAFT), false, 'a draft nobody has touched is clean')
      eq(
        draftDirty(draft({ title: '  ' })),
        false,
        'and so is a stray space — treating it as dirty would make the scrim stop working with ' +
          'nothing on screen saying why',
      )
      eq(draftDirty(draft({ title: 'x' })), true, 'a typed title is work worth protecting')
      eq(draftDirty(draft({ body: 'x' })), true, '...so is a body')
      eq(draftDirty(draft({ assignee: 'qa' })), true, '...so is choosing an assignee')
      eq(
        draftDirty(draft({ status: 'done' })),
        true,
        '...and so is moving the status off the default, which is a deliberate choice about a ' +
          'field the dialog is the only caller allowed to send',
      )
      eq(
        draftDirty(draft({ links: [{ kind: 'related', target: 't-1' }] })),
        true,
        '...and so is a picked link (M30) — work worth protecting from the scrim, exactly as a ' +
          'typed word is',
      )
      eq(
        draftDirty({ ...EMPTY_DRAFT }),
        false,
        'compared field by field and never by identity: the dialog rebuilds the object on every ' +
          'keystroke, so typing a character and deleting it again leaves a draft that is `!==` ' +
          'the constant and means exactly the same thing',
      )

      /* -- The ways out. Three are honoured always; one is refused while there is work. -- */

      eq(closeCompose(EMPTY_DRAFT, 'cancel'), true, 'Cancel closes an empty dialog')
      eq(
        closeCompose(draft({ body: 'half a paragraph' }), 'cancel'),
        true,
        '...and a full one. Cancel is aimed, and it says what it does',
      )
      eq(closeCompose(EMPTY_DRAFT, 'escape'), true, 'Escape closes an empty dialog')
      eq(
        closeCompose(draft({ title: 'typed' }), 'escape'),
        true,
        '...and a dirty one. Escape always discards here — there is nothing to commit, because ' +
          'the draft never reached Rust, which is the whole point of the dialog',
      )
      eq(closeCompose(EMPTY_DRAFT, 'dismiss'), true, 'the scrim closes a dialog with nothing in it')
      eq(
        closeCompose(draft({ body: 'a paragraph' }), 'dismiss'),
        false,
        'and is REFUSED over a dirty one. The one asymmetry with `closeCard`, where leaving ' +
          'means keeping what you typed so a stray click costs nothing; here leaving means ' +
          'discarding, with no undo and no draft on disk to say it happened',
      )

      /* -- What the list is showing once the task lands. -- */

      eq(
        filterAfterCreate(null, 'todo'),
        null,
        'a list showing everything is left exactly as it is',
      )
      eq(
        filterAfterCreate('doing', 'doing'),
        'doing',
        '...and so is a filter that already admits the new task',
      )
      eq(
        filterAfterCreate('doing', 'todo'),
        null,
        'a filter that would hide it is CLEARED. Filter to Doing, create a Todo task, and the ' +
          'row lands in a group the screen is not drawing: the dialog closes, nothing appears, ' +
          'and the only evidence is a file the user is not looking at',
      )
      eq(
        filterAfterCreate('doing', 'review'),
        null,
        'cleared rather than switched to the new task’s status, which would answer one surprise ' +
          'by hiding whatever the user had chosen to watch',
      )
    }

    /* -- the pure gates ------------------------------------------------------------------- */

    {
      const board = ready(9, [task({ id: 't-1' }), task({ id: 't-2' })])
      const open = board.tasks[0]
      const edit = { field: 'title', draft: 'typing' }

      eq(openTask(board, 't-2').id, 't-2', 'the open task is found by id')
      eq(openTask(board, 't-9'), null, 'an id the board does not hold opens no card')
      eq(openTask(board, null), null, 'and nothing selected opens none')
      eq(
        openTask(board, 'constructor'),
        null,
        'a prototype key finds nothing — `find` compares values and never walks the chain',
      )
      eq(openTask(UNREADABLE, 't-1'), null, 'an unparseable tracker opens no card at all')
      eq(openTask(BOARD_UNKNOWN, 't-1'), null, 'nor does one nobody has read')

      ok(activeEdit(board, open, edit) === edit, 'an edit on a task the board holds stands')
      eq(activeEdit(board, open, null), null, 'nothing in edit stays nothing')
      eq(activeEdit(board, null, edit), null, 'an edit with no card open is not an edit')
      eq(
        activeEdit(board, task({ id: 't-9' }), edit),
        null,
        'an edit on a task the board no longer holds is refused — somebody else deleted it, or ' +
          'it came from the project the user just left. React runs effects after paint, so the ' +
          'host clearing its state still leaves one frame with a Save button aimed at nothing',
      )
      eq(
        activeEdit(UNREADABLE, open, edit),
        null,
        'and nothing is editable over an unparseable tracker — `canWrite`’s answer, arrived at ' +
          'from the card’s side: an editor there offers to write a file the panel has promised ' +
          'not to touch',
      )
      eq(activeEdit(BOARD_UNKNOWN, open, edit), null, 'or before anything looked')
      eq(
        activeEdit(board, open, { field: 'status', draft: 'doing' }),
        null,
        'a field with no editor is not in edit however the state got that way',
      )
      ok(
        activeEdit(ready(10, board.tasks), open, edit) === edit,
        'a NEW rev does not clear an edit, and that is the difference from `armedDelete`. An ' +
          'arming is a claim about a screen that has been replaced; a draft is the user’s own ' +
          'sentence, and an agent commenting on the task must not take it away from them',
      )
    }

    ok(intents >= 13, `${intents} intents driven — the block above still runs`)
  }

  /* == the roles join, the hint under the dropdown, and the mention model ==================== */
  //
  // The assignee dropdown shipped empty for the whole life of one milestone because the host
  // passed `{}` where the roster's map belonged — a wiring bug at the one seam neither check
  // mounts. `rosterRoles` is now the join in model form, `assigneeHint` is the sentence that
  // keeps a legitimately-short list from reading as that bug, and the mention model is the
  // pure half of the @-popup. All three live here so the empty-forever state has a gate.

  {
    const join = (agents) => rosterRoles({ kind: 'ready', agents, runs: [], dispatching: true })

    for (const [name, roster] of [
      ['disabled', DISABLED],
      ['empty', EMPTY],
      ['unknown', ROSTER_UNKNOWN],
    ]) {
      eq(
        Object.keys(rosterRoles(roster)).length,
        0,
        `rosterRoles(${name}) offers nothing — a roster that is not ready has no ids to sell`,
      )
    }

    const roles = join([
      def({ id: 'constructor', label: 'Ctor' }),
      def({ id: '  ', label: 'Ghost' }),
      def({ id: 'qa', label: '' }),
      def({ id: 'developer', label: 'Developer' }),
    ])
    ok(
      Object.hasOwn(roles, 'constructor') && roles['constructor'] === 'Ctor',
      'an id off Object.prototype is an own entry, not an inherited function',
    )
    eq(roles['qa'], 'qa', 'a blank label falls back to the id, so the option still has a face')
    ok(
      !Object.keys(roles).some((id) => id.trim() === ''),
      'a blank id is dropped — its empty value would collide with the Unassigned option',
    )
    eq(
      assignableRoles(null, join(ROLES)),
      ['artist', 'blank', 'developer', 'qa', 'writer'],
      'the join round-trips into the dropdown sorted, whole roster in',
    )

    /* -- the hint --------------------------------------------------------------------------- */

    ok(
      assigneeHint('disabled').startsWith(OFF_FOR_THIS_PROJECT),
      'the disabled hint opens with the Agents panel’s own sentence, word for word — a ' +
        'deliberate second copy (this module is import-free so this script can compile it ' +
        'alone), and this line is what pins the two together',
    )
    ok(
      assigneeHint('empty').includes('.cide/agents/'),
      'the empty hint names the file that would add a role — the only action there is',
    )
    eq(assigneeHint('ready'), null, 'a ready roster needs no excuse')
    eq(
      assigneeHint('unknown'),
      null,
      'nobody-has-looked draws nothing — the roster convention; a hint here would claim ' +
        'knowledge this window does not have',
    )
    eq(assigneeHint('bogus'), null, 'an unrecognised kind claims nothing')
  }

  {
    const { mentionQuery, mentionOptions, applyMention } = mentions
    let mentionRows = 0
    const q = (text, caret, expected, why) => {
      mentionRows += 1
      eq(mentionQuery(text, caret), expected, why)
    }

    // Opens exactly where the Rust parser (`cide-agents/src/mentions.rs`) would act — a popup
    // that opens where the parser will not, or stays shut where it will, teaches a grammar the
    // system does not have.
    q('hi @dev', 7, { start: 3, end: 7, query: 'dev' }, 'a token under the caret opens')
    q('@', 1, { start: 0, end: 1, query: '' }, 'a bare @ at the start opens with everything')
    q('(@dev', 5, { start: 1, end: 5, query: 'dev' }, 'punctuation before the @ opens — parser parity')
    q('@dev', 2, { start: 0, end: 2, query: 'd' }, 'the caret mid-token queries what is behind it')
    q('a@b', 3, null, 'an alphanumeric before the @ closes it — user@example.com is prose')
    q('mail user@example', 14, null, 'the email case, spelled out')
    q('@dev x', 6, null, 'a space ends the token; the popup does not reopen from later prose')
    q('@dev', 0, null, 'no caret, no token')
    q(`@${'a'.repeat(45)}`, 46, null, 'a token past any legal id closes rather than scans on')

    const roles = {
      developer: 'Developer',
      qa: 'QA',
      'code-reviewer': 'Code Reviewer',
      '': 'Ghost',
    }
    const ids = (query) => mentionOptions(roles, query).map((option) => option.id)
    eq(
      ids(''),
      ['code-reviewer', 'developer', 'qa'],
      'the empty query offers the whole roster, id-sorted, blank ids dropped',
    )
    eq(ids('dev'), ['developer'], 'an id prefix wins')
    eq(
      ids('rev'),
      ['code-reviewer'],
      'a label word-prefix matches — the author’s own words are searchable',
    )
    eq(ids('zz'), [], 'no match, no rows — the component closes an empty popup')
    eq(
      mentionOptions({ constructor: 'Ctor' }, 'c').map((option) => option.label),
      ['Ctor'],
      'a prototype-key id ranks like any other',
    )

    const applied = applyMention('hi @dev x', { start: 3, end: 7, query: 'dev' }, 'developer')
    eq(applied.text, 'hi @developer  x', 'the splice replaces the token with `@<id> `')
    eq(applied.caret, 14, 'the caret lands after the inserted space, ready for prose')
    eq(
      mentionQuery(applied.text, applied.caret),
      null,
      'the insert closes the popup — the trailing space is what does it',
    )

    ok(mentionRows >= 9, `${mentionRows} mentionQuery rows driven — the table above still runs`)
  }

  /* == the formatting tools (M27) ============================================================ */
  //
  // `markdownTools.ts` is the toolbar's pure half — what pressing Bold does to the three
  // characters you selected — and the only alternative home for these rules is a DOM event
  // handler nothing in this repository can run. The properties that matter: every inline tool
  // is a *toggle* (press-press is identity, or the button is a one-way ratchet whose undo is
  // hand-deleting syntax it wrote), every transform is *total* (a `data-tool` nobody can name
  // must not eat a draft), and the selection lands where the next press or keystroke expects it.
  {
    const { MARKDOWN_TOOLS, applyTool, isMarkdownTool } = mdTools

    ok(MARKDOWN_TOOLS.length === 8, 'eight tools — the set the toolbar draws, no more')
    eq(
      MARKDOWN_TOOLS.length,
      new Set(MARKDOWN_TOOLS.map((spec) => spec.tool)).size,
      'no tool named twice',
    )
    ok(
      MARKDOWN_TOOLS.every((spec) => spec.icon.trim() !== '' && spec.label.trim() !== ''),
      'every tool carries an icon name and a spoken label — `check-ui-icons.mjs` pins the ' +
        'names against the vendored set',
    )
    ok(MARKDOWN_TOOLS.every((spec) => isMarkdownTool(spec.tool)), 'and the guard admits each')
    ok(!isMarkdownTool('constructor'), 'while a prototype key is not a tool')

    // The inline toggles. The selection in the result is over the content, which is exactly
    // what makes the second press find the markers outside it and take them off.
    const bold = applyTool('press word here', 6, 10, 'bold')
    eq(bold.text, 'press **word** here', 'Bold wraps the selection')
    eq([bold.selStart, bold.selEnd], [8, 12], 'and keeps it over the word')
    eq(
      applyTool(bold.text, bold.selStart, bold.selEnd, 'bold').text,
      'press word here',
      'pressing Bold again is the undo — a toggle, not a ratchet',
    )
    eq(
      applyTool('press **word** here', 6, 14, 'bold').text,
      'press word here',
      'selecting the markers along with the word unwraps too',
    )
    eq(
      applyTool('**word**', 2, 6, 'italic').text,
      '***word***',
      'Italic inside a bold pair nests rather than eating one of bold’s stars',
    )
    const empty = applyTool('', 0, 0, 'code')
    eq(empty.text, '`code`', 'an empty selection gets a placeholder')
    eq([empty.selStart, empty.selEnd], [1, 5], 'selected, so the next keystroke replaces it')

    // The line tools: whole lines, toggles, families that replace one another.
    eq(applyTool('title', 2, 2, 'heading').text, '## title', 'Heading works from a bare caret')
    eq(applyTool('## title', 0, 0, 'heading').text, 'title', 'and toggles off')
    eq(applyTool('a\nb', 0, 3, 'quote').text, '> a\n> b', 'Quote prefixes every selected line')
    eq(
      applyTool('1. a\n2. b', 0, 9, 'bullet').text,
      '- a\n- b',
      'switching families replaces the marker instead of stacking `- 1. `',
    )
    eq(
      applyTool('a\n\nb', 0, 4, 'ordered').text,
      '1. a\n\n2. b',
      'Numbered list counts items, not lines — a blank separator is neither prefixed nor numbered',
    )
    eq(applyTool('x\nx', 0, 3, 'ordered').text, '1. x\n2. x', 'duplicate lines still number apart')

    // The link, three shapes: the selection is the half the user supplied, and the selection
    // in the result is the half they still owe.
    const url = applyTool('see https://x.invalid/a', 4, 23, 'link')
    eq(url.text, 'see [text](https://x.invalid/a)', 'a selected URL becomes the destination')
    eq(url.text.slice(url.selStart, url.selEnd), 'text', 'with the label placeholder selected')
    const prose = applyTool('the docs', 4, 8, 'link')
    eq(prose.text, 'the [docs](url)', 'selected prose becomes the label')
    eq(prose.text.slice(prose.selStart, prose.selEnd), 'url', 'with the destination owed')

    // Total, for hostile and out-of-range input alike.
    eq(
      applyTool('draft', 0, 5, 'constructor'),
      { text: 'draft', selStart: 0, selEnd: 5 },
      'a tool this build cannot name changes nothing — the draft survives',
    )
    eq(applyTool('ab', 9, -3, 'bold').text, '**ab**', 'a selection outside the text clamps')
  }

  /* == the clock stamp, and the status log's order (M27) ===================================== */
  //
  // `clock` is asserted by shape rather than by value — the clock half is local time, and this
  // check runs in whatever timezone the machine has. `historyOrder` gets `commentOrder`'s three
  // assertions because it makes `commentOrder`'s three promises.
  {
    const { clock, historyOrder } = tasks
    ok(
      /^\d{2}:\d{2}:\d{2}$/.test(clock(1_700_000_000_000)),
      'a millisecond stamp renders as HH:MM:SS, 24-hour, always eight characters',
    )
    eq(clock(Number.NaN), '--:--:--', 'a value that is not a time is visibly broken, never NaN:NaN:NaN')
    eq(clock(Number.POSITIVE_INFINITY), '--:--:--', 'in either direction')

    const hop = (from, to, atMs) => ({ from, to, by: { kind: 'user' }, atMs })
    const scrambled = {
      history: [hop('doing', 'review', 3), hop('todo', 'doing', 1), hop('review', 'doing', 2)],
    }
    eq(
      historyOrder(scrambled).map((h) => h.atMs),
      [1, 2, 3],
      'the status log is oldest first whatever order the file holds — a merge can interleave',
    )
    const ordered = { history: [hop('todo', 'doing', 1), hop('doing', 'review', 2)] }
    ok(
      historyOrder(ordered) === ordered.history,
      'already-ordered history comes back as the identical array, so a memoising reader does not rebuild',
    )
  }

  // --- 7. the wire loop nothing else closes -------------------------------------------------
  //
  // `xtask contract-check` proves `generate_handler!` and `contract/commands.json` agree. Nothing
  // proves either agrees with `ui/src/ipc/client.ts` — and its own failure message asks for it in
  // prose ("make sure the frontend client in ui/src/ipc/client.ts moved with it") that no gate
  // enforces. M18 shipped four `#[tauri::command]`s no caller could reach — registered, in the
  // contract, unit-tested, clippy-clean, and invocable from nothing — and every existing gate was
  // satisfied, each for a good reason: `contract-check` detects *drift*, not reachability, and
  // `check:commands` walks the palette registry, which these had no row in. The opposite error
  // happened in the same milestone: a command that *was* wired and was the wrong design.
  //
  // So: a command the contract lists must be spelled somewhere in the client, and a command the
  // client invokes must be one the contract lists. Scoped to M18's own prefixes rather than the
  // whole surface, because `WindowFrame.tsx` is a documented second seam and window controls are
  // deliberately not commands — widening this is a separate job with its own exceptions to state.
  const contract = JSON.parse(read('../../contract/commands.json'))
  const client = read('../src/ipc/client.ts')
  const MINE = /^(agents?_|tasks?_)/

  let wired = 0
  for (const name of contract.filter((c) => MINE.test(c))) {
    ok(client.includes(`'${name}'`), `contract command \`${name}\` is invoked from client.ts`)
    wired += 1
  }
  for (const m of client.matchAll(/invoke<[^>]*>\(\s*'([a-z_]+)'/g)) {
    const name = m[1]
    if (!MINE.test(name)) continue
    ok(contract.includes(name), `client.ts invokes \`${name}\`, which the contract lists`)
  }

  /* == attachments (M39) ==================================================================== */

  {
    /* The kinds, pinned the way the statuses are: a kind Rust gained and the panel has not is a
       file the tracker can hold and the card cannot decide how to draw. */
    const rustKinds = variants(tasksRs, 'pub enum AttachmentKind {', 'AttachmentKind')
    ok(rustKinds.length === 2, `read ${rustKinds.length} AttachmentKind variants — the scan still matches`)
    eq(sorted(ATTACHMENT_KINDS), sorted(rustKinds), 'ATTACHMENT_KINDS is exactly `pub enum AttachmentKind`')

    /* One size ladder for a person and an agent: `cide_agents::tools::human_size` prints the
       same three rungs, and the two must agree on a file both are looking at. */
    eq(formatBytes(0), '0 B', 'bytes below a KiB are bytes')
    eq(formatBytes(1023), '1023 B', 'up to and not including 1024')
    eq(formatBytes(1024), '1 KiB', 'one KiB, whole')
    eq(formatBytes(12 * 1024 + 900), '12 KiB', 'KiB are floored, not rounded — Rust divides')
    eq(formatBytes(1024 * 1024), '1.0 MiB', 'MiB carry one decimal')
    eq(formatBytes(1024 * 1024 * 1.5), '1.5 MiB', 'and it is a real decimal')

    eq(basename('/home/u/shots/one.png'), 'one.png', 'a POSIX path ends in its name')
    eq(basename('C:\\Users\\u\\one.png'), 'one.png', 'and so does a Windows one — the desktop chooses')
    eq(basename('one.png'), 'one.png', 'a bare name is its own basename')
    eq(basename('/ends/in/slash/'), '/ends/in/slash/', 'a path with no last component is left as typed rather than emptied')

    /* The lightbox walk: the body's images, then each comment's in log order — and the log's
       order is `commentOrder`, not the array's, so a scrambled file still walks top to bottom. */
    const img = (id, addedMs) => ({ id, name: `${id}.png`, bytes: 1, kind: 'image', addedBy: { kind: 'user' }, addedMs })
    const doc = (id) => ({ id, name: `${id}.log`, bytes: 1, kind: 'file', addedBy: { kind: 'user' }, addedMs: 1 })
    const c = (id, atMs, attachments) => ({ id, author: { kind: 'user' }, text: id, atMs, editedMs: null, attachments })
    const walked = imageAttachmentsOf({
      id: 't-1', title: 't', body: '', status: 'todo', agent: null, change: null, session: null, links: [],
      attachments: [img('b1', 1), doc('b2'), img('b3', 2)],
      comments: [c('later', 200, [img('l1', 1)]), c('earlier', 100, [doc('e0'), img('e1', 1)])],
      history: [], createdBy: { kind: 'user' }, createdMs: 0, updatedMs: 0,
    }).map((a) => a.id)
    eq(walked, ['b1', 'b3', 'e1', 'l1'], 'body first, then comments oldest first, files skipped')

    /* The drop target's two directions round-trip, and a key nothing minted parses to nothing. */
    for (const target of [
      { kind: 'task', task: 't-1' },
      { kind: 'comment', task: 't-1', comment: 'c-9' },
      { kind: 'composer', task: 't-1' },
      { kind: 'compose' },
    ]) {
      eq(parseDropTarget(dropTargetKey(target)), target, `${dropTargetKey(target)} round-trips`)
    }
    for (const bad of ['', 'task', 'task:', 'comment:t-1', 'comment:t-1:', 'compose:x', 'nope:t-1', 'composer:']) {
      eq(parseDropTarget(bad), null, `\`${bad}\` is not a target`)
    }

    /* Physical to CSS pixels: one division, and a zero ratio (a webview that has not measured
       itself yet) must not produce Infinity and a hit test that never lands. */
    eq(cssPoint({ x: 200, y: 100 }, 2), { x: 100, y: 50 }, 'divided by the display scale')
    eq(cssPoint({ x: 200, y: 100 }, 0), { x: 200, y: 100 }, 'a scale of zero is treated as one')

    /* The innermost zone wins, so a comment inside the card takes the file rather than the card. */
    const zones = [
      { key: 'task:t-1', left: 0, top: 0, width: 400, height: 800 },
      { key: 'comment:t-1:c-1', left: 20, top: 500, width: 360, height: 100 },
      { key: 'composer:t-1', left: 20, top: 700, width: 360, height: 80 },
    ]
    eq(dropZoneAt({ x: 100, y: 100 }, zones), 'task:t-1', 'the card body')
    eq(dropZoneAt({ x: 100, y: 550 }, zones), 'comment:t-1:c-1', 'the comment inside it')
    eq(dropZoneAt({ x: 100, y: 750 }, zones), 'composer:t-1', 'the composer inside it')
    eq(dropZoneAt({ x: 500, y: 100 }, zones), null, 'outside every zone is nowhere')
    eq(dropZoneAt({ x: 100, y: 100 }, []), null, 'and no zones is nowhere')
  }

  if (failed > 0) {
    console.error(`\ncheck-agents: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-agents: ok (${TASK_STATUSES.length} statuses, ${LINK_KINDS.length} link kinds and ${RUN_PHASES.length} phases pinned ` +
      `to Rust, ${tableEntries} table entries non-empty, ${gated} role×roster pairs through ` +
      `the gate, ${chips} agentChip inputs survived, ${wired} commands wired end to end)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
