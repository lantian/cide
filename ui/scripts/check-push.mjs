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
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-push-'))

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
      files: [join(UI, 'src', 'chrome', 'pushModel.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'chrome', 'pushModel.js')}`)

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
    /styles\.primary\}`\}[\s\S]*?data-audit="pushCancel"/.test(dialog),
    'the accent is on Cancel — rule 3, and this dialog is opened by a keystroke',
  )
  ok(
    /ref=\{cancel\}/.test(dialog) && /cancel\.current\?\.focus\(\)/.test(dialog),
    'and so is the focus, placed once in an effect rather than in a ref callback that would ' +
      'run on every render and yank focus back off the force checkbox',
  )
  ok(
    /forcing \? styles\.danger/.test(dialog),
    'the red is on the confirm button only while forcing — --red means one thing in this app',
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

  const commands = read('../../crates/cide-core/src/commands.rs')
  ok(
    /Command::new\("git\.push", "Push to remote…"/.test(commands),
    'git.push carries the ellipsis — the table\'s mark for a command that opens something, ' +
      'which this one finally does',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log('push: ok')
