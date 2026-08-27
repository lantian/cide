/**
 * Checks `src/settings/agentsDraft.ts` — the rules behind Settings → Agents — and pins them to
 * the Rust that defines the same facts.
 *
 * Same shape as `check-theme.mjs` and `check-claude-cli.mjs`, and for the same reason: this
 * project has no JS test runner, `agentsDraft.ts` is deliberately import-free so the TypeScript
 * already in `node_modules` can compile it standalone, and a rule that lives inside a React
 * component is a rule no script can drive. If the compile below ever needs a tsconfig, something
 * has added an import and the node-testability of the rules has been lost.
 *
 * # The four failure classes this makes unrepresentable
 *
 * **A refusal with nowhere to land.** `cide_ipc::AgentField` exists so that a rejected save can
 * put each message under the box it is about — its own doc says a form that answers "invalid"
 * without saying where is a form the user cannot fix. That only works if *every* variant the Rust
 * enum can produce is a field this form actually draws an error slot for. So the enum is sliced
 * out of `crates/cide-ipc/src/agents.rs` and compared, as a set, with `AGENT_FIELDS` — and then
 * with the `errorsFor('…')` calls in `AgentsSection.tsx`, because a field in a list and a field
 * on screen are two different claims. A variant with nowhere to show its message is a message the
 * user never sees.
 *
 * **A vocabulary that drifts from Rust.** `AgentScope` and `Harness` are enums and
 * `cide_agents::defs::PERMISSION_MODES` is a const slice that Rust *refuses against*. All three
 * are restated in TypeScript so the form can draw them as choices rather than as free text, and
 * all three are compared here name for name. A mode added in Rust and not here is a mode the user
 * cannot pick; one added here and not there is a value the save refuses.
 *
 * **A closed list that closed over something open.** `effort` and `tools` are deliberately *not*
 * checked against anything, because their vocabularies belong to a CLI that updates itself
 * underneath a running cide — `AgentDraft::effort` says "validated against nothing" and
 * `KNOWN_TOOLS` says an unknown name must warn and never refuse. The obvious tidy is to make
 * `effort` a union like the other three, which would start silently refusing a knob a release
 * added last week. Both halves of that asymmetry are asserted.
 *
 * **A round trip to a refusal that was knowable.** An empty name and an empty system prompt can
 * never be written, so sending them is a file write that was never going to happen and a user
 * left waiting for it. `localProblems` refuses exactly those two, and this file asserts both that
 * it refuses them and that it refuses *nothing else* — a local validator that grows is a second
 * authority, which `cide_agents::defs::validate` argues against at length.
 *
 * **A gesture that arrives and is thrown away.** The Agents panel's Configure records the role
 * that was clicked and opens this screen. Three things can go wrong with that hand-off and all
 * three are silent: the request is consumed twice and jumps a *later* visit to a role nobody
 * asked about (`layout/spawnPlans.ts`'s bug in a different costume); it is answered before the
 * file listing arrives and reported as "no such role" in the normal case; or it lands on a dirty
 * form and replaces a half-written system prompt. `focusTarget`, `focusAction` and
 * `screenOpening` are those three decisions, and each one is driven here.
 *
 * **A dialog that cannot be left, or cannot be left safely.** The form is a modal now.
 * `closeRequest` is the rule that Escape closes a clean dialog and *asks* on a dirty one, and
 * never discards; `restoredModal` is the rule that a draft which survived an unmount comes back
 * on screen rather than living invisibly in `DRAFTS`. The rest of the dialog — where focus
 * starts, that Tab is trapped, that it is `OverlayCard` and therefore `position: fixed` rather
 * than something that scrolls with the settings pane — is asserted against the *source*, below,
 * which is weaker and is labelled as such.
 *
 * # What this does NOT cover, and nothing here should be read as claiming
 *
 *   - that the section renders. There is no DOM in this process. `tsc --noEmit` over
 *     `AgentsSection.tsx` is what pins the structural restatements in `agentsDraft.ts` to the
 *     generated wire types, and `check-agents.mjs` is what proves the three commands are wired.
 *   - **that the dialog behaves.** No settings section is SSR-rendered by any check in this
 *     suite, so initial focus, the Tab trap, the Escape route and the scrim are argued from the
 *     code and asserted as source patterns — never observed. A rename that kept the pattern and
 *     changed the behaviour would pass. The rules those handlers *call* are the part that is
 *     genuinely driven.
 *   - that Rust's *messages* say anything in particular. Every sentence on a refusal is Rust's
 *     and arrives on the wire; this side is asserted to have somewhere to put it.
 *   - that a save works. Nothing here touches a file or spawns a process.
 *
 * Run: `pnpm --dir ui run check:settings-agents`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-settings-agents-'))

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
  source.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/\/\/[^\n]*/g, ' ')

/** Drop everything from the first `#[cfg(test)]`. A gate that reads a fixture measures it. */
const shipping = (source) => source.split(/#\[cfg\(test\)\]/)[0]

/**
 * The body of a Rust item, found by its opening line and its `\n}`.
 *
 * Lifted from `check-commands.mjs`, which reads `fn build()` the same way, and used here exactly
 * as `check-agents.mjs` uses it on an enum: both enums below have their closing brace in column
 * zero and every variant on one line, so the first `\n}` after the opening ends the item.
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
 * Every enum read here carries `#[serde(rename_all = "camelCase")]`, and camelCasing a
 * PascalCase identifier is lowercasing its first letter — `PermissionMode` → `permissionMode`.
 * A regex rather than a parser, on `check-commands.mjs`' argument; the count assertion at each
 * call site is what stops a change of shape from silently matching nothing and passing.
 */
function variants(source, opening, what) {
  const body = stripComments(rustBody(source, opening, what))
  const inner = body.slice(body.indexOf('{') + 1)
  return [...inner.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*(\{|\(|,|$)/gm)].map(
    (m) => m[1].charAt(0).toLowerCase() + m[1].slice(1),
  )
}

/** The string literals of a `pub const NAME: &[&str] = &[ … ];` slice. */
function rustStrSlice(source, name) {
  const start = source.indexOf(`pub const ${name}: &[&str] = &[`)
  if (start < 0) throw new Error(`could not find ${name}`)
  const end = source.indexOf('];', start)
  if (end < 0) throw new Error(`could not find the end of ${name}`)
  return [...source.slice(start, end).matchAll(/"([^"]+)"/g)].map((m) => m[1])
}

const sorted = (list) => [...list].sort()

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/settings/agentsDraft.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The project sets it and this module is written for it.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit', cwd: UI },
  )

  const {
    AGENT_FIELDS,
    CLOSED_VOCABULARIES,
    EFFORT_SUGGESTIONS,
    HARNESSES,
    PERMISSION_MODES,
    isClaudeScope,
    REQUIRED_FIELDS,
    SCOPES,
    blankDraft,
    canSave,
    closeRequest,
    fileKey,
    focusAction,
    focusTarget,
    fromWire,
    isClosedVocabulary,
    isDirty,
    localProblems,
    modalFor,
    modalTitle,
    problemsByField,
    restoredModal,
    rowsFor,
    scopeChangeWarning,
    screenOpening,
    shouldRestore,
    titleCase,
    toWire,
    usability,
    usabilityLabel,
  } = await import(`file://${join(out, 'agentsDraft.js')}`)

  const agentsRs = shipping(read('../../crates/cide-ipc/src/agents.rs'))
  const defsRs = shipping(read('../../crates/cide-agents/src/defs.rs'))
  const sectionsTsx = read('../src/settings/sections.tsx')
  const sectionTsx = read('../src/settings/AgentsSection.tsx')
  const sectionCode = stripComments(sectionTsx)
  const clientTs = read('../src/ipc/client.ts')
  const storeTs = read('../src/sidebar/agentsStore.ts')
  const storeCode = stripComments(storeTs)
  const modalShellTsx = read('../src/overlays/ModalShell.tsx')
  const overlayCss = read('../src/overlays/Overlay.module.css')

  /* ------------------------------------------------------- every field a refusal can name */

  const rustFields = variants(agentsRs, 'pub enum AgentField {', 'AgentField')
  ok(rustFields.length >= 10, `read ${rustFields.length} AgentField variants — the regex still matches`)
  eq(
    sorted(rustFields),
    sorted(AGENT_FIELDS),
    'every AgentField variant is a field the form knows, and no more — a variant missing here ' +
      'is a refusal with nowhere to land, and an extra one is a field Rust can never speak about',
  )

  /*
   * …and the same claim about the *screen*, which is the one that actually matters. A field can
   * be in `AGENT_FIELDS` and still have no error slot in the JSX, in which case Rust's sentence
   * is fetched, bucketed, and thrown away one line before it would have been drawn.
   *
   * Comment-stripped, on `check-claude-cli.mjs`' argument: this component explains its fields by
   * name at length, and a grep over raw source would happily match the paragraph about a field
   * whose control had been deleted.
   */
  const drawn = [...sectionCode.matchAll(/errorsFor\('([A-Za-z]+)'\)/g)].map((m) => m[1])
  ok(drawn.length >= 10, `found ${drawn.length} errorsFor() calls in AgentsSection.tsx`)
  eq(
    sorted(rustFields).filter((field) => !drawn.includes(field)),
    [],
    'every AgentField variant has an errorsFor() call in AgentsSection.tsx, so its message has ' +
      'a box to appear under',
  )
  eq(
    sorted([...new Set(drawn)]).filter((field) => !rustFields.includes(field)),
    [],
    'and every errorsFor() call names a field Rust can actually produce',
  )

  /* ------------------------------------------------------------------ closed vocabularies */

  const rustScopes = variants(agentsRs, 'pub enum AgentScope {', 'AgentScope')
  eq(sorted(rustScopes), sorted(SCOPES), 'SCOPES is cide_ipc::AgentScope, as a set')
  eq(
    SCOPES.length,
    4,
    'four scopes: cide’s project and global directories, and Claude Code’s two — see ' +
      'cide_ipc::AgentScope’s "two families, four directories"',
  )
  // The families are a *behavioural* split — the name rule, the harness control, the scope
  // control and whether Save may create all turn on it — so the predicate is pinned rather than
  // left to four comparisons that can drift apart.
  eq(
    SCOPES.filter((scope) => isClaudeScope(scope)),
    ['claudeProject', 'claudeGlobal'],
    'isClaudeScope names exactly the two directories cide does not own',
  )

  const rustHarnesses = variants(agentsRs, 'pub enum Harness {', 'Harness')
  eq(sorted(rustHarnesses), sorted(HARNESSES), 'HARNESSES is cide_ipc::Harness, as a set')

  const rustModes = rustStrSlice(defsRs, 'PERMISSION_MODES')
  ok(rustModes.length >= 4, `read ${rustModes.length} permission modes out of defs.rs`)
  eq(
    sorted(rustModes),
    sorted(PERMISSION_MODES),
    'PERMISSION_MODES is cide_agents::defs::PERMISSION_MODES, as a set — a mode Rust refuses ' +
      'must not be offered, and one Rust accepts must not be unreachable from the form',
  )

  eq(
    sorted(Object.keys(CLOSED_VOCABULARIES)),
    ['harness', 'permissionMode', 'scope'],
    'exactly three fields take their value from a closed set',
  )
  for (const field of ['scope', 'harness', 'permissionMode']) {
    ok(isClosedVocabulary(field), `${field} is closed`)
  }
  /*
   * The other half of the asymmetry, and the half that a tidy-up deletes. `effort` and `tools`
   * are open ON PURPOSE — the sets belong to a CLI that updates itself underneath a running
   * cide — so this is not "we did not get round to it".
   */
  for (const field of ['effort', 'tools', 'model', 'name', 'label']) {
    eq(isClosedVocabulary(field), false, `${field} is deliberately not a closed vocabulary`)
  }
  ok(EFFORT_SUGGESTIONS.length > 0, 'effort still offers the common values as suggestions')
  eq(
    localProblems({ ...blankDraft('project'), name: 'qa', systemPrompt: 'x', effort: 'ultra' }),
    [],
    'an effort value from no list cide has is accepted and sent as written — the suggestions ' +
      'are a menu, not a rule',
  )

  /* --------------------------------------------------- and drawn as choices, not text boxes */

  const choiceLabels = [...sectionCode.matchAll(/<Choice\s+label="([^"]+)"/g)].map((m) => m[1])
  for (const [field, label] of [
    ['scope', 'Scope'],
    ['harness', 'Harness'],
    ['permissionMode', 'Permission mode'],
  ]) {
    ok(
      choiceLabels.includes(label),
      `${field} is drawn as a <Choice>, not a free-text input — the point of the screen is that ` +
        'the user stops typing YAML values',
    )
  }
  for (const name of ['SCOPES', 'HARNESSES', 'PERMISSION_MODES']) {
    ok(
      new RegExp(`\\b${name}\\b`).test(sectionCode),
      `AgentsSection builds its ${name} choices from the rule module rather than restating them`,
    )
  }

  /* ------------------------------------------------------------------ the local pre-flight */

  const good = {
    ...blankDraft('project'),
    name: 'code-reviewer',
    systemPrompt: 'You review this project’s diffs.',
  }
  eq(localProblems(good), [], 'a complete draft is not refused locally')
  eq(canSave(good), true, 'and Save is offered for it')

  eq(
    localProblems({ ...good, name: '' }).map((p) => p.field),
    ['name'],
    'an empty name is refused before it is sent, against the name field',
  )
  eq(
    localProblems({ ...good, systemPrompt: '   \n\n  ' }).map((p) => p.field),
    ['systemPrompt'],
    'a whitespace-only system prompt is refused before it is sent — it is the whole of what ' +
      'makes this a role rather than a name',
  )
  eq(
    localProblems({ ...good, name: '', systemPrompt: '' }).map((p) => p.field),
    ['name', 'systemPrompt'],
    'both at once are both reported, rather than one at a time',
  )
  eq(canSave({ ...good, name: '' }), false, 'and Save is withheld')
  eq(canSave({ ...good, systemPrompt: '' }), false, 'in both cases')
  ok(
    localProblems({ ...good, name: '' }).every((p) => p.message.length > 20),
    'each local refusal is a sentence, not a word',
  )

  /*
   * The rule that keeps this from becoming a second validator. Everything else is Rust's answer,
   * and every one of these is genuinely refused *there* — a `/` in a name, a permission mode from
   * another CLI, a tool name with a comma, a zero concurrency. If any of them started failing
   * here, this file would be enforcing rules the hand editor and the MCP writer do not.
   */
  eq(
    localProblems({ ...good, name: '../escape' }),
    [],
    'a path-unsafe name is NOT refused locally — cide_agents::defs::validate is the authority ' +
      'on it, and it answers with the paragraph about Path::join that this side does not have',
  )
  eq(
    localProblems({ ...good, permissionMode: 'fromTheFuture', maxConcurrent: 0, tools: ['a, b'] }),
    [],
    'nor is a stale permission mode, a zero concurrency or a comma in a tool name',
  )
  eq(sorted(REQUIRED_FIELDS), ['name', 'systemPrompt'], 'exactly two fields are required')
  eq(
    REQUIRED_FIELDS.filter((field) => !AGENT_FIELDS.includes(field)),
    [],
    'and both of them are fields the form draws',
  )

  /* -------------------------------------------------------------- problems, bucketed by box */

  const byField = problemsByField([
    { field: 'name', message: 'one' },
    { field: 'name', message: 'two' },
    { field: 'tools', message: 'three' },
  ])
  eq(byField.get('name'), ['one', 'two'], 'two refusals on one field are both kept')
  eq(byField.get('tools'), ['three'], 'and a field with one keeps it')
  eq(byField.get('systemPrompt'), undefined, 'a field with none answers undefined')
  // `check-problems.mjs`'s ROGUE lesson: an object literal would answer this with a function,
  // React refuses a function as a child, and className stringifies it into the source of Object.
  eq(byField.get('constructor'), undefined, 'and so does a prototype key')

  /* --------------------------------------------------------------- what unsaved edits do */

  eq(isDirty(good, good), false, 'a draft nobody has touched is not dirty')
  eq(
    isDirty(good, { ...good, systemPrompt: `${good.systemPrompt} ` }),
    true,
    'one character of the system prompt makes it dirty — this is the whole of the guard that ' +
      'stops a click on another role from throwing writing away',
  )
  eq(isDirty(good, { ...good, scope: 'global' }), true, 'so does changing scope')
  eq(isDirty(good, { ...good, tools: ['Read'] }), true, 'so does adding a tool')
  eq(
    isDirty(null, blankDraft('project')),
    false,
    'a new role that is still blank is not dirty, so opening New role and clicking away is free',
  )
  eq(isDirty(null, { ...blankDraft('project'), name: 'q' }), true, 'one typed character is')

  eq(shouldRestore(null), false, 'nothing cached, nothing to restore')
  eq(
    shouldRestore({ saved: good, draft: good }),
    false,
    'a CLEAN cached draft is dropped and read again — the file has three other writers, and a ' +
      'form populated from a stale read reverts whatever landed in between',
  )
  eq(
    shouldRestore({ saved: good, draft: { ...good, systemPrompt: 'edited' } }),
    true,
    'a dirty one survives the section switch that unmounted the form',
  )

  /* ------------------------------------------------------------------- the list, and shadowing */

  const entries = [
    { scope: 'global', name: 'qa', label: 'QA' },
    { scope: 'project', name: 'qa', label: 'QA' },
    { scope: 'global', name: 'architect', label: 'Architect' },
  ]
  const rows = rowsFor(entries)
  eq(rows.map((r) => `${r.scope}:${r.name}`), ['global:architect', 'project:qa', 'global:qa'],
    'sorted by name, project before global, so a shadowed row is drawn under the row that ' +
      'shadows it')
  eq(rows.map((r) => r.shadowed), [false, false, true], 'the global qa is shadowed and nothing else is')
  eq(rows.map((r) => r.shadows), [false, true, false], 'and the project qa is what shadows it')
  eq(
    rows.map((r) => r.effective),
    [true, true, false],
    'exactly one file per name is what a dispatch would use — two rows that looked alike would ' +
      'be the worst possible drawing of "the project one wins whole-file"',
  )
  eq(
    rowsFor([{ scope: 'global', name: 'qa', label: 'QA' }]).map((r) => [r.shadowed, r.effective]),
    [[false, true]],
    'a global role with no project twin is live, not shadowed',
  )
  eq(fileKey('global', 'qa'), 'global:qa', 'a file is identified by scope and name together')

  /* --------------------------------------------------- what changing scope is about to do */

  eq(
    scopeChangeWarning({ ...good, original: { scope: 'project', name: 'code-reviewer' } }, new Set()),
    null,
    'editing a role in place says nothing — a warning on every keystroke is a warning nobody reads',
  )
  eq(scopeChangeWarning(good, new Set()), null, 'and neither does creating a role on a free name')

  const moved = scopeChangeWarning(
    { ...good, scope: 'global', original: { scope: 'project', name: 'code-reviewer' } },
    new Set(),
  )
  ok(typeof moved === 'string', 'moving a role between scopes says so BEFORE the save')
  ok(
    moved.includes('repository'),
    'and names the consequence that is not visible from the control: the file leaves the ' +
      'repository and its history',
  )

  const renamed = scopeChangeWarning(
    { ...good, name: 'reviewer', original: { scope: 'project', name: 'code-reviewer' } },
    new Set(),
  )
  ok(typeof renamed === 'string' && renamed.includes('rename'), 'a rename says it is a rename')

  const collision = scopeChangeWarning(
    { ...good, scope: 'global', original: { scope: 'project', name: 'code-reviewer' } },
    new Set(['global:code-reviewer']),
  )
  ok(
    typeof collision === 'string' && collision.includes('refused'),
    'and a move onto a name the other scope already uses says the save will be REFUSED — which ' +
      'is what cide_agents::defs::save does, and finding that out by pressing Save is finding ' +
      'out about a collision the screen already knew',
  )
  ok(
    typeof scopeChangeWarning({ ...good, name: 'qa' }, new Set(['project:qa'])) === 'string',
    'creating a role over an existing file is refused rather than merged, and says so first',
  )

  /* ------------------------------------------------------------------------------- the wire */

  const full = {
    ...good,
    label: 'Code Reviewer',
    harness: 'claude',
    description: 'Reviews a diff.',
    model: 'sonnet',
    effort: 'high',
    tools: ['Read', ' Grep ', ''],
    permissionMode: 'acceptEdits',
    maxConcurrent: 2,
    original: { scope: 'project', name: 'code-reviewer' },
  }
  const wire = toWire(full)
  eq(wire.tools, ['Read', 'Grep'], 'tool rows are trimmed and blanks dropped')
  eq(wire.maxConcurrent, 2, 'a set concurrency is sent')
  eq(
    toWire({ ...good, label: '', model: null, maxConcurrent: null }).label,
    undefined,
    'an unset optional is an ABSENT key, never an explicit undefined — AgentDraft is ' +
      'deny_unknown_fields and its optionals are #[ts(optional)]',
  )
  eq('label' in toWire({ ...good, label: '' }), false, 'literally absent, not merely undefined')
  eq('maxConcurrent' in toWire(good), false, 'and so is an unset concurrency')
  eq(
    toWire({ ...good, name: '  code-reviewer  ' }).name,
    'code-reviewer',
    'the name is trimmed — it is a file stem',
  )
  eq(
    toWire({ ...good, systemPrompt: '\nBody.\n\n' }).systemPrompt,
    '\nBody.\n\n',
    'the system prompt is NOT trimmed here: it is the body of the file, and normalising it is ' +
      'cide_agents::defs::normalize’s decision, made once, on the side that writes the file',
  )
  eq(fromWire(toWire(full)), { ...full, tools: ['Read', 'Grep'] }, 'a full draft round-trips')
  eq(
    fromWire({
      scope: 'global',
      name: 'qa',
      description: '',
      tools: [],
      systemPrompt: 'x',
      extras: [],
    }),
    { ...blankDraft('global'), name: 'qa', systemPrompt: 'x' },
    'and a wire draft with every optional absent comes back as a form with every one unset',
  )

  eq(titleCase('code-reviewer'), 'Code Reviewer', 'a name with no label reads as a title')

  /* ------------------------------------------- whether a row's role would actually run */

  /*
   * `AgentDef.unavailable` is Rust's sentence for "this cannot be dispatched, and here is why",
   * and it arrives on a **merged** roster row — one entry per name, answering for whichever file
   * wins. Copying it onto both rows of a shadowed pair would put "its system prompt is empty"
   * under the *global* file because the project file that shadows it has none, sending the user
   * to edit the file that is not the problem.
   */
  /*
   * And the same rule across the two *families*. (M30)
   *
   * A `.cide/agents/` role wins over a Claude Code subagent of the same name — that is the
   * loader's merge order — so the subagent's row must draw as inert. Before the scopes went from
   * two to four this function asked `scope === 'global'`, which would have answered
   * `shadowed: false` here and drawn the file that will never run as the one that does.
   */
  {
    const across = rowsFor([
      { scope: 'claudeProject', name: 'qa', label: 'QA', harness: null, unavailable: null },
      { scope: 'project', name: 'qa', label: 'QA', harness: null, unavailable: null },
      { scope: 'claudeGlobal', name: 'solo', label: 'Solo', harness: null, unavailable: null },
    ])
    eq(
      across.map((r) => [r.scope, r.effective, r.shadowed, r.shadows]),
      [
        ['project', true, false, true],
        ['claudeProject', false, true, false],
        ['claudeGlobal', true, false, false],
      ],
      'cide’s own directory wins over Claude Code’s, the stronger row sorts first, and a name ' +
        'only one file declares shadows nothing whichever scope it is in',
    )
  }

  const shadowedPair = [
    { scope: 'project', name: 'qa', label: 'QA', harness: null, unavailable: 'claude is not on PATH.' },
    { scope: 'global', name: 'qa', label: 'QA', harness: 'opencode', unavailable: 'claude is not on PATH.' },
  ]
  const paired = rowsFor(shadowedPair)
  eq(
    paired.map((r) => r.unavailable),
    ['claude is not on PATH.', null],
    "the merged roster's unavailable sentence stays on the file it is about — a shadowed row " +
      'never inherits it, because the roster answered for the row that shadows it',
  )
  eq(
    paired.map((r) => usability(r).kind),
    ['blocked', 'shadowed'],
    'and the shadowed row answers *shadowed*, which is the stronger and more useful claim: ' +
      'nothing will run this file at all, so its own state is beside the point',
  )
  eq(
    usability(rowsFor([shadowedPair[1]])[0]),
    { kind: 'blocked', why: 'claude is not on PATH.' },
    'the same global file with no project twin is blocked, with the sentence — so the shadowed ' +
      'answer above is shadowing doing the work and not the sentence having been dropped',
  )
  eq(
    usability(rowsFor([{ scope: 'project', name: 'dev', label: 'Dev', harness: 'claude', unavailable: null }])[0]),
    { kind: 'ready' },
    'a role with a harness, a prompt and no twin is ready',
  )
  /*
   * `usability` checks `shadowed` FIRST, and this is the only input that can tell that apart from
   * checking `unavailable` first: a row that is shadowed *and* carries a sentence. `rowsFor` never
   * builds one — it blanks the sentence, which the assertion above pins — so without a hand-built
   * row the ordering inside this function would be untested, and an assertion driven only by
   * `rowsFor` output would be measuring the blanking twice under two names.
   *
   * The order still has to be right: two rules that agree today are two rules, and the day one is
   * changed the other is what says which answer the row draws.
   */
  eq(
    usability({
      scope: 'global',
      name: 'qa',
      label: 'QA',
      harness: null,
      unavailable: 'claude is not on PATH.',
      effective: false,
      shadowed: true,
      shadows: false,
    }),
    { kind: 'shadowed' },
    'shadowed outranks blocked: a flawless global role is exactly as inert as a broken one while ' +
      'the project defines the same name, and the two words send the user to different files — ' +
      '*blocked* means edit this one, *shadowed* means delete the other',
  )
  eq(
    [usabilityLabel({ kind: 'ready' }), usabilityLabel({ kind: 'shadowed' }), usabilityLabel({ kind: 'blocked', why: 'x' })],
    ['Ready', 'Shadowed', 'Cannot run'],
    'every row says one of exactly three words — a list that labelled only the broken rows ' +
      'would make an unlabelled row mean either healthy or not-yet-checked',
  )
  eq(
    rowsFor([{ scope: 'project', name: 'dev', label: 'Dev' }])[0].harness,
    null,
    'a file that names no harness answers null — "project default" is a real state, not a ' +
      'missing value, and `AgentDef.harness` cannot express it because the roster resolved it',
  )

  /* ------------------------------------------------------------- the dialog, and what closes it */

  eq(modalFor(good), { kind: 'new' }, 'a draft with no original opens the dialog as a create')
  eq(
    modalFor({ ...good, original: { scope: 'global', name: 'code-reviewer' } }),
    { kind: 'edit', file: { scope: 'global', name: 'code-reviewer' } },
    'and one read from a file opens it on that file — the same `original` field `agents_save` ' +
      'reads to tell a write from a move, so the two cannot disagree',
  )
  eq(modalTitle({ kind: 'new' }), 'New role', 'the create names the act')
  eq(
    modalTitle({ kind: 'edit', file: { scope: 'project', name: 'qa' } }),
    'qa.md',
    'and an edit names the FILE, not the label — the label is presentation and two scopes can ' +
      'share one',
  )

  eq(restoredModal(null), null, 'nothing cached, no dialog')
  eq(
    restoredModal({ saved: good, draft: good }),
    null,
    'a clean cached draft opens no dialog: it is dropped and read again, per shouldRestore',
  )
  eq(
    restoredModal({ saved: good, draft: { ...good, systemPrompt: 'edited' } }),
    { kind: 'new' },
    'but a DIRTY one reopens the dialog it was in — unsaved writing must be on screen, not ' +
      'parked invisibly in a module-level map with the list looking untouched',
  )
  eq(
    restoredModal({
      saved: null,
      draft: { ...good, systemPrompt: 'edited', original: { scope: 'project', name: 'code-reviewer' } },
    }),
    { kind: 'edit', file: { scope: 'project', name: 'code-reviewer' } },
    'and it comes back on the same file it was editing',
  )

  eq(closeRequest(false), 'close', 'Escape on a clean dialog closes it, like every other overlay')
  eq(
    closeRequest(true),
    'ask',
    'Escape on a DIRTY one asks instead. ConfirmDestructive can take Escape as cancel because ' +
      'nothing is lost there; a half-written system prompt is real work, and Escape is the key a ' +
      'user presses at whatever is in front of them',
  )
  ok(
    closeRequest(true) !== 'close' && closeRequest(true) !== 'discard',
    'and it is never the keystroke that discards — discarding takes a click on a named button',
  )

  /* ------------------------------------------------- the sidebar's Configure, and what it opens */

  const focusRows = rowsFor([
    { scope: 'global', name: 'qa', label: 'QA', harness: null, unavailable: null },
    { scope: 'project', name: 'qa', label: 'QA', harness: null, unavailable: null },
    { scope: 'global', name: 'architect', label: 'Architect', harness: null, unavailable: null },
  ])

  eq(
    focusTarget('qa', null),
    { kind: 'wait' },
    'a request that arrives before the listing is HELD, not answered — configure opens the tab ' +
      'and the screen mounts in the same frame, so this is the normal case and not the edge one',
  )
  eq(
    focusTarget('qa', focusRows),
    { kind: 'file', file: { scope: 'project', name: 'qa' } },
    'a name both scopes hold opens the PROJECT file: it is the row a dispatch of that name ' +
      'actually runs, and it is what the merged roster row the user pressed Configure on was ' +
      'reporting',
  )
  /*
   * …and it is `effective` that decides, not list position. `rowsFor` happens to sort project
   * before global, so a `matches[0]` would give the same answer for every input the screen can
   * produce — which would make the assertion above a test of the sort order and nothing else.
   * Hand-built, shadowed-first rows are the only way to tell the two implementations apart.
   */
  eq(
    focusTarget('qa', [
      { scope: 'global', name: 'qa', label: 'QA', harness: null, unavailable: null, effective: false, shadowed: true, shadows: false },
      { scope: 'project', name: 'qa', label: 'QA', harness: null, unavailable: null, effective: true, shadowed: false, shadows: true },
    ]),
    { kind: 'file', file: { scope: 'project', name: 'qa' } },
    'and the answer is the EFFECTIVE row wherever it sits in the list, not the first one',
  )
  eq(
    focusTarget('architect', focusRows),
    { kind: 'file', file: { scope: 'global', name: 'architect' } },
    'a name only the global scope holds opens the global file — the rule is "the effective ' +
      'row", not "always project"',
  )
  eq(
    focusTarget('nobody', focusRows),
    { kind: 'unknown' },
    'and a roster name with no definition file this screen could read is unknown, not a guess',
  )
  eq(focusTarget('qa', []), { kind: 'unknown' }, 'an empty listing is an answer, unlike a null one')

  const at = { kind: 'file', file: { scope: 'project', name: 'qa' } }
  eq(
    focusAction(at, false),
    { kind: 'open', file: { scope: 'project', name: 'qa' } },
    'with nothing at stake the request just opens the role',
  )
  eq(
    focusAction(at, true),
    { kind: 'confirm', file: { scope: 'project', name: 'qa' } },
    'over unsaved writing it asks. Switching anyway would replace a half-written system prompt ' +
      'through the one door that does not pass the list rows, and ignoring it would make ' +
      'Configure — the one control drawn on EVERY roster row — silently do nothing',
  )
  eq(focusAction({ kind: 'wait' }, true), { kind: 'wait' }, 'waiting is unaffected by the form')
  eq(focusAction({ kind: 'unknown' }, false), { kind: 'unknown' }, 'and so is a miss')

  const standing = { kind: 'edit', file: { scope: 'global', name: 'architect' } }
  eq(
    screenOpening('qa', focusRows, false, standing),
    { kind: 'focus', file: { scope: 'project', name: 'qa' } },
    'a focus request OUTRANKS the dialog already up — a screen that kept a restored draft over ' +
      'the role the user just clicked is a screen that ignored them',
  )
  eq(
    screenOpening('qa', focusRows, true, standing),
    { kind: 'confirm', file: { scope: 'project', name: 'qa' } },
    'but not over unsaved writing: a restored draft is by construction dirty, so the collision ' +
      'surfaces as a confirm naming both sides rather than as a silent switch',
  )
  eq(
    screenOpening(null, focusRows, true, standing),
    { kind: 'keep', modal: standing },
    'with no request, whatever is open stays open',
  )
  eq(screenOpening(null, focusRows, false, null), { kind: 'none' }, 'and with neither, nothing')
  eq(
    screenOpening('qa', null, false, standing),
    { kind: 'wait' },
    'a request the listing cannot resolve yet leaves the standing dialog alone rather than ' +
      'clearing the screen while it waits',
  )
  eq(
    screenOpening('nobody', focusRows, false, standing),
    { kind: 'unknown', name: 'nobody' },
    'and a miss carries the name, so the screen can say WHICH role it could not find rather ' +
      'than shrugging at a click the user made on purpose',
  )

  /* ------------------------------------- and the screen and the store actually do those things */

  /*
   * The store's half. `focusRole` was written in M-whatever with nothing reading it, and the
   * whole point of `takeFocusRole` is that it *clears*: `layout/spawnPlans.ts`'s `takeSpawnPlan`
   * has the argument — StrictMode mounts effects twice, so a value left in place is applied
   * twice, and here the second application lands on a later mount of this screen entirely.
   */
  // Sliced from `create<AgentsStore>` first, so this reads the **implementation** and not the
  // interface declaration above it. Without that, renaming the method would still match the
  // `takeFocusRole: () => AgentId | null` in the type and the scan would land in whatever body
  // came next.
  const storeImpl = storeCode.slice(storeCode.indexOf('create<AgentsStore>'))
  const takeAt = storeImpl.indexOf('takeFocusRole: () => {')
  ok(takeAt >= 0, 'agentsStore implements takeFocusRole()')
  const takeBody = storeImpl.slice(takeAt, takeAt < 0 ? 0 : storeImpl.indexOf('\n  },', takeAt))
  ok(
    /set\(\{\s*focusRole:\s*null\s*\}\)/.test(takeBody),
    'and it CLEARS the request as it reads it — a focus request that survives its own read is ' +
      'applied again on the next mount of the settings screen, jumping the form to a role the ' +
      'user has not asked about since',
  )
  ok(
    /takeFocusRole\(\)/.test(sectionCode),
    'AgentsSection takes the request through takeFocusRole() rather than reading focusRole ' +
      'directly, which would leave it set',
  )
  ok(
    /useAgents\(\(s\)\s*=>[^)]*s\.project === project/.test(sectionCode),
    'and the subscription is guarded on the project: the store clears focusRole on attach, but ' +
      'a second window takes its settings project from useActiveProject and the two can differ',
  )
  /*
   * Every arm of `Opening` that asks the screen to *do* something has a branch. A screen that
   * called `screenOpening` and then answered two of its three actionable arms would hold the
   * request for ever in the third — `unknown` above all, which is the arm that clears it.
   */
  for (const arm of ['focus', 'confirm', 'unknown']) {
    ok(
      sectionCode.includes(`opening.kind === '${arm}'`),
      `AgentsSection acts on the '${arm}' opening — an unhandled arm is a focus request that is ` +
        'never cleared, and therefore one that fires again on the next mount',
    )
  }
  for (const rule of ['screenOpening(', 'closeRequest(', 'restoredModal(', 'usability(']) {
    ok(
      sectionCode.includes(rule),
      `AgentsSection asks ./agentsDraft for ${rule.slice(0, -1)} rather than restating the rule ` +
        'inside a component, where no check script can reach it',
    )
  }

  /*
   * The dialog. There is no DOM here, so these are source claims — but each one is the claim
   * that, broken, turns a working dialog into one that cannot be dismissed, cannot be typed in,
   * or scrolls away with the section behind it.
   */
  ok(
    /<OverlayCard/.test(sectionCode),
    'the dialog is overlays/ModalShell.tsx’s OverlayCard — the same card ConfirmDestructive ' +
      'sits in — and not a fourth definition of what an overlay looks like in this app',
  )
  const overlayCardBody = modalShellTsx.slice(modalShellTsx.indexOf('export function OverlayCard'))
  ok(
    /role="dialog"/.test(overlayCardBody) && /aria-modal="true"/.test(overlayCardBody),
    'and that card is a real dialog: role="dialog" with aria-modal, which is what the role form ' +
      'inherits by using it rather than restating',
  )
  ok(
    /\.scrim\s*\{[^}]*position:\s*fixed/.test(overlayCss),
    'the scrim is position: fixed, which is the whole of the mount decision — SettingsTab’s ' +
      '.pane is the scroll container, and a dialog positioned inside it would scroll away with ' +
      'the form behind it',
  )
  ok(
    /name\.current\?\.focus\(\)/.test(sectionCode),
    'initial focus lands on the name field — the user opened this to type, so not Cancel; and ' +
      'not merely "the first control", which is the scope <select> where an arrow key is a file ' +
      'move',
  )
  ok(
    /keep\.current\?\.focus\(\)/.test(sectionCode),
    'and when the discard confirm arms, focus moves to Keep editing — ConfirmDestructive rule 3, ' +
      'so the Enter already in flight does not destroy the writing',
  )
  ok(
    /ev\.key === 'Escape'/.test(sectionCode) && /onRequestClose\(\)/.test(sectionCode),
    'Escape runs through the same funnel as the scrim and Cancel, so one dismissal cannot ' +
      'discard what another one asks about',
  )
  ok(
    /ev\.key !== 'Tab'/.test(sectionCode) && /shiftKey/.test(sectionCode),
    'and Tab is trapped in both directions — a Tab out of the card lands in the settings nav ' +
      'behind the scrim, where clicks do not reach',
  )
  ok(
    /:not\(\[tabindex="-1"\]\)/.test(sectionCode) && /button:not\(\[disabled\]\)/.test(sectionCode),
    'the trap skips disabled controls: Save is disabled until the draft can be saved at all, ' +
      'and wrapping onto it would put focus nowhere',
  )

  /* --------------------------------------------------------------- the list's two actions */

  for (const [label, why] of [
    ['Edit', 'every row opens the same dialog, populated — not a second form that drifts'],
    ['Delete', 'and every row can remove its own file'],
  ]) {
    ok(
      new RegExp(`>\\s*${label}\\s*<`).test(sectionCode),
      `the role list draws a ${label} action per row — ${why}`,
    )
  }
  ok(
    /onArmDelete\(file\)/.test(sectionCode) && /onDelete\(file\)/.test(sectionCode),
    'Delete arms on the first click and runs on the second, and both name the ROW’s file: ' +
      'two rows really can share a name across scopes, so an armed boolean would let a click on ' +
      'one delete the other',
  )
  ok(
    !/canDelete/.test(sectionCode),
    'and the dialog no longer carries its own Delete — a footer offering Save beside "destroy ' +
      'the thing you are editing" is one where the wrong button is a slip away',
  )

  /* ---------------------------------------------------------------- the screen is reachable */

  ok(/id: 'agents'/.test(sectionsTsx), "SECTIONS carries an 'agents' entry, so the nav draws it")
  ok(/case 'agents':/.test(sectionsTsx), "renderSection has a case for it, so the pane draws it")
  ok(
    variants(shipping(read('../../crates/cide-ipc/src/workspace.rs')), 'pub enum SettingsSection {', 'SettingsSection').includes('agents'),
    'and SettingsSection has the variant the tab is persisted under, so it survives a relaunch',
  )
  // `check-agents.mjs` proves the contract and the client agree. This proves the *screen* is the
  // caller — a wrapper nobody invokes is the same defect one layer out.
  // Whitespace-tolerant: two of the three are written as `void agentDefs\n  .save(…)`, and a
  // literal `includes` would report the form as calling nothing at all.
  for (const call of ['draft', 'save', 'delete']) {
    ok(
      new RegExp(`agentDefs\\s*\\.\\s*${call}\\s*\\(`).test(sectionCode),
      `AgentsSection calls agentDefs.${call}() — the form is the caller`,
    )
  }
  for (const command of ['agents_draft', 'agents_save', 'agents_delete']) {
    ok(clientTs.includes(`'${command}'`), `client.ts has a wrapper for ${command}`)
  }

  if (failed > 0) {
    console.error(`\ncheck-settings-agents: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-settings-agents: ok (${rustFields.length} AgentField variants drawn and answerable, ` +
      `${SCOPES.length + HARNESSES.length + PERMISSION_MODES.length} values across 3 closed ` +
      `vocabularies pinned to Rust, ${rows.length} list rows with shadowing resolved, ` +
      `2 required fields refused locally and ${5} rules deliberately left to Rust, ` +
      `${paired.length + 3} rows answered ready/shadowed/blocked, ` +
      `${4} dialog states derived from the draft, Escape asking on ${1} of 2 dirtinesses, ` +
      `${6} focus resolutions and ${4} orderings of request against dialog)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
