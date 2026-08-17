/**
 * No two files in one directory may have names that differ only in case.
 *
 * # The bug this exists to prevent, which shipped and cost a macOS build
 *
 * `chrome/CloseConfirm.tsx` held the dialog and `chrome/closeConfirm.ts` held its rules — the
 * documented split in this codebase, with the same name in the two casings the convention uses
 * for a component and a module. The same pairing existed for `PasteConfirm` and for `GoToLine`.
 * On Linux that is three unremarkable pairs of files.
 *
 * On macOS it is a build failure, and not an obvious one. APFS is case-**insensitive** by
 * default, and TypeScript resolves a bare specifier by trying extensions in order: `.ts` first,
 * `.tsx` second. So `import { CloseConfirm } from '@/chrome/CloseConfirm'` asks the filesystem
 * for `CloseConfirm.ts`, the filesystem answers with `closeConfirm.ts` because it does not care
 * about the capital C, and the component's import resolves to the *rules* module:
 *
 * ```text
 * error TS2305: Module '"@/chrome/CloseConfirm"' has no exported member 'CloseConfirm'.
 * error TS1149: File name '.../CloseConfirm.ts' differs from already included file name
 *               '.../closeConfirm.ts' only in casing.
 * ```
 *
 * Seven errors across three files, all of them blaming imports that are correct. The first Mac
 * to run `./build.sh` got that far and stopped, and nothing on Linux had ever suggested a
 * problem: here the two names are two files, every import resolves to the one it names, and
 * `tsc --noEmit` is green. The fix was to rename the rules modules — `closeConfirmModel.ts`,
 * `pasteConfirmModel.ts`, `gotoLineModel.ts`, following `branchModel.ts` and `menuModel.ts`,
 * which had the same shape and never collided because they were never a component's name.
 *
 * # Why this is a gate and not a note in CLAUDE.md
 *
 * It is invisible from the platform this project is developed on, in both directions: the
 * collision cannot be observed here, and the fix cannot be verified here either. A rule that
 * only a Mac can enforce is a rule that reaches a Mac as a failed build, twenty minutes into a
 * packaging run. This script is the same rule, on Linux, in about a second.
 *
 * # The two rules, in rising order of how badly they fail
 *
 *  1. **A whole file name that repeats in another casing.** `Foo.ts` beside `foo.ts` cannot be
 *     checked out at all on a case-insensitive filesystem: git writes one path twice and the
 *     working tree comes out with one file, silently missing the other. Any file type — a
 *     stylesheet, an icon, a Rust source — so this rule walks the whole repository.
 *  2. **A module *stem* that repeats in another casing**, e.g. `CloseConfirm.tsx` beside
 *     `closeConfirm.ts`. Both files check out fine; it is resolution that breaks, and only off
 *     Linux. Module extensions only, because a specifier is what carries the ambiguity —
 *     `import './Foo.module.css'` names its extension and resolves exactly.
 *
 * Run: `pnpm --dir ui run check:casing`
 */
import { readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * The repository root, derived from this file's own location rather than from the working
 * directory.
 *
 * Every other script here reads `src/…` and so assumes `ui/` is the cwd, which `pnpm --dir ui
 * run …` and CI both give it; run from elsewhere they fail with ENOENT and nothing is lost.
 * This one *walks a tree*, so the same assumption spelled `resolve('..')` would quietly scan
 * the parent of wherever it was started — somebody's home directory, for a `node
 * ui/scripts/check-casing.mjs` typed at the repository root.
 *
 * Rule 1 is about every file and not only the frontend's: a colliding pair under `crates/` or
 * `docs/` breaks a Mac checkout just as well.
 */
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

/**
 * Directories never walked into.
 *
 * `target/` and `node_modules/` are the expensive ones and neither is ours: a dependency that
 * ships `README.md` beside `readme.md` is a real hazard for someone unpacking it on a Mac, but
 * it is not a hazard this repository can fix, and reporting it would make this gate noise.
 * `.claude/worktrees/` holds complete copies of the tree, which would report every finding
 * once per worktree.
 */
const SKIP = new Set(['.git', 'node_modules', 'target', 'dist', '.claude', '.venv', '.direnv'])

/** What TypeScript will resolve without being told the extension. */
const MODULE_EXT = /\.(?:ts|tsx|js|jsx|mjs|cjs|json)$/

/** `CloseConfirm.tsx` -> `CloseConfirm`; `closeConfirm.module.css` -> `closeConfirm.module`. */
const stem = (name) => name.replace(/\.[^.]+$/, '')

/**
 * Names that collide under a case-insensitive filesystem, as `[key, [names…]]`.
 *
 * `key` is the lowercased form both spellings share, so the message can say what they collapse
 * to. A group of one is not a collision, and neither is a group whose members are all spelled
 * identically — that cannot happen inside one directory, but it can once `stem` has trimmed two
 * different extensions off, which is exactly rule 2's `Foo.ts` + `Foo.tsx` case. That pair is a
 * resolution ambiguity of its own and TypeScript prefers `.ts`, but it is not a *casing* bug and
 * this gate does not claim to be about it.
 */
function collisions(names) {
  const groups = new Map()
  for (const name of names) {
    const key = name.toLowerCase()
    groups.set(key, [...(groups.get(key) ?? []), name])
  }
  return [...groups].filter(([, group]) => new Set(group).size > 1)
}

/** Every directory under `dir`, as `[absolute path, entry names]`. */
function* directories(dir) {
  const entries = readdirSync(dir, { withFileTypes: true })
  yield [dir, entries.filter((e) => e.isFile()).map((e) => e.name)]
  for (const entry of entries) {
    if (entry.isDirectory() && !SKIP.has(entry.name)) {
      yield* directories(join(dir, entry.name))
    }
  }
}

/**
 * The detector, run against a fabricated directory before it is trusted against a real one.
 *
 * Every assertion below is "this repository contains no such pair", so a detector that has
 * quietly stopped detecting — a regex that no longer matches, a grouping that lost its key —
 * reports a clean tree and goes green for ever. The control is the state this repository was
 * actually in, and it must be found.
 */
function selfTest() {
  const broken = ['CloseConfirm.tsx', 'closeConfirm.ts', 'CloseConfirm.module.css']
  const fixed = ['CloseConfirm.tsx', 'closeConfirmModel.ts', 'CloseConfirm.module.css']

  const failures = []
  if (collisions(broken.map(stem)).length !== 1) {
    failures.push('the stem detector no longer flags CloseConfirm.tsx beside closeConfirm.ts')
  }
  if (collisions(fixed.map(stem)).length !== 0) {
    failures.push('the stem detector flags the repaired names, which would make this gate noise')
  }
  if (collisions(['LICENSE', 'license.svg']).length !== 0) {
    failures.push('the name detector treats a different extension as a collision')
  }
  if (collisions(['README.md', 'readme.md']).length !== 1) {
    failures.push('the name detector no longer flags README.md beside readme.md')
  }
  return failures
}

const controlFailures = selfTest()
for (const failure of controlFailures) {
  console.error(`FAIL positive control: ${failure}`)
}

let failed = controlFailures.length
let dirs = 0
let files = 0

for (const [dir, names] of directories(ROOT)) {
  dirs++
  files += names.length
  const where = relative(ROOT, dir) || '.'

  for (const [key, group] of collisions(names)) {
    console.error(
      `FAIL ${where}/ holds ${group.join(' and ')}, which are one path (${key}) on a ` +
        'case-insensitive filesystem: a macOS or Windows checkout gets one of them and ' +
        'silently loses the other.',
    )
    failed++
  }

  const modules = names.filter((name) => MODULE_EXT.test(name))
  for (const [, group] of collisions(modules.map(stem))) {
    // The suggestion names the *camelCase* member, because that is the rules half in this
    // codebase's convention and the half that gets the suffix — renaming the component instead
    // would leave a file whose name no longer matches the component it exports.
    const rules = group.find((name) => /^[a-z]/.test(name)) ?? group[0]
    console.error(
      `FAIL ${where}/ holds modules ${group.join(' and ')}, whose names differ only in case. ` +
        `On macOS an \`import './${group[0]}'\` resolves to whichever of them TypeScript's ` +
        "extension order reaches first — `.ts` before `.tsx` — and the wrong module's exports " +
        `are then reported missing. Rename one: \`${rules}Model\` is what this repository did.`,
    )
    failed++
  }
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log(`casing: ${files} files in ${dirs} directories, no name collides in another casing`)
