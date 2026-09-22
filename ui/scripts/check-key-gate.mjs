/**
 * The key gate's unit test: **every entry point resolves every chord identically**.
 *
 * Without this, someone adds a binding to one path and Ctrl+P works everywhere except
 * terminals — a failure with no error and no symptom until a user types `^P` into a shell
 * and gets a shell command instead of the file picker.
 *
 * The corpus is not the shipped keymap. It is the *whole* chord space that matters: every
 * one of the sixteen modifier combinations against a spread of key names, plus every
 * multi-stroke sequence in the fixture. Testing only the bindings that exist today would
 * pass on a keymap whose resolution is wrong for everything else, and would need editing
 * every time `cide-core::keymap::defaults` gains a line.
 *
 * Since M41 a sweep also runs under a non-Latin layout — the events a Russian, Greek or Hebrew
 * keyboard produces, `code` physical and `key` foreign — holding the verdict to the US twin's
 * *and* asserting what the event reads as afterwards, because `keys/latin.ts` rewrites it in
 * place at both entry points and everything behind the gate depends on that.
 *
 * This project has no JS test runner and adding one for four pure modules would be a larger
 * commitment than the code it tests, so this follows `check-status-format.mjs`: compile the
 * modules with the TypeScript that is already in `node_modules`, then exercise the output.
 * CommonJS emit rather than ESM, because `import './chords'` without a file extension —
 * which is what `moduleResolution: bundler` lets the app source write — does not resolve
 * under Node's ESM loader, whereas `require('./chords')` does.
 *
 * Run: `pnpm --dir ui run check:keys`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * The `(key, command)` pairs of `cide_core::keymap::defaults()`, read out of the Rust source.
 *
 * The fixture below mirrors that table by hand, and the specific claims this script makes —
 * `ctrl+p` is `picker.files`, `ctrl+shift+\`` is `terminal.splitBelow` — are only worth
 * anything if the mirror is current. Reading the Rust is the difference between a rename over
 * there failing here and a rename over there leaving a green suite asserting a binding that no
 * longer exists.
 *
 * A regex rather than a parser because the shape being read is a literal array of string
 * tuples in one function; if that ever stops being true, the count assertion below fails
 * loudly rather than silently matching nothing.
 */
function rustDefaults() {
  const path = fileURLToPath(new URL('../../crates/cide-core/src/keymap.rs', import.meta.url))
  const source = readFileSync(path, 'utf8')
  const start = source.indexOf('pub fn defaults() -> Vec<Binding> {')
  if (start < 0) throw new Error(`could not find defaults() in ${path}`)
  const end = source.indexOf('\n}', start)
  const body = source.slice(start, end)
  /*
   * **Three-tuples as well as two, and this was a real hole rather than tidiness.**
   *
   * `defaults()` grew a `.chain(...)` block of `(key, command, when)` entries in M12, and the
   * two-tuple regex below cannot see them at all — so `ctrl+b → navigate.definition` was in the
   * shipped table, absent from the fixture below, and invisible to the "every bound key reaches
   * a dispatcher" loop at the foot of this file. A binding whose command nothing dispatches
   * swallows the keystroke and does nothing, which is precisely what that loop exists to catch,
   * and "the regex did not match it" is the quietest way for a gate to fail. `check-commands.mjs`
   * had already been taught the same lesson and carries the same pair of patterns.
   */
  /*
   * `\s*` before the opening quote as well as after the commas, and that is not belt-and-braces
   * either. `rustfmt` breaks a tuple across four lines the moment it passes 100 columns, and it
   * did exactly that to `("ctrl+alt+equal", "editor.unfoldRecursively", "editorFocused")` in
   * M19 — at which point a one-line pattern stopped seeing a shipped binding, which is the same
   * silent hole the paragraph above describes arriving by a different route. Caught by the
   * mirror assertion in the other direction, which is the half that exists for this.
   */
  const two = [...body.matchAll(/\(\s*"([^"]+)",\s*"([^"]+)",?\s*\)/g)].map((m) => ({
    key: m[1],
    command: m[2],
  }))
  const three = [...body.matchAll(/\(\s*"([^"]+)",\s*"([^"]+)",\s*"([^"]+)",?\s*\)/g)].map((m) => ({
    key: m[1],
    command: m[2],
    when: m[3],
  }))
  if (three.length === 0) {
    throw new Error('no `when`-carrying default binding matched — the 3-tuple regex has broken')
  }
  return [...two, ...three]
}

/**
 * The bindings `platform_layer` **pushes** — the macOS-only additions, not the rewrites.
 *
 * A second reader beside `rustDefaults` rather than a wider regex over the whole file, because
 * the two answer different questions and share none of their shape: `defaults()` is a table of
 * tuples, and this is a run of `out.push(Binding::new(...))` calls after a rewrite loop. Reading
 * them together would make a rewritten binding indistinguishable from a pushed one, and the
 * difference is the whole point of the ⌘[ note in `keymap.rs`.
 *
 * Sliced from `fn platform_layer` to the next top-level `fn`, so the `Binding::new` calls in
 * that file's *tests* — which name several of these same chords — cannot be read as shipping
 * code. A gate that reads a fixture measures the fixture.
 */
function rustPlatformPushes() {
  const path = fileURLToPath(new URL('../../crates/cide-core/src/keymap.rs', import.meta.url))
  const source = readFileSync(path, 'utf8')
  const start = source.indexOf('fn platform_layer(macos: bool) -> Vec<Binding> {')
  if (start < 0) throw new Error(`could not find platform_layer in ${path}`)
  const end = source.indexOf('\n}\n', start)
  const body = source.slice(start, end)
  return [...body.matchAll(/Binding::new\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => ({
    key: m[1],
    command: m[2],
  }))
}

const out = mkdtempSync(join(tmpdir(), 'cide-keys-'))
let failed = 0

const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  if (!Object.is(actual, expected)) {
    fail(what, `actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
  }
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/keys/chords.ts',
      'src/keys/when.ts',
      'src/keys/keymap.ts',
      'src/keys/gate.ts',
      // The two stateful claims on a stroke, so the sweeps below drive the real rules rather
      // than a stand-in that agrees with itself. The recorder is the modal one: while Settings
      // → Keymap is waiting for a chord it consumes *everything*, which is a much stronger
      // claim than the switcher's and therefore worth measuring at both entry points.
      'src/keys/switcher.ts',
      'src/keys/recorder.ts',
      // The non-Latin rewrite the gate runs first. `tsc` would follow the import anyway; it is
      // listed so that a reader of this list sees every module the sweeps exercise.
      'src/keys/latin.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      // The gate names `window` and `setTimeout` behind runtime guards, so it needs the DOM
      // declarations to typecheck even though nothing here touches a DOM.
      '--lib', 'es2023,dom',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const { createKeyGate, currentStroke, mouseNavGate } = require(join(out, 'keys/gate.js'))
  const { buildKeymap } = require(join(out, 'keys/keymap.js'))
  const { chipLabel, normalizeSequence, strokeFromEvent } = require(join(out, 'keys/chords.js'))
  const { evaluateWhen } = require(join(out, 'keys/when.js'))
  const switcher = require(join(out, 'keys/switcher.js'))
  const recorder = require(join(out, 'keys/recorder.js'))
  const latin = require(join(out, 'keys/latin.js'))

  /* ------------------------------------------------------------------ the fixture keymap */

  /*
   * Mirrors `cide_core::keymap::defaults()` plus two things the shipped table does not have
   * yet and the gate must handle anyway: a multi-stroke sequence, and a `when`-gated
   * binding that shadows a terminal key.
   */
  const BINDINGS = [
    { key: 'ctrl+alt+right', command: 'pane.split.right', when: null },
    { key: 'ctrl+alt+down', command: 'pane.split.down', when: null },
    { key: 'ctrl+shift+n', command: 'claude.split.newSession', when: null },
    { key: 'ctrl+shift+d', command: 'pane.detachToWindow', when: null },
    { key: 'ctrl+shift+`', command: 'terminal.splitBelow', when: null },
    // The spawn chords, asked for as Ctrl+(/)/{/} and bound as the physical keys those are
    // the shifted faces of. `Digit9`, `Digit0`, `BracketLeft` and `BracketRight` are in the
    // sweep's key corpus below, so both entry points are held to swallowing them identically
    // — and to leaving the *unshifted* ctrl+bracket chords alone, which matter: Ctrl+[ IS the
    // ESC byte to a terminal, and a resolver that folded shift away would eat it.
    { key: 'ctrl+shift+9', command: 'claude.split.right', when: null },
    { key: 'ctrl+shift+0', command: 'claude.addRow', when: null },
    { key: 'ctrl+shift+bracketleft', command: 'terminal.splitRight', when: null },
    { key: 'ctrl+shift+bracketright', command: 'terminal.addRow', when: null },
    { key: 'ctrl+p', command: 'picker.files', when: null },
    { key: 'ctrl+shift+p', command: 'palette.commands', when: null },
    // Find in files. `KeyF` is in the sweep below, so this covers the property that matters
    // for a chord the terminal would otherwise receive: both entry points must swallow it
    // identically, or ⌃⇧F opens the panel *and* sends `^F` to whatever pty had focus.
    { key: 'ctrl+shift+f', command: 'sidebar.search', when: null },
    // M13. *Select opened file*, unconditional — and `KeyE` is in the sweep below, which is
    // what pins the property that matters for a chord a terminal would otherwise get: both
    // entry points must swallow it identically. xterm encodes a control byte only for
    // `ctrl && !shift && !alt && !meta`, so ⌃⇧E costs a pty nothing; ⌃E, one row down in this
    // fixture, is the shape that *would* cost something and is deliberately not shipped.
    { key: 'ctrl+shift+e', command: 'file.reveal', when: null },
    // M13. A new scratch file. Alt is in the sweep's modifier product, so this also covers the
    // one case an Alt binding has that a Ctrl one does not: `alt+shift+s` must resolve while
    // `alt+s` and `ctrl+alt+shift+s` pass through.
    { key: 'alt+shift+s', command: 'scratch.new', when: null },
    { key: 'ctrl+w', command: 'tab.close', when: null },
    { key: 'ctrl+s', command: 'file.save', when: null },
    // M15. Ctrl+Shift+T reopens the last closed tab, which is what the chord means in every
    // browser; `theme.toggle` had it and now ships unbound. `KeyT` is in the sweep's key list,
    // so this and `ctrl+t` below together pin the property that matters for the pair.
    { key: 'ctrl+shift+t', command: 'tab.reopenClosed', when: null },
    // Git pull. `KeyT` is already in the sweep below, so this covers the property that
    // matters for a chord the terminal would otherwise get: both entry points must swallow
    // it identically. If they disagree, Ctrl+T pulls *and* sends `^T` to the pty — which is
    // readline's transpose-chars, so the user's line would be silently scrambled by the
    // keystroke that pulled. The sweep also pins that `ctrl+shift+t` still reaches
    // `tab.reopenClosed` beside it rather than being shadowed by the shorter chord.
    { key: 'ctrl+t', command: 'git.pull', when: null },
    // M14. Ctrl+Tab is the *tab* switcher now, and the project switcher moved to Ctrl+`.
    // `Tab`, `Backquote` and `Digit1` are all in the sweep's key list below, so these four
    // cover the case that matters most for a walk binding: both entry points must swallow the
    // chord identically, or the stroke that switched tabs also inserts a tab character into
    // whatever had focus.
    //
    // All four carry `shellWindow`, which is why `CONTEXTS` below has a shell-window entry: a
    // `when` nothing in the sweep satisfies is a binding the sweep never actually exercises.
    { key: 'ctrl+tab', command: 'tab.switcher.next', when: 'shellWindow' },
    { key: 'ctrl+shift+tab', command: 'tab.switcher.prev', when: 'shellWindow' },
    { key: 'ctrl+`', command: 'project.switcher.next', when: 'shellWindow' },
    { key: 'ctrl+1', command: 'tab.console', when: 'shellWindow' },
    // M14. F4 toggles the left panel. xterm *encodes* F4 as `ESC O S`, so this is the one
    // binding in the table whose whole cost is measured by the sweep below: both entry points
    // must swallow it in a shell window and pass it through in a detached one, or `mc` and
    // `htop` see a key press the user did not make.
    { key: 'f4', command: 'sidebar.toggle', when: 'shellWindow' },
    { key: 'ctrl+alt+h', command: 'pane.navigate.left', when: null },
    { key: 'ctrl+alt+l', command: 'pane.navigate.right', when: null },
    { key: 'ctrl+alt+k', command: 'pane.navigate.up', when: null },
    { key: 'ctrl+alt+j', command: 'pane.navigate.down', when: null },
    // The pane moves' second spelling, asked for by name. The horizontal pair is
    // unconditional — an editor pane needs some alt+arrow that still leaves it — and costs
    // CodeMirror's syntax-step motion plus the pty's `ESC [ 1;3 D/C`. `ArrowLeft`/`ArrowRight`
    // are in the sweep below, so both entry points are held to swallowing them identically:
    // if they disagreed, the stroke that moved focus would also word-jump the shell's caret.
    { key: 'alt+left', command: 'pane.navigate.left', when: null },
    { key: 'alt+right', command: 'pane.navigate.right', when: null },
    { key: 'ctrl+comma', command: 'settings.open', when: null },
    { key: 'ctrl+k ctrl+s', command: 'settings.keymap', when: null },
    { key: 'ctrl+k ctrl+w', command: 'tab.closeOthers', when: null },
    { key: 'ctrl+shift+c', command: 'terminal.clear', when: 'terminalFocused' },
    /*
     * A **compound** `when`, which nothing in the shipped table has: the gate must evaluate
     * `a && !b` on both entry points, and that is a property of the gate rather than of any
     * binding. Held in `EXTRAS` below as fixture-only for exactly that reason.
     *
     * It used to name `file.reveal`, and that was the smoking gun of this milestone's defect:
     * the gate had been tested against a `file.reveal` hotkey for a whole milestone while the
     * command shipped with **no binding at all**. `file.reveal` now ships on ⌃⇧E above, so this
     * row points at a different command — otherwise a reader would take it for a second, real
     * binding and the two would eventually be reconciled in the wrong direction.
     */
    { key: 'ctrl+e', command: 'sidebar.search', when: 'editorFocused && !overlayOpen' },
    // M12. `alt+up`/`alt+down` carry a `when` because the gate is a window capture listener:
    // unconditionally bound they would fire in a terminal pane and move a caret nobody can see.
    { key: 'ctrl+f12', command: 'structure.file', when: null },
    { key: 'ctrl+alt+shift+n', command: 'picker.symbols', when: null },
    { key: 'alt+down', command: 'navigate.nextMember', when: 'editorFocused' },
    { key: 'alt+up', command: 'navigate.prevMember', when: 'editorFocused' },
    { key: 'alt+pagedown', command: 'gitlab.nextFile', when: 'gitlabReviewActive && !overlayOpen' },
    { key: 'alt+pageup', command: 'gitlab.previousFile', when: 'gitlabReviewActive && !overlayOpen' },
    // The complement: the same two keys are the pane moves everywhere a buffer is not
    // focused, and Alt+Enter is the maximize toggle under the same clause — inside a buffer
    // that chord is CodeMirror's *send lines to Claude*, which the capture gate would kill
    // rather than shadow. The first negated clause in the shipped table, so the sweep now
    // exercises `!flag` on both entry points against `ArrowUp`/`ArrowDown`/`Enter`, in the
    // editor-focused context where these must pass through and the terminal one where they
    // must fire.
    { key: 'ctrl+alt+shift+h', command: 'pane.move.left', when: 'terminalFocused' },
    { key: 'ctrl+alt+shift+l', command: 'pane.move.right', when: 'terminalFocused' },
    { key: 'ctrl+alt+shift+k', command: 'pane.move.up', when: 'terminalFocused' },
    { key: 'ctrl+alt+shift+j', command: 'pane.move.down', when: 'terminalFocused' },
    { key: 'alt+up', command: 'pane.navigate.up', when: '!editorFocused' },
    { key: 'alt+down', command: 'pane.navigate.down', when: '!editorFocused' },
    { key: 'alt+enter', command: 'pane.maximize', when: '!editorFocused' },
    // Go to Declaration. Scoped, and the scope is load-bearing: unconditional, the window
    // capture listener would swallow ⌃B in every terminal pane, which is `tmux`'s prefix key.
    // `KeyB` is in the sweep below, so both entry points are held to agreeing about it in a
    // terminal-focused context as well as an editor-focused one.
    //
    // It was missing from this fixture for a whole milestone because `rustDefaults()` above
    // could not see a 3-tuple — see the note there.
    { key: 'ctrl+b', command: 'navigate.definition', when: 'editorFocused' },
    { key: 'f1', command: 'navigate.documentation', when: 'editorFocused' },
    // M14. Find usages, on IDEA's chord. Scoped for the same reason ⌃B is, and it is not
    // theoretical here: F7 encodes as `ESC [ 18 ~` and Alt+F7 as `ESC ESC [ 18 ~`, so an
    // unconditional binding would take a key `mc` puts a menu on, in every terminal pane in every
    // window. `F7` is in the sweep below, so both entry points are held to agreeing about it.
    { key: 'alt+f7', command: 'navigate.usages', when: 'editorFocused' },
    // M18. Go to implementation, on IDEA's chord. Scoped for the same reason ⌃B is, and it is not
    // theoretical: Ctrl+Alt+B is `ESC ^B` to a terminal that maps Alt to an Escape prefix, which
    // is readline's `backward-word`. `KeyB` is in the sweep below, so the two entry points are
    // held to agreeing about it — including that a terminal-focused window passes it through.
    { key: 'ctrl+alt+b', command: 'navigate.implementation', when: 'editorFocused' },
    // Go to line. Scoped for the same reason ⌃B is, and one sharper: `^G` is readline's abort,
    // so an unconditional binding would take the cancel key of every shell in every pane.
    // `KeyG` is in the sweep below, so both entry points are held to agreeing about it — and to
    // *passing it through* in a terminal-focused context, which is the half that matters.
    { key: 'ctrl+g', command: 'navigate.line', when: 'editorFocused' },
    // M26. Reformat code, on VS Code's own chord for Format Document. It shipped as
    // `ctrl+alt+f` and reached nobody on a keyboard carrying `altwin:swap_lalt_lwin` — the key
    // labelled Alt emits `meta` there, so the stroke was `ctrl+meta+f` and the gate correctly
    // matched nothing, silently, which is indistinguishable from the feature being broken.
    // Scoped for the reason ⌃B is; `KeyF` is in the sweep below, so both entry points are held
    // to agreeing about it, including passing it through in a terminal-focused window.
    { key: 'alt+shift+f', command: 'editor.format', when: 'editorFocused' },
    // M16. The file picker's library scope. `filePickerOpen` and not `overlayOpen`, and the
    // difference is the whole reason that flag was added: ⌥L is `ESC l` to a shell — readline's
    // `downcase-word` — so a binding scoped to *any* overlay would have the window capture
    // listener eat it in a terminal pane whenever a menu or a palette happened to be up. `KeyL`
    // is in the sweep below, so both entry points are held to agreeing about it, and to leaving
    // it alone in every context that is not this one.
    { key: 'alt+l', command: 'picker.libraries', when: 'filePickerOpen' },
    // The mouse's thumb buttons, as pseudo-keys. Unscoped, unlike every other M12 binding: a
    // thumb button is pressed wherever the pointer is, which in this app is most often over a
    // terminal — and unlike ⌃B or ⌃G there is nothing to take away, since no shell reads GDK
    // button 8. `mouseback` is also in the sweep below, so the *keyboard* entry points are held
    // to leaving it alone in every modifier combination: a key named `mouseback` does not exist,
    // and the day one did it must not resolve here.
    { key: 'mouseback', command: 'navigate.back', when: null },
    { key: 'mouseforward', command: 'navigate.forward', when: null },
    /*
     * M19. Code folding — IDEA's chords, and the first commands in this table to stand on two
     * of them each.
     *
     * `minus`, `equal` and `plus` are the *code*-derived names (`CODE_NAMES` in `chords.ts`), so
     * one `ctrl+minus` covers the main row's `-` and the keypad's `−`, while `=` and `+` are two
     * different physical keys and therefore two lines. `Minus` and `Equal` are in the sweep
     * corpus below, which is what holds all three entry points to agreeing about them — and to
     * leaving them alone in every context that is not a focused editor, because xterm encodes
     * 0x1f for a plain `Ctrl+-` and an unscoped binding would take that from every shell.
     */
    { key: 'ctrl+minus', command: 'editor.fold', when: 'editorFocused' },
    { key: 'ctrl+equal', command: 'editor.unfold', when: 'editorFocused' },
    { key: 'ctrl+plus', command: 'editor.unfold', when: 'editorFocused' },
    { key: 'ctrl+shift+minus', command: 'editor.foldAll', when: 'editorFocused' },
    { key: 'ctrl+shift+equal', command: 'editor.unfoldAll', when: 'editorFocused' },
    { key: 'ctrl+shift+plus', command: 'editor.unfoldAll', when: 'editorFocused' },
    { key: 'ctrl+alt+minus', command: 'editor.foldRecursively', when: 'editorFocused' },
    { key: 'ctrl+alt+equal', command: 'editor.unfoldRecursively', when: 'editorFocused' },
    { key: 'ctrl+alt+plus', command: 'editor.unfoldRecursively', when: 'editorFocused' },
    { key: 'ctrl+period', command: 'editor.toggleFold', when: 'editorFocused' },
    /*
     * M25. The changes iterator, and the first shipped bindings with a **compound** clause.
     *
     * `diffFocused` alone would swallow F7 in a shell pane split beside a diff tab that is in
     * front — the gate is a window capture listener and F7 is `ESC [ 18 ~`, which `mc` puts a
     * menu on. A diff pane and a merge pane are both `kind == 'editor'`, so `!terminalFocused`
     * is false in exactly the states the feature needs and true in exactly the states a
     * terminal needs the key back. `F7` is already in the sweep corpus below, so all sixteen
     * modifier combinations on it are swept in every context.
     */
    { key: 'f7', command: 'navigate.nextChange', when: 'diffFocused && !terminalFocused' },
    { key: 'shift+f7', command: 'navigate.prevChange', when: 'diffFocused && !terminalFocused' },
  ]

  /* The fixture is a mirror, so check it against the thing it mirrors before trusting it. */
  {
    const rust = rustDefaults()
    ok(rust.length >= 16, `read ${rust.length} defaults out of keymap.rs — the regex still matches`)
    for (const { key, command, when } of rust) {
      /*
       * **The `when` is compared, and it was not until a mutation test caught that.**
       *
       * Matching on `(key, command)` alone left the scope unmirrored, so deleting `editorFocused`
       * from a binding in `keymap.rs` — which is what turns ⌃G from Go to line into a chord that
       * swallows readline's abort in every terminal pane in every window — changed nothing here.
       * The fixture kept its own copy of the clause and every assertion below kept testing that
       * copy. Same class as the 3-tuple hole described in `rustDefaults`: not a wrong answer, a
       * question nobody asked.
       *
       * `?? null` on both sides because the two spell "unconditional" differently: the regex
       * leaves the field absent, the fixture writes `null`.
       */
      const mirrored = BINDINGS.some(
        (b) => b.key === key && b.command === command && (b.when ?? null) === (when ?? null),
      )
      ok(
        mirrored,
        `the fixture mirrors cide_core::keymap::defaults(): ${key} → ${command}` +
          `${when === undefined ? '' : ` when ${when}`}`,
      )
    }

    /*
     * **And the other direction, which was missing — a third instance of the same hole.**
     *
     * The loop above walks Rust and demands the fixture contain each entry, so a *rename* over
     * there fails here. A **deletion** did not: the Rust row simply stopped being iterated, the
     * fixture kept its own copy, and every assertion below carried on testing a binding that no
     * longer shipped. Found by mutation — removing `mouseback`/`mouseforward` from
     * `defaults()` left this script green.
     *
     * Same shape as the two holes already recorded in `rustDefaults` and beside the `when`
     * comparison: not a wrong answer, a question nobody asked.
     *
     * `EXTRAS` is the deliberate difference. The fixture carries a multi-stroke sequence and two
     * `when`-gated bindings that the shipped table does not have, because the *gate* must handle
     * them whether or not anything is bound that way today. Anything else in the fixture with no
     * Rust counterpart is drift.
     */
    const EXTRAS = new Set([
      'ctrl+k ctrl+s',
      'ctrl+k ctrl+w',
      'ctrl+shift+c',
      'ctrl+e',
    ])
    for (const binding of BINDINGS) {
      if (EXTRAS.has(binding.key)) continue
      const shipped = rust.some(
        (b) =>
          b.key === binding.key &&
          b.command === binding.command &&
          (b.when ?? null) === (binding.when ?? null),
      )
      ok(
        shipped,
        `and mirrors nothing that has been deleted from it: ${binding.key} → ${binding.command}`,
      )
    }
  }

  /** Physical keys worth sweeping every modifier combination against. */
  const KEYS = [
    { code: 'KeyP', key: 'p' },
    { code: 'KeyS', key: 's' },
    { code: 'KeyW', key: 'w' },
    { code: 'KeyK', key: 'k' },
    { code: 'KeyN', key: 'n' },
    { code: 'KeyD', key: 'd' },
    { code: 'KeyT', key: 't' },
    { code: 'KeyC', key: 'c' },
    { code: 'KeyE', key: 'e' },
    { code: 'KeyF', key: 'f' },
    { code: 'KeyH', key: 'h' },
    { code: 'KeyJ', key: 'j' },
    { code: 'KeyL', key: 'l' },
    { code: 'KeyA', key: 'a' },
    // ⌃B is `tmux`'s prefix and is bound here only inside an editor, and ⌃C/⌃V are bound
    // nowhere at all — the terminal resolves those two itself, focus-scoped, in
    // `src/terminal/keys.ts`. All three are swept so that "the gate does not touch them" is a
    // measured claim rather than an assumption about a table.
    { code: 'KeyB', key: 'b' },
    { code: 'KeyV', key: 'v' },
    // ⌃G is Go to line inside an editor and `^G` — readline's abort — everywhere else, so it is
    // swept for the same reason ⌃B is.
    { code: 'KeyG', key: 'g' },
    // The undo family. Bound by nothing here and asserted so below; sweeping them is what makes
    // "the gate does not touch them" a measured claim rather than an assumption about a table.
    { code: 'KeyZ', key: 'z' },
    { code: 'KeyY', key: 'y' },
    { code: 'KeyU', key: 'u' },
    // F3 is find-next inside CodeMirror, in the buffer and in the find field both. Unbound here.
    { code: 'F3', key: 'F3' },
    // F4 is the panel toggle and its neighbour is not, which is the pair worth sweeping
    // together: xterm encodes both, and a resolver that folded f-keys would take find-next from
    // the find bar in exchange for a panel that already has a button.
    { code: 'F4', key: 'F4' },
    // M14. ⌥F7 is Find usages inside an editor and `ESC ESC [ 18 ~` — a key `mc` puts a menu on
    // — everywhere else, so it is swept for exactly the reason ⌃B and ⌃G are: the claim worth
    // measuring is that the *other fifteen* modifier combinations on this key are left alone, and
    // that the one that is bound is passed through in a terminal.
    { code: 'F7', key: 'F7' },
    { code: 'Digit1', key: '1' },
    { code: 'PageUp', key: 'PageUp' },
    { code: 'PageDown', key: 'PageDown' },
    { code: 'ArrowUp', key: 'ArrowUp' },
    { code: 'ArrowDown', key: 'ArrowDown' },
    { code: 'ArrowLeft', key: 'ArrowLeft' },
    { code: 'ArrowRight', key: 'ArrowRight' },
    { code: 'Comma', key: ',' },
    /*
     * M19. The folding keys, and the reason the gate reads `code` written out one more time.
     *
     * Shift on `Minus` produces `_` and shift on `Equal` produces `+`, so a resolver that read
     * `key` would spell `ctrl+shift+minus` as `ctrl+shift+_` and never match — while
     * `ctrl+shift+equal`, which is what a US keyboard reports for the chord IDEA draws as
     * `Ctrl+Shift++`, would resolve to a key named `plus` that a *different* physical key also
     * produces. Both are swept in all sixteen combinations, in a focused editor and in a
     * terminal, because a plain `Ctrl+-` is 0x1f to a pty and the claim worth measuring is that
     * these are taken *only* where the `editorFocused` clause is true.
     */
    { code: 'Minus', key: '-' },
    { code: 'Equal', key: '=' },
    // The keypad's `+` and `-`, which fold onto the same two names — so IDEA's chords work on a
    // full keyboard, and one `ctrl+minus` in `defaults()` is genuinely covering two keys.
    { code: 'NumpadSubtract', key: '-' },
    { code: 'NumpadAdd', key: '+' },
    // `ctrl+period` is Toggle fold; its neighbour `Comma` above is Settings, so the pair is
    // swept together the way F3/F4 are.
    { code: 'Period', key: '.' },
    // Shift on this key produces `~`, which is exactly why the gate reads `code`.
    { code: 'Backquote', key: '`' },
    /*
     * The spawn chords' keys. Shift on these produces `(`, `)`, `{` and `}` — the faces the
     * ask was spelled in — so they are the folding keys' story one more time: the gate reads
     * `code`, and `ALIASES` folds the faces onto these names so the literal spellings work in
     * a `keymap.json`. The brackets carry the sharper claim: only the ctrl+shift combination
     * is bound, and the *unshifted* Ctrl+[ is the ESC byte to every terminal ever made, so
     * the fifteen other modifier combinations being passed through — measured in the
     * terminal-focused contexts below — is most of what these two rows are here for.
     */
    { code: 'Digit9', key: '9' },
    { code: 'Digit0', key: '0' },
    { code: 'BracketLeft', key: '[' },
    { code: 'BracketRight', key: ']' },
    { code: 'Enter', key: 'Enter' },
    { code: 'Escape', key: 'Escape' },
    { code: 'Tab', key: 'Tab' },
    { code: 'F1', key: 'F1' },
    { code: 'Space', key: ' ' },
  ]

  const CONTEXTS = [
    {},
    { terminalFocused: true },
    { editorFocused: true },
    { editorFocused: true, overlayOpen: true },
    { gitlabReviewActive: true },
    { gitlabReviewActive: true, overlayOpen: true },
    { claudePaneFocused: true, repoOpen: true },
    /*
     * A shell window with a terminal focused, which is where every M14 binding is both
     * *applicable* and *expensive*.
     *
     * Without a context that sets `shellWindow`, the five bindings that carry it would be swept
     * only in states where their clause is false — so the sweep would prove that the two entry
     * points agree about passing them through, and nothing at all about the state the binding
     * exists for. `terminalFocused` beside it is the half that makes the claim worth having:
     * F4 is `ESC O S` to a pty and Ctrl+Tab is a tab character, so "both entry points swallow
     * them here, and neither swallows them in the `{}` context above" is the whole of what a
     * user of `mc` in a detached pane is relying on.
     */
    { shellWindow: true, terminalFocused: true },
    /*
     * M25. A diff in front, and the same diff with a terminal focused beside it.
     *
     * Both are needed or the compound clause is never actually exercised: without the first,
     * F7 is only ever swept in states its clause excludes; without the second, the half that
     * gives the key back to `mc` is asserted nowhere.
     */
    { diffFocused: true },
    { diffFocused: true, terminalFocused: true },
  ]

  const event = (spec) => ({
    type: 'keydown',
    code: spec.code,
    key: spec.key,
    ctrlKey: spec.ctrl === true,
    altKey: spec.alt === true,
    shiftKey: spec.shift === true,
    metaKey: spec.meta === true,
    prevented: 0,
    stopped: 0,
    preventDefault() {
      this.prevented += 1
    },
    stopPropagation() {
      this.stopped += 1
    },
  })

  /**
   * A live switcher walk, wired the way `keys/switcherStore.ts` wires one.
   *
   * The store's `switcherCapture` is four lines over `switcher.capture`, and those four lines
   * are reproduced here rather than imported because the store pulls in zustand, the IPC
   * client and the workspace mirror. What is *not* reproduced is the decision itself — that
   * comes from the compiled module, so a change to the capture rules is felt here.
   *
   * `key` is the walk's own opening key and defaults to Tab. It is a parameter because the
   * capture compares against it rather than against the literal `'tab'` — which is what makes
   * one implementation serve Ctrl+Tab and Ctrl+` — and because a walk built without it would
   * quietly claim nothing at all.
   */
  const walking = (order, hold, key = 'tab') => {
    let walk = { order, index: 1, hold, key }
    return {
      state: () => walk,
      capture: (stroke) => {
        if (walk === null) return false
        const action = switcher.capture(walk, stroke)
        if (action.kind === 'advance') walk = switcher.advance(walk, action.step)
        else if (action.kind === 'cancel') walk = null
        return action.consumed
      },
    }
  }

  /**
   * A live keystroke recorder, wired the way `keys/recorderStore.ts` wires one.
   *
   * Same arrangement as `walking` above and for the same reason: the four lines of the store
   * are reproduced here because the store pulls in zustand and the workspace mirror, while the
   * *decision* comes from the compiled module, so a change to the recorder's rules is felt in
   * the sweep below.
   *
   * `armed` is what the popup being open corresponds to. Escape and Enter close it, which is
   * why the sweep builds a fresh one per stroke.
   */
  const recording = (armedNow = true) => {
    let armed = armedNow
    let state = recorder.EMPTY
    let saved = null
    return {
      state: () => (armed ? state : null),
      saved: () => saved,
      /** The click on *Edit*, as it would be without `startRecording`'s prefix reset. */
      arm: () => {
        armed = true
      },
      capture: (stroke) => {
        if (!armed) return false
        const action = recorder.capture(state, stroke)
        if (action.kind === 'record') state = action.next
        else if (action.kind === 'cancel') armed = false
        else if (action.kind === 'commit') {
          saved = recorder.sequenceOf(state)
          armed = false
        }
        return true
      },
    }
  }

  const CTRL = { ctrl: true, alt: false, meta: false }

  /**
   * A gate with a frozen clock, plus the log of what it dispatched.
   *
   * `walk` optionally arms the switcher's capture. A *fresh* one per gate, never shared: the
   * capture is stateful, and handing both entry points one object would let the first call
   * decide the second's answer — which is the very asymmetry this script exists to detect,
   * planted by the test rather than found in the code.
   */
  const makeGate = (ctx, walk) => {
    const dispatched = []
    let clock = 0
    const gate = createKeyGate({
      bindings: () => BINDINGS,
      context: () => ctx,
      run: (command, args) => dispatched.push({ command, args }),
      now: () => clock,
      timeoutMs: 1000,
      ...(walk === undefined ? {} : { capture: walk.capture }),
    })
    return {
      gate,
      dispatched,
      walk,
      advance: (ms) => {
        clock += ms
      },
    }
  }

  /* --------------------------------------------- the claim: both entry points agree */

  // The rewrite's latch is per module instance and the fixture rows below all carry US keys,
  // so it must be — and stay — Latin here, or a row's `key` becomes order-dependent on which
  // section ran before it. The non-Latin section at the foot resets it and asserts the same.
  eq(latin.latinLayoutIsNonLatin(), false, 'the sweeps start with a Latin latch')

  let compared = 0
  for (const ctx of CONTEXTS) {
    for (const spec of KEYS) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const label = `${JSON.stringify(mods)} ${spec.code} in ${JSON.stringify(ctx)}`

        // Two independent gates so the prefix machine of one cannot colour the other, and
        // two distinct event objects so the decision memo cannot hand the second entry
        // point the first one's answer.
        const a = makeGate(ctx)
        const b = makeGate(ctx)
        const evA = event({ ...spec, ...mods })
        const evB = event({ ...spec, ...mods })

        const viaWindow = a.gate.windowHandler(evA)
        const viaTerminal = b.gate.terminalHandler(evB)

        compared += 1
        if (viaWindow !== viaTerminal) {
          fail(`entry points disagree on pass-through: ${label}`, `window=${viaWindow} terminal=${viaTerminal}`)
        }
        if (JSON.stringify(a.dispatched) !== JSON.stringify(b.dispatched)) {
          fail(
            `entry points dispatched differently: ${label}`,
            `window=${JSON.stringify(a.dispatched)} terminal=${JSON.stringify(b.dispatched)}`,
          )
        }
        if (a.gate.pending() !== b.gate.pending()) {
          fail(`entry points left different prefixes armed: ${label}`)
        }
        // A swallowed chord must never reach the browser's own default on either path.
        if (!viaWindow && evA.prevented === 0) fail(`window entry did not preventDefault: ${label}`)
        if (!viaTerminal && evB.prevented === 0) {
          fail(`terminal entry did not preventDefault: ${label}`)
        }
        // Only the window entry stops the event reaching its target; the terminal entry is
        // consulted after the event has already arrived, and calling it there would be a
        // lie about what the handler can do.
        if (evB.stopped !== 0) fail(`terminal entry called stopPropagation: ${label}`)
      }
    }
  }
  ok(compared === CONTEXTS.length * KEYS.length * 16, 'swept the whole modifier × key space')

  let mouseSwept = 0

  /* ------------------------------------ entry point 3: the mouse's thumb buttons (M12) */

  /*
   * The gate grew a third entry point, and the whole premise of this script is that entry
   * points do not get to have their own opinions.
   *
   * The mouse buttons cannot arrive as `KeyboardEvent`s — WebKitGTK flattens GDK buttons 8 and 9
   * to `button === 0` before the DOM sees them, which is why they come from Rust as an event and
   * enter the gate as a *token* rather than as a stroke to be parsed. So the comparison cannot
   * be "the same event through three functions"; it is "the same stroke string through the
   * resolver, reached three ways". What is pinned here is that the token the mouse handler
   * builds is the token the keymap is indexed by — an off-by-one in modifier *order* would make
   * `ctrl+mouseback` resolve to nothing, silently, for ever.
   */
  {
    // `eq` compares with `Object.is`, which is right for the strings and booleans everything
    // above asserts on and useless for a dispatch log. One local helper rather than widening
    // the shared one, so nothing already green changes meaning.
    const same = (actual, expected, what) => {
      const a = JSON.stringify(actual)
      const b = JSON.stringify(expected)
      if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
    }
    let mouseCompared = 0
    // Published so the summary line can say how much of the sweep was the mouse's.

    for (const ctx of CONTEXTS) {
      for (const button of ['mouseback', 'mouseforward']) {
        for (let bits = 0; bits < 16; bits++) {
          const mods = {
            ctrl: (bits & 1) !== 0,
            alt: (bits & 2) !== 0,
            shift: (bits & 4) !== 0,
            meta: (bits & 8) !== 0,
          }
          const label = `${JSON.stringify(mods)} ${button} in ${JSON.stringify(ctx)}`
          const bare = bits === 0

          const viaMouse = makeGate(ctx)
          const decision = viaMouse.gate.mouseHandler(button, mods)

          // The stroke the gate says it resolved is the one the keymap would be indexed by.
          // `normalizeSequence` is the function `buildKeymap` runs over every binding's key, so
          // comparing against it is comparing against the index itself.
          const spelled = [
            mods.ctrl ? 'ctrl+' : '',
            mods.alt ? 'alt+' : '',
            mods.shift ? 'shift+' : '',
            mods.meta ? 'meta+' : '',
            button,
          ].join('')
          eq(
            decision.sequence,
            normalizeSequence(spelled),
            `the mouse handler spells the stroke the way the keymap is indexed: ${label}`,
          )

          if (bare) {
            eq(decision.command, button === 'mouseback' ? 'navigate.back' : 'navigate.forward',
              `a bare thumb press dispatches, in every context: ${label}`)
            eq(decision.passThrough, false, `and is consumed: ${label}`)
            eq(viaMouse.dispatched.length, 1, `exactly once: ${label}`)
          } else {
            // Nothing is bound with modifiers, so a modified press must fall through rather
            // than being quietly eaten — the same rule an unbound chord gets.
            eq(decision.command, null, `a modified thumb press is not the bare binding: ${label}`)
            eq(decision.passThrough, true, `and is not swallowed: ${label}`)
          }

          mouseCompared += 1
        }
      }
    }
    ok(
      mouseCompared === CONTEXTS.length * 2 * 16,
      `swept both thumb buttons across every modifier and context (${mouseCompared})`,
    )
    mouseSwept = mouseCompared

    /*
     * **No real keystroke can spell a thumb button.**
     *
     * The token spaces are deliberately *shared* — that is the whole trick, and it is what lets
     * a user write `{"key":"mouseback",...}` in `keymap.json` and get modifiers, `when` clauses
     * and unbinding for free. The safety property is therefore not "the vocabularies are
     * disjoint" (a `KeyboardEvent` carrying `key: 'mouseback'` would resolve, and asserting
     * otherwise would be asserting something false); it is that **nothing a keyboard can
     * produce lands on one**. `strokeFromEvent` is the only thing that turns a physical key into
     * a token, so that is what is checked, over the whole swept key space.
     */
    for (const spec of KEYS) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const stroke = strokeFromEvent({
          code: spec.code,
          key: spec.key,
          ctrlKey: mods.ctrl,
          altKey: mods.alt,
          shiftKey: mods.shift,
          metaKey: mods.meta,
        })
        ok(
          stroke === null || (!stroke.endsWith('mouseback') && !stroke.endsWith('mouseforward')),
          `no keystroke spells a thumb button: ${spec.code} → ${stroke}`,
        )
      }
    }

    /*
     * The prefix machine sees the mouse the same way it sees a key.
     *
     * With `ctrl+k` armed, a thumb press extends the sequence, completes nothing, is swallowed
     * and disarms — which is the rule `resolveStroke` applies to any unbound second stroke. The
     * alternative (a mouse handler that bypassed the prefix state) would leave `ctrl+k` armed
     * and eat the user's *next* keystroke, in a terminal, with no trace of why.
     */
    const armed = makeGate({})
    armed.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true }))
    eq(armed.gate.pending(), 'ctrl+k', 'the prefix is armed')
    const after = armed.gate.mouseHandler('mouseback')
    eq(after.sequence, 'ctrl+k mouseback', 'a thumb press extends the armed prefix')
    eq(after.command, null, 'completes nothing')
    eq(after.passThrough, false, 'is swallowed anyway')
    eq(armed.gate.pending(), null, 'and disarms')
    same(armed.dispatched, [], 'nothing was dispatched by the pair')

    /*
     * A press with no gate installed. `mouseNavGate` is a module-level stable reference handed
     * to a Tauri listener that resolves a tick late, so it can genuinely be called after the
     * window's gate has been torn down.
     */
    same(
      mouseNavGate('mouseback'),
      { passThrough: true, command: null, sequence: null },
      'with no gate installed a thumb press passes through rather than throwing',
    )
  }

  /* Multi-stroke sequences, both entry points, stroke by stroke. */
  for (const sequence of [
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyS', key: 's', ctrl: true }],
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyW', key: 'w', ctrl: true }],
    // Second stroke completes nothing: swallowed, prefix disarmed.
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyZ', key: 'z', ctrl: true }],
    // Second stroke is an unmodified letter — must not reach the PTY either.
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyA', key: 'a' }],
  ]) {
    const label = sequence.map((s) => s.code).join(' ')
    const a = makeGate({})
    const b = makeGate({})
    const results = { window: [], terminal: [] }
    for (const spec of sequence) {
      results.window.push(a.gate.windowHandler(event(spec)))
      results.terminal.push(b.gate.terminalHandler(event(spec)))
    }
    eq(results.window.join(','), results.terminal.join(','), `sequence pass-through: ${label}`)
    eq(
      JSON.stringify(a.dispatched),
      JSON.stringify(b.dispatched),
      `sequence dispatch: ${label}`,
    )
    ok(results.window.every((through) => through === false), `sequence fully swallowed: ${label}`)
    eq(a.gate.pending(), null, `sequence disarmed the prefix: ${label}`)
  }

  /* ------------------------------- the same claim again, with the switcher's capture armed */

  /*
   * The capture makes the gate **stateful for the duration of a Ctrl+Tab walk**, and a
   * stateful gate is exactly where the two entry points drift apart: the window listener runs
   * in the capture phase and the terminal handler runs after the event has reached xterm, so
   * a claim that mutated on one path and not the other would swallow Tab in a terminal and
   * pass it through everywhere else — or the reverse, which sends a tab character to the
   * shell in the middle of switching projects.
   *
   * So the whole sweep runs a second time with a walk open. Two gates, two captures, two
   * event objects, same three assertions.
   */
  let capturedCompared = 0
  for (const ctx of CONTEXTS) {
    for (const spec of KEYS) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const label = `switcher open: ${JSON.stringify(mods)} ${spec.code} in ${JSON.stringify(ctx)}`
        const order = ['p1', 'p2', 'p3']

        const a = makeGate(ctx, walking(order, CTRL))
        const b = makeGate(ctx, walking(order, CTRL))
        const evA = event({ ...spec, ...mods })
        const evB = event({ ...spec, ...mods })

        const viaWindow = a.gate.windowHandler(evA)
        const viaTerminal = b.gate.terminalHandler(evB)

        capturedCompared += 1
        if (viaWindow !== viaTerminal) {
          fail(`entry points disagree on pass-through: ${label}`, `window=${viaWindow} terminal=${viaTerminal}`)
        }
        if (JSON.stringify(a.dispatched) !== JSON.stringify(b.dispatched)) {
          fail(`entry points dispatched differently: ${label}`)
        }
        if (JSON.stringify(a.walk.state()) !== JSON.stringify(b.walk.state())) {
          fail(
            `entry points left the walk in different states: ${label}`,
            `window=${JSON.stringify(a.walk.state())} terminal=${JSON.stringify(b.walk.state())}`,
          )
        }
        if (!viaWindow && evA.prevented === 0) fail(`window entry did not preventDefault: ${label}`)
        if (!viaTerminal && evB.prevented === 0) fail(`terminal entry did not preventDefault: ${label}`)
        if (evB.stopped !== 0) fail(`terminal entry called stopPropagation: ${label}`)
      }
    }
  }
  ok(
    capturedCompared === CONTEXTS.length * KEYS.length * 16,
    'swept the whole modifier × key space again with a walk open',
  )

  /* ------------------------------- and a third time, with the keystroke recorder armed */

  /*
   * **While Settings → Keymap is recording, nothing else in the app may hear a keystroke.**
   *
   * This is the property the whole recorder design turns on, and it is the one a plausible
   * implementation gets wrong: a dialog that listened for its own `keydown` would never see
   * Ctrl+P at all, because the gate is a window **capture** listener and would have opened the
   * file picker before the event reached the box asking the user to press a shortcut. So the
   * recorder is the gate's `capture`, and while it is armed the claim is total.
   *
   * Total is exactly what is measured: over the whole modifier × key space, in every context,
   * at both entry points — pass-through is always false, `preventDefault` is always called, and
   * **no command is ever dispatched**. `ctrl+p` in the sweep below is the file picker not
   * opening; `f4` is the sidebar not toggling; `ctrl+tab` is the tab switcher not opening
   * behind the popup.
   */
  let recordedCompared = 0
  for (const ctx of CONTEXTS) {
    for (const spec of KEYS) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const label = `recorder armed: ${JSON.stringify(mods)} ${spec.code} in ${JSON.stringify(ctx)}`

        const a = makeGate(ctx, recording())
        const b = makeGate(ctx, recording())
        const evA = event({ ...spec, ...mods })
        const evB = event({ ...spec, ...mods })

        const viaWindow = a.gate.windowHandler(evA)
        const viaTerminal = b.gate.terminalHandler(evB)

        recordedCompared += 1
        if (viaWindow !== viaTerminal) {
          fail(`entry points disagree on pass-through: ${label}`, `window=${viaWindow} terminal=${viaTerminal}`)
        }
        if (viaWindow !== false) {
          fail(`a stroke escaped the recorder: ${label}`)
        }
        if (a.dispatched.length !== 0 || b.dispatched.length !== 0) {
          fail(
            `a command ran while a chord was being recorded: ${label}`,
            `window=${JSON.stringify(a.dispatched)} terminal=${JSON.stringify(b.dispatched)}`,
          )
        }
        if (JSON.stringify(a.walk.state()) !== JSON.stringify(b.walk.state())) {
          fail(
            `entry points left the recorder in different states: ${label}`,
            `window=${JSON.stringify(a.walk.state())} terminal=${JSON.stringify(b.walk.state())}`,
          )
        }
        if (evA.prevented === 0) fail(`window entry did not preventDefault: ${label}`)
        if (evB.prevented === 0) fail(`terminal entry did not preventDefault: ${label}`)
        if (evB.stopped !== 0) fail(`terminal entry called stopPropagation: ${label}`)
      }
    }
  }
  ok(
    recordedCompared === CONTEXTS.length * KEYS.length * 16,
    'swept the whole modifier × key space a third time with the recorder armed',
  )

  /* ------------------------------------- a non-Latin layout is the same keystroke (M41) */

  /*
   * Under a Russian layout WebKitGTK reports `key 'я'` and `keyCode 0` for the Z key — only
   * `code` is physical — and `keys/latin.ts` rewrites such a chord in place, from both entry
   * points, before anything reads it. Three claims, each measured for every context and all
   * sixteen modifier sets at both entry points:
   *
   * 1. **The gate's verdict is invariant.** A Cyrillic, Greek or Hebrew event resolves to the
   *    same pass-through, the same command and the same armed prefix as its US twin. The gate
   *    reads `code`, so this was true before the rewrite existed; it is measured so that it
   *    stays true if `strokeFromEvent` ever grows a `key` branch for one of these codes.
   * 2. **Under Ctrl, Alt or Meta the event reads as its US twin afterwards** — `key`,
   *    `keyCode` and `which` — because that is what xterm, CodeMirror and every `ev.key`
   *    matcher behind the gate see. Without a chord modifier, or with Shift alone, it reads
   *    exactly as the browser built it: that is typing, and the character must reach the field.
   * 3. **`code` is never touched.**
   *
   * The rows pair a physical key with what a foreign layout prints on it; `Digit3` carries the
   * Russian Shift face `№` so a non-ASCII character on a *digit* key is swept too. As with
   * `KEYS`, one `key` serves all sixteen modifier sets — the unshifted rows are a fiction under
   * Shift, and the claim is about the rewrite's verdict, not the layout's.
   */
  const NON_LATIN = [
    { code: 'KeyP', key: 'з' },
    { code: 'KeyZ', key: 'я' },
    { code: 'KeyF', key: 'а' },
    { code: 'KeyC', key: 'с' },
    { code: 'KeyZ', key: 'ζ' },
    { code: 'KeyZ', key: 'ז' },
    { code: 'Backquote', key: 'ё' },
    { code: 'BracketLeft', key: 'х' },
    { code: 'Comma', key: 'б' },
    { code: 'Period', key: 'ю' },
    { code: 'Semicolon', key: 'ж' },
    { code: 'Digit3', key: '№' },
  ]
  const ENTRIES = [
    ['window', (g, e) => g.gate.windowHandler(e)],
    ['terminal', (g, e) => g.gate.terminalHandler(e)],
  ]
  let nonLatinCompared = 0
  for (const ctx of CONTEXTS) {
    for (const spec of NON_LATIN) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const chord = mods.ctrl || mods.alt || mods.meta
        const twin = latin.usFace(spec.code, mods.shift)
        const label = `${JSON.stringify(mods)} ${spec.code} ${JSON.stringify(spec.key)} in ${JSON.stringify(ctx)}`
        for (const [entry, run] of ENTRIES) {
          const a = makeGate(ctx)
          const b = makeGate(ctx)
          const foreign = event({ ...spec, ...mods })
          const us = event({ code: spec.code, key: twin.key, ...mods })
          const viaForeign = run(a, foreign)
          const viaUs = run(b, us)
          nonLatinCompared += 1

          if (viaForeign !== viaUs) {
            fail(`${entry}: a non-Latin keystroke passes through differently from its US twin: ${label}`, `foreign=${viaForeign} us=${viaUs}`)
          }
          if (JSON.stringify(a.dispatched) !== JSON.stringify(b.dispatched)) {
            fail(`${entry}: a non-Latin keystroke dispatched differently from its US twin: ${label}`, `foreign=${JSON.stringify(a.dispatched)} us=${JSON.stringify(b.dispatched)}`)
          }
          if (a.gate.pending() !== b.gate.pending()) {
            fail(`${entry}: a non-Latin keystroke left a different prefix armed from its US twin: ${label}`)
          }
          if (foreign.code !== spec.code) fail(`${entry}: the rewrite touched code: ${label}`)
          if (chord) {
            if (foreign.key !== twin.key || foreign.keyCode !== twin.keyCode || foreign.which !== twin.keyCode) {
              fail(
                `${entry}: after the gate the event does not read as its US twin: ${label}`,
                `key=${JSON.stringify(foreign.key)} keyCode=${foreign.keyCode} which=${foreign.which}, wanted ${JSON.stringify(twin)}`,
              )
            }
          } else if (foreign.key !== spec.key || Object.hasOwn(foreign, 'keyCode')) {
            fail(`${entry}: typing was rewritten: ${label}`, `key=${JSON.stringify(foreign.key)}`)
          }
        }
      }
    }
  }
  ok(
    nonLatinCompared === CONTEXTS.length * NON_LATIN.length * 16 * ENTRIES.length,
    'swept every non-Latin row against its US twin through both entry points',
  )

  /*
   * Claim 1 from the other side: for every code the rewrite knows, `strokeFromEvent` spells the
   * same chord whatever `key` says. This is the property that lets the rewrite happen *inside*
   * the gate's handlers without changing a single verdict, and it holds because `latin.ts`'s
   * table is a subset of the codes `chords.ts`'s `nameFromCode` names. A code added to one
   * table and not the other fails here by name.
   */
  let invariant = 0
  for (const code of latin.LATIN_CODES) {
    for (let bits = 0; bits < 16; bits++) {
      const mods = {
        ctrl: (bits & 1) !== 0,
        alt: (bits & 2) !== 0,
        shift: (bits & 4) !== 0,
        meta: (bits & 8) !== 0,
      }
      const twin = latin.usFace(code, mods.shift)
      const foreign = strokeFromEvent(event({ code, key: 'я', ...mods }))
      const us = strokeFromEvent(event({ code, key: twin.key, ...mods }))
      invariant += 1
      if (foreign !== us) {
        fail(`strokeFromEvent consults key for ${code}, so a non-Latin layout spells a different chord`, `foreign=${foreign} us=${us}`)
      }
    }
  }
  ok(invariant === latin.LATIN_CODES.length * 16, 'held the gate invariant over every code the rewrite knows')

  // The rows above taught the latch a non-Latin layout. Put it back before anything after
  // this section runs, and say so if it did not take.
  latin.resetLatinLayout()
  eq(latin.latinLayoutIsNonLatin(), false, 'the non-Latin section left no latch behind')

  /* What the recorder does with the four strokes that are not simply data. */
  {
    // Ctrl+P is recorded rather than opening the picker — the whole point.
    const g = makeGate({ terminalFocused: true }, recording())
    eq(
      g.gate.terminalHandler(event({ code: 'KeyP', key: 'p', ctrl: true })),
      false,
      'ctrl+p is captured before the pty and before the keymap',
    )
    eq(g.dispatched.length, 0, 'and the file picker does not open')
    eq(g.walk.state().strokes.join(' '), 'ctrl+p', 'it was recorded')

    // A fumbled second chord replaces; it does not silently become a two-stroke sequence.
    g.gate.terminalHandler(event({ code: 'KeyO', key: 'o', ctrl: true, shift: true }))
    eq(g.walk.state().strokes.join(' '), 'ctrl+shift+o', 'a second press replaces the first')
  }
  {
    // …unless a second stroke was asked for, which is what makes `ctrl+k ctrl+s` recordable.
    const g = makeGate({}, recording())
    g.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true }))
    eq(g.gate.pending(), null, 'a recorded ctrl+k does not arm the gate’s own prefix machine')
    const extended = recorder.extend(g.walk.state())
    eq(extended.extending, true, 'the “+ second stroke” button opens the slot')
    eq(
      recorder.record(extended, 'ctrl+s').strokes.join(' '),
      'ctrl+k ctrl+s',
      'and the next stroke extends',
    )
  }
  {
    // Escape closes and is swallowed, so it never reaches a terminal as `^[`.
    const g = makeGate({ terminalFocused: true }, recording())
    eq(
      g.gate.terminalHandler(event({ code: 'Escape', key: 'Escape' })),
      false,
      'escape during a recording is swallowed',
    )
    eq(g.walk.state(), null, 'and closes the recorder')
    eq(g.saved, undefined, 'writing nothing')
  }
  {
    // Enter saves what is there; Enter with nothing recorded does nothing at all.
    const empty = makeGate({}, recording())
    eq(empty.gate.windowHandler(event({ code: 'Enter', key: 'Enter' })), false, 'enter is ours')
    ok(
      empty.walk.state() !== null,
      'and an empty recording is not saveable — the popup stays up rather than writing nothing',
    )
    eq(empty.walk.state()?.strokes.length ?? -1, 0, 'with nothing recorded')
    eq(empty.walk.saved(), null, 'so nothing was written')

    const full = makeGate({}, recording())
    full.gate.windowHandler(event({ code: 'KeyO', key: 'o', ctrl: true, shift: true }))
    full.gate.windowHandler(event({ code: 'Enter', key: 'Enter' }))
    eq(full.walk.saved(), 'ctrl+shift+o', 'enter saves the chord')
    eq(full.walk.state(), null, 'and closes')
  }
  {
    // The modified spellings stay recordable, which is what keeps the cost of the two above
    // bounded: only *bare* Enter and *bare* Escape are unreachable from this screen.
    for (const spec of [
      { code: 'Escape', key: 'Escape', shift: true },
      { code: 'Enter', key: 'Enter', ctrl: true },
      { code: 'Tab', key: 'Tab' },
      { code: 'Space', key: ' ' },
    ]) {
      const g = makeGate({ shellWindow: true }, recording())
      g.gate.windowHandler(event(spec))
      ok(
        g.walk.state() !== null && g.walk.state().strokes.length === 1,
        `${spec.code}${spec.shift === true ? '+shift' : ''}${spec.ctrl === true ? '+ctrl' : ''} is data, not a verb`,
      )
      eq(g.dispatched.length, 0, 'and ran no command')
    }
  }
  {
    /*
     * **An armed prefix outranks the recorder, which is why arming has to reset it.**
     *
     * The gate's documented order is `an armed prefix > the capture > the keymap`, and it is
     * right: a half-typed `ctrl+k` is a sequence the user has already started. The consequence
     * for this feature is the hole reproduced here — a `ctrl+k` typed *before* the click on
     * *Edit* is still armed when the popup opens, so the first stroke the user means to record
     * completes somebody else's sequence and runs a command instead.
     *
     * The gate is not the place to fix it (changing that order would strand a half-typed
     * sequence), so `startRecording` calls `currentKeyGate()?.reset()` and
     * `check-keymap.mjs` pins that line. This block is the proof that the line is load-bearing:
     * it arms a recorder the way a click would *without* the reset, and watches the stroke go
     * somewhere else.
     */
    const claim = recording(false)
    const g = makeGate({}, claim)
    eq(g.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true })), false, 'ctrl+k arms')
    eq(g.gate.pending(), 'ctrl+k', 'the prefix is live')
    claim.arm()
    eq(g.gate.windowHandler(event({ code: 'KeyS', key: 's', ctrl: true })), false, 'the second stroke completes')
    eq(g.dispatched[0]?.command, 'settings.keymap', 'and runs the sequence rather than being recorded')
    eq(claim.state().strokes.length, 0, 'the recorder never saw it — hence the reset on arm')

    // With the prefix cleared first, which is what the store actually does, the same stroke is
    // recorded and nothing runs.
    const clean = recording(false)
    const h = makeGate({}, clean)
    h.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true }))
    h.gate.reset()
    clean.arm()
    h.gate.windowHandler(event({ code: 'KeyS', key: 's', ctrl: true }))
    eq(h.dispatched.length, 0, 'nothing ran')
    eq(clean.state().strokes.join(' '), 'ctrl+s', 'and the stroke was recorded')
  }

  /* The specific things the capture must and must not do. */
  {
    // Tab, with the hold down, is the switcher's — swallowed, and the keymap never sees it,
    // so `project.switcher.next` does not run a second time.
    const g = makeGate({ terminalFocused: true }, walking(['p1', 'p2', 'p3'], CTRL))
    eq(
      g.gate.terminalHandler(event({ code: 'Tab', key: 'Tab', ctrl: true })),
      false,
      'ctrl+tab during a walk is swallowed before the PTY',
    )
    eq(g.dispatched.length, 0, 'and dispatches nothing — the capture outranks the keymap')
    eq(g.walk.state().index, 2, 'it advanced the walk instead')
    eq(
      g.gate.terminalHandler(event({ code: 'Tab', key: 'Tab', ctrl: true, shift: true })),
      false,
      'ctrl+shift+tab is swallowed too',
    )
    eq(g.walk.state().index, 1, 'and walks back')
  }
  {
    // Bare Tab, hold lost. Not the switcher's: the walk cancels and the stroke goes on to
    // whatever it was for, which is a tab character in a shell.
    const g = makeGate({}, walking(['p1', 'p2'], CTRL))
    eq(g.gate.windowHandler(event({ code: 'Tab', key: 'Tab' })), true, 'a bare Tab passes through')
    eq(g.walk.state(), null, 'and cancels the walk — this is the lost-keyup case')
  }
  {
    // Escape is ours, with or without the modifier, and must never reach a terminal as `^[`.
    for (const spec of [
      { code: 'Escape', key: 'Escape' },
      { code: 'Escape', key: 'Escape', ctrl: true },
    ]) {
      const g = makeGate({ terminalFocused: true }, walking(['p1', 'p2'], CTRL))
      eq(g.gate.terminalHandler(event(spec)), false, 'escape during a walk is swallowed')
      eq(g.walk.state(), null, 'and cancels')
      eq(g.dispatched.length, 0, 'and runs no command')
    }
  }
  {
    // An armed prefix outranks the capture. `ctrl+k` then `ctrl+tab` completes nothing, so it
    // is swallowed and disarms — and the walk is left exactly where it was.
    const g = makeGate({}, walking(['p1', 'p2', 'p3'], CTRL))
    eq(g.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true })), false, 'ctrl+k arms')
    eq(g.gate.windowHandler(event({ code: 'Tab', key: 'Tab', ctrl: true })), false, 'the second stroke is swallowed')
    eq(g.gate.pending(), null, 'the prefix disarmed')
    eq(g.walk.state().index, 1, 'and the capture never saw the stroke')
    eq(g.dispatched.length, 0, 'nothing ran')
  }
  {
    // A chord that is not the switcher's still works while the popup is up.
    const g = makeGate({}, walking(['p1', 'p2'], CTRL))
    eq(g.gate.windowHandler(event({ code: 'KeyP', key: 'p', ctrl: true })), false, 'ctrl+p resolves')
    eq(g.dispatched[0]?.command, 'picker.files', 'and runs its command')
    eq(g.walk.state().index, 1, 'leaving the walk open and where it was')
  }
  {
    /*
     * **No third entry point.** The release that commits a walk is watched by a modifier latch
     * in `keys/switcherStore.ts`, not by the gate — see that module's note. The gate's own
     * contract is unchanged and that is what this asserts: a keyup is still `PASS` on both
     * paths, with a walk open, whatever the capture would have said about the same stroke on
     * a keydown.
     */
    for (const entry of ['windowHandler', 'terminalHandler']) {
      const g = makeGate({}, walking(['p1', 'p2'], CTRL))
      for (const type of ['keyup', 'keypress']) {
        const ev = { ...event({ code: 'Tab', key: 'Tab', ctrl: true }), type }
        eq(g.gate[entry](ev), true, `${type} passes through ${entry} during a walk`)
      }
      eq(g.walk.state().index, 1, `and never reached the capture via ${entry}`)
      eq(g.dispatched.length, 0, `and dispatched nothing via ${entry}`)
    }
  }
  {
    /*
     * `currentStroke()` — the only thing a handler has that says a *key* ran it.
     *
     * `run` takes two strings, so a command cannot otherwise tell a keystroke from a command
     * palette click. The project switcher's whole shape turns on that difference: with a
     * stroke it opens a held-modifier walk and waits for the release, with `null` it switches
     * immediately. Stop publishing the stroke and Ctrl+Tab silently becomes a two-item toggle
     * — the exact behaviour the switcher replaced, with every other assertion in this file
     * still green. So it is pinned here, on the gate that publishes it.
     */
    const seenDuring = []
    const g = makeGate({})
    g.gate.windowHandler(event({ code: 'KeyP', key: 'p', ctrl: true }))
    eq(currentStroke(), null, 'no stroke is published outside a dispatch')

    // `shellWindow`, because the switcher bindings carry that clause since M14 and a gate whose
    // context does not satisfy it dispatches nothing at all — which would leave this block
    // asserting that an empty log equals an empty log.
    const publishing = createKeyGate({
      bindings: () => BINDINGS,
      context: () => ({ shellWindow: true }),
      run: () => seenDuring.push(currentStroke()),
    })
    publishing.windowHandler(event({ code: 'Tab', key: 'Tab', ctrl: true }))
    publishing.windowHandler(event({ code: 'Tab', key: 'Tab', ctrl: true, shift: true }))
    // `eq` here is `Object.is`, so the log is compared as a string rather than as an array.
    eq(seenDuring.join(','), 'ctrl+tab,ctrl+shift+tab', 'the dispatching stroke is what the handler sees')
    eq(currentStroke(), null, 'and it is withdrawn again the moment the handler returns')

    // A handler is allowed to throw; a stroke left published would make the *next* palette
    // invocation believe a modifier was being held.
    const throwing = createKeyGate({
      bindings: () => BINDINGS,
      context: () => ({ shellWindow: true }),
      run: () => {
        throw new Error('handler blew up')
      },
    })
    let threw = false
    try {
      throwing.windowHandler(event({ code: 'Tab', key: 'Tab', ctrl: true }))
    } catch {
      threw = true
    }
    ok(threw, 'a throwing handler still propagates')
    eq(currentStroke(), null, 'and leaves no stroke published behind it')
  }

  {
    // A gate with no capture at all behaves exactly as it did before this feature existed.
    const bare = makeGate({ shellWindow: true })
    eq(
      bare.gate.windowHandler(event({ code: 'Tab', key: 'Tab', ctrl: true })),
      false,
      'with no walk open, ctrl+tab is an ordinary binding',
    )
    eq(bare.dispatched[0]?.command, 'tab.switcher.next', 'and it opens the tab switcher')
  }

  /* ------------------------------------------------ the M14 chords, and what each one costs */

  {
    /*
     * **Ctrl+` is the project switcher, and Ctrl+Shift+` still splits a terminal.**
     *
     * The two are one Shift apart and mean unrelated things, which is exactly the pair a
     * resolver gets wrong by folding Shift away — and the failure is silent in the worst
     * direction: the switcher would split a terminal every time somebody walked backwards.
     * `Backquote` is already in the sweep above, so agreement between the entry points is
     * measured; this pins *which way* they agree.
     *
     * The `key: '~'` on the shifted press is not decoration. That is what a US layout actually
     * reports for Shift and this key, and reading `code` rather than `key` is the whole reason
     * the shipped `terminal.splitBelow` binding works at all.
     */
    const shell = makeGate({ shellWindow: true, terminalFocused: true })
    eq(
      shell.gate.terminalHandler(event({ code: 'Backquote', key: '`', ctrl: true })),
      false,
      'ctrl+` is claimed in a shell window, before the pty',
    )
    eq(shell.dispatched[0]?.command, 'project.switcher.next', 'and it opens the project switcher')

    const split = makeGate({ shellWindow: true, terminalFocused: true })
    split.gate.windowHandler(event({ code: 'Backquote', key: '~', ctrl: true, shift: true }))
    eq(split.dispatched[0]?.command, 'terminal.splitBelow', 'and one Shift away still splits')

    // A user's `keymap.json` may spell the same key as the literal tilde — which is what the
    // request for this chord said — and `ALIASES` folds it onto the same token the event
    // produces. Without that entry the binding would be inert with nothing reporting it.
    eq(normalizeSequence('ctrl+~'), 'ctrl+backquote', 'a written tilde folds onto the named key')
    eq(normalizeSequence('ctrl+tilde'), 'ctrl+backquote', 'as do the two spellings beside it')
    eq(normalizeSequence('ctrl+grave'), 'ctrl+backquote')
  }

  {
    /*
     * **A project walk is opened by Ctrl+`, so Ctrl+` is what walks it — including backwards.**
     *
     * This is the assertion the capture's parameterisation exists for. With the walk key
     * hard-coded to `'tab'`, a second Ctrl+` would fall through to the keymap (and work by
     * accident, as one more dispatch) while `ctrl+shift+` resolved to `terminal.splitBelow` —
     * so walking a project switcher backwards would split a terminal, mid-gesture, in the
     * window the user is looking at.
     */
    const g = makeGate(
      { shellWindow: true, terminalFocused: true },
      walking(['p1', 'p2', 'p3'], CTRL, 'backquote'),
    )
    eq(
      g.gate.terminalHandler(event({ code: 'Backquote', key: '`', ctrl: true })),
      false,
      'ctrl+` during a project walk is swallowed before the pty',
    )
    eq(g.walk.state().index, 2, 'and advances the walk')
    eq(
      g.gate.terminalHandler(event({ code: 'Backquote', key: '~', ctrl: true, shift: true })),
      false,
      'ctrl+shift+` is the switcher’s too, for as long as the popup is up',
    )
    eq(g.walk.state().index, 1, 'and walks back')
    eq(g.dispatched.length, 0, 'nothing was dispatched — no terminal was split')

    // …and Tab is not the project walk's key, so it goes to the keymap as usual.
    eq(
      g.gate.terminalHandler(event({ code: 'Tab', key: 'Tab', ctrl: true })),
      false,
      'ctrl+tab during a project walk resolves through the keymap',
    )
    eq(g.dispatched[0]?.command, 'tab.switcher.next', 'to the tab switcher')
  }

  {
    /*
     * **F4 toggles the panel in a shell window and reaches the pty everywhere else.**
     *
     * The clause is the whole of this binding's manners. xterm encodes F4 as `ESC O S`, and the
     * gate is a window capture listener, so an unscoped binding would take the key from `mc` and
     * `htop` in a torn-out terminal window that has no sidebar to toggle in the first place.
     *
     * Ctrl+1 is asserted in the same shape beside it, and F3 below it: F3 is find-next inside
     * CodeMirror and is bound by nothing, so "the neighbouring f-key still reaches the buffer"
     * is measured rather than assumed.
     */
    for (const [spec, command] of [
      [{ code: 'F4', key: 'F4' }, 'sidebar.toggle'],
      [{ code: 'Digit1', key: '1', ctrl: true }, 'tab.console'],
    ]) {
      const shell = makeGate({ shellWindow: true, terminalFocused: true })
      eq(
        shell.gate.terminalHandler(event(spec)),
        false,
        `${command} claims its chord in a shell window`,
      )
      eq(shell.dispatched[0]?.command, command, `and runs ${command}`)

      const detached = makeGate({ terminalFocused: true })
      const ev = event(spec)
      eq(
        detached.gate.terminalHandler(ev),
        true,
        `and reaches the pty in a window with no sidebar (${command})`,
      )
      eq(ev.prevented, 0, 'keeping its default action')
      eq(detached.dispatched.length, 0, 'and running nothing')
    }

    for (const ctx of CONTEXTS) {
      const w = makeGate(ctx)
      const t = makeGate(ctx)
      eq(w.gate.windowHandler(event({ code: 'F3', key: 'F3' })), true, 'F3 stays the find bar’s')
      eq(t.gate.terminalHandler(event({ code: 'F3', key: 'F3' })), true, 'at both entry points')
      eq(w.dispatched.length + t.dispatched.length, 0, 'and runs no command')
    }
  }

  /* ------------------------------------------------------------------ specific claims */

  // The M8 acceptance criterion: Ctrl+P with a terminal focused opens the picker and `^P`
  // never reaches the shell.
  {
    const t = makeGate({ terminalFocused: true })
    const through = t.gate.terminalHandler(event({ code: 'KeyP', key: 'p', ctrl: true }))
    eq(through, false, 'ctrl+p is swallowed before the PTY')
    eq(t.dispatched.length, 1, 'ctrl+p dispatched exactly one command')
    eq(t.dispatched[0]?.command, 'picker.files', 'ctrl+p ran picker.files')
  }

  /*
   * **Ctrl+C and Ctrl+V pass through the gate, on both paths, in every context.**
   *
   * They are the terminal's own — resolved in `src/terminal/keys.ts` from xterm's custom key
   * handler, which is the app's only focus-scoped keyboard entry point — and binding them here
   * instead is the obvious-looking way to implement copy and paste and is silently catastrophic:
   * the window listener runs in the capture phase, so the chord would be taken from the file
   * tree's own clipboard, from the commit box, from every rename field and from every text input
   * in the app, and Ctrl+C would stop being the interrupt in every pane. `cide_core::keymap`'s
   * `the_terminal_clipboard_chords_are_not_bound_here` says the same thing on the Rust side;
   * this is the half that also proves the *gate* does nothing with them.
   */
  /*
   * …and so do **Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z, Ctrl+U, Alt+U and F3**, for the same reason
   * spelled a different way.
   *
   * Those five are `@codemirror/commands`' `historyKeymap` and `@codemirror/search`'s find-next,
   * and they work in a buffer today *because* `cide_core::keymap` is silent about them. Binding
   * one here would not add a capability, it would remove several: the window listener runs in the
   * capture phase, so Ctrl+Z would stop reaching CodeMirror, the find field beside it, the commit
   * message box, every rename field in the file tree and every plain `<input>` in the app — all
   * of which have working native undo precisely because nothing intercepts the chord. And in a
   * terminal pane Ctrl+Z is **SIGTSTP**, the only keyboard way to suspend a foreground job.
   *
   * `cide_core::keymap`'s `the_editor_undo_chords_are_not_bound_here` and
   * `nothing_binds_the_find_bars_f_keys` say the same thing on the Rust side; this is the half
   * that also proves the *gate* does nothing with them, at both entry points, in every context.
   *
   * Ctrl+G is deliberately **not** in this list: it *is* bound, to `navigate.line`, and the
   * comment on that binding names what it costs (find-next moves to F3, which is why F3 is here).
   */
  {
    /*
     * `ctrl+f` joined this list in M19. It is the terminal's find bar, resolved in
     * `src/terminal/keys.ts` from xterm's own handler for exactly the reason ⌃C and ⌃V are —
     * and it is the one entry here that CodeMirror *also* wants (`Mod-f` opens the editor's
     * find bar, and reaches it only because no default claims the key). A `when: "terminalFocused"`
     * default would have been swallowed by the window capture entry in every rename field and
     * every text input in a window whose focused pane is a terminal;
     * `keymap::ctrl_f_is_not_bound_here_because_two_panes_mean_two_things_by_it` is the Rust end
     * of the same claim, and the sweep below is the half that measures it.
     */
    for (const key of ['ctrl+c', 'ctrl+v', 'ctrl+f', 'ctrl+z', 'ctrl+y', 'ctrl+shift+z', 'ctrl+u', 'alt+u', 'f3', 'shift+f3']) {
      ok(
        !rustDefaults().some((b) => b.key === key),
        `${key} is bound by no default — the terminal or CodeMirror owns it, focus-scoped`,
      )
    }
    for (const ctx of CONTEXTS) {
      for (const spec of [
        { code: 'KeyC', key: 'c', ctrl: true },
        { code: 'KeyV', key: 'v', ctrl: true },
        { code: 'KeyF', key: 'f', ctrl: true },
        { code: 'KeyZ', key: 'z', ctrl: true },
        { code: 'KeyY', key: 'y', ctrl: true },
        { code: 'KeyZ', key: 'z', ctrl: true, shift: true },
        { code: 'KeyU', key: 'u', ctrl: true },
        { code: 'KeyU', key: 'u', alt: true },
        { code: 'F3', key: 'F3' },
        { code: 'F3', key: 'F3', shift: true },
      ]) {
        const w = makeGate(ctx)
        const t = makeGate(ctx)
        const evW = event(spec)
        eq(w.gate.windowHandler(evW), true, `${spec.key} passes the window entry in ${JSON.stringify(ctx)}`)
        eq(evW.stopped, 0, 'and is not stopped before its target — a text input still gets it')
        eq(evW.prevented, 0, 'and keeps its default action, which is what a text input pastes from')
        eq(
          t.gate.terminalHandler(event(spec)),
          true,
          `${spec.key} reaches xterm in ${JSON.stringify(ctx)} — where ETX and the clipboard rule live`,
        )
        eq(w.dispatched.length + t.dispatched.length, 0, 'and runs no command on either path')
      }
    }
  }

  /*
   * **Ctrl+G opens Go to line in an editor and reaches the shell everywhere else.**
   *
   * The `when` is the whole of this binding's safety. `^G` is readline's `abort` and emacs'
   * universal cancel, so an unscoped `ctrl+g` would be swallowed by the window capture listener
   * in every terminal pane in every window — the same failure the `ctrl+b`/tmux comment in
   * `keymap.rs` describes, on a chord people press far more often. The sweep above already proves
   * the two entry points agree about it; this pins *which way* they agree in each context, which
   * a sweep for agreement alone cannot.
   */
  {
    const spec = { code: 'KeyG', key: 'g', ctrl: true }
    const editor = makeGate({ editorFocused: true })
    eq(editor.gate.windowHandler(event(spec)), false, 'ctrl+g is claimed with an editor focused')
    eq(editor.dispatched[0]?.command, 'navigate.line', 'and it opens Go to line')

    const term = makeGate({ terminalFocused: true })
    const ev = event(spec)
    eq(term.gate.terminalHandler(ev), true, 'ctrl+g reaches the pty with a terminal focused')
    eq(ev.prevented, 0, 'and keeps its default action — ^G is readline abort')
    eq(term.dispatched.length, 0, 'nothing ran')
  }

  /*
   * **⌥F7 finds usages in an editor and reaches the pty everywhere else.** (M14)
   *
   * Same shape as ⌃B and ⌃G above, and it earns its own block for the same reason theirs do: the
   * sweep proves the two entry points *agree*, and only this pins which way. F7 is a byte a
   * terminal application genuinely wants — `mc` puts its menu on the F keys — so an unscoped
   * binding would take it in every terminal pane in every window, and the failure would be
   * invisible to anyone not running one.
   *
   * The bare F7 assertion is the other half: nothing in the table binds it, so the gate must leave
   * find-next-style F-key traffic alone rather than folding F7 onto ⌥F7.
   */
  {
    const spec = { code: 'F7', key: 'F7', alt: true }
    const editor = makeGate({ editorFocused: true })
    eq(editor.gate.windowHandler(event(spec)), false, 'alt+f7 is claimed with an editor focused')
    eq(editor.dispatched[0]?.command, 'navigate.usages', 'and it runs Find usages')

    const term = makeGate({ terminalFocused: true })
    const ev = event(spec)
    eq(term.gate.terminalHandler(ev), true, 'alt+f7 reaches the pty with a terminal focused')
    eq(ev.prevented, 0, 'keeping its default action — ESC ESC [ 18 ~ is a key mc reads')
    eq(term.dispatched.length, 0, 'nothing ran')

    const bare = makeGate({ editorFocused: true })
    const plain = event({ code: 'F7', key: 'F7' })
    eq(bare.gate.windowHandler(plain), true, 'bare F7 is bound to nothing and passes through')
    eq(bare.dispatched.length, 0, 'and dispatches nothing')
  }

  // One event object seen by both entry points dispatches once, not twice.
  {
    const g = makeGate({})
    const ev = event({ code: 'KeyP', key: 'p', ctrl: true })
    const first = g.gate.windowHandler(ev)
    const second = g.gate.terminalHandler(ev)
    eq(first, second, 'a shared event decides the same way twice')
    eq(g.dispatched.length, 1, 'a shared event dispatches exactly once')
  }

  // keyup and keypress are not chords. xterm consults the custom handler for all three.
  {
    const g = makeGate({})
    for (const type of ['keyup', 'keypress']) {
      const ev = { ...event({ code: 'KeyP', key: 'p', ctrl: true }), type }
      eq(g.gate.terminalHandler(ev), true, `${type} passes through`)
    }
    eq(g.dispatched.length, 0, 'only keydown dispatches')
  }

  // A bare modifier is not a chord.
  {
    const g = makeGate({})
    eq(
      g.gate.windowHandler(event({ code: 'ControlLeft', key: 'Control', ctrl: true })),
      true,
      'a held Control passes through',
    )
    eq(g.gate.pending(), null, 'a held Control arms nothing')
  }

  // The prefix expires. A stale `ctrl+k` must not eat the next real keystroke.
  {
    const g = makeGate({})
    eq(g.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true })), false, 'ctrl+k arms')
    eq(g.gate.pending(), 'ctrl+k', 'ctrl+k is the pending prefix')
    g.advance(1000)
    eq(
      g.gate.windowHandler(event({ code: 'KeyA', key: 'a' })),
      true,
      'a plain letter after the timeout passes through',
    )
    eq(g.gate.pending(), null, 'the stale prefix was dropped')
    eq(g.dispatched.length, 0, 'nothing ran')
  }

  // `when` decides, on both paths, and unknown flags read false.
  {
    const withTerm = makeGate({ terminalFocused: true })
    const without = makeGate({})
    const spec = { code: 'KeyC', key: 'c', ctrl: true, shift: true }
    eq(withTerm.gate.windowHandler(event(spec)), false, 'terminal.clear binds when a terminal has focus')
    eq(withTerm.dispatched[0]?.command, 'terminal.clear', 'terminal.clear ran')
    eq(without.gate.windowHandler(event(spec)), true, 'and passes through otherwise')
    eq(without.dispatched.length, 0, 'nothing ran without the context')
  }
  {
    const editor = makeGate({ editorFocused: true })
    const overlay = makeGate({ editorFocused: true, overlayOpen: true })
    const spec = { code: 'KeyE', key: 'e', ctrl: true }
    eq(editor.gate.windowHandler(event(spec)), false, '`editorFocused && !overlayOpen` holds')
    eq(overlay.gate.windowHandler(event(spec)), true, 'and is false with the overlay open')
  }

  // Shift + backquote reports `~` as `key`. Reading `code` is what makes the binding work.
  {
    const g = makeGate({})
    const through = g.gate.terminalHandler(
      event({ code: 'Backquote', key: '~', ctrl: true, shift: true }),
    )
    eq(through, false, 'ctrl+shift+` resolves despite `key` reporting ~')
    eq(g.dispatched[0]?.command, 'terminal.splitBelow', 'ctrl+shift+` ran terminal.splitBelow')
  }

  /* ------------------------------------------------------------ spelling and normalisation */

  eq(strokeFromEvent(event({ code: 'KeyP', key: 'p', ctrl: true })), 'ctrl+p', 'stroke spelling')
  eq(
    strokeFromEvent(event({ code: 'KeyP', key: 'P', ctrl: true, shift: true, alt: true, meta: true })),
    'ctrl+alt+shift+meta+p',
    'modifier order matches cide-core::keymap',
  )
  eq(strokeFromEvent(event({ code: 'Comma', key: ',', ctrl: true })), 'ctrl+comma', 'comma is named')
  eq(strokeFromEvent(event({ code: 'ArrowRight', key: 'ArrowRight' })), 'right', 'arrows are named')

  eq(normalizeSequence('Shift+Ctrl+P'), 'ctrl+shift+p', 'normalisation orders modifiers')
  eq(normalizeSequence('ctrl+,'), 'ctrl+comma', 'a written comma folds onto the named key')
  eq(normalizeSequence('CTRL+K   ctrl+S'), 'ctrl+k ctrl+s', 'sequences collapse whitespace')
  eq(normalizeSequence('ctrl+`'), 'ctrl+backquote', 'a written backtick folds onto the named key')

  eq(chipLabel('ctrl+shift+p'), '⌃⇧P', 'chip for a letter chord')
  eq(chipLabel('ctrl+alt+right'), '⌃⌥→', 'chip for an arrow chord')
  eq(chipLabel('ctrl+k ctrl+s'), '⌃K ⌃S', 'chip for a sequence')
  eq(chipLabel('ctrl+comma'), '⌃,', 'chip for a named punctuation key')

  /* ---------------------------------------------------------------- when-clause grammar */

  eq(evaluateWhen(null, {}), true, 'no clause always holds')
  eq(evaluateWhen('  ', {}), true, 'an empty clause always holds')
  eq(evaluateWhen('a && b', { a: true, b: true }), true, 'conjunction')
  eq(evaluateWhen('a && b', { a: true }), false, 'conjunction with a missing flag')
  eq(evaluateWhen('a || b', { b: true }), true, 'disjunction')
  eq(evaluateWhen('!a', {}), true, 'negation of an unknown flag')
  eq(evaluateWhen('a && !b || c', { a: true, b: true }), false, '&& binds tighter than ||')
  eq(evaluateWhen('a && (!b || c)', { a: true, c: true, b: true }), true, 'parentheses')
  eq(evaluateWhen('a &&', { a: true }), false, 'a truncated clause never fires')
  eq(evaluateWhen('a == b', { a: true }), false, 'an unsupported operator never fires')

  /* ------------------------------------------------------------------ keymap resolution */

  {
    // Later bindings win: that is how a user override shadows a default.
    const km = buildKeymap([
      { key: 'ctrl+p', command: 'picker.files', when: null },
      { key: 'ctrl+p', command: 'terminal.paste', when: null },
    ])
    const r = km.resolve('ctrl+p', {})
    eq(r.kind, 'run', 'a shadowed key still resolves')
    eq(r.command, 'terminal.paste', 'the last applicable binding wins')
  }
  {
    // A `-command` removal directive is Rust's business and must never bind anything here.
    const km = buildKeymap([{ key: 'ctrl+p', command: '-picker.files', when: null }])
    eq(km.resolve('ctrl+p', {}).kind, 'unbound', 'a removal directive binds nothing')
  }
  {
    // A prefix whose only continuation is gated off must not swallow the stroke.
    const km = buildKeymap([
      { key: 'ctrl+k ctrl+s', command: 'settings.keymap', when: 'editorFocused' },
    ])
    eq(km.resolve('ctrl+k', {}).kind, 'unbound', 'an inapplicable prefix does not arm')
    eq(km.resolve('ctrl+k', { editorFocused: true }).kind, 'prefix', 'an applicable prefix arms')
  }
  {
    const km = buildKeymap(BINDINGS)
    eq(km.keyFor('settings.keymap'), 'ctrl+k ctrl+s', 'keyFor finds a sequence')
    eq(km.chipFor('picker.files'), '⌃P', 'chipFor renders the chip')
    eq(km.chipFor('nope.nothing'), null, 'chipFor is null for an unbound command')
    // Destructured, because a caller writing this must not get a `this` error.
    const { chipFor } = km
    eq(chipFor('palette.commands'), '⌃⇧P', 'chipFor works detached from its object')
  }

  /* -------------------------------------------------- every bound key reaches a dispatcher */

  /*
   * A binding whose command nothing dispatches is worse than no binding at all.
   *
   * `ctrl+s` was bound to `file.save` in `cide_core::keymap::defaults()` from the day the
   * table was written, and nothing in the frontend had a case for it. The gate did exactly
   * what it is supposed to do — resolved the chord, swallowed the keystroke, called the
   * dispatcher — and the dispatcher logged a line and returned. The effect was that Ctrl+S
   * did nothing *and* that CodeMirror's own `Mod-s`, which would have saved the file, never
   * saw the event. Every assertion above passed throughout: they check that a chord resolves
   * and dispatches, not that anything is listening.
   *
   * So this reads the two files that dispatch — `keys/dispatch.ts` and the app's fallback in
   * `App.tsx` — and asserts that every command in the shipped table reaches one of them.
   * Source text rather than an import because `App.tsx` cannot be compiled here (it is the
   * whole application) and because the question is genuinely "is this string written down as
   * a case", which is what a `switch` is.
   */
  {
    const readSource = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
    const dispatchers = readSource('../src/keys/dispatch.ts') + readSource('../src/App.tsx')
    const dispatched = new Set()
    for (const [, id] of dispatchers.matchAll(/case '([a-zA-Z][\w.]*)':/g)) dispatched.add(id)
    for (const [, id] of dispatchers.matchAll(/command === '([a-zA-Z][\w.]*)'/g)) dispatched.add(id)
    ok(dispatched.size >= 10, `found ${dispatched.size} dispatched commands — the scan still works`)

    /*
     * There is no `KNOWN_DEAD` list any more, and its absence is the result this round was
     * for.
     *
     * It held nine bindings — `tab.close` (the Ctrl+W the user reported), `theme.toggle`,
     * `settings.open`, `pane.detachToWindow`, `pane.promoteToTab` and the four
     * `pane.navigate.*` — each with a note saying it needed `App.tsx`. Every one of them is
     * now handled in `keys/dispatch.ts` off the workspace store, except `pane.promoteToTab`,
     * which has no domain operation behind it and is therefore `Command::unavailable` and
     * unbound rather than bound and dead.
     *
     * The stronger version of this assertion lives in `check-commands.mjs`, which compares
     * the switch against the *whole registry* rather than against the bound subset — a
     * palette row with no key was invisible to the loop below. This one stays because it is
     * the check a keymap change runs.
     */
    for (const { key, command } of rustDefaults()) {
      ok(
        dispatched.has(command),
        `${key} is bound to ${command} and something dispatches it ` +
          '(a bound key whose command nobody handles swallows the keystroke and does nothing)',
      )
    }

    /*
     * A `case` that forwards to `fallback` is not a dispatcher, and the scan above cannot
     * tell the difference.
     *
     * `file.save` shipped for review in exactly that shape: a case existed, so every
     * assertion above passed, and the body's first branch handed the command straight back
     * to `deps.fallback` because the host had not supplied a `focusedTab`. Ctrl+S still did
     * nothing; the only change was that the log line came from a different switch arm. The
     * assertion has to reach into the body or it certifies the bug it was written for.
     *
     * Comments are stripped first. The prose in this file argues about `fallback` at length
     * and a check that reads its own rationale as code would fail for the wrong reason.
     */
    const withoutComments = readSource('../src/keys/dispatch.ts')
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/\/\/[^\n]*/g, '')
    const marks = [...withoutComments.matchAll(/case '([a-zA-Z][\w.]*)':/g)]
    for (let i = 0; i < marks.length; i++) {
      const command = marks[i][1]
      const from = marks[i].index + marks[i][0].length
      const next = i + 1 < marks.length ? marks[i + 1].index : withoutComments.indexOf('default:', from)
      const body = withoutComments.slice(from, next < 0 ? withoutComments.length : next)
      ok(
        !body.includes('deps.fallback('),
        `dispatch.ts handles ${command} rather than forwarding it to fallback ` +
          '(a case whose body calls `fallback` is the dead command it was meant to fix)',
      )
    }

    /*
     * `file.saveAll` has no default binding, so the loop over `rustDefaults()` never looks at
     * it — and removing its case leaves every other assertion in this file green. It is in
     * `cide_core::commands::registry()`, which is what the palette lists, so it is reachable
     * and has to work.
     *
     * It used to be the *only* palette-only command anyone had written down, next to a note
     * that 23 of the 39 registry commands were undispatched and that building the list was
     * somebody else's job. `check-commands.mjs` builds it now, from the registry itself, and
     * fails on any gap; this line survives as the smoke test for that script's premise.
     */
    ok(dispatched.has('file.saveAll'), 'file.saveAll is dispatched (palette-only, no binding)')
  }

  /* ------------------------------------------------------------------------------------
   * M16: the chords `editorKeys.ts` re-homes must stay unbound in the app keymap.
   *
   * # The conflict neither side can see alone
   *
   * `defaultKeymap` binds `Alt-ArrowUp`/`Alt-ArrowDown` to move-line, and this app's keymap
   * takes `alt+up`/`alt+down` for `navigate.prevMember`/`navigate.nextMember`. The gate is a
   * window *capture* listener, so it resolves the stroke and swallows it **before** CodeMirror
   * is offered the event: move-line was simply gone from every buffer, in every window, with a
   * comment in `keymap.rs` claiming `EditorSurface` compensated for it and `grep moveLineUp
   * ui/src` returning nothing. That comment had been wrong since M12.
   *
   * `lineEditKeymap` now re-homes both to `Mod-Shift-Arrow` (and `Mod-Alt-Shift-Arrow` on
   * macOS). The compensation is only worth anything while those chords stay free, and **no
   * existing gate can see that**: `the_default_keymap_has_no_conflicts` sees one table,
   * `check-editor.mjs` sees the other, and neither knows the other exists. A `ctrl+shift+up`
   * added to `defaults()` next year deletes move-line a second time in exactly the way it was
   * deleted the first.
   *
   * **The bindings moved out of `EditorSurface.tsx` in M17** — into `editor/editorKeys.ts`,
   * beside Ctrl+D, because the diff pane and the merge pane install that array too and had
   * duplicate-line with no way to move a line. This block reads them from there now. Reading
   * the array rather than the surface is also the stricter choice: three surfaces mount it, so
   * a chord that goes free here goes free in all three at once.
   *
   * Both sides are folded through `normalizeSequence` rather than compared as literals. That is
   * not defensive: this side builds a stroke in its own modifier order (`alt+shift+meta+up`)
   * and a Rust author writes `meta+alt+shift+up`, so a literal `includes('Binding::new("…"')`
   * goes green against the very binding it forbids. It did, in the first draft.
   * ---------------------------------------------------------------------------------- */
  {
    const surfacePath = fileURLToPath(new URL('../src/editor/editorKeys.ts', import.meta.url))
    // Comments stripped, and load-bearing: the paragraph beside those two bindings names both
    // chords and both command names at length, so a grep over raw source finds the feature's
    // *explanation* after the feature has been deleted.
    const surface = readFileSync(surfacePath, 'utf8')
      .replace(/\/\*[\s\S]*?\*\//g, ' ')
      .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

    /**
     * CodeMirror's chord spelling as this app's stroke, for a given platform.
     *
     * `Mod` is Ctrl off macOS and Cmd on it — which is the entire reason the mac spelling is
     * different and the entire reason both have to be checked. `normalizeSequence` then orders
     * the modifiers and folds the key name, so the two sides are compared in one vocabulary.
     */
    const asStroke = (chord, mac) =>
      normalizeSequence(
        chord
          .split('-')
          .map((part) => {
            if (part === 'Mod') return mac ? 'meta' : 'ctrl'
            if (part === 'Cmd') return 'meta'
            if (/^Arrow/.test(part)) return part.slice(5).toLowerCase()
            return part.toLowerCase()
          })
          .join('+'),
      )

    /*
     * Every literal binding in the file, not only the two that carry a `mac:` spelling.
     *
     * The `mac:` group is optional because most of these are one chord on every platform —
     * IDEA's `Shift-Alt-Arrow` for move-line, `Mod-Alt-d` for duplicate-above, `Alt-j` for
     * select-next-occurrence — and every one of them is a chord the app keymap must leave alone
     * for the same reason the re-homed pair is: the gate is a window CAPTURE listener, so a
     * `defaults()` entry on any of them swallows the stroke before the editor is asked. When a
     * binding names no `mac:`, CodeMirror uses `key` on both platforms, so the mac stroke is the
     * same spelling with `Mod` resolved the other way — which is what `asStroke(key, true)` does.
     *
     * The tail is `[\s,}]` rather than `\s*\}` because `preventDefault: true` follows `run:` in
     * three of these. Requiring the brace meant the regex silently skipped exactly the bindings
     * that had an extra property, which is the quiet half of a check that cannot fail.
     */
    const rehomed = [
      ...surface.matchAll(
        /\{\s*key:\s*'([^']+)'\s*,\s*(?:mac:\s*'([^']+)'\s*,\s*)?run:\s*(\w+)\s*[,}]/g,
      ),
    ].map((m) => ({ key: m[1], mac: m[2] ?? m[1], run: m[3] }))

    ok(
      rehomed.length > 0,
      'editorKeys.ts re-homes at least one command taken by the app keymap. Every assertion ' +
        'below is vacuous without one, and the state this whole block exists for is exactly ' +
        'the state where the re-homing has been deleted and its paragraph left behind',
    )

    const boundIn = (bindings, stroke) => {
      const hit = bindings.find((b) => normalizeSequence(b.key) === stroke)
      return hit === undefined ? null : hit.command
    }
    const defaults = rustDefaults()

    for (const { key, mac, run } of rehomed) {
      const linux = asStroke(key, false)
      eq(
        boundIn(defaults, linux),
        null,
        `${linux} (${run}, re-homed from a chord the app keymap took) is unbound in ` +
          'cide_core::keymap::defaults() — bind it there and the capture listener eats the ' +
          'keystroke before CodeMirror is asked, which is how move-line disappeared the first ' +
          'time',
      )

      /*
       * The macOS spelling, and it has two ways of becoming bound rather than one.
       *
       * `platform_layer` rewrites **every** `ctrl+…` default to `meta+…` blanket, so a chord
       * bound off macOS reappears as its meta form on it. Checking only the explicit pushes
       * would miss that entirely — which is a whole class of collision that exists on exactly
       * one platform and on nobody's development machine.
       */
      const macStroke = asStroke(mac, true)
      const preimage = macStroke.replace(/\bmeta\b/, 'ctrl')
      eq(
        boundIn(defaults, normalizeSequence(preimage)),
        null,
        `nor does the macOS layer produce ${macStroke} by rewriting ${normalizeSequence(preimage)} — ` +
          'that rewrite is blanket, so a ctrl chord bound off macOS reappears as a meta chord on it',
      )
      eq(
        boundIn(rustPlatformPushes(), macStroke),
        null,
        `${macStroke} (${run}) is not pushed by platform_layer either`,
      )
    }
  }

  /* ------------------------------------------------------------------------------------
   * M16: ⌘[ and ⌘] reach the macOS layer.
   *
   * Added to the layer rather than produced by it, because the ctrl spelling cannot exist:
   * Ctrl+[ **is** the ESC byte on every terminal ever made, and this gate is a capture
   * listener, so `("ctrl+[", "navigate.back")` in `defaults()` would swallow Escape-equivalent
   * in every terminal pane in every window on Linux. A push is therefore the only shape
   * available, and a push is the shape a later tidy-up deletes.
   * ---------------------------------------------------------------------------------- */
  {
    const pushes = rustPlatformPushes()
    for (const [key, command] of [
      ['meta+bracketleft', 'navigate.back'],
      ['meta+bracketright', 'navigate.forward'],
    ]) {
      const hit = pushes.find((b) => normalizeSequence(b.key) === key)
      eq(
        hit?.command ?? null,
        command,
        `the macOS platform layer binds ${key} to ${command}`,
      )
      /*
       * A convention rather than a correctness requirement, and labelled as one.
       *
       * `keys/keymap.ts` folds every incoming binding through `normalizeSequence`, so a Rust
       * author who wrote `meta+[` would still be understood — the two sides would *not*
       * disagree. What a written bracket costs is findability: `grep bracketleft` over the
       * repository would then answer for one side only, which is how the two spellings drift
       * into meaning different things to a reader.
       */
      eq(
        normalizeSequence(hit?.key ?? key),
        hit?.key ?? key,
        `and spells it in the normalised vocabulary (${key}), so one grep answers for both ` +
          'sides. Not a correctness requirement — `keys/keymap.ts` folds every binding it is ' +
          'handed — a findability one',
      )
    }
  }

  if (failed > 0) {
    console.error(`\ncheck-key-gate: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-key-gate: ok (${compared + capturedCompared + recordedCompared + nonLatinCompared} ` +
      `chords through the keyboard entry points, ${capturedCompared} of them with a switcher ` +
      `walk open, ${recordedCompared} with the keystroke recorder armed and ${nonLatinCompared} ` +
      `under a non-Latin layout; ${mouseSwept} thumb-button presses through the third)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
