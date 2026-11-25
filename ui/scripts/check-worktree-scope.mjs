/**
 * Checks `src/chrome/worktreeScope.ts` — one choice of agent checkout driving the Explorer, the
 * Git panel and the Log (M207) — and the wiring that makes those rules reachable.
 *
 * # What this is defending
 *
 * The Explorer can show an agent checkout under `.cide/worktrees/` instead of the project, from
 * an index of its own in Rust (`crate::worktree_fs`). The checkout's watcher reports on the same
 * `cide://fs-changed` / `cide://fs-status` events as the project's, **tagged** with the checkout,
 * and every listener has to decide what a tag means to it. Each wrong decision type-checks and
 * looks fine until the moment it matters:
 *
 *   * the project's tree, branch readout or change count repainting on an agent's `cargo build`
 *     in its checkout;
 *   * a checkout's tree adopting the project's bursts, whose paths it does not hold;
 *   * a Log that keeps walking the project after the Explorer moved to a checkout;
 *   * a consumer whose checkout list is older than the user's fresh pick dropping that pick for
 *     everyone — the race one shared list exists to remove;
 *   * a tree command that forgot the scope, answering rows from the project's index into a tree
 *     that is drawing the checkout.
 *
 * The rules are compiled standalone and driven below; the tail reads the wiring as *source*,
 * comments stripped first (house-style comments name the failure, so the needle is in the prose).
 *
 * Run: `pnpm --dir ui run check:worktree-scope`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const REPO = resolve(UI, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-worktree-scope-'))
let failed = 0
let passed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  } else passed++
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  } else passed++
}

/**
 * Blank out comments, string-aware: `client.ts` is full of `'cide://fs-changed'`, and a stripper
 * that took `//` inside a string for a comment would cut every event name in half.
 */
function stripComments(text) {
  let res = ''
  let i = 0
  while (i < text.length) {
    const c = text[i]
    const two = text.slice(i, i + 2)
    if (c === '"' || c === "'" || c === '`') {
      let j = i + 1
      while (j < text.length && text[j] !== c) j += text[j] === '\\' ? 2 : 1
      res += text.slice(i, j + 1)
      i = j + 1
    } else if (two === '//') {
      while (i < text.length && text[i] !== '\n') i++
    } else if (two === '/*') {
      const end = text.indexOf('*/', i + 2)
      i = end < 0 ? text.length : end + 2
    } else {
      res += c
      i++
    }
  }
  return res
}
const src = (rel) => stripComments(readFileSync(join(UI, 'src', rel), 'utf8'))
const count = (text, re) => (text.match(re) ?? []).length

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/worktreeScope.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit', cwd: UI },
  )
  const m = await import(`file://${join(out, 'chrome', 'worktreeScope.js')}`)

  const A = '/work/demo/.cide/worktrees/developer-t-7'
  const B = '/work/demo/.cide/worktrees/qa-t-9'

  // --- which tree a burst is about ---------------------------------------------------------

  eq(m.adoptsBurst(null, null), true, 'the project tree adopts the project watcher')
  eq(m.adoptsBurst(null, A), false, 'the project tree ignores a checkout view\'s watcher')
  eq(m.adoptsBurst(A, null), false, 'a checkout tree ignores the project\'s, whose paths it does not hold')
  eq(m.adoptsBurst(A, A), true, 'a checkout tree adopts its own watcher')
  eq(m.adoptsBurst(A, B), false, 'and not another checkout\'s')

  eq(m.projectHears(null), true, 'a project mirror (count, branch, the list) hears the project')
  eq(m.projectHears(A), false, 'and never a checkout: it cannot move the project\'s branch or count')

  eq(m.checkoutHears(null, null), true, 'a project-scoped git surface hears the project')
  eq(m.checkoutHears(null, A), false, '…and not a checkout')
  eq(
    m.checkoutHears(A, null),
    true,
    'a checkout-scoped git surface hears the project watcher too: it sees the shared refs move',
  )
  eq(m.checkoutHears(A, A), true, '…and its own checkout\'s, which alone sees its HEAD and index')
  eq(m.checkoutHears(A, B), false, '…and not another checkout\'s')

  // --- the shared choice -------------------------------------------------------------------

  eq(m.keptChoice(A, [{ repo: { root: A } }, { repo: { root: B } }]), A, 'a listed checkout stays chosen')
  eq(m.keptChoice(A, [{ repo: { root: B } }]), null, 'a checkout Integrate removed drops to the project')
  eq(m.keptChoice(A, null), A, 'an unreadable list judges nothing')
  eq(m.keptChoice(null, [{ repo: { root: A } }]), null, 'the project stays the project')

  // --- the Log follows it --------------------------------------------------------------------

  const current = { repo: { id: 'repo-of-a' } }
  eq(m.worktreeLogScope(true, current), { kind: 'one', repo: 'repo-of-a' }, 'the Log tab walks the checkout alone')
  eq(m.worktreeLogScope(false, current), null, 'a History tab is one file in one repository already')
  eq(m.worktreeLogScope(true, null), null, 'no checkout chosen: fall through to the merged scope')
  eq(m.logScopeChain('one', 'pin', 'wt', 'all'), 'one', 'a History tab\'s repository wins')
  eq(m.logScopeChain(null, 'pin', 'wt', 'all'), 'pin', 'then a filter\'s — it narrows without clearing the choice')
  eq(m.logScopeChain(null, null, 'wt', 'all'), 'wt', 'then the shared checkout')
  eq(m.logScopeChain(null, null, null, 'all'), 'all', 'then the project\'s repositories merged')

  // --- the wiring ----------------------------------------------------------------------------

  const uw = src('chrome/useWorktrees.ts')
  ok(
    /export function useWorktrees\(project: ProjectId \| null\)/.test(uw) && !/WorktreeSurface/.test(uw),
    'one choice per project: no per-surface key survives',
  )
  ok(
    count(uw, /keptChoice\(/g) === 1 && /keptChoice\(entry\.chosen, list\)/.test(uw),
    'the stale-choice fallback is judged once, in the store, against the one shared list',
  )
  ok(/projectHears\(tag\)/.test(uw), 'the list re-reads on the project\'s bursts, not a checkout\'s')

  const ex = src('sidebar/Explorer.tsx')
  ok(/const choice = useWorktrees\(project\)/.test(ex), 'the Explorer reads the shared choice')
  ok(
    /useFileTree\.getState\(\)\.attach\(project, worktree\)/.test(ex)
      && /useGitStatus\.getState\(\)\.attach\(project, worktree\)/.test(ex),
    'and attaches the rows and the tags to the same pair',
  )
  ok(/fs\.status\(project, worktree\)/.test(ex), 'and reads the status of the tree it shows')
  ok(count(ex, /adoptsBurst\(worktree, tag\)/g) >= 3, 'every Explorer listener adopts only its own tree\'s bursts')
  ok(
    /choice\.worktrees\.length > 0 &&/.test(ex) && /<WorktreeSelect[\s\S]*?audit="explorerWorktree"/.test(ex),
    'the selector is drawn only when the project has checkouts',
  )
  ok(/worktree === null &&[\s\S]{0,40}<IndexWarning/.test(ex), 'the project index\'s warnings stay the project\'s')

  const badge = src('chrome/gitCountStore.ts')
  ok(
    /gitApi\.status\(project, false, worktree\)/.test(badge) && /checkoutHears\(store\.worktree, tag\)/.test(badge),
    'the rail badge counts the tree the shared choice shows, and hears that tree\'s bursts',
  )
  ok(
    /tag === store\.worktree/.test(badge) && /store\.worktree !== worktree\) return/.test(badge),
    'and adopts a broadcast or a panel\'s tree only for that same tree',
  )
  ok(
    /useGitChangeCount\(badgeProject, badgeCheckout\)/.test(src('App.tsx'))
      && /useWorktrees\(badgeProject\)\.worktree/.test(src('App.tsx')),
    'App passes the shared choice to the badge',
  )
  ok(
    /noteChangeCount\(project, tree, worktree\)/.test(src('sidebar/GitPanel/useGitPanel.ts'))
      && /holdChangeCount\(project, worktree\)/.test(src('sidebar/GitPanel/useGitPanel.ts')),
    'the Git panel feeds and holds the badge for the tree it shows',
  )
  ok(count(src('chrome/BranchSelector.tsx'), /projectHears\(tag\)/g) >= 2, 'both branch-readout listeners do too')
  ok(
    /checkoutHears\(worktree, tag\)/.test(src('sidebar/GitPanel/useGitPanel.ts')),
    'the Git panel hears the project plus its own checkout',
  )
  ok(/useWorktrees\(props\.project\)/.test(src('sidebar/GitPanel/GitPanelHost.tsx')), 'and reads the shared choice')
  ok(/checkoutHears\(scopedCheckoutRoot, tag\)/.test(src('gitlog/LogTab.tsx')), 'the Log likewise')

  const tree = src('sidebar/treeStore.ts')
  const calls = tree.match(/fsApi\.\w+\([^)]*\)/g) ?? []
  ok(calls.length > 15, `the tree store's fsApi calls were found (${calls.length})`)
  for (const call of calls) ok(/, worktree\)$/.test(call), `every tree call is scoped: ${call}`)
  ok(!/get\(\)\.project !== project/.test(tree), 'every guard after a round trip asks inScope, not the project alone')
  ok(/treeStatus\(project, worktree\)/.test(src('sidebar/gitStatusStore.ts')), 'the tags are the shown tree\'s')

  const client = src('ipc/client.ts')
  const rust = readFileSync(join(REPO, 'crates/cide-app/src/cmd/fs.rs'), 'utf8')
  const gitRust = readFileSync(join(REPO, 'crates/cide-app/src/cmd/git.rs'), 'utf8')
  for (const command of [
    'fs_status', 'fs_tree_count', 'fs_tree_rows', 'fs_tree_match', 'fs_expand', 'fs_collapse',
    'fs_browse_page', 'fs_expand_all', 'fs_collapse_all', 'fs_refresh', 'fs_reveal',
    'fs_writable_roots', 'fs_reveal_roots', 'git_tree_status',
  ]) {
    const at = client.indexOf(`'${command}', {`)
    ok(at >= 0 && /worktree/.test(client.slice(at, client.indexOf('}', at))), `client.ts passes the scope to ${command}`)
    const body = command.startsWith('git_') ? gitRust : rust
    const sig = body.indexOf(`pub async fn ${command}(`)
    ok(
      sig >= 0 && /worktree: Option<PathBuf>/.test(body.slice(sig, body.indexOf(') -> ', sig))),
      `Rust's ${command} takes the scope`,
    )
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`check-worktree-scope: ${failed} failure(s)`)
  process.exit(1)
}
console.log(`check-worktree-scope: ok (${passed} assertions)`)
