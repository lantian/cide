/**
 * Checks `src/settings/cliHandshake.ts` — the Settings screen's account of the last `claude`
 * to complete the IDE handshake on this machine — and the wire shape it reads.
 *
 * # Why this exists
 *
 * The handshake record is one small object, and the sentence the screen builds from it has
 * five states in it. Two of those states — `staleBuild` and `outsideRange` — look identical
 * from a distance and say opposite things: one means "this observation is not about the build
 * you are running", the other means "it is about this build, and the version is not one anyone
 * checked". Collapsing them shows a user a reassuring date about a cide they upgraded away
 * from. That is precisely the class of rule that gets written inline in JSX and then cannot be
 * checked by anything, and this repository has already shipped one bug that way.
 *
 * So `cliHandshake.ts` is import-free, this script compiles it standalone with the TypeScript
 * in `node_modules`, and every state below is driven rather than read.
 *
 * Three kinds of proof:
 *
 *   - The states. Each of the five, from a constructed record, asserted on the `state` tag
 *     *and* on something the sentence has to contain. A tag alone would pass on five
 *     identical strings.
 *   - The wire shape. The field names come out of `crates/cide-core/src/handshake.rs`'s serde
 *     attributes, so a rename in Rust that never reached `client.ts` fails here instead of
 *     arriving in the webview as `undefined` and being drawn as "never" — which is a
 *     *plausible* answer, and therefore the worst kind of wrong.
 *   - The extraction. `sections.tsx` must still call into this module rather than having
 *     grown its own copy, which is the failure mode that would leave this file passing while
 *     checking nothing that ships.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a handshake was ever recorded. That is `cide_core::handshake`'s tests, and
 *     ultimately `cargo xtask verify-cli` against a real CLI.
 *   - that `SUPPORTED_CLI` is correct. Nothing in TypeScript can know that; the verdict
 *     arrives already computed from Rust, deliberately.
 *
 * Run: `pnpm --dir ui run check:handshake`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-handshake-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (condition, what, detail = '') => {
  if (!condition) {
    console.error(`FAIL ${what}${detail === '' ? '' : `\n  ${detail}`}`)
    failed++
  }
}

const repoFile = (rel) =>
  readFileSync(fileURLToPath(new URL(`../../${rel}`, import.meta.url)), 'utf8')
const uiFile = (rel) => readFileSync(fileURLToPath(new URL(`../${rel}`, import.meta.url)), 'utf8')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/settings/cliHandshake.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const { handshakeNote, relativeTime, sentence, badge } = await import(
    `file://${join(out, 'cliHandshake.js')}`
  )

  const RANGE = '2.1.224–2.1.231'
  const NOW = 1_700_000_000_000
  const HOUR = 3_600_000

  const support = (over = {}) => ({
    version: '2.1.231',
    verifiedRange: RANGE,
    warning: null,
    handshake: null,
    ...over,
  })
  const record = (over = {}) => ({
    version: '2.1.231',
    atUnixMs: NOW - 3 * HOUR,
    verifiedRange: RANGE,
    ...over,
  })

  // --- the five states -----------------------------------------------------------------

  // Nothing has ever connected. Not an error — the user may not have opened a Claude pane —
  // so the sentence says what to do rather than what went wrong.
  {
    const note = handshakeNote(support(), NOW)
    eq(note.state, 'never', 'no record reads as `never`')
    ok(/Claude pane/.test(sentence(note)), 'the never case says what to do', sentence(note))
    eq(badge(support()), 'never', 'and the control column is never blank')
  }

  // The good case, and the only one with no second sentence: a screen that explains itself on
  // every launch is one nobody reads on the launch it matters.
  {
    const s = support({ handshake: record() })
    const note = handshakeNote(s, NOW)
    eq(note.state, 'ok', 'a recent in-range handshake reads as ok')
    eq(note.detail, null, 'the ok case says nothing further')
    ok(/2\.1\.231/.test(note.headline), 'and it names the version', note.headline)
    ok(/3 hours ago/.test(note.headline), 'and when', note.headline)
    eq(badge(s), '2.1.231', 'the control column carries the version')
  }

  // The state that only exists because the record stores the range it was measured against.
  // Without that field this is indistinguishable from `ok`, and an upgraded user is shown a
  // reassuring date about a build they are no longer running.
  {
    const s = support({ handshake: record({ verifiedRange: '2.1.224–2.1.227' }) })
    const note = handshakeNote(s, NOW)
    eq(note.state, 'staleBuild', 'a record from another build is not evidence about this one')
    ok(
      /2\.1\.224–2\.1\.227/.test(sentence(note)) && new RegExp(RANGE).test(sentence(note)),
      'and the sentence names both ranges, or it is unactionable',
      sentence(note),
    )
  }

  // It connected — so the integration works here — and it is not a version anybody checked.
  // Both halves have to be in the sentence; either alone is misleading.
  {
    const s = support({ warning: 'claude 2.1.240 is newer than the range', handshake: record({ version: '2.1.240' }), version: '2.1.240' })
    const note = handshakeNote(s, NOW)
    eq(note.state, 'outsideRange', 'a handshake by an unchecked version says so')
    ok(/working|connected/i.test(sentence(note)), 'and admits that it worked', sentence(note))
    ok(new RegExp(RANGE).test(sentence(note)), 'and names the range', sentence(note))
  }

  // The CLI self-updates. The version that last proved the integration works is not
  // necessarily the one that would run next, and saying so is the honest line.
  {
    const s = support({ version: '2.1.240', handshake: record({ version: '2.1.231' }) })
    const note = handshakeNote(s, NOW)
    eq(note.state, 'outsideRange', 'a CLI that moved on since the record is worth flagging')
    ok(
      /2\.1\.231/.test(note.headline) && /2\.1\.240/.test(note.headline),
      'and both versions are named, or the reader cannot tell which is which',
      note.headline,
    )
  }

  // "Something connected and would not say what it was" has nothing in common with "nothing
  // has ever connected", however similar an empty version looks to an empty record.
  {
    const s = support({ handshake: record({ version: null }) })
    const note = handshakeNote(s, NOW)
    eq(note.state, 'unnamed', 'a nameless connection is its own state')
    ok(/works here/.test(sentence(note)), 'and it worked, which is the point', sentence(note))
    eq(badge(s), 'unnamed', 'and the control column does not read `never`')
  }

  // --- the ordering inside `handshakeNote` --------------------------------------------------
  //
  // A stale-build record is answered as stale even when the version is also out of range: the
  // observation is not about this build, so comparing it against this build's range would be
  // comparing it against a range it was never measured against.
  {
    const s = support({
      warning: 'claude 2.1.240 is newer than the range',
      version: '2.1.240',
      handshake: record({ version: '2.1.240', verifiedRange: 'an older build' }),
    })
    eq(
      handshakeNote(s, NOW).state,
      'staleBuild',
      'the build check comes first: a record from another cide is not evidence about this one',
    )
  }

  // --- relative time --------------------------------------------------------------------

  eq(relativeTime(NOW, NOW), 'just now', 'this instant')
  eq(relativeTime(NOW - 44_000, NOW), 'just now', 'under a minute')
  eq(relativeTime(NOW - 60_000, NOW), '1 minute ago', 'singular')
  eq(relativeTime(NOW - 120_000, NOW), '2 minutes ago', 'plural')
  eq(relativeTime(NOW - HOUR, NOW), '1 hour ago', 'an hour')
  eq(relativeTime(NOW - 25 * HOUR, NOW), '1 day ago', 'past a day, the unit changes')
  eq(relativeTime(NOW - 72 * HOUR, NOW), '3 days ago', 'and stops at days')
  // A clock that moved, or one machine's state directory opened on another. Rendering a
  // negative interval as "-3 hours ago" would read as a bug in the app rather than in the
  // clock, which is where the problem actually is.
  ok(
    !relativeTime(NOW + HOUR, NOW).includes('-'),
    'a record from the future does not render a negative interval',
    relativeTime(NOW + HOUR, NOW),
  )

  // --- the wire shape, read out of the Rust ------------------------------------------------
  //
  // `ClaudeCliSupport` is a plain `serde::Serialize` local to `cmd/settings.rs`, not a ts-rs
  // DTO, so nothing generates the TypeScript for it and there is no codegen check standing
  // behind these names. A rename in Rust arrives in the webview as `undefined`, which
  // `badge` draws as `never` — a perfectly plausible answer, and therefore the worst kind of
  // wrong. These are the keys serde actually emits.

  const rust = repoFile('crates/cide-core/src/handshake.rs')
  ok(
    /#\[serde\(rename_all = "camelCase"\)\]\s*\npub struct Handshake/.test(rust),
    'Handshake is camelCase on the wire, which is what the TypeScript below assumes',
  )
  const fields = [...rust.matchAll(/^\s{4}pub (\w+): /gm)].map((m) => m[1])
  eq(
    fields,
    ['version', 'at_unix_ms', 'verified_range'],
    'the record has exactly the three fields this module reads',
  )

  const client = uiFile('src/ipc/client.ts')
  for (const key of ['atUnixMs', 'verifiedRange', 'handshake']) {
    ok(
      new RegExp(`\\b${key}\\b`).test(client),
      `client.ts mirrors \`${key}\``,
      'ClaudeCliSupport is hand-mirrored — no codegen stands behind it',
    )
  }

  /*
   * The command answers this shape even when it fails, and the degraded value has to carry
   * **every** field or the consumers of the missing ones see `undefined` on a machine where
   * the command is not registered — which for `handshake` is drawn as `never`, a perfectly
   * plausible answer and therefore the worst kind of wrong.
   *
   * This used to be a regex over one line of source: `/verifiedRange: 'unknown', warning:
   * null, handshake: null/`. It pinned the *formatting* rather than the property — M16 added
   * three fields, Prettier wrapped the object across lines, and the assertion failed while
   * the value it was checking had grown strictly more correct. Worse, it would have gone
   * green on a fallback that dropped `version`, which is a field it was never watching.
   *
   * So: read the interface's own field list, extract the fallback object, and require each
   * name to appear in it. Neither side is restated here, which is what makes the assertion
   * survive the next field.
   */
  {
    const declared = client.match(/export interface ClaudeCliSupport \{([\s\S]*?)\n\}/)
    ok(declared != null, 'ClaudeCliSupport is still an interface this script can read')
    // Strip comments first: every field of this interface carries a doc block, and several of
    // them name *other* fields in prose. A scan over raw source would read those as
    // declarations and then find them trivially present in the fallback.
    const body = (declared?.[1] ?? '')
      .replace(/\/\*[\s\S]*?\*\//g, ' ')
      .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')
    const declaredFields = [...body.matchAll(/^\s{2}(\w+)[?]?:/gm)].map((m) => m[1])
    ok(
      declaredFields.length >= 4,
      `read ${declaredFields.length} fields off ClaudeCliSupport — the scan still matches`,
    )

    const fallback = client.match(/pendingCommand<ClaudeCliSupport>\([\s\S]*?\n {6}\{([\s\S]*?)\n {6}\},/)
    ok(fallback != null, "the degraded value passed to `pendingCommand` is still readable")
    const degraded = fallback?.[1] ?? ''
    eq(
      declaredFields.filter((name) => !new RegExp(`\\b${name}:`).test(degraded)),
      [],
      'the degraded cliSupport value carries every field the interface declares — a field '
        + 'missing from it reaches the screen as `undefined` on any machine where the command '
        + 'is not registered, which is exactly the build this fallback exists for',
    )
  }

  // --- the screen still uses this module ---------------------------------------------------

  const sections = uiFile('src/settings/sections.tsx')
  ok(/from '\.\/cliHandshake'/.test(sections), 'sections.tsx imports the checked module')
  ok(/handshakeNote\(/.test(sections), 'and calls it rather than re-deciding')
  ok(
    !/state === '(never|ok|staleBuild|outsideRange|unnamed)'/.test(sections),
    'sections.tsx does not branch on a handshake state itself; that is `handshakeNote`’s job',
  )
  ok(
    !/hours ago|days ago|just now/.test(sections),
    'nor does it format a time — that rule is in the module too',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('handshake: ok (5 states, 3 wire fields, mirrored against cide-core/src/handshake.rs)')
} finally {
  rmSync(out, { recursive: true, force: true })
}
