/**
 * The key gate — the ONLY place a chord is resolved, with two entry points that must agree.
 *
 * ```
 * entry 1 (terminals):    term.attachCustomKeyEventHandler(terminalKeyGate)
 * entry 2 (everything):   window.addEventListener('keydown', gate, true)
 * ```
 *
 * # Why two
 *
 * A window listener alone is neither sufficient nor correct. `Ctrl+P` and `Ctrl+K` mean
 * something to readline, so the decision to swallow a chord has to happen *before* the
 * bytes are forwarded to the PTY — and xterm's own handler is the only hook that runs
 * before xterm writes. `attachCustomKeyEventHandler` returning `false` is what makes `^P`
 * never reach the shell. Conversely the terminal handler alone would leave every keystroke
 * outside a terminal unbound.
 *
 * # Why they cannot drift
 *
 * Both entry points are thin wrappers over one [`KeyGate.decide`]. They differ only in what
 * they do with a decision they have already been given — the window listener also has to
 * stop the event reaching the DOM, the terminal handler only has to return `false`. The
 * resolution itself, the prefix machine and the dispatch are shared code, and
 * `ui/scripts/check-key-gate.mjs` walks the whole modifier × key space asserting that the
 * two return the same verdict and dispatch the same command for every one of them.
 *
 * # Ordering, and why decisions are memoised
 *
 * A window capture listener fires before the event reaches its target, so for a keystroke
 * typed into a terminal, entry 2 runs first and — when it handles the chord — calls
 * `stopPropagation`, and xterm never sees the event at all. Entry 1 is what covers the
 * cases where that is not true: a detached pane window that mounts terminals before the
 * shell's listener is installed, and any future host that does not own `window`.
 *
 * Because both may see the *same* event object, every decision is memoised against it in a
 * `WeakMap`. Without that, a chord that reached both entry points would run its command
 * twice — and for `claude.restart` or `git.commit` twice is not a cosmetic bug.
 *
 * # Prefix sequences
 *
 * `ctrl+k ctrl+s` needs a prefix state machine with a ~1 s timeout, because CodeMirror's
 * `KeyBinding.key` parser handles single chords only and nothing else in the stack tracks
 * multi-stroke sequences. See [`PREFIX_TIMEOUT_MS`].
 *
 * # The capture, and why it does not fight the prefix machine
 *
 * The gate is stateless per chord apart from that prefix machine — with one exception, added
 * for the held-modifier project switcher (`keys/switcher.ts`). While that popup is up it has
 * to own Tab, or the stroke that moves the highlight also inserts a tab character into
 * whatever had focus, which for a shell pane means completing a filename.
 *
 * So [`KeyGateHost::capture`] is a *stateful claim on a stroke*, consulted in one place with
 * one rule:
 *
 * ```text
 * an armed prefix  >  the capture  >  the keymap
 * ```
 *
 * An armed prefix outranks it because a half-typed `ctrl+k` is a sequence the user has
 * already started, and stealing its second stroke would make the gate lie about what is in
 * flight. The capture outranks the keymap because a switcher that only worked while
 * `ctrl+tab` happened to be bound to it would break the moment a user rebound the chord —
 * and because a `when` clause going false mid-walk would otherwise strand the popup open.
 *
 * The two can never both be armed by accident: a walk only starts on a chord that dispatched
 * a command, which is exactly the branch that clears the prefix. `ctrl+k` then `ctrl+tab` is
 * an unbound sequence — swallowed, prefix disarmed, no walk — which is deterministic and is
 * what the ordering above buys.
 *
 * The capture is **not** a second resolution path. It never reads the keymap, never names a
 * command, and answers only "consumed / not consumed"; `check-key-gate.mjs` sweeps it through
 * both entry points alongside everything else for exactly that reason.
 */
import { strokeFromEvent, type KeyStroke } from './chords'
import { buildKeymap, type KeyBinding, type KeyContext, type Keymap } from './keymap'

/**
 * How long a pending prefix survives.
 *
 * One second, matching the plan's "~1 s". Long enough for a deliberate two-stroke chord,
 * short enough that a forgotten `ctrl+k` does not silently eat the next real keystroke
 * — which for a terminal means a character that never reaches the shell.
 */
export const PREFIX_TIMEOUT_MS = 1000

/** Everything the gate needs from the app, so it can be driven by a fixture in tests. */
export interface KeyGateHost {
  /** The resolved keymap from `app_get_bootstrap`. Re-read on every keystroke, so a
   *  rebinding takes effect without reinstalling the gate. */
  bindings: () => readonly KeyBinding[]
  /** Context flags for `when` clauses: which pane kind has focus, whether a repo is open. */
  context: () => KeyContext
  /** Run a command. The gate never performs an action itself. */
  run: (command: string, args: unknown) => void
  /** Called whenever the pending prefix changes, for a status-bar readout like `⌃K …`. */
  onPending?: ((sequence: string | null) => void) | undefined
  /**
   * A stateful claim on a stroke, for a modal keyboard gesture — today only the project
   * switcher. Returns `true` when the stroke was consumed and must not reach the keymap, the
   * DOM or the PTY.
   *
   * Ranked below an armed prefix and above the keymap; see the module note. Called at most
   * once per event, because [`KeyGate::decide`] memoises, so an implementation may act on it.
   */
  capture?: ((stroke: string) => boolean) | undefined
  /** Injectable clock, so the timeout is testable without waiting a second. */
  now?: (() => number) | undefined
  timeoutMs?: number | undefined
}

/** What the gate decided about one keystroke. */
export interface Decision {
  /** `true` = let it through: the DOM, the editor, or the PTY may have it. */
  passThrough: boolean
  /** The command that was dispatched, or `null`. */
  command: string | null
  /** The full sequence this stroke completed or extended, or `null` for a bare modifier. */
  sequence: string | null
}

const PASS: Decision = { passThrough: true, command: null, sequence: null }

/**
 * Minimal shape of the events the gate accepts.
 *
 * Typed structurally rather than as `KeyboardEvent` so the check script can feed it plain
 * objects, and so this module's core stays free of DOM lib types. `KeyboardEvent` satisfies
 * it.
 */
export type GateEvent = KeyStroke & {
  type?: string | undefined
  preventDefault?: (() => void) | undefined
  stopPropagation?: (() => void) | undefined
}

export interface KeyGate {
  /** The shared resolution. Both entry points are wrappers over this. */
  decide: (ev: GateEvent) => Decision
  /** Entry 2 — `window.addEventListener('keydown', …, true)`. */
  windowHandler: (ev: KeyboardEvent) => boolean
  /** Entry 1 — `term.attachCustomKeyEventHandler(…)`. */
  terminalHandler: (ev: KeyboardEvent) => boolean
  /** The pending prefix, e.g. `ctrl+k`, or `null`. */
  pending: () => string | null
  /** Drop any pending prefix and its timer. */
  reset: () => void
  /** The indexed keymap as of now. Rebuilt when `bindings()` returns a different array. */
  keymap: () => Keymap
}

/**
 * The stroke currently being handed to a command handler, or `null`.
 *
 * Module scope rather than per-gate, because the reader is a command handler and handlers do
 * not know which gate invoked them — and only one gate is ever installed. See the `run`
 * branch of `resolveStroke` for why this exists at all.
 */
let dispatchingStroke: string | null = null

/**
 * The keystroke that is dispatching the command running right now, e.g. `ctrl+tab`.
 *
 * `null` when the command was not started by a key at all — which is the command palette, and
 * is exactly the distinction the project switcher needs: no key means no modifier is being
 * held, so there is no release to wait for.
 */
export function currentStroke(): string | null {
  return dispatchingStroke
}

export function createKeyGate(host: KeyGateHost): KeyGate {
  const clock = host.now ?? (() => Date.now())
  const timeout = host.timeoutMs ?? PREFIX_TIMEOUT_MS

  /** Memoised decisions, so an event seen by both entry points dispatches once. */
  const seen = new WeakMap<object, Decision>()

  let pendingSequence: string | null = null
  let pendingAt = 0
  let pendingTimer: ReturnType<typeof setTimeout> | null = null

  /** Cache the indexed keymap against the array identity `bindings()` returns. */
  let cachedSource: readonly KeyBinding[] | null = null
  let cachedKeymap: Keymap = buildKeymap([])

  function keymap(): Keymap {
    const source = host.bindings()
    if (source !== cachedSource) {
      cachedSource = source
      cachedKeymap = buildKeymap(source)
    }
    return cachedKeymap
  }

  function setPending(sequence: string | null): void {
    const changed = sequence !== pendingSequence
    pendingSequence = sequence
    pendingAt = clock()

    if (pendingTimer !== null) {
      clearTimeout(pendingTimer)
      pendingTimer = null
    }
    /*
     * The timer is only for the *readout*. Expiry itself is decided by comparing timestamps
     * on the next keystroke, because a background webview has its timers throttled and a
     * timer that fires late would leave a stale prefix armed for as long as the throttle
     * lasts. Two mechanisms, one authority.
     */
    if (sequence !== null && typeof setTimeout === 'function') {
      pendingTimer = setTimeout(() => {
        pendingTimer = null
        if (pendingSequence !== null && clock() - pendingAt >= timeout) {
          pendingSequence = null
          host.onPending?.(null)
        }
      }, timeout)
    }
    if (changed) host.onPending?.(sequence)
  }

  function decide(ev: GateEvent): Decision {
    // xterm calls its custom handler for keydown, keypress *and* keyup. Only keydown is a
    // chord; resolving on keyup would fire every command a second time. `type` is optional
    // so a hand-built stroke in a test is treated as a keydown.
    if (ev.type !== undefined && ev.type !== 'keydown') return PASS

    const cached = seen.get(ev)
    if (cached !== undefined) return cached

    const stroke = strokeFromEvent(ev)
    if (stroke === null) {
      // A bare modifier. Not memoised: it carries no decision worth remembering and the
      // WeakMap would grow one entry per shift press.
      return PASS
    }

    const decision = resolveStroke(stroke)
    seen.set(ev, decision)
    return decision
  }

  function resolveStroke(stroke: string): Decision {
    const ctx = host.context()

    // Expire a stale prefix before it can absorb this stroke.
    if (pendingSequence !== null && clock() - pendingAt >= timeout) setPending(null)

    // An armed prefix outranks the capture; the capture outranks the keymap. See the module
    // note for why that order and not the other one.
    if (pendingSequence === null && host.capture?.(stroke) === true) {
      return { passThrough: false, command: null, sequence: stroke }
    }

    const sequence = pendingSequence === null ? stroke : `${pendingSequence} ${stroke}`
    const armed = pendingSequence !== null
    const resolution = keymap().resolve(sequence, ctx)

    if (resolution.kind === 'prefix') {
      setPending(sequence)
      // Swallowed: the first stroke of `ctrl+k ctrl+s` must not reach the PTY either.
      return { passThrough: false, command: null, sequence }
    }

    if (resolution.kind === 'run') {
      setPending(null)
      /*
       * The stroke is published for the duration of the dispatch, and only for that.
       *
       * A command that opens a held-modifier gesture has to know *which modifiers were down
       * when it ran* — a `KeyboardEvent` is the only thing that knows, and by design the
       * dispatcher never sees one (the palette calls it with two strings). Widening `run` to
       * carry the event was the alternative and it loses: it would put a DOM type in the
       * signature the palette also calls, for a fact exactly one command wants.
       *
       * `try`/`finally` because a handler is allowed to throw, and a stroke left published
       * would make the *next* palette invocation believe a key was held.
       */
      dispatchingStroke = stroke
      try {
        host.run(resolution.command, resolution.args)
      } finally {
        dispatchingStroke = null
      }
      return { passThrough: false, command: resolution.command, sequence }
    }

    if (armed) {
      /*
       * A prefix was armed and this stroke completed nothing. Swallow it and disarm.
       *
       * Passing it through was the alternative and it loses: the user has already given up
       * one stroke to the prefix, and letting the second land in the shell means a stray
       * `^S` — which on a terminal is flow control and freezes the pane with no visible
       * cause. VS Code behaves the same way and shows "unbound sequence".
       */
      setPending(null)
      return { passThrough: false, command: null, sequence }
    }

    return { passThrough: true, command: null, sequence }
  }

  return {
    decide,
    keymap,
    pending: () => pendingSequence,
    reset: () => setPending(null),

    windowHandler(ev) {
      const decision = decide(ev)
      if (!decision.passThrough) {
        // Both, and in this order. `preventDefault` stops the browser's own default (a
        // Ctrl+P print dialog in a webview that has one); `stopPropagation` from a capture
        // listener on `window` stops the event reaching its target at all, which is what
        // keeps a focused xterm from ever being offered the keystroke.
        ev.preventDefault()
        ev.stopPropagation()
      }
      return decision.passThrough
    },

    terminalHandler(ev) {
      const decision = decide(ev)
      // No `stopPropagation` here: by the time xterm consults this handler the event has
      // already reached its target, and xterm's own contract — return `false` and it will
      // not process or forward the key — is the whole mechanism. `preventDefault` still
      // matters so the browser default does not fire behind it.
      if (!decision.passThrough) ev.preventDefault()
      return decision.passThrough
    },
  }
}

/*
 * The process-wide gate.
 *
 * A singleton because `attachCustomKeyEventHandler` takes a plain function and terminals are
 * created from several places over the life of a window; handing each one a closure over a
 * particular gate instance is how two gates end up with two prefix machines, of which only
 * one is ever armed. `gate` and `terminalKeyGate` below are stable references that always
 * route to whichever gate is currently installed.
 */
let installed: KeyGate | null = null

/** Entry 2. Stable reference; safe to pass to `addEventListener` before installation. */
export function gate(ev: KeyboardEvent): boolean {
  return installed?.windowHandler(ev) ?? true
}

/** Entry 1. Stable reference; safe to pass to `attachCustomKeyEventHandler`. */
export function terminalKeyGate(ev: KeyboardEvent): boolean {
  return installed?.terminalHandler(ev) ?? true
}

/** The installed gate, for a status-bar prefix readout or a diagnostic. */
export function currentKeyGate(): KeyGate | null {
  return installed
}

/**
 * Install the gate and attach entry 2. Returns a teardown that removes both.
 *
 * Installing twice replaces the first gate rather than stacking a second listener —
 * `addEventListener` deduplicates on (function, capture) and `gate` is one stable reference,
 * so there is only ever one listener however many times this is called.
 *
 * That single listener is exactly why the teardown checks ownership first. `removeEventListener`
 * with the same reference removes *the* listener, not "the one this call added", so a stale
 * teardown running after a later install would leave `installed` pointing at a live gate that
 * no longer receives any keystrokes — entry point 2 dead with nothing on screen to say so. A
 * superseded teardown therefore does nothing; the gate that replaced it owns the listener and
 * will remove it when its own teardown runs.
 */
export function installKeyGate(host: KeyGateHost): () => void {
  const created = createKeyGate(host)
  installed = created

  const listening = typeof window !== 'undefined'
  if (listening) window.addEventListener('keydown', gate, true)

  return () => {
    created.reset()
    if (installed !== created) return
    if (listening) window.removeEventListener('keydown', gate, true)
    installed = null
  }
}

/** Attach entry 1 to a terminal. Call once per `Terminal`, right after it is constructed. */
export function attachKeyGate(term: {
  attachCustomKeyEventHandler: (handler: (ev: KeyboardEvent) => boolean) => void
}): void {
  term.attachCustomKeyEventHandler(terminalKeyGate)
}
