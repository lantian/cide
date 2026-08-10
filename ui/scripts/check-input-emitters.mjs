/**
 * Checks `src/terminal/inputRouting.ts` against a model of every emitter xterm 6 has on the
 * typing path.
 *
 * # Why this is a check script and not a real test of the terminal
 *
 * The emitters that have the bug live in xterm's `CoreBrowserTerminal` and
 * `CompositionHelper`. Neither is reachable: they need a real textarea, xterm's DI container
 * and a live `IRenderService`, `@xterm/xterm` exports no handle to either, and this project
 * has no DOM test environment at all — every one of the `check:*` scripts is plain
 * `node scripts/*.mjs`. So the shape of the fix is what makes it testable: the decision is a
 * pure module, and `Xterm` below is a transcription of the five emitters it has to survive,
 * with the line numbers it was transcribed from. Every branch modelled here was read out of
 * `node_modules/@xterm/xterm/lib/xterm.mjs` — the bundle Vite actually loads — not remembered.
 *
 * # The property
 *
 * **n printable characters typed produce exactly n writes to the child, each being the
 * character typed, regardless of what the textarea held beforehand.**
 *
 * The last clause is the one two previous fixes missed. Nothing in xterm empties that
 * textarea during ordinary typing — the only writers of `''` are `_handleTextAreaBlur`
 * (line 292), `_keyDown` for Ctrl+C and Enter only (line 1087) and `Clipboard.paste`
 * (line 55) — so it accumulates for the life of the pane, and two of the five emitters
 * measure their payload against a base captured while it was accumulating. Every scenario
 * below therefore runs three times: from an empty textarea, from one holding the reported
 * `\pwd`, and from one holding forty characters of something else.
 *
 * # The reproduction
 *
 * The user typed `pwd` into a pane whose textarea still held `\pwd` from earlier, and the
 * child received `\pwdp\pwdpw\pwdpwd`. `legacyGuard` below is the routing layer that
 * shipped when that happened, transcribed from it; the test asserts it reproduces that exact
 * string, and that the current code turns the same event sequence into `pwd`. So this file
 * fails against the old code and passes against the new one, with no git archaeology needed.
 *
 * Run: `pnpm --dir ui run check:input`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-input-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

const DEL = '\x7f'
const ETX = '\x03'
const CR = '\r'

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/inputRouting.ts',
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
    { stdio: 'inherit' },
  )

  const { InputGuard, imeFiltered, insertedText } = await import(
    `file://${join(out, 'inputRouting.js')}`
  )

  // === the model =========================================================================
  //
  // One xterm terminal, one hidden textarea, one queue of `setTimeout(0)` callbacks, and a
  // list of everything `coreService.triggerDataEvent` has handed to `onData` — which is what
  // `TerminalPane` forwards to the PTY, so it is literally what the child receives.

  const NOTHING = { stop: false, emit: null, clear: false, defer: false }

  /** A guard that does nothing at all: xterm on its own, with no routing layer. */
  const inertGuard = () => ({
    keydown: () => NOTHING,
    input: () => NOTHING,
    compositionstart: () => NOTHING,
    compositionend: () => NOTHING,
    commitSettled: () => NOTHING,
  })

  /**
   * The routing layer that was shipped when the user reported `\pwdp\pwdpw\pwdpwd`.
   *
   * Transcribed from it: a claim per printable keydown, redeemed by a later `insertText`;
   * `insertText` decisions stop the event and empty the textarea, everything else is
   * *deferred* and leaves it alone. The defer path is the one an input method's commit takes,
   * which is why the textarea in that report had been accumulating since the pane opened.
   */
  const legacyGuard = () => {
    let claims = 0
    return {
      keydown: (k) => {
        const text = !imeFiltered(k) && k.key.length === 1 && !k.ctrlKey && !k.altKey && !k.metaKey
        if (text) claims += 1
        return NOTHING
      },
      input: (ev) => {
        if (ev.inputType !== 'insertText' || ev.isComposing || !ev.data) return NOTHING
        if (claims > 0) {
          claims -= 1
          return { stop: true, emit: null, clear: true, defer: false }
        }
        return { stop: true, emit: ev.data, clear: true, defer: false }
      },
      compositionstart: () => NOTHING,
      compositionend: () => NOTHING,
      commitSettled: () => NOTHING,
    }
  }

  class Xterm {
    constructor(guard, stale = '', { bail = true, muteKeypress = true } = {}) {
      this.guard = guard
      /** Whether `xterm.ts` returns false for an IME-filtered keydown. False = round 1. */
      this.bail = bail
      /**
       * Whether `xterm.ts` returns false for `keypress`, disarming E5.
       *
       * False models the code without that bail, which is what makes the capital-letter
       * assertion below a real test rather than a description of the fix.
       */
      this.muteKeypress = muteKeypress
      /** `_keyDownHandled`, line 1020. Only ever set with `screenReaderMode` on. */
      this.keyDownHandled = false
      /** `textarea.value`. */
      this.ta = stale
      /** Everything that reached `onData`, in order. */
      this.writes = []
      /** Pending `setTimeout(…, 0)` callbacks, FIFO — the only scheduling xterm uses here. */
      this.timers = []
      /** `_keyDownSeen`, line 106. Set in `_keyDown` (1023), cleared in `_keyUp` (1120). */
      this.keyDownSeen = false
      /** `CompositionHelper` state: lines 25-56. `_compositionPosition.start` starts at 0. */
      this.composing = false
      this.compStart = 0
      this.compEnd = 0
      this.sending = false
      this.dataAlreadySent = ''
    }

    // --- what reaches the child ---------------------------------------------------------
    /** `coreService.triggerDataEvent` — the single funnel every emitter goes through. */
    emit(text) {
      this.writes.push(text)
    }

    /** Apply what the guard asked for. Mirrors `apply()` in `inputHost.ts` exactly. */
    act(reaction) {
      if (reaction.clear) this.ta = ''
      if (reaction.emit !== null) this.emit(reaction.emit)
      if (reaction.defer) this.timers.push(() => this.act(this.guard.commitSettled()))
      return reaction.stop
    }

    flush() {
      for (let i = 0; i < 64 && this.timers.length > 0; i++) {
        const run = this.timers.shift()
        run()
      }
    }

    // --- DOM events ----------------------------------------------------------------------
    /**
     * A keydown. Returns true when xterm cancelled it, i.e. when no `input` event can follow.
     *
     * Order: the guard's listener is capture-phase on an ancestor of the textarea, so it runs
     * before xterm's own (registered on the textarea, line 379).
     */
    keydown(k) {
      this.act(this.guard.keydown(k))

      // `_keyDown`, line 1021.
      this.keyDownSeen = true
      // `attachCustomKeyEventHandler` in `xterm.ts`: an IME-filtered key returns false, which
      // stops `_keyDown` at line 1026 — before `_compositionHelper.keydown` at line 1032.
      if (this.bail && imeFiltered(k)) return false
      // `CompositionHelper.keydown`, line 110: a keyCode-229 key that got this far arms
      // `_handleAnyTextareaChanges`. Reachable only with the bail above removed, which is
      // what round 1 of this bug was.
      if (imeFiltered(k)) {
        if (k.keyCode === 229) this.handleAnyTextareaChanges()
        return false
      }
      // `evaluateKeyboardEvent` → line 1092, then `cancel(event, true)` at line 1097.
      const key = encode(k)
      if (key === null) return false
      /*
       * Line 1075, and the emitter every previous reading of this file missed: an unmodified
       * capital letter returns `true` here — no `triggerDataEvent`, and no `cancel`, so the
       * event's default action stands. `_keyPress` is what writes those, and behind it the
       * browser inserts the character into the textarea. Without this branch the model shows
       * `_keyDown` writing and cancelling `P`, which is not what the shipped bundle does.
       */
      if (isCapital(k)) return false
      // Line 1087: Ctrl+C and Enter, and nothing else, empty the textarea.
      if (key === ETX || key === CR) this.ta = ''
      this.emit(key)
      return true
    }

    /**
     * `_keyPress`, line 1157 — E5. Reached only for a keydown that neither wrote nor
     * cancelled, which after the branch above means the capital letters.
     *
     * `this.cancel(e)` at line 1163 is *not* forced and `cancelEvents` defaults to false, so
     * it does nothing: the insertion goes ahead and an `input` event follows this write.
     */
    keypress(k) {
      // `attachCustomKeyEventHandler` in `xterm.ts` returns false for `keypress`, which stops
      // `_keyPress` at line 1159, before the write.
      if (this.muteKeypress) return
      if (this.keyDownHandled) return
      this.emit(k.key)
    }

    keyup() {
      this.keyDownSeen = false
    }

    /**
     * The browser or the input method appends to the textarea at the caret, then fires
     * `input`. `event: false` is the input method writing with no `input` event at all.
     */
    insert(text, { inputType = 'insertText', isComposing = false, event = true, data } = {}) {
      this.ta += text
      if (!event) return
      const facts = { inputType, data: data === undefined ? text : data, isComposing }
      const stopped = this.act(this.guard.input(facts))
      if (stopped) return
      // `_inputEvent`, line 1196. `ev.composed` is always true for a real input event, and
      // `screenReaderMode` is off, so the guard collapses to `!this.keyDownSeen`.
      if (facts.data && facts.inputType === 'insertText' && !this.keyDownSeen) {
        this.emit(facts.data)
      }
    }

    /**
     * The browser replaces the live preedit — the run from `compositionstart`'s caret to the
     * end of the textarea — with `text`, and fires `input` with `insertCompositionText`.
     * `isComposing` is true for a preedit update and false for the final commit, which is the
     * only difference between the two.
     */
    preedit(text, isComposing = true) {
      this.ta = this.ta.slice(0, this.compStart) + text
      // Whether the guard stops it or not makes no difference here: `_inputEvent` (line 1196)
      // requires `insertText`, so it never fires for composition text.
      this.act(this.guard.input({ inputType: 'insertCompositionText', data: text, isComposing }))
    }

    /** Guard first: its listener is capture-phase, xterm's is on the target (line 381). */
    compositionstart() {
      this.act(this.guard.compositionstart())
      this.composing = true
      this.compStart = this.ta.length
      this.dataAlreadySent = ''
    }

    /**
     * xterm first: `_finalizeComposition` is on the target (line 383) and the guard's listener
     * is bubble-phase, so the clear it schedules is queued behind the read below.
     */
    compositionend() {
      // `_finalizeComposition(true)`, lines 128-178.
      this.composing = false
      const at = { start: this.compStart, end: this.compEnd }
      this.sending = true
      this.timers.push(() => {
        if (!this.sending) return
        this.sending = false
        at.start += this.dataAlreadySent.length // line 161
        const text = this.composing
          ? this.ta.substring(at.start, this.compStart)
          : this.ta.substring(at.start)
        if (text.length > 0) this.emit(text)
      })
      this.act(this.guard.compositionend())
    }

    /** `_handleAnyTextareaChanges`, lines 186-207. */
    handleAnyTextareaChanges() {
      const oldValue = this.ta
      this.timers.push(() => {
        if (this.composing) return
        const newValue = this.ta
        // The whole-buffer emit: `String.replace` with a string pattern that is not a
        // substring returns `newValue` untouched.
        const diff = newValue.replace(oldValue, '')
        this.dataAlreadySent = diff
        if (newValue.length > oldValue.length) this.emit(diff)
        else if (newValue.length < oldValue.length) this.emit(DEL)
        else if (newValue !== oldValue) this.emit(newValue)
      })
    }

    /** The `paste` event: `handlePasteEvent` → `paste`, lines 43-56. */
    paste(text) {
      this.emit(text) // bracketed by `bracketTextForPaste`; not modelled, it is opaque here
      this.ta = ''
      // `handlePasteEvent` calls `stopPropagation` but NOT `preventDefault`, so the browser
      // goes ahead and inserts the pasted text into the textarea behind the write above.
      this.insert(text, { inputType: 'insertFromPaste', data: null })
    }
  }

  /** `evaluateKeyboardEvent`, reduced to the keys these scenarios use. */
  function encode(k) {
    if (k.key === 'Enter') return CR
    if (k.key === 'Backspace') return DEL
    if (k.key.length === 1 && k.ctrlKey) {
      const c = k.key.toUpperCase().charCodeAt(0)
      return c >= 64 && c <= 95 ? String.fromCharCode(c - 64) : null
    }
    if (k.key.length === 1 && !k.altKey && !k.metaKey) return k.key
    return null
  }

  /**
   * `_keyDown`'s early `return true`, line 1075: `ev.key.length === 1` and charCode 65-90
   * with no ctrl/alt/meta — i.e. `A`-`Z`, a shifted letter — is left for `_keyPress`.
   */
  function isCapital(k) {
    if (k.ctrlKey || k.altKey || k.metaKey || k.key.length !== 1) return false
    const c = k.key.charCodeAt(0)
    return c >= 65 && c <= 90
  }

  const key = (k, extra = {}) => ({
    key: k,
    keyCode: k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    ...extra,
  })
  /** A key an input method has taken: WebKitGTK reports keyCode 229 and `key: 'Process'`. */
  const imeKey = () => key('Process', { keyCode: 229 })

  // === the shapes an input method's delivery can take =====================================
  //
  // Which of these a machine produces depends on the engine, the toolkit and the phase of the
  // moon, and the two previous fixes each assumed one of them. The property has to hold for
  // all of them, so every scenario is a function of one character and the terminal.

  const shapes = {
    /** Commit arrives as `input` while the key is still down. */
    'insertText, key down': (t, c) => {
      t.keydown(imeKey())
      t.insert(c)
      t.keyup()
      t.compositionend()
    },
    /** The same, with the commit landing after keyup — this is what arms `_inputEvent`. */
    'insertText, after keyup': (t, c) => {
      t.keydown(imeKey())
      t.keyup()
      t.insert(c)
      t.compositionend()
    },
    /** Commit as composition text, no `compositionstart` — the reported case. */
    'insertCompositionText, no compositionstart': (t, c) => {
      t.keydown(imeKey())
      t.insert(c, { inputType: 'insertCompositionText', isComposing: true })
      t.keyup()
      t.compositionend()
    },
    /** `compositionend` before the textarea is updated and before the `input` event. */
    'compositionend first': (t, c) => {
      t.keydown(imeKey())
      t.compositionend()
      t.insert(c, { inputType: 'insertCompositionText' })
      t.keyup()
    },
    /** No `input` event at all: the textarea changes and only `compositionend` announces it. */
    'compositionend only': (t, c) => {
      t.keydown(imeKey())
      t.insert(c, { event: false })
      t.keyup()
      t.compositionend()
    },
    /** No composition events at all: a bare commit into the textarea. */
    'bare insertText': (t, c) => {
      t.keydown(imeKey())
      t.insert(c)
      t.keyup()
    },
    /**
     * No input method at all.
     *
     * A lowercase letter is encoded by `_keyDown`, which then cancels the event, so nothing
     * else in the chain ever sees it. A *capital* letter is not: `_keyDown` returns early
     * without writing and without cancelling, `_keyPress` is offered it, and the browser's
     * own insertion raises an `input` event behind that. Both halves are modelled, because
     * the difference between them is where E5 hides.
     */
    'no input method': (t, c) => {
      const k = key(c)
      const cancelled = t.keydown(k)
      if (!cancelled) {
        t.keypress(k)
        // The default action nobody prevented.
        t.insert(c)
      }
      t.keyup()
    },
  }

  const type = (make, text, stale, shape) => {
    const t = new Xterm(make(), stale)
    for (const c of text) {
      shape(t, c)
      t.flush()
    }
    t.flush()
    return t
  }

  // === the user's reproduction ============================================================
  //
  // Round 3, verbatim: a textarea still holding `\pwd` from earlier in the session (the stray
  // backslash plus the previous round's `pwd`; backspace sent DEL at the child and never
  // touched the textarea), then `pwd` typed into it.
  const STALE = '\\pwd'
  const REPRO = shapes['insertCompositionText, no compositionstart']

  const broken = type(legacyGuard, 'pwd', STALE, REPRO)
  eq(
    broken.writes.join(''),
    '\\pwdp\\pwdpw\\pwdpwd',
    'the shipped router reproduces the reported string exactly',
  )
  eq(
    broken.writes,
    ['\\pwdp', '\\pwdpw', '\\pwdpwd'],
    'as three emissions, one per keystroke, each the whole textarea',
  )
  eq(
    type(legacyGuard, 'pwd', '', REPRO).writes.join(''),
    'ppwpwd',
    'and with an empty textarea it reproduces round 2, which is the same bug',
  )

  const fixed = type(() => new InputGuard(), 'pwd', STALE, REPRO)
  eq(fixed.writes.join(''), 'pwd', 'the fix writes exactly what was typed')
  eq(fixed.writes, ['p', 'w', 'd'], 'as one write per keystroke')

  // Round 1, before any routing layer existed: `_handleAnyTextareaChanges` was still armed
  // (nothing returned false for keyCode 229 yet), so it raced `_inputEvent`. The exact
  // chunking the user saw — `pwwdwd` — depends on which timer won each keystroke, which is
  // why the report said "no fixed pattern"; what is stable is that three keystrokes produce
  // more than three writes.
  const round1 = new Xterm(inertGuard(), '', { bail: false })
  // `p`: the commit lands while the key is still down, so `_inputEvent` is suppressed and the
  // diff is the only emitter — one write, correct.
  round1.keydown(imeKey())
  round1.insert('p')
  round1.keyup()
  round1.flush()
  // `w`: the commit lands after keyup, so `_inputEvent` fires *as well as* the diff.
  round1.keydown(imeKey())
  round1.keyup()
  round1.insert('w')
  round1.flush()
  eq(round1.writes.join(''), 'pww', 'without the bail, a late commit is delivered twice')
  ok(round1.writes.length > 2, 'two keystrokes, three writes — round 1, in one keystroke')
  // Which emitter wins is decided per keystroke by whether the commit beat `keyup`, and that
  // is why the reported string (`pwwdwd`) was ragged rather than a clean doubling.
  eq(
    new Xterm(inertGuard(), '', { bail: false }).keydown(imeKey()),
    false,
    'the 229 keydown itself never writes, either way',
  )

  // === the property, over every delivery shape and both textarea states ====================
  for (const [name, shape] of Object.entries(shapes)) {
    for (const stale of ['', STALE, 'x'.repeat(40)]) {
      const t = type(() => new InputGuard(), 'pwd', stale, shape)
      eq(t.writes.join(''), 'pwd', `${name}: three keystrokes write "pwd" (stale ${stale.length})`)
      eq(t.writes.length, 3, `${name}: three keystrokes write three times (stale ${stale.length})`)
      eq(t.ta, '', `${name}: and leave nothing in the textarea (stale ${stale.length})`)
    }
  }

  // The same, with no input method at all and a long line — the overwhelmingly common case,
  // and the one a fix aimed at IMEs is most likely to break.
  const plain = type(() => new InputGuard(), 'echo hello world', STALE, shapes['no input method'])
  eq(plain.writes.join(''), 'echo hello world', 'ordinary typing is untouched')

  // === E5, the capital letters =============================================================
  //
  // The case that has no input method in it at all, and therefore the one nobody looking for
  // an IME bug thinks to type. `_keyDown` hands an unmodified `A`-`Z` to `_keyPress` (it
  // returns true at line 1075 without writing and without cancelling), `_keyPress` writes it
  // and does *not* prevent the default, so the character is also inserted into the textarea
  // and arrives at `InputGuard.input` as a perfectly ordinary `insertText`.
  const shouted = type(() => new InputGuard(), 'Hello World', STALE, shapes['no input method'])
  eq(shouted.writes.join(''), 'Hello World', 'a capital letter is written once, not twice')
  eq(shouted.writes.length, 'Hello World'.length, 'one write per key, shifted or not')

  // The same run with `xterm.ts`'s keypress bail removed. This is the assertion that makes the
  // one above a test: without it `_keyPress` writes as well and every capital is doubled.
  const unmuted = new Xterm(new InputGuard(), STALE, { muteKeypress: false })
  for (const c of 'Hi') {
    shapes['no input method'](unmuted, c)
    unmuted.flush()
  }
  eq(unmuted.writes.join(''), 'HHi', 'without the keypress bail, `_keyPress` doubles capitals')

  // And a capital is genuinely delivered by the textarea path, not swallowed with it — the
  // failure a `preventDefault` on keypress would have caused instead of the doubling.
  const oneCap = type(() => new InputGuard(), 'X', '', shapes['no input method'])
  eq(oneCap.writes, ['X'], 'and muting `_keyPress` does not drop the character')

  // === a real composition =================================================================
  //
  // Three keystrokes of preedit and one commit must deliver the committed text once. The
  // textarea is the only place that text exists, so this is the case a fix that empties the
  // textarea too eagerly destroys.
  const cjk = new Xterm(new InputGuard(), STALE)
  cjk.keydown(imeKey())
  cjk.compositionstart()
  cjk.preedit('n')
  cjk.keyup()
  cjk.keydown(imeKey())
  cjk.preedit('ni')
  cjk.keyup()
  cjk.keydown(imeKey())
  cjk.compositionend()
  cjk.preedit('你', false)
  cjk.keyup()
  cjk.flush()
  eq(cjk.writes, ['你'], 'a composition delivers its committed text exactly once')
  eq(cjk.ta, '', 'and leaves the textarea empty for the next keystroke')

  // The same composition with the trailing `input` event arriving *before* `compositionend`,
  // which is the other order browsers use. `_finalizeComposition` is the emitter here rather
  // than the input event, and the count must not change.
  const cjk2 = new Xterm(new InputGuard(), STALE)
  cjk2.keydown(imeKey())
  cjk2.compositionstart()
  cjk2.preedit('ni')
  cjk2.preedit('你')
  cjk2.compositionend()
  cjk2.keyup()
  cjk2.flush()
  eq(cjk2.writes, ['你'], 'the other compositionend ordering also delivers exactly once')

  // Two compositions back to back, with no plain keystroke between them to tidy up.
  const cjk3 = new Xterm(new InputGuard(), STALE)
  for (const word of ['你', '好']) {
    cjk3.keydown(imeKey())
    cjk3.compositionstart()
    cjk3.preedit('ni')
    cjk3.compositionend()
    cjk3.preedit(word, false)
    cjk3.keyup()
    cjk3.flush()
  }
  eq(cjk3.writes, ['你', '好'], 'consecutive compositions do not accumulate')

  // Consecutive compositions with the second one *starting inside* the first one's commit
  // window — the settle macrotask has not run yet. Emptying the textarea there would be
  // emptying it behind a `compositionstart` that has already measured its base against it,
  // and the second commit would come out truncated.
  const cjk4 = new Xterm(new InputGuard(), STALE)
  cjk4.keydown(imeKey())
  cjk4.compositionstart()
  cjk4.preedit('ni')
  cjk4.compositionend()
  cjk4.preedit('你', false)
  // The next composition opens before the deferred clear runs.
  cjk4.keydown(imeKey())
  cjk4.compositionstart()
  cjk4.flush()
  cjk4.preedit('hao')
  cjk4.compositionend()
  cjk4.preedit('好', false)
  cjk4.keyup()
  cjk4.flush()
  eq(cjk4.writes, ['你', '好'], 'a commit window overlapping the next composition writes both')

  // An aborted composition: the browser takes the preedit back out of the textarea and
  // `compositionend` commits nothing. It must write nothing and must not leave the guard
  // swallowing the next keystroke.
  const aborted = new Xterm(new InputGuard(), STALE)
  aborted.keydown(imeKey())
  aborted.compositionstart()
  aborted.preedit('n')
  aborted.preedit('', false)
  aborted.compositionend()
  aborted.flush()
  eq(aborted.writes, [], 'an abandoned composition writes nothing')
  aborted.keydown(key('a'))
  aborted.keyup()
  aborted.flush()
  eq(aborted.writes, ['a'], 'and the next keystroke goes through')

  // === the rest of the keyboard ============================================================

  // Backspace. It reaches the child as DEL from `_keyDown`, which cancels the event, so the
  // textarea is never edited — and against the empty textarea this module maintains there is
  // nothing to delete, so `deleteContentBackward` cannot arise at all. That is what makes
  // `_handleAnyTextareaChanges`' phantom-DEL branch unreachable.
  const back = new Xterm(new InputGuard(), STALE)
  back.keydown(key('Backspace'))
  back.keyup()
  back.flush()
  eq(back.writes, [DEL], 'backspace sends one DEL')
  eq(back.ta, '', 'and the textarea it does not edit is empty anyway')

  // Enter and Ctrl+C: encoded at keydown, textarea emptied by xterm itself (line 1087).
  const enter = new Xterm(new InputGuard(), STALE)
  enter.keydown(key('Enter'))
  enter.keyup()
  enter.flush()
  eq(enter.writes, [CR], 'Enter sends one CR')

  const ctrlC = new Xterm(new InputGuard(), STALE)
  ctrlC.keydown(key('c', { ctrlKey: true }))
  ctrlC.keyup()
  ctrlC.flush()
  eq(ctrlC.writes, [ETX], 'Ctrl+C sends one ETX')

  // Paste. xterm's own handler writes it (bracketed) and empties the textarea, but it only
  // calls `stopPropagation`, so the browser still inserts the text and an `insertFromPaste`
  // arrives afterwards. Writing that would paste twice; ignoring it without clearing is how
  // the textarea came to hold `\pwd` in the first place.
  const pasted = new Xterm(new InputGuard(), STALE)
  pasted.paste('ls -la\rwhoami')
  pasted.flush()
  eq(pasted.writes, ['ls -la\rwhoami'], 'a paste is written once, by xterm, and not repeated')
  eq(pasted.ta, '', 'and the pasted text does not stay in the textarea')

  // And the keystroke after a paste is still one character, which is the regression the
  // residue caused: the paste sat in the textarea and every later emitter swept it up.
  pasted.keydown(imeKey())
  pasted.insert('x')
  pasted.keyup()
  pasted.compositionend()
  pasted.flush()
  eq(pasted.writes, ['ls -la\rwhoami', 'x'], 'and the next keystroke is one character')

  // A multi-character insertion that is *not* a paste — an IME committing a whole word, or a
  // dictation engine. There is no other emitter behind it, so this path is its only delivery.
  const word = new Xterm(new InputGuard(), STALE)
  word.keydown(imeKey())
  word.insert('hello')
  word.keyup()
  word.flush()
  eq(word.writes, ['hello'], 'a multi-character commit is written once, whole')

  // An insertion with no keystroke behind it at all: an emoji picker, a candidate window
  // click. Nothing else will ever deliver it.
  const emoji = new Xterm(new InputGuard(), STALE)
  emoji.insert('🙂')
  emoji.flush()
  eq(emoji.writes, ['🙂'], 'an insertion nobody typed still reaches the child')

  // === the pure predicates =================================================================
  ok(imeFiltered(key('a', { keyCode: 229 })), 'keyCode 229 is the input method placeholder')
  ok(imeFiltered(key('Process')), "so is key === 'Process'")
  ok(imeFiltered(key('a', { isComposing: true })), 'and so is a keydown inside a composition')
  ok(!imeFiltered(key('a')), 'an ordinary letter is not filtered')

  const facts = (inputType, data, isComposing = false) => ({ inputType, data, isComposing })
  eq(insertedText(facts('insertText', 'p')), 'p', 'an insertion carries its text')
  eq(insertedText(facts('insertText', '')), null, 'an empty insertion carries nothing')
  eq(insertedText(facts('insertText', null)), null, 'and neither does a null one')
  eq(insertedText(facts('deleteContentBackward', null)), null, 'a deletion is not an insertion')
  eq(insertedText(facts('historyUndo', 'x')), null, 'nor is an undo')
  eq(
    insertedText(facts('insertFromPaste', 'x')),
    null,
    'paste has its own handler in xterm and must not be written twice',
  )
  eq(insertedText(facts('insertReplacementText', 'ü')), 'ü', 'a replacement is a real insertion')

  // === the guard's own state ===============================================================
  const guard = new InputGuard()
  eq(guard.keydown(key('a')).clear, true, 'an ordinary keydown empties the textarea first')
  guard.compositionstart()
  eq(
    guard.keydown(imeKey()).clear,
    false,
    'a keydown inside a composition must not destroy the preedit',
  )
  eq(guard.input(facts('insertCompositionText', 'n', true)).emit, null, 'preedit is not written')
  guard.compositionend()
  eq(
    guard.state(),
    { composing: false, commitPending: true },
    'compositionend opens the commit window',
  )
  eq(
    guard.input(facts('insertText', '你')).emit,
    null,
    'and inside it `_finalizeComposition` is the emitter, not this app',
  )
  eq(guard.commitSettled().clear, true, 'the window closes by emptying the textarea')
  eq(guard.state(), { composing: false, commitPending: false }, 'and the guard is idle again')

  // …but not if the next composition opened inside the window. Its `compositionstart` has
  // already measured `_compositionPosition.start` against the textarea as it stands, so a
  // clear here would leave `_finalizeComposition` reading past the end of a shorter string
  // and the next commit would arrive truncated or not at all.
  const overlap = new InputGuard()
  overlap.compositionend()
  overlap.compositionstart()
  eq(overlap.commitSettled().clear, false, 'a deferred clear never empties a live preedit')
  eq(
    overlap.state(),
    { composing: true, commitPending: false },
    'and it still closes the window it was scheduled for',
  )
  eq(guard.input(facts('insertText', 'p')).emit, 'p', 'after which insertions are ours again')
  guard.compositionstart()
  guard.reset()
  eq(guard.state(), { composing: false, commitPending: false }, 'reset drops a stuck composition')

  // === the half of the fix that lives in the DOM wiring ====================================
  //
  // `muteKeypress` above models `xterm.ts` returning false from the custom key handler for
  // `keypress`. That file cannot be imported here — it pulls in `@xterm/xterm` and a DOM — so
  // the link between the model and the shipped code is asserted against its source, the same
  // way `check-key-gate.mjs` scans `dispatch.ts`. Delete the bail and this fails, rather than
  // silently leaving the model describing code that no longer exists.
  const xtermSource = readFileSync(
    fileURLToPath(new URL('../src/terminal/xterm.ts', import.meta.url)),
    'utf8',
  )
  ok(
    /if \(ev\.type === 'keypress'\) return false/.test(xtermSource),
    'xterm.ts still disarms `_keyPress`, which is what makes capitals arrive once',
  )
  ok(
    !/'keypress'[\s\S]{0,120}preventDefault/.test(xtermSource),
    'and does not preventDefault it — that would drop the character instead of doubling it',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-input-emitters: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-input-emitters: ok')
