/**
 * The file tree must not flicker under `cide://fs-changed`.
 *
 * The bug: `treeStore.refresh()` answered every watcher burst by emptying the row cache, so
 * every visible row's `rowAt` went `undefined` until `fs_tree_rows` came back — a blank frame
 * and a refill, once per burst, whether or not a row had moved. Most bursts move nothing: the
 * watcher watches `.git/HEAD`, `.git/index` and the refs deliberately (the git tags need
 * them) and `Index::apply` skips those paths by name because they can change no row.
 *
 * The two properties pinned here are the two halves of "not flickering":
 *
 *   * a burst that moved nothing the tree is showing produces **zero re-renders**, and
 *   * a burst that did move rows lands in one update that already has the rows in it, so no
 *     frame ever shows fewer rows than the frame before it.
 *
 * The second is the one a naive fix misses, which is why the digest carries the row count for
 * *every* render rather than only the last.
 *
 * Same harness as `check-rows.mjs`: SSR-bundle an entry, run it under node, read its digest.
 * There is no DOM — see `src/sidebar/treeRefreshSmoke.ts` for how a "render" is counted.
 *
 * Run: `node scripts/check-tree-flicker.mjs` (from `ui/`)
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

// Under `node_modules/.cache` rather than the system temp dir: the SSR bundle keeps zustand
// and `@tauri-apps/api` external, so node resolves them relative to wherever the output sits.
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-tree-flicker-'))
let failed = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/sidebar/treeRefreshSmoke.ts',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  // `@tauri-apps/api` reads `window` when a command is invoked, and the smoke entry installs
  // its fake backend on `__TAURI_INTERNALS__`.
  globalThis.window = globalThis
  globalThis.location = { search: '' }

  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'treeRefreshSmoke.js')}`)
  // The entry ends on a timer, so the digest is not printed by the time the import resolves.
  await new Promise((r) => setTimeout(r, 1500))
  console.log = log

  const d = JSON.parse(printed.at(-1))

  // Key order is not part of the claim: `callsSince` emits commands in first-call order.
  const stable = (v) =>
    v !== null && typeof v === 'object' && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b)))
      : v
  const eq = (actual, expected, what) => {
    const a = JSON.stringify(stable(actual))
    const b = JSON.stringify(stable(expected))
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }
  const ok = (cond, what) => {
    if (!cond) {
      console.error(`FAIL ${what}`)
      failed++
    }
  }

  // --- the tree is primed: 30 rows on screen -------------------------------------------------

  eq(d.primed.rows, 30, 'the viewport is showing its 30 rows before anything is asked of it')

  // --- a burst that moved nothing ------------------------------------------------------------

  eq(d.quiet.renders, 0, 'a burst that moved no row must not re-render the tree at all')
  eq(d.quiet.drawn, [], 'and so must not draw a frame')
  eq(d.quiet.rows, 30, 'the 30 rows are still there afterwards')
  eq(
    d.quiet.calls,
    { fs_tree_count: 1, fs_tree_rows: 1, git_tree_status: 1 },
    'it costs one count, one visible chunk and the tag map — not a re-read of the cache',
  )

  // --- a burst that did move rows ------------------------------------------------------------

  ok(d.changed.renders >= 1, 'a real insertion has to reach the tree')
  eq(d.changed.renders, 1, 'and in one update, not a blank one followed by a refill')
  ok(
    d.changed.drawn.every((n) => n === 30),
    `no frame may show fewer rows than the 30 already on screen; saw ${JSON.stringify(d.changed.drawn)}`,
  )
  eq(d.changed.count, 501, 'the count follows the insertion')
  eq(d.changed.rowFive, 'new.rs', 'and row 5 is the new file, not the one it displaced')
  eq(
    d.changed.calls,
    { fs_tree_count: 1, fs_tree_rows: 1 },
    'still one count and one visible chunk: a burst costs the viewport, not CHUNK_CAP',
  )

  // --- a burst that only moved rows off screen -----------------------------------------------

  eq(d.offscreen.renders, 0, 'a change the user cannot see must not repaint what they can')
  eq(
    d.offscreen.calls,
    { fs_tree_count: 1, fs_tree_rows: 1 },
    'the off-screen chunk is marked stale rather than re-read on the burst',
  )
  // `a-249` and not `a-250`: the insertion above shifted every row below it down one.
  eq(d.offscreen.staleName, 'a-249', 'its cached row is still readable — it was not evicted')
  eq(
    d.offscreen.duringRevalidate,
    'a-249',
    'and scrolling back to it shows the old row rather than a placeholder while it revalidates',
  )
  eq(d.offscreen.revalidatedName, 'renamed.rs', 'which then becomes the new one')

  // --- the two things the driver cannot see --------------------------------------------------
  //
  // Everything above drives the *store*, and reverting `treeStore.ts` alone fails 11 of those
  // assertions. `FileTree.tsx` is not in that number: there is no DOM here, so the driver
  // models React's bailout rather than observing it, and both of the component's changes could
  // be reverted with every assertion above still green. Read as source instead, which is ugly
  // but is the difference between a claim that can fail and one that cannot.
  const tree = readFileSync('src/sidebar/FileTree.tsx', 'utf8')

  // `gitStatusStore` installs a fresh `statuses` object on every refresh, equal or not, and it
  // refreshes on the same burst the store now answers with no write. A plain identity selector
  // re-renders the whole tree for it and puts the flicker straight back.
  ok(
    /useGitStatus\(\s*useShallow\(/.test(tree),
    'FileTree must select the status map with useShallow, or the burst re-renders anyway',
  )

  // A virtualized list's children are positions. Keying by path renames every key below an
  // insertion, so a one-row change unmounts and remounts the visible window.
  ok(
    !/key=\{`?(?:pending-\$\{item\.index\}|row\.path)`?\}/.test(tree),
    'FileTree rows must not be keyed by row.path or by a pending-<index> string',
  )
  ok(
    (tree.match(/key=\{item\.key\}/g) ?? []).length === 2,
    'both the row and its placeholder must be keyed by the virtualizer key',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-tree-flicker: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-tree-flicker: ok')
