/**
 * Checks `src/sidebar/treeStatus.ts` — the file tree's half of the git-status join.
 *
 * Same shape as `check-git-tree.mjs`, and for the same reason: this project has no JS test
 * runner, and adding one for two pure functions would be a larger commitment than the code it
 * tests. The Rust half is covered by `crates/cide-git/tests/tree_status.rs`, which drives real
 * temporary repositories. What is left over — and what is pinned here — is the behaviour that
 * exists only because the wire format is deliberately sparse:
 *
 *   * a row inside an untracked or ignored directory inherits it, because the backend
 *     deliberately does not enumerate what is inside one (that is the whole reason a fresh
 *     100k-file checkout does not put 100k entries on the wire),
 *   * a `modified` rollup mark on a folder is NOT inherited downwards — the bug that would
 *     paint every sibling of one edited file blue,
 *   * the resolver terminates on a malformed key rather than spinning inside a render,
 *   * directories take the colour and no letter.
 *
 * Run: `pnpm --dir ui run check:tree-status`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tree-status-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/treeStatus.ts',
      '--outDir', out,
      // Pinned so the emitted layout is `<out>/sidebar/treeStatus.js` whether or not the
      // type-only import of `../ipc/generated` widens the inferred common root.
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The app's own tsconfig sets this, and it is what makes a map miss a `undefined` the
      // resolver has to handle rather than a `TreeStatus` it may assume.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'sidebar', 'treeStatus.js')}`)

  let failed = 0
  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // The map a backend would produce for: `src/app.rs` edited, `src/new/` a whole new
  // untracked directory, `vendor/` ignored, `docs/gone.md` deleted from the working tree.
  // Note what the backend does NOT send: anything inside `src/new` or `vendor`.
  const R = '/w/app'
  const map = {
    [R]: 'modified',
    [`${R}/src`]: 'modified',
    [`${R}/src/app.rs`]: 'modified',
    [`${R}/src/new`]: 'untracked',
    [`${R}/docs`]: 'modified',
    [`${R}/vendor`]: 'ignored',
  }

  // --- exact hits ------------------------------------------------------------------------

  eq(m.statusAt(map, `${R}/src/app.rs`), 'modified', 'an exact hit is the status')
  eq(m.statusAt(map, `${R}/src`), 'modified', 'a rolled-up folder shows as modified')
  eq(m.statusAt(map, R), 'modified', 'so does the root row of a multi-root project')

  // --- the rollup mark is not inherited --------------------------------------------------

  eq(
    m.statusAt(map, `${R}/src/untouched.rs`),
    'clean',
    'a sibling of an edited file is clean — inheriting a `modified` rollup downwards would '
      + 'paint every file in the project blue, since the rollup reaches the root',
  )
  eq(
    m.statusAt(map, `${R}/docs/index.md`),
    'clean',
    'and so is a file beside a deleted one, even though its folder carries the deletion',
  )
  eq(m.statusAt(map, `${R}/README.md`), 'clean', 'a file directly under a marked root too')

  // --- container statuses ARE inherited --------------------------------------------------

  eq(
    m.statusAt(map, `${R}/src/new/mod.rs`),
    'untracked',
    'a file inside an untracked directory is untracked — the backend never enumerated it, so '
      + 'this is the only place that answer can come from',
  )
  eq(
    m.statusAt(map, `${R}/src/new/deep/a/b/c.rs`),
    'untracked',
    'however deep, and without an entry at any level in between',
  )
  eq(
    m.statusAt(map, `${R}/vendor/zlib/z.c`),
    'ignored',
    'the same rule carries `ignored`, which is reachable because the fs walk does not read '
      + '.gitignore files above a project root',
  )
  eq(
    m.statusAt(map, `${R}/src/new`),
    'untracked',
    'and the directory itself is still an exact hit, not an inheritance',
  )

  // --- outside the project ---------------------------------------------------------------

  eq(m.statusAt(map, '/w/other/x.rs'), 'clean', 'a path under no known ancestor is clean')
  eq(m.statusAt({}, `${R}/src/app.rs`), 'clean', 'an empty map — the first paint — is clean')
  eq(
    m.statusAt(m.NO_STATUS.statuses, `${R}/src/app.rs`),
    'clean',
    'and so is the placeholder the store holds before git has answered',
  )

  // --- termination -----------------------------------------------------------------------

  eq(m.statusAt(map, '/'), 'clean', 'the filesystem root terminates rather than looping')
  eq(m.statusAt(map, ''), 'clean', 'an empty key too')
  eq(m.statusAt(map, 'relative/path.rs'), 'clean', 'a key with no leading separator too')
  eq(
    m.statusAt(map, `${R}/${'a/'.repeat(200)}x.rs`),
    'clean',
    'a path deeper than the ancestor bound gives up and answers clean, rather than spinning '
      + 'inside a render',
  )
  eq(
    m.statusAt({ [`${R}/src`]: 'exploded' }, `${R}/src/x.rs`),
    'clean',
    'a status this build has never heard of is not treated as a container: an older webview '
      + 'against a newer backend under-tags rather than inheriting something it cannot draw',
  )

  // --- letters ---------------------------------------------------------------------------

  eq(m.letterFor('modified', false), 'M', 'M for a modified file')
  eq(m.letterFor('added', false), 'A', 'A for an added file')
  eq(m.letterFor('deleted', false), 'D', 'D for a deleted file')
  eq(m.letterFor('clean', false), '', 'a clean row draws nothing in the tag column')
  eq(
    m.letterFor('untracked', false),
    '',
    'untracked has colour and no letter — the mock draws no glyph for it and inventing a `?` '
      + 'would be a design decision made in a lookup table',
  )
  eq(m.letterFor('ignored', false), '', 'ignored likewise')
  eq(
    m.letterFor('modified', true),
    '',
    "a directory takes the colour and no letter, which is IDEA's treatment and keeps the tag "
      + 'column meaning "this file" rather than "something under here"',
  )
  eq(m.letterFor('untracked', true), '', 'every directory, whatever its status')
  eq(m.letterFor('exploded', false), '', 'an unknown status draws no letter rather than throwing')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('file tree status: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
