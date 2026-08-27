/**
 * Checks the OpenSpec configuration surface — `openspec/config.yaml` as a form. (M28)
 *
 * Two halves, `check-openspec.mjs` + `check-openspec-render.mjs` in one script because they share
 * one subject: `tsc` compiles the import-free `configModel.ts` and node drives it, then Vite
 * SSR-bundles `ConfigSmokeEntry.tsx` and the markup is asserted on. Neither half can see what the
 * other does, and every failure below is silent in the app.
 *
 * # The failure classes this makes unrepresentable
 *
 * **A Save that rewrites a committed file having changed nothing.** `cide_spec::config` promises
 * that writing a value back unchanged changes no byte and does not move the mtime, and it keeps
 * that promise by comparing each value against what the *reader* produces. A textarea does not
 * produce that: it ends in a newline, it can hold `\r\n`, and a list row somebody opened and
 * abandoned is an empty string rather than an absent item. `normaliseBlock`/`normaliseList` are
 * the mirror of Rust's `tidy_block`/`tidy_list`, and the load-bearing assertion is the round
 * trip: `editsFor(draftFrom(config), config)` must be **empty** for every config shape. Break it
 * and nothing throws — the Save button simply never goes dark, and every press of it turns up in
 * somebody's `git status` having changed nothing they typed.
 *
 * **A `schema:` line added to a file that never asked for one.** An unstated key *means*
 * `spec-driven`, and the wire keeps `null` so this can be told apart from a file that states the
 * default. Emitting an edit for the first case adds a line to a committed file nobody typed.
 *
 * **A `<select>` showing a value it does not offer.** `spec_schemas` degrades to a single row
 * whenever the CLI cannot be asked, which is a supported state — the file is read and written by
 * cide's own scanner, so the form works with no `openspec` on PATH at all. A select whose `value`
 * matches no option does not fail: it displays the *first* option, and the next Save writes that
 * schema over the user's. Asserted from both ends — `schemaOptions` in the model, and the
 * rendered `<option selected>` in the markup.
 *
 * **A wizard that asks after it has already acted.** The whole argument for asking for the
 * project context during set-up is that it is the one moment somebody will write it. A context
 * box below the button is a box that was skipped, and no unit test and no screenshot can see the
 * difference. The digest reads document order.
 *
 * **A Save that can be pressed twice.** `apply` is an atomic rename of a file an agent may be
 * holding. Every writing control must be inert while one is in flight.
 *
 * **A vocabulary that drifts from Rust.** `GUIDED_OPERATIONS` is a const in
 * `crates/cide-spec/src/config.rs` and a frozen list in `configModel.ts` — it has to exist in
 * TypeScript because the form draws a section for each *whether or not the file states one*,
 * which is the whole point of a form over a file whose keys are commented out. Nothing else in
 * the build can see a third operation arriving with no section.
 *
 * # What this does NOT cover
 *
 *   - the writer's round trip itself. `cargo test -p cide-spec config` owns that, and it is the
 *     thing this check is written *against* rather than a substitute for.
 *   - `tidy_block`/`tidy_list` in Rust. `cargo test -p cide-app spec` states those directly; the
 *     cases below are deliberately the same cases, so a divergence shows up as one side passing.
 *   - that the section is reachable. `check-ext.mjs` pins every `SettingsSection` variant against
 *     its `SECTIONS` entry and its `renderSection` case.
 *
 * Run: `pnpm --dir ui run check:openspec-config`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-openspec-config-'))
mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
const ssr = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-openspec-config-'))

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

/** A `SpecConfig`, with everything the model needs and nothing it does not. */
const config = (over = {}) => ({
  schema: null,
  defaultSchema: 'spec-driven',
  context: null,
  rules: [],
  operations: [],
  path: '/repo/openspec/config.yaml',
  exists: true,
  ...over,
})

const SCHEMAS = [
  {
    name: 'spec-driven',
    description: 'Default OpenSpec workflow',
    artifacts: ['proposal', 'specs', 'design', 'tasks'],
    source: 'package',
  },
  { name: 'lean', description: null, artifacts: ['proposal', 'tasks'], source: 'project' },
]

try {
  /* ============================================================ half one: the pure model */

  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/OpenSpecPanel/configModel.ts',
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
  const M = await import(`file://${join(out, 'configModel.js')}`)

  /* ------------------------------------------------------------- the vocabulary, vs Rust */

  const configRs = read('../../crates/cide-spec/src/config.rs')
  const guided = /GUIDED_OPERATIONS: \[&str; \d+\] = \[([^\]]*)\]/.exec(configRs)?.[1] ?? ''
  const rustOperations = [...guided.matchAll(/"([^"]+)"/g)].map((m) => m[1])
  ok(rustOperations.length >= 2, 'the GUIDED_OPERATIONS scan found the Rust constant')
  eq(
    [...M.GUIDED_OPERATIONS],
    rustOperations,
    'every operation Rust gives guidance for has a section in the form, in the same order, ' +
      'and no more — an operation with no section is guidance the file can hold and the form ' +
      'can never show',
  )

  // A lookup that misses returns `undefined`, and a prototype key returns a *function*, which
  // React refuses as a child. An artifact and an operation are both free strings read out of
  // somebody's YAML, so `constructor` is a value the file can genuinely contain.
  for (const operation of [...M.GUIDED_OPERATIONS, 'constructor', 'toString', '', 'publish']) {
    const label = M.operationLabel(operation)
    ok(
      typeof label === 'string' && label.length > 0,
      `operationLabel(${JSON.stringify(operation)}) is a non-empty string`,
    )
    const hint = M.operationHint(operation)
    ok(
      typeof hint === 'string' && hint.length > 0,
      `operationHint(${JSON.stringify(operation)}) is a non-empty string`,
    )
  }

  /* ---------------------------------------------- normalisation: the same cases as Rust */

  eq(M.normaliseLine('  keep   it  short  '), 'keep it short', 'a rule is one line, collapsed')
  eq(M.normaliseLine('two\nlines'), 'two lines', 'and a pasted paragraph cannot become two items')
  eq(M.normaliseLine('   '), '', 'a row somebody opened and abandoned is not a rule')
  eq(M.normaliseList(['a', '  ', 'b  b']), ['a', 'b b'], 'blanks are dropped, not written as `- `')

  eq(M.normaliseBlock('a\n'), 'a', 'a textarea ends in a newline; the block scalar does not')
  eq(M.normaliseBlock('a\n\n\n'), 'a', 'and clip chomping drops every trailing blank')
  eq(M.normaliseBlock('a\n   \nb'), 'a\n\nb', 'a whitespace-only line reads back as an empty one')
  eq(M.normaliseBlock('a\r\nb'), 'a\nb', 'a CR from the webview is a line ending, not a byte')
  eq(M.normaliseBlock('  a\n    b'), '  a\n    b', 'and indentation inside the block is content')
  eq(M.normaliseBlock('   \n  '), '', 'a box with nothing but whitespace in it is no context')

  /* ------------------------------------------------------- the round trip: no phantom Save */

  for (const [name, value] of Object.entries({
    bare: config(),
    stated: config({ schema: 'spec-driven' }),
    other: config({ schema: 'lean' }),
    prose: config({ context: 'Tech stack: Rust.\n\nLinux-first.' }),
    full: config({
      schema: 'spec-driven',
      context: 'one',
      rules: [{ artifact: 'design', rules: ['a', 'b'] }],
      operations: [{ operation: 'archive', guidance: ['c'] }],
    }),
    unknownArtifact: config({ rules: [{ artifact: 'brief', rules: ['one paragraph'] }] }),
    unknownOperation: config({ operations: [{ operation: 'publish', guidance: ['x'] }] }),
  })) {
    eq(
      M.editsFor(M.draftFrom(value), value, SCHEMAS),
      [],
      `${name}: a form opened and not touched writes nothing at all — the Save button is dark, ` +
        'and it is dark because there is genuinely nothing to write',
    )
    eq(M.isDirty(M.draftFrom(value), value, SCHEMAS), false, `${name}: and says so`)
  }

  /* ------------------------------------------------------------------- the schema rule */

  {
    const bare = config()
    eq(
      M.editsFor({ ...M.draftFrom(bare), schema: 'spec-driven' }, bare, SCHEMAS),
      [],
      'a file that states no schema already MEANS spec-driven: writing the default back would ' +
        'add a line to a committed file that nobody typed',
    )
    eq(
      M.editsFor({ ...M.draftFrom(bare), schema: 'lean' }, bare, SCHEMAS),
      [{ kind: 'schema', schema: 'lean' }],
      'and choosing anything else does add it',
    )
    const stated = config({ schema: 'lean' })
    eq(
      M.editsFor({ ...M.draftFrom(stated), schema: 'spec-driven' }, stated, SCHEMAS),
      [{ kind: 'schema', schema: 'spec-driven' }],
      'a file that states a schema is edited to the default like any other value',
    )
  }

  /* -------------------------------------------------------------------- context removal */

  {
    const prose = config({ context: 'Tech stack: Rust.' })
    eq(
      M.editsFor({ ...M.draftFrom(prose), context: '' }, prose, SCHEMAS),
      [{ kind: 'context', context: null }],
      'clearing the box removes the key rather than writing `context: ""` — an empty key with ' +
        'the commented example sitting under it is worse than no key at all',
    )
    eq(
      M.editsFor({ ...M.draftFrom(prose), context: 'Tech stack: Rust.\n\n' }, prose, SCHEMAS),
      [],
      'and a trailing newline is not an edit, which is the whole reason normaliseBlock exists',
    )
  }

  /* ------------------------------------------------- switching schema deletes nobody's rules */

  {
    const configured = config({
      schema: 'spec-driven',
      rules: [
        { artifact: 'design', rules: ['name the option that lost'] },
        { artifact: 'specs', rules: ['one requirement per behaviour'] },
      ],
    })
    const moved = { ...M.draftFrom(configured), schema: 'lean' }
    eq(
      M.editsFor(moved, configured, SCHEMAS),
      [{ kind: 'schema', schema: 'lean' }],
      'switching to a schema that declares neither `design` nor `specs` writes ONLY the schema: ' +
        'the rules stay in the file, because a form that draws fewer sections has not been told ' +
        'to delete the ones it stopped drawing',
    )
    eq(
      M.artifactRows(moved, SCHEMAS).map((row) => `${row.artifact}|${row.declared}`),
      ['proposal|true', 'tasks|true', 'design|false', 'specs|false'],
      'and both are still drawn, marked as not declared by the chosen schema — rules the agent ' +
        'still reads and the form hid would be a screen that disagrees with the file',
    )
  }

  /* ------------------------------------------------------------------- clearing a list */

  {
    const configured = config({ rules: [{ artifact: 'design', rules: ['a'] }] })
    const cleared = { ...M.draftFrom(configured), rules: [{ artifact: 'design', rules: [] }] }
    eq(
      M.editsFor(cleared, configured, SCHEMAS),
      [{ kind: 'rules', artifact: 'design', rules: [] }],
      'removing the last row removes the entry — an empty list is how config::Edit spells it',
    )
    const blanked = { ...M.draftFrom(configured), rules: [{ artifact: 'design', rules: ['a', ''] }] }
    eq(
      M.editsFor(blanked, configured, SCHEMAS),
      [],
      'and an empty row that has not been typed into yet is not an edit, so pressing Add does ' +
        'not by itself light the Save button',
    )
  }

  /* ----------------------------------------------- the select always offers what it shows */

  for (const [name, value] of Object.entries({
    bare: config(),
    listed: config({ schema: 'lean' }),
    unlisted: config({ schema: 'house-style' }),
    blank: config({ schema: '   ' }),
  })) {
    const options = M.schemaOptions(value, SCHEMAS)
    const current = M.resolvedSchema(value)
    ok(
      options.some((option) => option.value === current),
      `${name}: the schema the file states is among the options — a select whose value matches ` +
        'no option displays a different one, silently, and the next Save writes that one instead',
    )
    for (const option of options) {
      ok(typeof option.label === 'string' && option.label.length > 0, `${name}: every row is named`)
    }
  }
  {
    const only = M.schemaOptions(config({ schema: 'house-style' }), [])
    eq(only.length, 1, 'with no schemas listed at all the file’s own is still offered')
    eq(only[0].unlisted, true, 'and marked, so the form can say why it is unlike the others')
    ok(only[0].hint.length > 0, 'with a sentence rather than a bare flag')
  }

  /* ------------------------------------------------------------------- the wizard states */

  for (const [context, busy, expected] of [
    ['', false, 'setUpWithoutContext'],
    ['   \n  ', false, 'setUpWithoutContext'],
    ['Tech stack: Rust.', false, 'setUpWithContext'],
    ['Tech stack: Rust.', true, 'working'],
    ['', true, 'working'],
  ]) {
    eq(M.wizardAction(context, busy), expected, `wizardAction(${JSON.stringify(context)}, ${busy})`)
  }
  for (const action of ['setUpWithContext', 'setUpWithoutContext', 'working', 'constructor', '']) {
    const label = M.wizardLabel(action)
    ok(
      typeof label === 'string' && label.length > 0,
      `wizardLabel(${JSON.stringify(action)}) is a non-empty string — a button whose label came ` +
        'back undefined is an empty box that is nonetheless clickable',
    )
  }

  /* ================================================================ half two: the markup */

  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr',
      'src/sidebar/OpenSpecPanel/ConfigSmokeEntry.tsx',
      '--outDir',
      ssr,
      '--logLevel',
      'error',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  // `@tauri-apps/api` touches `window` on import. Nothing deeper is faked.
  globalThis.window = globalThis
  globalThis.location = { search: '' }

  const printed = []
  const say = console.log
  console.log = (line) => printed.push(String(line))
  try {
    await import(`file://${resolve(ssr, 'ConfigSmokeEntry.js')}`)
  } finally {
    console.log = say
  }
  const stories = Object.fromEntries(
    JSON.parse(printed.at(-1)).map((digest) => [digest.story, digest]),
  )
  const t = (name) => {
    const digest = stories[name]
    if (digest === undefined) throw new Error(`no story ${name}`)
    return digest
  }

  /* ---------------------------------------------------- every hooked element is classed */

  for (const name of Object.keys(stories)) {
    eq(
      t(name).unclassed,
      0,
      `${name}: every element a digest reads carries a class — a hook on an unstyled element is ` +
        'a hook on something that is not the control, and `undefined` in a class list is a ' +
        'CSS module member that does not exist',
    )
  }

  /* ------------------------------------------------------------------------- the form */

  {
    const configured = t('configured')
    eq(configured.root, 'form', 'the form renders')
    eq(configured.path, 'shown', 'and prints the file it is editing')
    eq(configured.openFile, 'on', 'with a live way through to it — the form shows less than the ' +
      'file, and the eight hundred bytes of comments it hides are the only docs these keys have')
    eq(configured.context, true, 'the context box is there')
    ok(
      configured.saysEveryPrompt,
      'and the screen says the words that make it worth filling in: that it is injected into ' +
        'every artifact-generation prompt. An unexplained box is left empty',
    )
    eq(configured.save, 'off', 'a clean form cannot be saved')
    eq(configured.revert, 'off', 'and has nothing to discard')
    eq(configured.status, 'idle', 'and says it is up to date')

    eq(
      configured.schemaValue,
      'spec-driven',
      'the select really marks the option the project is on — React renders the value as ' +
        '`<option selected>` and nothing on the select itself',
    )
    ok(
      configured.schemaOptions.includes(configured.schemaValue),
      `the value is among the options: ${JSON.stringify(configured.schemaOptions)}`,
    )
    eq(
      configured.ruleLists,
      ['proposal|1', 'specs|0', 'design|2', 'tasks|0'],
      'one list per artifact the schema declares, in the schema’s workflow order, each with the ' +
        'rows the file states',
    )
    eq(
      configured.guidanceLists,
      ['apply|0', 'archive|1'],
      'and one per guided operation, drawn whether or not the file states it — a section that ' +
        'appeared only once the key existed could never be used to create the key',
    )
  }

  {
    const bare = t('bare')
    eq(
      bare.schemaUnlisted,
      'false',
      'the schema the CLI listed is not marked unlisted',
    )
    ok(
      bare.writeControls.includes('specConfigSchema'),
      'the file init wrote still offers every control — being unconfigured is the state this ' +
        'screen exists for',
    )
  }

  eq(t('unlisted').schemaUnlisted, 'true', 'a schema the CLI never listed is marked on screen')
  ok(
    t('unlisted').schemaOptions.includes('house-style'),
    'and offered, so the select cannot silently show a different schema',
  )
  eq(t('noCli').cliNote, 'shown', 'no CLI: the reason is drawn')
  ok(
    t('noCli').writeControls.length > 1,
    'and nothing is taken away — the file is read and written by cide itself, so a project is ' +
      'configurable on a machine with no openspec on PATH',
  )

  eq(t('dirty').save, 'on', 'an edited form can be saved')
  eq(t('dirty').revert, 'on', 'and discarded')
  eq(t('rejected').error, 'shown', 'a refusal is drawn, as the sentence Rust composed')
  eq(t('rejected').save, 'on', 'and the edits are still there to try again')

  {
    const unchanged = t('unchanged')
    eq(unchanged.status, 'unchanged', 'a Save that wrote nothing has its own state')
    ok(
      unchanged.statusText !== null && /not touched/i.test(unchanged.statusText),
      `and says so rather than claiming a commit git status will not show: ${unchanged.statusText}`,
    )
  }

  {
    const saving = t('saving')
    eq(
      saving.liveWrites,
      [],
      'a Save in flight leaves NOTHING that writes — `apply` is an atomic rename of a file an ' +
        'agent may be holding, and a doubled press is a second one racing the first',
    )
    ok(saving.writeControls.length > 0, 'and the controls are still on screen, merely inert')
  }

  /* ------------------------------------------------------------------------ the wizard */

  {
    const empty = t('wizard-empty')
    eq(empty.root, 'wizard', 'a project with no openspec/ gets the wizard')
    eq(empty.wizardPath, 'shown', 'which names the directory it would create')
    eq(
      empty.contextBeforeSetUp,
      true,
      'and the context box comes BEFORE the button in document order. That is the whole ' +
        'argument for asking during set-up: a field read after the click is a field that was ' +
        'skipped, and nothing else in the build can see the difference',
    )
    ok(
      empty.saysEveryPrompt,
      'the wizard says what the box is for in the same words the form does',
    )
    eq(
      empty.setUpAction,
      'setUpWithoutContext',
      'with nothing typed the button says it is about to skip, rather than leaving somebody ' +
        'wondering whether they failed a required field',
    )
    eq(t('wizard-filled').setUpAction, 'setUpWithContext', 'and changes its words once it is not')
    eq(t('wizard-busy').setUp, 'off', 'an init in flight cannot be started again')
    eq(t('wizard-busy').liveWrites, [], 'and neither can the box be typed into while it runs')
    eq(t('wizard-failed').error, 'shown', 'a refused init is drawn')
    eq(t('wizard-failed').setUp, 'on', 'and can be tried again')
  }

  /* --------------------------------------- one schema is the usual case, not a fault (M28) */

  /*
   * OpenSpec ships exactly one workflow — `spec-driven`, `source: "package"` — so the picker has
   * a single entry in almost every project. That reads as a broken control or as a value cide
   * hard-coded, and it is neither: `spec_schemas` runs `openspec schemas --json` and renders what
   * comes back. Forking one (`openspec schema fork spec-driven <name>`) makes the list two,
   * verified against the real CLI.
   *
   * Three stories, and the pair either side is what makes the sentence a claim rather than
   * decoration.
   */
  eq(
    t('onlySchema').schemaOnlyOne,
    true,
    'a list holding only the schema upstream ships says so, and names the command that adds ' +
      'another — the question this answers was asked of the shipped build',
  )
  eq(
    t('configured').schemaOnlyOne,
    false,
    'a project with a second schema does not: there is nothing to explain',
  )
  eq(
    t('noCli').schemaOnlyOne,
    false,
    'and neither does the degraded case — there the single row is cide\'s fallback rather than ' +
      'the CLI\'s answer, so saying "this list is whatever `openspec schemas` reports" would be ' +
      'false in exactly the case where the CLI could not be asked',
  )
  ok(
    M.SCHEMA_FORK_HINT.includes('openspec schema fork'),
    'the sentence names the real command, so what is learned here transfers to the CLI',
  )
  eq(
    M.onlyDefaultSchema([
      { value: 'spec-driven', label: '', hint: '', artifacts: [], unlisted: false },
      { value: 'house-style', label: '', hint: '', artifacts: [], unlisted: true },
    ]),
    true,
    'the `unlisted` row does not count — it is there because the file names a schema the CLI did ' +
      'not list, which is a different situation with a sentence of its own',
  )

  /* ---------------------------------------------- the modal frame, as source (M28) */

  /*
   * These two cannot be rendered here — `ConfigDialog` is the host, it fetches, and `OverlayCard`
   * portals to `document.body`, which `react-dom/server` refuses outright. Both are read as
   * source instead, and both are failures that changed nothing any renderer could see.
   */
  {
    const css = read('../src/sidebar/OpenSpecPanel/ConfigForm.module.css')
    const dialog = /\.dialog\s*\{([^}]*)\}/.exec(css)?.[1] ?? ''
    ok(dialog !== '', 'the dialog frame has a rule at all')
    /*
     * **It must state no width.**
     *
     * `overlays/Overlay.module.css`'s `.card` is `width: 620px` with `overflow: hidden`, so a
     * child asking to be wider does not widen it — it overflows, and the card centres, so the
     * excess is clipped off *both* edges. It shipped at `min(760px, 92vw)` and drew as a dialog
     * with its left-hand quarter sliced away: the title, the config path and every label lost
     * their first characters, with nothing anywhere saying why. `TasksPanel`'s `.taskCard` is the
     * house pattern and sets no width either.
     */
    ok(
      !/(^|[\s;])width\s*:/.test(dialog),
      `the config dialog must not set its own width — \`.card\` is 620px with \`overflow: hidden\`, ` +
        `so a wider child is clipped off both edges rather than widening the card: ${dialog.trim()}`,
    )
    ok(
      /max-height\s*:/.test(dialog),
      'but it must cap its height, or the form scrolls the page instead of its own body and ' +
        'Save ends up below the window',
    )

    /*
     * The read is three calls and two of them spawn the CLI, so the dialog is on screen with
     * nothing in it for the better part of a second. It returned `null` for that whole window —
     * an in-flight read drawn as an absence, which is the same failure the task card had, in the
     * same feature, and had to be reported before anyone noticed either.
     */
    const host = read('../src/sidebar/OpenSpecPanel/ConfigDialog.tsx')
    ok(
      host.includes('data-audit="specConfigReading"'),
      'the dialog says it is reading rather than drawing an empty titled frame',
    )
    ok(
      host.includes('data-audit="specConfigFailed"'),
      'and a read that failed is a sentence, not a spinner that never stops',
    )
  }

  if (failed === 0) console.log('check-openspec-config: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
  rmSync(ssr, { recursive: true, force: true })
}

process.exit(failed === 0 ? 0 : 1)
