/**
 * Checks code completion's two import-free modules, and the call sites that cannot be compiled.
 *
 * Same shape as `check-outline.mjs` and `check-problems.mjs`: no JS test runner in this project,
 * so the TypeScript already in `node_modules` compiles `completionGate.ts` and `lspSnippet.ts`
 * standalone and this file drives them.
 *
 * # What is worth pinning here, and why each one is invisible without a test
 *
 * Every rule in this feature fails *quietly*. There is no exception, no red mark and no missing
 * pixel — the popup keeps appearing and simply becomes wrong:
 *
 *  1. **`label` is the filter text and `displayLabel` is the label.** CodeMirror matches typing
 *     against `Completion.label`; LSP's `label` is a display string (`push(…)`, ellipsis and all)
 *     and its `filterText` is the match target. Getting this backwards scores `push(…)` against
 *     `pus`, and the symptom is "the popup stops narrowing when I type" with every row still
 *     visibly present.
 *  2. **`validFor` is refused when the list is `incomplete` or `truncated`.** Set wrongly, the
 *     popup goes on filtering a list that no longer contains what the user is typing towards. It
 *     keeps working; it just stops offering the answer.
 *  3. **The snippet translator degrades rather than mangling.** CodeMirror reads `#{` as a field
 *     opener as well as `${` and has no `\$` escape, and its placeholder default is `[^{}]*` so a
 *     nested LSP placeholder has no representation. Both cases must fall back to plain text —
 *     the alternative is `${1` appearing as source in somebody's file.
 *  4. **`shouldAsk` fires after a trigger character.** Without that branch the popup never opens
 *     on the gesture people actually make, because immediately after a `.` there is no prefix.
 *  5. **The call sites.** `defaultKeymap: false` (or CodeMirror's own `Escape` outranks the find
 *     bar's), `icons: false` (or it draws Unicode glyphs and `check:ui-icons` is a lie), **both**
 *     Tab and Enter bound to `acceptCompletion`, the extension mounted before the main keymap so
 *     Tab beats `indentWithTab`, and `syncNow` awaited before the request. Every one of those is a
 *     *missing or reordered call*, which a test of the surrounding code passes over.
 *
 * Run: `pnpm --dir ui run check:completion`   (or `node ui/scripts/check-completion.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-completion-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  if (actual !== expected) {
    fail(what, `actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
  }
}
const ok = (cond, what, detail) => {
  if (!cond) fail(what, detail)
}
/** Strip comments, so a source assertion cannot be satisfied by a sentence about the code. */
const strip = (text) =>
  text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, (_m, lead) => lead)

let checks = 0
const counted =
  (fn) =>
  (...args) => {
    checks++
    return fn(...args)
  }
const EQ = counted(eq)
const OK = counted(ok)

try {
  /*
   * Both modules import nothing, so a bare `tsc` with no tsconfig is enough. If either ever needs
   * one, something has added an import and the standalone compile — the whole reason they are
   * separate files — is gone. The last section asserts that directly.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/completionGate.ts',
      'src/editor/lspSnippet.ts',
      '--outDir',
      out,
      '--module',
      'esnext',
      '--target',
      'es2022',
      '--moduleResolution',
      'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const gate = await import(`file://${join(out, 'completionGate.js')}`)
  const snip = await import(`file://${join(out, 'lspSnippet.js')}`)

  // --- 1. should we ask at all -------------------------------------------------------------

  for (const [before, want, why] of [
    ['let x = foo', true, 'an identifier in progress is the ordinary case'],
    ['v.', true, 'a trigger character — the gesture people actually make'],
    ['use std::', true, 'the second colon of `::`, which is `:` either way'],
    ['obj->', true, '`->`, for a server that declares it'],
    ['"src/', true, 'a path inside a string, for a path-completing server'],
    ['    ', false, 'whitespace is not worth a whole-crate candidate search'],
    ['}', false, 'a closing brace completes nothing'],
    ['x;', false, 'a statement terminator completes nothing'],
    ['', false, 'the start of a line, with nothing typed'],
  ]) {
    EQ(gate.shouldAsk(before, false), want, `shouldAsk(${JSON.stringify(before)}) — ${why}`)
  }

  // Explicit overrides every one of them, including the empty line, which is where Ctrl+Space is
  // most useful and where every prefix test says no.
  for (const before of ['', '    ', '}', 'foo']) {
    EQ(gate.shouldAsk(before, true), true, `Ctrl+Space asks regardless of ${JSON.stringify(before)}`)
  }

  // --- 2. where the replaced word starts ----------------------------------------------------

  for (const [before, want, why] of [
    ['let x = foo', 8, 'back over the identifier only'],
    ['v.', 2, 'after a dot the range is empty — a `from` that ate the dot would score every row a miss'],
    ['foo', 0, 'a word at the start of the line'],
    ['', 0, 'nothing typed'],
    ['a.b_c$d', 2, 'underscores and `$` are identifier characters'],
    ['let ünïcode', 4, 'non-ASCII identifiers are identifiers'],
  ]) {
    EQ(gate.wordStart(before), want, `wordStart(${JSON.stringify(before)}) — ${why}`)
  }

  // --- 3. mapping a row to a CodeMirror completion ------------------------------------------

  const row = (over = {}) => ({
    label: 'push(…)',
    filterText: 'push',
    detail: null,
    description: null,
    kind: 'method',
    insert: 'push',
    snippet: false,
    sort: 0,
    resolve: null,
    deprecated: false,
    ...over,
  })

  {
    const mapped = gate.mapCompletion(row())
    // The single most consequential line in the feature — see the header.
    EQ(mapped.label, 'push', 'the matcher gets `filterText`, not the display label')
    EQ(mapped.displayLabel, 'push(…)', 'the popup draws the display label')
    EQ(mapped.type, 'method', 'the kind reaches CodeMirror as the type, for the badge class')
    OK(
      !('detail' in mapped),
      'a null detail is an absent key, not `undefined` — `exactOptionalPropertyTypes` is on and ' +
        "CodeMirror's `detail` is optional",
    )
  }
  {
    const mapped = gate.mapCompletion(row({ detail: '(use std::collections::HashMap)' }))
    EQ(
      mapped.detail,
      '(use std::collections::HashMap)',
      'the import hint is the popup’s only warning that accepting also edits the top of the file',
    )
  }

  // The boost carries the server's ranking without being able to override the fuzzy matcher.
  {
    const first = gate.mapCompletion(row({ sort: 0 })).boost
    const later = gate.mapCompletion(row({ sort: 20 })).boost
    const far = gate.mapCompletion(row({ sort: 500 })).boost
    OK(first > later, 'a better-ranked row gets a larger boost')
    OK(later > 0, 'a row inside the boosted depth still gets one')
    EQ(far, 0, 'past the boosted depth the fuzzy score is left alone entirely')
    OK(
      first < 99,
      'the boost stays well inside CodeMirror’s ±99, or six typed characters could not outrank ' +
        'the server’s first suggestion',
    )
    EQ(
      gate.mapCompletion(row({ deprecated: true, sort: 0 })).boost,
      -99,
      'a deprecated row sinks rather than being hidden — hiding it would be an editor deciding a ' +
        'user may not call something they can see in their own dependency',
    )
  }

  // --- 4. may the list be filtered locally --------------------------------------------------

  EQ(gate.canFilterLocally(false, false), true, 'a complete, untruncated list may be filtered')
  EQ(
    gate.canFilterLocally(true, false),
    false,
    '`isIncomplete` means the server will answer differently for a longer prefix',
  )
  EQ(
    gate.canFilterLocally(false, true),
    false,
    'a truncated list may be missing exactly the row the user is typing towards',
  )
  EQ(gate.canFilterLocally(true, true), false, 'both at once')

  // --- 5. which rows need a resolve ---------------------------------------------------------

  EQ(gate.needsResolve(row()), false, 'an ordinary row accepts with no request')
  EQ(
    gate.needsResolve(row({ resolve: 3 })),
    true,
    'an auto-import row must be resolved first — for rust-analyzer the edit does not exist yet',
  )

  // --- 6. the snippet translator -------------------------------------------------------------

  const plan = (text, isSnippet = true) => snip.toCodeMirrorSnippet(text, isSnippet)

  EQ(plan('push', false).kind, 'text', 'a plain item is never scanned for snippet syntax')
  EQ(
    plan('cost: $5 or ${money}', false).template,
    'cost: $5 or ${money}',
    'a plain item is passed through untouched — a `$` in it is a `$`',
  )

  for (const [lsp, want, why] of [
    ['push(${1:value})', 'push(${1:value})', 'the common form is already CodeMirror’s'],
    ['push($1)', 'push(${1})', 'a bare `$1` is braced'],
    ['f($1, $2)$0', 'f(${1}, ${2})${0}', 'several stops, and the final one'],
    ['${1|a,b,c|}', '${1:a}', 'a choice becomes its first option'],
    ['a\\$b', 'a$b', 'an escaped dollar is a literal dollar'],
    // LSP's `\}` is a literal brace; CodeMirror spells a literal brace `\}` too, so the
    // round trip is unescape-then-re-escape and the output legitimately looks like the input.
    ['a\\}b', 'a\\}b', 'an escaped brace survives as an escaped brace'],
    ['$TM_FILENAME', '', 'an unresolvable variable is dropped rather than inserted as text'],
    ['${TM_SELECTED_TEXT:fallback}', 'fallback', 'a variable’s default is the useful half'],
    ['cost 5$', 'cost 5$', 'a lone dollar is literal and common in shell'],
  ]) {
    const got = plan(lsp)
    EQ(got.template, want, `snippet ${JSON.stringify(lsp)} — ${why}`)
  }

  // A brace in a literal is escaped, so CodeMirror's own parser cannot mistake it for a field.
  {
    const got = plan('fn() {$0}')
    EQ(got.kind, 'snippet', 'a brace in literal text does not defeat the translation')
    OK(
      got.template.includes('\\{') && got.template.includes('\\}'),
      'literal braces are escaped for CodeMirror',
      got.template,
    )
  }

  // The two refusals — the whole reason this module is a module.
  {
    const got = plan('echo ${HOME}/x', true)
    // `${HOME}` parses as a variable and is dropped, so this is *not* the refusal case; it is
    // here to prove the two are told apart rather than both landing in the fallback.
    EQ(got.kind, 'snippet', 'a brace-wrapped variable is a variable, not an unrepresentable field')
  }
  {
    const got = plan('${1:${2:inner}}')
    EQ(got.kind, 'text', 'a nested placeholder cannot be represented and must degrade')
    EQ(got.refused, 'nested-placeholder', 'and the refusal says which rule it hit')
    OK(
      !got.template.includes('${'),
      'the fallback inserts the *text*, never the raw template — inserting the template is the ' +
        'exact failure the refusal exists to avoid, reached by another road',
      got.template,
    )
    EQ(got.template, 'inner', 'the fallback keeps the innermost default as literal text')
  }
  {
    // A literal `#{` survives translation and would be read by CodeMirror as a field opener.
    const got = plan('interpolate \\#{x} then $1', true)
    OK(
      got.kind === 'text' || !/[#$]\{/.test(stripFields(got.template)),
      'a literal field-opener in the output must either be refused or not be there',
      `${got.kind}: ${got.template}`,
    )
  }

  // Nothing this module emits may contain a field opener outside a field it meant. Fuzzed over
  // the shapes the servers cide ships actually produce, because the failure is a template that
  // *parses* — just into something other than what was written.
  for (const template of [
    'a${1}b',
    '${1:x}${2:y}',
    'foo(${1:a}, ${2:b})$0',
    'if $1 {\n\t$0\n}',
    'w\\{x\\}y${1:z}',
    '$1$2$3',
    '${0}',
  ]) {
    const got = plan(template)
    if (got.kind !== 'snippet') continue
    OK(
      !/[#$]\{/.test(stripFields(got.template)),
      `no stray field opener survives ${JSON.stringify(template)}`,
      got.template,
    )
  }

  // --- 7. the wire type has not drifted from the restatement --------------------------------

  {
    const generated = readFileSync(join(UI, 'src/ipc/generated.ts'), 'utf8')
    const block = /export type CompletionItem = \{([\s\S]*?)\};/.exec(generated)
    OK(block !== null, 'CompletionItem is in generated.ts')
    if (block !== null) {
      // ts-rs emits several fields on one line when their doc comments are short, so the field
      // names cannot be found by anchoring to the start of a line. Comments are stripped first
      // instead, and then every `name:` in what is left is a field.
      const body = strip(block[1])
      const wire = new Set([...body.matchAll(/([a-zA-Z][a-zA-Z0-9]*)\s*:/g)].map((m) => m[1]))
      const restated = readFileSync(join(UI, 'src/editor/completionGate.ts'), 'utf8')
      const local = /export interface CompletionRow \{([\s\S]*?)\n\}/.exec(restated)
      OK(local !== null, 'CompletionRow is in completionGate.ts')
      if (local !== null) {
        const mine = [...local[1].matchAll(/readonly ([a-zA-Z][a-zA-Z0-9]*)\s*:/g)].map((m) => m[1])
        for (const field of mine) {
          OK(
            wire.has(field),
            `CompletionRow.${field} still exists on the wire type — the restatement is the price ` +
              'of this module importing nothing, and this is what stops it drifting silently',
          )
        }
      }
    }
  }

  // --- 8. the call sites, which is where this class of bug actually lives -------------------

  const completion = strip(readFileSync(join(UI, 'src/editor/completion.ts'), 'utf8'))
  const surface = strip(readFileSync(join(UI, 'src/editor/EditorSurface.tsx'), 'utf8'))

  OK(
    /defaultKeymap:\s*false/.test(completion),
    'autocompletion() sets `defaultKeymap: false` — CodeMirror’s own completionKeymap binds ' +
      'Enter to acceptCompletion at Prec.highest, and Enter-accepts was decided against',
  )
  OK(
    /icons:\s*false/.test(completion),
    'autocompletion() sets `icons: false` — CodeMirror’s base theme draws its completion icons ' +
      'as Unicode glyphs in ::after content, which check:ui-icons bans outside an allowlist',
  )
  OK(
    /\{\s*key:\s*'Tab',\s*run:\s*acceptCompletion\s*\}/.test(completion),
    'Tab is bound to acceptCompletion — the accept key this feature was asked for',
  )
  OK(
    /\{\s*key:\s*'Enter',\s*run:\s*acceptCompletion\s*\}/.test(completion),
    'Enter accepts too — Tab-only shipped first and was reported as "Enter does nothing". The ' +
      'fallthrough is what makes it safe: acceptCompletion returns false with no popup open, so ' +
      'Enter reaches insertNewlineAndIndent in every buffer where nothing is being suggested',
  )
  OK(
    /key:\s*'Ctrl-Space',\s*run:\s*startCompletion/.test(completion),
    'Ctrl+Space opens the popup on demand',
  )
  OK(
    !/Prec\.(highest|high)/.test(completion),
    'no Prec override — the extension’s position in EditorSurface’s array is what orders it, and ' +
      'raising it above findExtensions() would make Escape close a popup instead of an open find bar',
  )
  OK(
    /await syncNow\(/.test(completion),
    'the document is synced before the question is asked — docSync is a 300ms trailing throttle, ' +
      'and completing against text that does not contain the caret is meaningless',
  )
  {
    const syncAt = completion.indexOf('await syncNow(')
    const askAt = completion.indexOf('diagnosticsApi.completion(')
    OK(
      syncAt >= 0 && askAt >= 0 && syncAt < askAt,
      'syncNow is awaited *before* the completion request, not after',
    )
  }
  EQ(
    (completion.match(/diagnosticsApi\.completion\(/g) ?? []).length,
    1,
    'exactly one caller of the completion command in the app, so the sync-first rule has one home',
  )
  OK(
    /if \(answer\.kind === 'unavailable'\)[\s\S]{0,200}if \(explicit\) report\(/.test(completion),
    'an implicit failure is silent and only an explicit Ctrl+Space reports — this popup opens by ' +
      'itself many times a minute, and a notice per failure is a notification storm',
  )
  OK(
    /report\(resolveFailureSentence\(/.test(completion),
    'a failed resolve refuses the accept with a sentence — inserting a symbol without its import ' +
      'leaves the file not compiling, which is worse than not completing',
  )

  // The ordering that makes Tab work. Both halves are computed from the file, not asserted from
  // a comment, because a comment claiming an ordering is exactly what has been wrong before.
  {
    const find = surface.indexOf('findExtensions()')
    const slot = surface.indexOf('completionSlot.of(')
    const keys = surface.indexOf('keymap.of([')
    // `lastIndexOf`, because the *import* of `indentWithTab` is at the top of the file and
    // `indexOf` would compare every position against line 25 and always succeed.
    const tab = surface.lastIndexOf('indentWithTab')
    OK(find >= 0 && slot >= 0 && keys >= 0 && tab >= 0, 'all four anchors are present')
    OK(
      find < slot,
      'the completion slot comes *after* findExtensions(), so Escape closes an open find bar ' +
        'before it closes the popup',
    )
    OK(
      slot < keys && keys < tab,
      'the completion slot comes *before* the main keymap, so Tab reaches acceptCompletion ' +
        'before indentWithTab indents',
    )
  }
  OK(
    /completionSlot = new Compartment\(\)/.test(surface),
    'completion lives in a Compartment — the build effect is keyed [path, reloadKey] and adding ' +
      'a setting to that array rebuilds the view, costing undo history and unsaved edits',
  )
  OK(
    !/\[path, reloadKey, completion/.test(surface),
    'the build effect’s dependency array did not grow',
  )
  OK(
    /readOnly \|\| oversize\) return \[\]/.test(surface),
    'no completion in a read-only or oversize buffer — above the sync cap the server’s copy is ' +
      'permanently stale, so every answer would be about a file that no longer exists',
  )

  // The badge vocabulary is shared with the pickers, and the stylesheet has to agree with it.
  {
    const format = readFileSync(join(UI, 'src/overlays/format.ts'), 'utf8')
    const badges = /export function completionBadge[\s\S]*?\n\}/.exec(format)
    OK(badges !== null, 'completionBadge exists beside symbolBadge')
    const css = readFileSync(join(UI, 'src/editor/EditorSurface.module.css'), 'utf8')
    OK(
      /completionBadge\(/.test(strip(readFileSync(join(UI, 'src/editor/completion.ts'), 'utf8'))),
      'the popup builds its badge with completionBadge, not a second table — the File Structure ' +
        'popup and Go-to-Symbol use the same one, and a function must not read blue in those and ' +
        'unlabelled here',
    )
    if (badges !== null) {
      // CodeMirror never creates its own `.cm-completionIcon` element when `icons: false` — it is
      // built inside `if (config.icons)` — so styling `.cm-completionIcon-*` is dead CSS. This is
      // the assertion that would have caught that: every tone the shared table can return needs a
      // rule against the attribute `completion.ts` actually writes.
      const tones = [...badges[0].matchAll(/tone: '([^']+)'/g)].map((m) => m[1])
      for (const tone of new Set(tones)) {
        OK(
          css.includes(`[data-cide-badge='${tone}']`),
          `the completion popup has a rule for the ${tone} tone`,
        )
      }
      OK(
        !/cm-completionIcon/.test(css),
        'nothing styles .cm-completionIcon — with `icons: false` that element is never created, ' +
          'so every such rule is dead and the badges would silently not appear',
      )
    }
  }

  // --- 9. the modules still import nothing ---------------------------------------------------

  for (const file of ['src/editor/completionGate.ts', 'src/editor/lspSnippet.ts']) {
    const source = readFileSync(join(UI, file), 'utf8')
    OK(
      !/^\s*import\s/m.test(source),
      `${file} must import nothing, or this script can no longer compile it standalone`,
    )
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`completion: ok (${checks} checks)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

/**
 * Blank out the fields a template legitimately contains, leaving only its literal text.
 *
 * So that "no stray field opener survives" can be asked of the part that is *not* meant to be a
 * field, which is the only part where a `${` is a bug.
 */
function stripFields(template) {
  return template.replace(/[#$]\{(?:\d+(?::[^{}]*)?|(?:\\[{}]|[^{}])*)\}/g, '')
}
