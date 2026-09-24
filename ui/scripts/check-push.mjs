/**
 * Checks `src/chrome/pushModel.ts` and the dialog around it — *Push Commits*. (M31)
 *
 * `check-pull-strategy.mjs`'s recipe, and its header's argument for why a dialog gets its own
 * check rather than another two hundred assertions in `check-branches.mjs` holds unchanged.
 * What is worth pinning *here*:
 *
 *   - **Force starts unticked and is reset on every open.** It is the one control in cide that
 *     destroys work on a machine that is not the user's, and nothing cide offers undoes it.
 *   - **Force is only offered where it is an answer.** `forceApplies` is false unless a ticked
 *     row has diverged, and a checkbox that reads the same whether it would overwrite a
 *     colleague's work or do nothing at all is one people learn to tick.
 *   - **`force` is sent per row, not per gesture.** `pushRun.ts` masks it with `preview.diverged`
 *     — a `--force-with-lease` on a fast-forward is a no-op, and sending one anyway puts the flag
 *     on pushes that never needed it.
 *   - **The count is on the button.** `ConfirmDestructive`'s rule 1: the user is acting on a
 *     specific set and a bare *Push* is the one thing they cannot check before clicking.
 *   - **A blocked row says why.** Three states, three remedies, three sentences — a disabled row
 *     with no explanation reads as a bug in cide rather than a state of the repository.
 *   - **The two lease wordings name who performs the lease.** git's own on the binary route,
 *     cide's comparison on the libgit2 one. Telling somebody git will protect them when it is
 *     cide doing it is a false statement about which program is watching.
 *
 * What this does NOT cover: that the dialog opens, or that confirming pushes anything. That is
 * JSX and a Tauri round trip; `crates/cide-git/tests/staging.rs` owns the other end against real
 * repositories, including the lease.
 *
 * Run: `pnpm --dir ui run check:push`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-push-'))

/*
 * A second output directory, and it has to be **inside `ui`**.
 *
 * `gitOpStore.ts` is driven below as well as read, and unlike the two models it is not
 * import-free: it imports `zustand`. A bare `import { create } from 'zustand'` is resolved
 * relative to the emitted file's own path, so from `tmpdir()` there is no `node_modules` above
 * it and the import dies with ERR_MODULE_NOT_FOUND before an assertion runs.
 * `check-docker-render.mjs` places its bundle here for exactly this reason.
 */
mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
const storeOut = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-push-store-'))

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

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

/** Source with comments removed, so a grep cannot match the explanation of a rule. */
const stripComments = (source) => {
  let out = ''
  let i = 0
  let mode = 'code'
  let quote = ''
  while (i < source.length) {
    const two = source.slice(i, i + 2)
    if (mode === 'code') {
      if (two === '//') {
        mode = 'line'
        i += 2
        continue
      }
      if (two === '/*') {
        mode = 'block'
        i += 2
        continue
      }
      if (source[i] === "'" || source[i] === '"' || source[i] === '`') {
        mode = 'string'
        quote = source[i]
        out += source[i]
        i += 1
        continue
      }
      out += source[i]
      i += 1
      continue
    }
    if (mode === 'line') {
      if (source[i] === '\n') {
        mode = 'code'
        out += '\n'
      }
      i += 1
      continue
    }
    if (mode === 'block') {
      if (two === '*/') {
        mode = 'code'
        i += 2
        continue
      }
      if (source[i] === '\n') out += '\n'
      i += 1
      continue
    }
    // string
    if (source[i] === '\\') {
      out += source.slice(i, i + 2)
      i += 2
      continue
    }
    if (source[i] === quote) mode = 'code'
    out += source[i]
    i += 1
  }
  return out
}

try {
  // --- the wire shapes, first --------------------------------------------------------------
  //
  // The model declares these structurally so it can compile alone, which means a rename in
  // `crates/cide-ipc/src/git.rs` would typecheck on both sides and produce `undefined` in a
  // dialog. This is the join that catches it.

  const generated = read('../src/ipc/generated.ts')
  const shape = (type) => {
    const at = generated.indexOf(`export type ${type} = `)
    if (at === -1) return ''
    return generated.slice(at, generated.indexOf('\n\n', at))
  }

  for (const [type, fields] of [
    [
      'PushPreview',
      [
        'repo',
        'head',
        'remote',
        'remotes',
        'refspec',
        'publish',
        'commits',
        'more',
        'diverged',
        'shellsOut',
        'blocked',
      ],
    ],
    ['PulledCommit', ['shortOid', 'summary', 'author']],
    ['PushRequest', ['remote', 'refspec', 'setUpstream', 'force']],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still its name on the wire`)
    }
  }
  ok(
    /export type PushBlock = "unborn" \| "detached" \| "noRemote"/.test(generated),
    'PushBlock still spells the three states blockedNote switches on — a fourth would fall ' +
      'through to an empty sentence and draw a disabled row explaining nothing',
  )
  ok(
    /"kind": "pushLeaseStale", "detail": \{ branch: string, expected: string, actual: string, \}/.test(
      generated,
    ),
    "GitError still carries pushLeaseStale with both oids — the lease's refusal is the one " +
      'refusal whose point is naming what the user has not seen',
  )
  ok(/"kind": "push", "detail": \{ output: string, \}/.test(generated), 'and the push arm itself')

  // --- compile the model on its own ---------------------------------------------------------

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
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      // Two models in one compile, the shape `check-log-actions.mjs` uses. Both are import-free,
      // which is what makes `types: []` survivable.
      files: [
        join(UI, 'src', 'chrome', 'pushModel.ts'),
        join(UI, 'src', 'chrome', 'gitOpModel.ts'),
      ],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'chrome', 'pushModel.js')}`)
  const g = await import(`file://${join(out, 'chrome', 'gitOpModel.js')}`)

  // --- fixtures ------------------------------------------------------------------------------

  const commit = (oid, summary, author = 'Ivan') => ({ shortOid: oid, summary, author })
  const preview = (over = {}) => ({
    repo: { id: 'r1', name: 'cide' },
    head: { head: 'master', upstream: 'origin/master' },
    remote: 'origin',
    remotes: ['origin'],
    refspec: 'refs/heads/master:refs/heads/master',
    publish: false,
    commits: [commit('a1b2c3d4', 'fix pane host leak'), commit('9f8e7d6c', 'add push dialog')],
    more: 0,
    diverged: false,
    shellsOut: true,
    ...over,
  })

  // --- what a row would send -----------------------------------------------------------------

  eq(m.outgoing(preview()), 2, 'outgoing counts the listed commits')
  eq(
    m.outgoing(preview({ commits: [commit('a1b2c3d4', 'x')], more: 40 })),
    41,
    'and the ones over the cap, which is the whole point of `more` travelling',
  )
  ok(m.pushable(preview()), 'a row with commits and no block is pushable')
  ok(
    !m.pushable(preview({ commits: [], more: 0 })),
    'a row already up to date is not — ticking it would report a push of nothing as a success',
  )
  ok(
    !m.pushable(preview({ blocked: 'detached', commits: [], refspec: '' })),
    'and neither is a blocked one',
  )

  eq(
    m.defaultChecked([preview(), preview({ repo: { id: 'r2', name: 'up-to-date' }, commits: [] })]),
    ['r1'],
    'only rows with something to send open ticked',
  )

  // --- the target label ----------------------------------------------------------------------

  eq(m.targetLabel(preview()), 'master → origin: master', 'the row names branch, remote and dest')
  eq(
    m.targetLabel(preview({ publish: true })),
    'master → origin: master (new branch)',
    'a publish says so — it is the push people are least sure about',
  )
  eq(
    m.targetLabel(preview({ refspec: 'refs/heads/master:refs/heads/trunk' })),
    'master → origin: trunk',
    'and the destination is read off the refspec, because a push may rename on the way',
  )
  eq(
    m.targetLabel(preview({ refspec: '', blocked: 'noRemote' })),
    'origin',
    'a blocked row has no refspec to parse and falls back to the remote, which is still true',
  )

  // --- blocked rows --------------------------------------------------------------------------

  for (const [blocked, needle] of [
    ['unborn', 'No commits yet'],
    ['detached', 'Check a branch out'],
    ['noRemote', 'git remote add'],
  ]) {
    const note = m.blockedNote(preview({ blocked, commits: [], refspec: '' }))
    ok(note !== '', `${blocked} has a sentence`)
    ok(note.includes(needle), `and ${blocked} names its remedy, not just its state`)
  }
  eq(m.blockedNote(preview()), '', 'a row that can push carries no blocked note')

  // --- the force warning, and who performs the lease ------------------------------------------

  eq(m.forceNote(preview()), '', 'a fast-forward row carries no force warning')
  const shelled = m.forceNote(preview({ diverged: true, shellsOut: true }))
  const local = m.forceNote(preview({ diverged: true, shellsOut: false }))
  ok(shelled.includes('git refuses'), 'on the binary route the warning says GIT performs the lease')
  ok(local.includes('cide checks'), 'and on the libgit2 route it says CIDE does — libgit2 has none')
  ok(shelled !== local, 'the two wordings are genuinely different, not one string with a variable')
  ok(
    shelled.includes('origin/master') && shelled.includes('overwrites'),
    'and both name the ref at stake and what forcing does to it',
  )

  // --- the button ----------------------------------------------------------------------------

  const two = [preview(), preview({ repo: { id: 'r2', name: 'salsa' }, commits: [commit('5c4b', 'patch')] })]
  eq(m.outgoingTotal(two, ['r1', 'r2']), 3, 'the total is every ticked row')
  eq(m.outgoingTotal(two, ['r2']), 1, 'and follows the ticks')
  eq(m.outgoingTotal(two, []), 0, 'nothing ticked is nothing to push')

  eq(m.confirmLabel(2, false), 'Push 2 commits', 'the count is ON the button')
  eq(m.confirmLabel(1, false), 'Push 1 commit', 'and it is singular for one')
  eq(m.confirmLabel(2, true), 'Force push 2 commits', 'forcing changes the verb, not the count')
  eq(m.confirmLabel(0, false), 'Push', 'and a count of nothing is left off rather than read "0"')

  // --- force applies only where it answers something -----------------------------------------

  ok(!m.forceApplies(two, ['r1', 'r2']), 'no diverged row means force can change nothing')
  const diverged = [preview({ diverged: true }), two[1]]
  ok(m.forceApplies(diverged, ['r1']), 'a ticked diverged row is what makes force live')
  ok(
    !m.forceApplies(diverged, ['r2']),
    'and unticking it takes the offer away — force must not stay live over rows it cannot help',
  )
  ok(
    !m.forceApplies([preview({ diverged: true, blocked: 'detached', commits: [] })], ['r1']),
    'a blocked row cannot make force live either, since nothing about it will be sent',
  )

  // --- the body sentence ----------------------------------------------------------------------

  ok(m.pushSummary([preview()], ['r1']).includes('2 commits to master → origin: master'))
  ok(m.pushSummary(two, ['r1', 'r2']).includes('2 repositories'), 'and names the count of roots')
  ok(
    m.pushSummary(two, []).includes('Tick a repository'),
    'nothing ticked says how to proceed rather than reporting nothing to do',
  )
  ok(
    m.pushSummary([preview({ commits: [] })], []).includes('already up to date'),
    'and nothing pushable anywhere says THAT instead — a different fact, a different sentence',
  )

  // --- the model imports nothing ---------------------------------------------------------------

  const model = read('../src/chrome/pushModel.ts')
  ok(
    !/^\s*import\b/m.test(model),
    'pushModel.ts imports nothing, not even type-only — that is what lets this script compile ' +
      'it standalone, and a single import turns this check into a module-resolution error',
  )

  // --- the dialog ------------------------------------------------------------------------------

  const dialog = stripComments(read('../src/chrome/PushDialog.tsx'))
  ok(dialog.includes('setForce(false)'), 'force is reset for every new question')
  ok(
    /useEffect\(\(\) => \{[\s\S]*?setForce\(false\)[\s\S]*?\}, \[pending\]\)/.test(dialog),
    'and the reset is keyed on `pending`, so a reopen after a dismissal cannot inherit a tick',
  )
  ok(
    !/useState\(true\)/.test(dialog),
    'nothing in this dialog starts ticked by hand — force above all. ConfirmDestructive ticks ' +
      'its option by default because that option makes an act recoverable; this one makes an ' +
      'act irreversible on a machine that is not the user’s',
  )
  ok(
    /variant="primary"\s+data-audit="pushCancel"/.test(dialog),
    'the accent is on Cancel — rule 3, and this dialog is opened by a keystroke',
  )
  ok(
    /ref=\{cancel\}/.test(dialog) && /cancel\.current\?\.focus\(\)/.test(dialog),
    'and so is the focus, placed once in an effect rather than in a ref callback that would ' +
      'run on every render and yank focus back off the force checkbox',
  )
  ok(
    /variant=\{forcing \? 'danger' : 'secondary'\}/.test(dialog),
    'the danger fill is on the confirm button only while forcing — it means one thing in this app',
  )
  ok(
    /disabled=\{total === 0\}/.test(dialog),
    'and the button is dead when nothing would be sent, rather than sending nothing',
  )
  ok(
    /const forcing = force && canForce/.test(dialog),
    'a tick made irrelevant by unticking its row does not travel — the state is kept so ' +
      're-ticking restores the answer, but the wire sees only what applies',
  )
  ok(
    /if \(ev\.key === 'Escape'\)[\s\S]*?ev\.stopPropagation\(\)/.test(dialog),
    'Escape is handled on the card and stops there, never on a window listener',
  )

  // --- the runner --------------------------------------------------------------------------------

  const run = stripComments(read('../src/chrome/pushRun.ts'))
  ok(
    /force: force && preview\.diverged/.test(run),
    'force is masked per row: a --force-with-lease over a fast-forward is a no-op, and sending ' +
      'one anyway puts the flag on pushes that never needed it',
  )
  ok(
    /setUpstream: preview\.publish/.test(run),
    'and --set-upstream comes from the preview rather than being decided at each call site, ' +
      'which is how *Commit and Push* and the branch popup disagreed about it before M20',
  )
  ok(
    /Promise\.allSettled\(attempts\)/.test(run) && /for \(const attempt of attempts\)/.test(run),
    'allSettled over the SAME promises each individually caught — Promise.all reports the ' +
      'first failure and marks the rest handled',
  )
  ok(
    /explain\(error, 'push'\)/.test(run),
    "and a rejection becomes a sentence: GitError's push arm is {kind, detail:{output}}, whose " +
      'detail is an object, so describe() falls through to the bare word `push`',
  )
  ok(
    /pushReport\(done\)/.test(run),
    'successes are aggregated into ONE notice — notices.admit dedupes by text, so five ' +
      'submodules saying "already up to date" would show one toast speaking for five',
  )

  // --- and every surface goes through it ---------------------------------------------------------

  for (const [file, what] of [
    ['../src/keys/dispatch.ts', 'the palette'],
    ['../src/chrome/BranchSelector.tsx', 'the branch popup'],
    ['../src/sidebar/GitPanel/useGitPanel.ts', 'Commit and Push…'],
  ]) {
    const source = stripComments(read(file))
    ok(
      /openPushDialog\(/.test(source),
      `${what} opens the dialog rather than pushing — two routes to one gesture that disagreed ` +
        'about whether it asks first is the split M20 already paid for here',
    )
    ok(
      !/gitApi\.push\(/.test(source),
      `and ${what} no longer calls git_push directly`,
    )
  }

  // --- the in-flight indicator (M65) -------------------------------------------------------
  //
  // Between pressing Push and the toast there was nothing at all, and `git_push` is one blocking
  // round trip its own doc says can sit on the network "for minutes". These pin the parts of the
  // answer that fail silently.

  const gitOpModel = read('../src/chrome/gitOpModel.ts')
  ok(
    !/^\s*import\b/m.test(gitOpModel),
    'gitOpModel.ts imports nothing either — same rule as pushModel.ts above, and the same ' +
      'reason: one import and this script becomes a module-resolution error',
  )

  const running = (over = {}) => ({
    id: 0,
    project: 'p1',
    kind: 'push',
    done: 0,
    total: 1,
    ...over,
  })

  ok(g.gitOpLabel([], 'p1') === '', 'nothing running is the empty string, which draws nothing')
  ok(
    g.gitOpLabel([running({ project: 'other' })], 'p1') === '',
    "and another project's push draws nothing HERE — the indicator is scoped exactly as a " +
      'notice is, because in Stacked window mode one window holds every open project',
  )
  for (const [kind, verb] of [
    ['push', 'Pushing…'],
    ['pull', 'Pulling…'],
    ['fetch', 'Fetching…'],
    ['merge', 'Merging…'],
  ]) {
    ok(
      g.gitOpLabel([running({ kind })], 'p1') === verb,
      `${kind} says ${verb} — four verbs, because "Working…" over four different operations is ` +
        'the readout saying less than the button that started it',
    )
  }
  ok(
    g.gitOpLabel([running({ total: 3 })], 'p1') === 'Pushing 0/3…',
    'a fan-out over three repositories opens at 0/3 — `done` counts answers RECEIVED, so the ' +
      'honest opening number is zero and it is the one that moves',
  )
  ok(
    g.gitOpLabel([running({ total: 3, done: 2 })], 'p1') === 'Pushing 2/3…',
    'and climbs as they land',
  )
  ok(
    g.gitOpLabel([running({ id: 0, kind: 'push' }), running({ id: 1, kind: 'fetch' })], 'p1')
      === 'Pushing…',
    'two overlapping gestures: the OLDEST wins the label. Summing across kinds gives a ' +
      'fraction whose halves count different things, and showing the newest makes the label ' +
      'jump backwards the moment a second one starts — which reads as the first having failed',
  )
  ok(
    g.gitOpTitle([running({ id: 0 }), running({ id: 1, kind: 'fetch' })], 'p1')
      === 'Pushing · Fetching',
    'the TOOLTIP is where the second gesture becomes visible — it has the room the 26px bar ' +
      'does not, and it is the only surface that admits to more than one',
  )
  ok(g.gitOpTitle([], 'p1') === '', 'and it is empty when nothing is running')

  // The store's two traps, both of which are invisible on a push that succeeds.
  const store = stripComments(read('../src/chrome/gitOpStore.ts'))
  ok(
    /work\.then\(settle, settle\)/.test(store) && !/work\.finally\(/.test(store),
    'trackGitOp attaches `then(settle, settle)` and never `.finally`. `.finally` returns a NEW ' +
      'promise that rejects when the original does, with nothing attached to it — so one failed ' +
      'fetch fires a second unhandledrejection and Failures raises a second toast, whose text ' +
      'is the bare word `fetch` because describe() falls through a GitError with no message',
  )
  ok(
    /return work\b/.test(store),
    'and it hands back the ORIGINAL promise, not the derived one — otherwise whether a failure ' +
      'is reported at all depends on every call site using the return value',
  )

  ok(
    /beginGitOp\(project, 'push', previews\.length\)/.test(run),
    'the push gesture is begun with previews.length, so four repositories draw `Pushing 0/4…` ' +
      'and count up rather than one undifferentiated spinner for the lot',
  )
  ok(
    /void sent\.then\(settle, settle\)/.test(run) && !/\.finally\(settle\)/.test(run),
    'and pushPass settles the same way, for the same reason — this one is on top of an error ' +
      'path that already toasts, so the symptom is two toasts for one failed push',
  )

  // Comments stripped, and in this repository that is mandatory rather than tidy: the house
  // style is to name the failure a rule prevents, so this stylesheet's own prose spells
  // `prefers-reduced-motion` and `font-size` while stating that it declares neither. Both
  // assertions below passed by matching their own explanation the first time they ran.
  const indicatorCss = stripComments(read('../src/chrome/GitOpIndicator.module.css'))
  ok(
    /animation-play-state:\s*var\(--motion-loop/.test(indicatorCss),
    'the spinner answers reduced motion through --motion-loop. Nothing else in the suite can ' +
      'see this: check:motion fences `transition` and is completely blind to `animation`. And ' +
      'the tokens block deliberately does NOT zero --dur-sweep, because an infinite animation ' +
      'at 0.01ms strobes — which is the opposite of what the setting asks for',
  )
  ok(
    !/prefers-reduced-motion/.test(indicatorCss),
    'and it does NOT declare its own media query — check:motion asserts exactly one such block ' +
      'exists in ui/src and that it is in styles/tokens.css',
  )
  ok(
    /animation:\s*statusbar-spin var\(--dur-sweep\)/.test(indicatorCss)
      && /@keyframes statusbar-spin/.test(indicatorCss),
    'the loop runs for --dur-sweep, the token that exists for exactly this',
  )
  ok(
    !/font-size/.test(indicatorCss),
    'and states no font-size — it inherits --fs-ui-12 from .bar, and a bare px value is a ' +
      'label that silently stops following the UI font size',
  )

  /*
   * The settle arithmetic, driven. (M65)
   *
   * The source rules above pin the *shape* of `trackGitOp`; this drives the counter, because
   * every way it can be wrong is silent and permanent in one direction or the other. Settle once
   * too few and the bar turns for the rest of the session over a push that finished; settle once
   * too many and it goes clean while commits are still leaving the machine, which is the exact
   * claim the indicator exists to make honestly.
   */
  const storeTsconfig = join(storeOut, 'tsconfig.json')
  writeFileSync(
    storeTsconfig,
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
        outDir: storeOut,
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      // `gitOpStore.ts`'s other two imports are `import type`, which `verbatimModuleSyntax`
      // erases — so the emitted file's only real dependency is zustand.
      files: [join(UI, 'src', 'chrome', 'gitOpStore.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', storeTsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })
  const live = await import(`file://${join(storeOut, 'chrome', 'gitOpStore.js')}`)
  const label = () => g.gitOpLabel(live.useGitOps.getState().running, 'p1')

  const oneUnit = live.beginGitOp('p1', 'push')
  ok(label() === 'Pushing…', 'beginGitOp puts a gesture on the bar')
  oneUnit()
  ok(label() === '', 'and its settle takes it off again')
  oneUnit()
  ok(
    label() === '',
    'settling twice is a no-op. `then(settle, settle)` is the cheapest correct way to catch ' +
      'both outcomes, so the guard is what makes that spelling safe',
  )

  const three = live.beginGitOp('p1', 'push', 3)
  ok(label() === 'Pushing 0/3…', 'a fan-out over three repositories opens at 0/3')
  three()
  three()
  ok(label() === 'Pushing 2/3…', 'and counts answers as they land')
  three()
  ok(label() === '', 'the last answer clears it')
  three()
  three()
  ok(label() === '', 'and further settles can neither resurrect it nor drive `done` past `total`')

  const elsewhere = live.beginGitOp('p2', 'fetch')
  ok(
    label() === '' && g.gitOpLabel(live.useGitOps.getState().running, 'p2') === 'Fetching…',
    'a gesture is drawn only in its own project — the scope a notice has, for the reason a ' +
      'notice has it',
  )
  elsewhere()

  const zero = live.beginGitOp('p1', 'push', 0)
  ok(
    label() === '',
    'a gesture with nothing to wait for never appears at all. Without this guard a zero-total ' +
      'row sits in `running` for ever, because nothing will ever arrive to settle it',
  )
  zero()

  const boom = new Error('boom')
  const rejected = Promise.reject(boom)
  const handed = live.trackGitOp('p1', 'pull', rejected)
  ok(handed === rejected, 'trackGitOp hands back the ORIGINAL promise, not a derived one')
  ok(label() === 'Pulling…', 'and marks it running')
  let reached = null
  try {
    await handed
  } catch (error) {
    reached = error
  }
  ok(reached === boom, 'the rejection still reaches the caller untouched')
  await new Promise((r) => setTimeout(r, 0))
  ok(
    label() === '',
    'and a REJECTED promise clears the indicator too — the failure path is the one a naive ' +
      '`await work; settle()` leaves turning for ever',
  )

  const bar = stripComments(read('../src/chrome/StatusBar.tsx'))
  const branchAt = bar.indexOf('<BranchSelector />')
  const indicatorAt = bar.indexOf('<GitOpIndicator />')
  const trailAt = bar.indexOf('styles.path')
  ok(
    branchAt >= 0 && indicatorAt > branchAt && trailAt > indicatorAt,
    'and the indicator sits between the branch and the file trail — it is the branch\'s news, ' +
      'and it is the one slot on the bar that appears and disappears, so it goes beside the ' +
      'item that is `flex: none` and ahead of the one that gives way',
  )

  const commands = read('../../crates/cide-core/src/commands.rs')
  ok(
    /Command::new\("git\.push", "Push to remote…"/.test(commands),
    'git.push carries the ellipsis — the table\'s mark for a command that opens something, ' +
      'which this one finally does',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
  rmSync(storeOut, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log('push: ok')
