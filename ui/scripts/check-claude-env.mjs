/**
 * Checks that the Claude environment switches in Settings reach a child process.
 *
 * > *"scrolling a Claude session"*
 *
 * # The defect this exists for
 *
 * `ClaudeSettings` declared `disableMouse`, `altScreenFullRepaint` and
 * `disableAlternateScreen`, each with a doc comment naming the environment variable it sets.
 * All three were persisted, bound to TypeScript, and drawn as switches in Settings under a
 * panel headed **"Applied at spawn — these reach a pane's child process when it starts"**.
 * Nothing in the workspace read any of them. `resumeAllOnLaunch` was the only field of that
 * struct with a consumer.
 *
 * That is this project's most-repeated defect, and it was not a small instance of it: *"Disable
 * the alternate screen"* is the exact control a user chasing a scrollable transcript reaches
 * for, and pressing it did nothing at all. Every gate in the repository was green throughout.
 *
 * # Why the assertions have this shape
 *
 * A switch dies in one of two places, so closing one door is not enough:
 *
 * * **the rule never runs** — `claude_env` emits the variable and no spawn site folds it. That
 *   is checked by reading `cmd/session.rs` for the call, and by refusing the old hardcoded
 *   `CLAUDE_CODE_SCROLL_SPEED` literal, which would silently outrank the user's number;
 * * **the rule never learns** — a field is added to the struct and drawn in Settings, and the
 *   line that turns it into a variable is never written. That is checked by comparing the two
 *   *sets* of variable names, so a control naming a variable no spawn sets fails, and a
 *   variable no control offers fails too.
 *
 * Field coverage closes the last gap: a field with no control at all is invisible rather than
 * broken, and would slip past a comparison of variable names because it contributes none.
 *
 * The sets are compared rather than merely intersected on purpose. Asserting only "every
 * variable the UI names is emitted" would pass a rule that emits a variable the UI never
 * mentions — an environment cide sets behind the user's back, from a struct whose whole
 * purpose is to be the user's account of it.
 *
 * # Comment stripping is load-bearing here
 *
 * Both files explain these variables *by name* at length — `settings.rs` documents each field
 * with the variable it sets, and `child_env.rs` opens with three paragraphs about the failure.
 * A grep over raw source therefore matches the explanation of a feature that has been deleted,
 * which is precisely how a gate stays green over a dead switch. Every assertion below runs on
 * comment-stripped text, and the mutation transcript for this file includes the case where the
 * code is removed and its paragraph left behind.
 *
 * Run: `pnpm --dir ui run check:claude-env`
 */
import { readFileSync } from 'node:fs'

let failed = 0

const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

/**
 * Remove comments before grepping.
 *
 * One stripper for Rust and TSX because the comment syntaxes that matter here are the same in
 * both — `//` to end of line and `/* … *\/` — and JSX's `{/* … *\/}` is the block form wrapped
 * in braces, so the braces are all that survive and they match nothing.
 *
 * String literals are deliberately kept: the variable names this file is entirely about live
 * inside string literals in both languages, so blanking them would make every assertion below
 * unwritable. Comments are where the hazard is, and comments are what goes.
 *
 * The `[^:]` guard keeps `https://` inside a surviving string literal from eating the rest of
 * its line. Deliberately naive about regex literals; none of the three files contains one.
 */
const strip = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

/**
 * Drop everything from the first `#[cfg(test)]` onwards.
 *
 * Not tidiness — this was a hole, found by mutation. `child_env.rs`'s own tests carry a `VARS`
 * array naming all four variables so that a field added to the struct and forgotten in the rule
 * shows up there. That array is *code*, so comment stripping leaves it, and the scan below
 * happily read it as evidence that the rule emits what the array merely lists. Deleting the
 * alternate-screen line from `claude_env` left this check green while three Rust tests went red.
 *
 * A gate that reads a test fixture is measuring the fixture. The rule is what ships.
 */
const withoutTests = (src) => src.split(/#\[cfg\(test\)\]/)[0]

/**
 * Every `CLAUDE_CODE_*` name mentioned in shipping code, deduplicated and sorted.
 *
 * Tests are cut before comments are, because a `#[cfg(test)]` sitting inside a doc comment would
 * otherwise truncate the file early and silently shrink the set to nothing — which passes the
 * `emitted.length > 0` guard's opposite and would make set equality trivially satisfiable.
 */
const vars = (src) =>
  [...new Set(strip(withoutTests(src)).match(/CLAUDE_CODE_[A-Z0-9_]+/g) ?? [])].sort()

const settingsRs = read('../../crates/cide-ipc/src/settings.rs')
const childEnvRs = read('../../crates/cide-core/src/child_env.rs')
const sessionRs = read('../../crates/cide-app/src/cmd/session.rs')
const sectionsTsx = read('../src/settings/sections.tsx')

// --- 1. the two sets of variable names agree ---------------------------------------------

const emitted = vars(childEnvRs)
const advertised = vars(sectionsTsx)

ok(
  emitted.length > 0,
  '`cide_core::child_env` emits at least one CLAUDE_CODE_* variable — the rule that turns the '
    + 'settings struct into an environment exists at all',
)

eq(
  advertised,
  emitted,
  'every CLAUDE_CODE_* variable the Settings screen names is one a spawn actually sets, and '
    + 'vice versa. A name on the left and not the right is a switch wired to nothing, which is '
    + 'what all three of these were; a name on the right and not the left is an environment '
    + 'cide sets without telling anyone',
)

// --- 2. the rule is reached from the spawn path -------------------------------------------
//
// Set equality above is satisfied by a `claude_env` nobody calls. This is the other door.

const sessionCode = strip(sessionRs)

ok(
  /claude_env\s*\(/.test(sessionCode),
  '`cmd/session.rs` folds `cide_core::child_env::claude_env` into the spec it spawns from — '
    + 'without this call the whole struct is inert again, and every assertion above still passes',
)

ok(
  /fn base_env\s*\([^)]*ClaudeSettings/.test(sessionCode),
  '`base_env` takes the settings, rather than reading them somewhere a spawn cannot see them',
)

// The literal this replaced. Leaving it in place would append a second, later value for the
// same name — and `SpawnSpec::env` is applied in order, so the constant would win and the
// control would move a number no child ever read.
ok(
  !/env\s*\(\s*"CLAUDE_CODE_SCROLL_SPEED"\s*,\s*"\d+"\s*\)/.test(sessionCode),
  '`base_env` no longer hardcodes a CLAUDE_CODE_SCROLL_SPEED literal beside the settings pass; '
    + 'a constant applied after the fold silently outranks the user\'s setting',
)

// --- 3. no field of the struct is invisible -----------------------------------------------

const structBody = strip(settingsRs).match(/pub struct ClaudeSettings\s*\{([\s\S]*?)\n\}/)
ok(structBody != null, '`ClaudeSettings` is still a struct this script can read fields from')

if (structBody) {
  const fields = [...structBody[1].matchAll(/pub\s+([a-z_][a-z0-9_]*)\s*:/g)].map((m) => m[1])
  const camel = (s) => s.replace(/_([a-z])/g, (_, c) => c.toUpperCase())

  ok(
    fields.length >= 5,
    `every field of ClaudeSettings is found by the field scan (found ${fields.length})`,
  )

  const sectionsCode = strip(sectionsTsx)
  const invisible = fields.filter((f) => !sectionsCode.includes(`claude.${camel(f)}`))
  eq(
    invisible,
    [],
    'every field of ClaudeSettings is bound to a control on the Settings screen. A field with '
      + 'no control is not wired to nothing — it is worse, because nobody can even find it to '
      + 'discover that it does nothing',
  )
}

// --- 4. the scroll rate is a control, and its bounds are the CLI's ------------------------
//
// This is the one lever cide owns over how far a Claude pane scrolls: the CLI moves
// `CLAUDE_CODE_SCROLL_SPEED` transcript lines per wheel *report*, and xterm.js sends at most
// one report per wheel event. The bounds are not decoration. The CLI parses the value with
// `parseFloat` and discards anything `NaN` or `<= 0`, falling back to a per-renderer default
// that is 1 for a terminal announcing itself as xterm.js — which cide's XTVERSION reply
// deliberately does. So a 0 escaping the control would not mean "no change", it would mean a
// third of the scrolling the default gives, from a box the user typed a 0 into to slow it down.

const sectionsCode = strip(sectionsTsx)

ok(
  /value=\{claude\.scrollSpeed\}/.test(sectionsCode),
  'the Settings screen offers the scroll rate as a control bound to `claude.scrollSpeed`',
)

const speedField = sectionsCode.match(/value=\{claude\.scrollSpeed\}[\s\S]{0,200}?\/>/)
ok(speedField != null, 'the scroll-rate control is readable as a single element')
if (speedField) {
  ok(
    /min=\{1\}/.test(speedField[0]),
    'the scroll-rate control floors at 1, because 0 is a value the CLI throws away in favour '
      + 'of a default that scrolls less',
  )
  ok(
    /max=\{20\}/.test(speedField[0]),
    'and caps at 20, which is the CLI\'s own clamp',
  )
}

// The Rust side must agree, since it is what actually reaches the child; a control that lets a
// 0 through onto a clamp that fixes it is fine, but a clamp that disagreed with the control
// would make the Settings screen show a number no child ever got.
ok(
  /SCROLL_SPEED\s*:\s*std::ops::RangeInclusive<u8>\s*=\s*1\s*..=\s*20/.test(strip(settingsRs)),
  '`ClaudeSettings::SCROLL_SPEED` is the same 1..=20 range the control offers',
)

ok(
  /clamp\s*\(/.test(strip(childEnvRs)),
  'and `claude_env` clamps into it rather than trusting the control, since a persisted '
    + 'settings file is hand-editable and an out-of-range value there scrolls worse than none',
)

// --- 5. "off" is a removal, never a zero --------------------------------------------------
//
// The CLI gates each of these on `!== undefined` and then reads the value for truthiness, so
// the string "0" means *on*. Spelling "off" as `=0` would disable the mouse from a switch
// sitting at off. `claude_env` returns `None` for an off switch, which `apply_env_changes`
// turns into an `env_remove`; this asserts nobody has since written the tempting literal.
ok(
  !/"CLAUDE_CODE_[A-Z0-9_]+"\s*\.to_string\(\)\s*,\s*Some\(\s*"0"/.test(strip(childEnvRs)),
  'no CLAUDE_CODE_* switch is spelled off with "0" — the CLI reads any defined value as on, '
    + 'so a 0 would turn the feature on from a switch that is off',
)

if (failed > 0) {
  console.error(`\ncheck-claude-env: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-claude-env: ok')
