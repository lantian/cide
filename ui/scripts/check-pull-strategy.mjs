/**
 * Checks `src/chrome/pullStrategyModel.ts` and the store beside it — the *merge or rebase?*
 * dialog. (M20)
 *
 * `check-branches.mjs`'s recipe, and its header's argument for why this shape exists at all
 * holds unchanged. What is worth pinning *here* is narrower and sharper than a list of rows:
 *
 *   - **`divergenceOf` must survive anything.** It runs inside a `catch`, and a catch block that
 *     itself threw would turn a recoverable question into an unhandled rejection — the one
 *     failure this whole surface exists to prevent.
 *   - **Merge must be `choices[0]`.** `ConfirmDestructive` resolves an unknown `chosen` —
 *     including the `null` a dialog opens with — to the first choice, so the ordering *is* the
 *     default answer. Getting it backwards would make a reflexive click rewrite history.
 *   - **Exactly one `danger`, and it is rebase.** `--red` means one thing in this app; spending
 *     it on the answer that rewrites nothing is how it stops meaning anything on the one that
 *     does.
 *   - **The lists must name what each answer risks, per choice.** Rule 1 of
 *     `ConfirmDestructive` holds per choice, and a rebase risks *your* commits — listing the
 *     incoming ones under both answers would name things at risk under neither.
 *
 * What this does NOT cover: that the dialog opens, or that answering it pulls anything. That is
 * JSX and a Tauri round trip; `crates/cide-git/tests/pull.rs` owns the other end against real
 * repositories.
 *
 * Run: `pnpm --dir ui run check:pull-strategy`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-pull-strategy-'))

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
      'Divergence',
      ['branch', 'remote', 'upstream', 'ahead', 'behind', 'incoming', 'moreIncoming', 'local', 'moreLocal'],
    ],
    ['PulledCommit', ['shortOid', 'summary', 'author']],
  ]) {
    const body = shape(type)
    ok(body !== '', `${type} exists in generated.ts`)
    for (const field of fields) {
      ok(new RegExp(`\\b${field}\\b`).test(body), `${type}.${field} is still its name on the wire`)
    }
  }
  ok(
    /"kind": "pullNeedsStrategy", "detail": Divergence/.test(generated),
    'GitError still carries pullNeedsStrategy with a Divergence payload — the shape divergenceOf unpacks',
  )
  ok(
    /export type PullStrategy = "fastForward" \| "merge" \| "rebase"/.test(generated),
    'PullStrategy still spells its three variants the way the model does',
  )
  ok(
    /export type PullDefault = "ask" \| "fastForward" \| "merge" \| "rebase"/.test(generated),
    'PullDefault still carries `ask`, which is the only value that opens this dialog',
  )
  ok(
    /pullStrategy: PullDefault/.test(generated),
    'GitSettings still names the default the Settings screen writes',
  )

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
      files: [join(UI, 'src', 'chrome', 'pullStrategyModel.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'chrome', 'pullStrategyModel.js')}`)

  // --- fixtures ------------------------------------------------------------------------------

  const commit = (oid, summary, author = 'Ivan') => ({ shortOid: oid, summary, author })
  const divergence = (over = {}) => ({
    branch: 'main',
    remote: 'origin',
    upstream: 'origin/main',
    ahead: 2,
    behind: 5,
    incoming: [commit('aaaaaaa1', 'their work', 'Someone')],
    moreIncoming: 0,
    local: [commit('bbbbbbb1', 'my work'), commit('bbbbbbb2', 'more of mine')],
    moreLocal: 0,
    ...over,
  })
  const rejection = (over = {}) => ({ kind: 'pullNeedsStrategy', detail: divergence(over) })
  const repo = (name = 'app', over = {}) => ({ name, repo: `id-${name}`, diverged: divergence(over) })

  // --- divergenceOf survives anything --------------------------------------------------------

  eq(m.divergenceOf(rejection())?.branch, 'main', 'divergenceOf unpacks the variant it is for')
  eq(m.divergenceOf(rejection())?.ahead, 2, 'and both counts come through')
  eq(m.divergenceOf(rejection()).local.length, 2, 'and the local commits, which rebase risks')

  for (const junk of [
    null,
    undefined,
    'boom',
    42,
    {},
    { kind: 7 },
    { kind: 'notFastForward', detail: {} },
    { kind: 'pullNeedsStrategy' },
    { kind: 'pullNeedsStrategy', detail: null },
    { kind: 'pullNeedsStrategy', detail: 'nope' },
    { kind: 'pullNeedsStrategy', detail: { branch: 1 } },
  ]) {
    eq(m.divergenceOf(junk), null, `divergenceOf answers null for ${JSON.stringify(junk)}`)
  }
  eq(
    m.divergenceOf({ kind: 'pullNeedsStrategy', detail: { branch: 'x', ahead: 1, behind: 1 } })
      ?.incoming,
    [],
    'a payload missing its lists still parses — the dialog degrades rather than throwing',
  )

  // --- the ask -------------------------------------------------------------------------------

  const ask = m.strategyAsk([repo()])
  ok(ask !== null, 'one diverged repository produces a dialog')
  eq(ask.choices.length, 2, 'two answers, merge and rebase — fast-forward-only is a setting')
  eq(ask.choices[0].id, 'merge', 'MERGE IS FIRST: ConfirmDestructive falls back to choices[0]')
  eq(ask.choices[1].id, 'rebase', 'and rebase is second')
  eq(
    ask.choices.filter((c) => c.danger).map((c) => c.id),
    ['rebase'],
    'exactly one danger, and it is the answer that rewrites commits',
  )
  eq(ask.choices[0].files, [], 'merge risks nothing of yours, and the empty list is the argument')
  ok(
    ask.choices[1].files.some((f) => f.includes('bbbbbbb1')),
    'rebase lists YOUR commits — the ones it would rewrite',
  )
  ok(
    !ask.choices[1].files.some((f) => f.includes('aaaaaaa1')),
    'and never the incoming ones, which are at risk under neither answer',
  )
  eq(
    ask.mark,
    'refresh-cw',
    'the mark is not the removal dash: nothing is removed under either answer',
  )
  eq(ask.split, false, 'entries are commits, not paths — see ConfirmState.split')
  ok(!JSON.stringify(ask).includes('[object Object]'), 'nothing renders as [object Object]')
  ok(
    ask.choices.every((c) => c.body.length > 0 && c.confirmLabel.length > 0),
    'every choice has a body and a button label',
  )

  // Rule 2: it only appears when something is at risk.
  eq(m.strategyAsk([]), null, 'no repositories, no dialog')
  eq(m.strategyAsk([repo('app', { ahead: 0 })]), null, 'nothing of yours at stake, no dialog')

  // The overflow line, because Rust caps the list and this side never had the rows.
  const capped = m.strategyAsk([repo('app', { moreLocal: 9 })])
  ok(
    capped.choices[1].files.some((f) => f.includes('9 commits more')),
    'a capped list says how many more there were rather than truncating in silence',
  )

  // --- several repositories, one answer -------------------------------------------------------

  const many = m.strategyAsk([repo('app'), repo('vendor')])
  ok(many.title.includes('2 repositories'), 'the title says how many it is answering for')
  ok(
    many.choices[0].files.length === 2,
    'merge names every repository the single answer covers, which is what makes it honest',
  )
  ok(
    many.choices[1].files.every((f) => f.startsWith('app: ') || f.startsWith('vendor: ')),
    'and rebase labels each commit with the repository it is from',
  )

  // --- strategyOf, which must agree with ConfirmDestructive's own fallback ----------------------

  eq(m.strategyOf('rebase'), 'rebase', 'the rebase radio')
  eq(m.strategyOf('merge'), 'merge', 'the merge radio')
  eq(m.strategyOf(null), 'merge', 'NULL FALLS BACK TO MERGE, matching choices[0]')
  eq(m.strategyOf(undefined), 'merge', 'and so does undefined')
  eq(m.strategyOf('nonsense'), 'merge', 'and so does anything unrecognised')

  // --- the remember label ----------------------------------------------------------------------

  const one = [repo('app')]
  ok(m.rememberLabel(one, 'rebase').includes('rebase'), 'the label names the act, not "this choice"')
  ok(m.rememberLabel(one, 'merge').includes('merge'), 'and follows the radio')
  ok(
    m.rememberLabel(one, null).includes('merge'),
    'and agrees with strategyOf about the default',
  )
  ok(
    m.rememberLabel(one, 'rebase').includes('pull.rebase'),
    'and names the config key, because that is the file it is about to change',
  )
  ok(
    m.rememberLabel([repo('app'), repo('vendor')], 'merge').includes('2 repositories'),
    'and says how many repositories it would write to',
  )

  // --- source pins -----------------------------------------------------------------------------

  const model = read('../src/chrome/pullStrategyModel.ts')
  ok(
    !/^\s*import\s/m.test(stripComments(model)),
    'pullStrategyModel.ts has NO imports at all — a value import is what stops this check compiling it',
  )

  const app = stripComments(read('../src/App.tsx'))
  const mounts = app.split('<PullStrategyGate />').length - 1
  eq(
    mounts,
    2,
    'PullStrategyGate is mounted in BOTH App.tsx branches — the key gate is installed before the ' +
      'detached-pane early return, so Ctrl+T is live in a torn-out window and a gate wired only ' +
      'into the shell would ask nobody',
  )

  const dispatch = stripComments(read('../src/keys/dispatch.ts'))
  ok(dispatch.includes('requestPullStrategy('), 'dispatch raises the dialog')
  ok(dispatch.includes('divergenceOf('), 'and recognises the refusal that opens it')
  ok(
    dispatch.includes('Promise.allSettled(attempts)'),
    'and still reads the SAME promises both loops read — the property the case comment depends on',
  )
  ok(
    /strategy === null && divergenceOf\(error\)/.test(dispatch),
    'the divergence is only swallowed on the FIRST pass, so a repeat cannot loop the dialog',
  )
  ok(
    /skipFetch: strategy !== null/.test(dispatch),
    'and the retry skips the fetch, so the answer acts on the refs the user was shown',
  )

  const gate = stripComments(read('../src/chrome/PullStrategyGate.tsx'))
  ok(gate.includes('setRemember(false)'), 'the remember box is reset for every new question')
  ok(
    !/checked: true/.test(gate),
    'and starts UNTICKED — it makes an answer permanent in a config file, which inverts ' +
      "ConfirmDestructive's usual rule for an option that makes an act recoverable",
  )

  const confirm = stripComments(read('../src/chrome/ConfirmDestructive.tsx'))
  ok(
    /choices\.find\(\(c\) => c\.id === state\.chosen\) \?\? choices\[0\]/.test(confirm),
    'ConfirmDestructive still resolves an unknown chosen to choices[0] — the rule strategyOf mirrors',
  )
  ok(/state\.split !== false/.test(confirm), 'and still honours split, which this dialog needs')

  const commands = read('../../crates/cide-core/src/commands.rs')
  ok(
    !/Command::new\("git\.pull", "Pull \(fast-forward only\)"/.test(commands),
    'git.pull is no longer titled "fast-forward only" — it merges and rebases now',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log('pull strategy: ok')
