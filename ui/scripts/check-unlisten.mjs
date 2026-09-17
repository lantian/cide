/**
 * Checks `src/ipc/unlisten.ts` — the wrapper that makes an event unsubscribe safe to call at
 * the instant the handle arrives.
 *
 * # Why this is pinned rather than trusted
 *
 * The module exists because of a race nothing in this repository can reproduce on demand:
 * Tauri answers `listen()` over the `ipc://` fetch while registering the listener over a
 * *queued eval*, so an unsubscribe fired the moment the id resolves can find no entry to
 * remove and throws. `ipc/unlisten.ts` has the full account. What matters here is that the
 * fix has two halves and each fails silently on its own:
 *
 *   - **Retrying** is what closes the leak. A version that only caught the throw would hide
 *     the toast, keep the stale subscription, and look completely fixed.
 *   - **Giving up** is what keeps a dying webview from retrying for ever, and giving up
 *     *quietly* is what keeps this off `chrome/Failures.tsx`. A ladder that rethrows at the
 *     end puts the same toast back.
 *
 * Plus idempotence, which is not tidiness: an effect cleanup can run twice inside the ladder's
 * own window, and two ladders over one handle report one subscription's failure twice.
 *
 * Same shape as `check-exit-marker.mjs` — there is no JS test runner in this project, and this
 * is a pure module the TypeScript in `node_modules` can compile on its own. `sleep` is
 * injected so the ladder runs in no time at all and the delays can be *observed* rather than
 * waited on.
 *
 * Run: `pnpm --dir ui run check:unlisten`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-unlisten-'))
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

/** The `TypeError` Tauri's generated `unregisterListener` throws when the id is not there yet. */
const raceError = () => new TypeError("undefined is not an object (evaluating 'listeners[eventId].handlerId')")

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/ipc/unlisten.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const { guardUnlisten, UNLISTEN_RETRY_MS } = await import(`file://${join(out, 'unlisten.js')}`)

  // --- the happy path stays one call ----------------------------------------------------
  {
    let calls = 0
    const waited = []
    const unlisten = guardUnlisten(
      () => {
        calls++
      },
      { sleep: async (ms) => void waited.push(ms), onGiveUp: () => eq('gave up', 'no give-up', 'a handle that worked was reported as a failure') },
    )
    await unlisten()
    eq(calls, 1, 'a handle that works is called exactly once')
    eq(waited, [], 'a handle that works does not wait')
  }

  // --- the race: late registration, and the retry is what closes the leak ---------------
  //
  // The whole point. A guard that swallowed instead of retrying would pass every assertion
  // about the toast and leave the subscription live in Rust for the life of the window.
  {
    let calls = 0
    const waited = []
    let reported = 0
    const unlisten = guardUnlisten(
      () => {
        calls++
        // Lands on the third attempt, the way a queued eval does.
        if (calls < 3) throw raceError()
      },
      { sleep: async (ms) => void waited.push(ms), onGiveUp: () => reported++ },
    )
    await unlisten()
    eq(calls, 3, 'the handle is retried until the registration lands')
    eq(waited, UNLISTEN_RETRY_MS.slice(0, 2), 'each retry waits the next rung of the ladder')
    eq(reported, 0, 'a retry that came good is not reported as a failure')
  }

  // --- and it gives up, quietly ----------------------------------------------------------
  {
    let calls = 0
    const waited = []
    const reported = []
    const unlisten = guardUnlisten(
      () => {
        calls++
        throw raceError()
      },
      { sleep: async (ms) => void waited.push(ms), onGiveUp: (e) => reported.push(e) },
    )
    let rejected = false
    await unlisten().catch(() => {
      rejected = true
    })
    eq(calls, UNLISTEN_RETRY_MS.length + 1, 'the ladder is bounded: one try per rung, plus the first')
    eq(waited, [...UNLISTEN_RETRY_MS], 'every rung of the ladder is used before giving up')
    eq(
      rejected,
      false,
      'the handle rejected — `chrome/Failures.tsx` listens for `unhandledrejection`, so a ' +
        'rejection here is the red toast this module was written to remove',
    )
    eq(reported.length, 1, 'giving up is reported exactly once')
    ok(
      reported[0] instanceof TypeError,
      'the give-up carries the last error, not a sentence this module invented',
    )
  }

  // --- idempotence, over a ladder that is still running ----------------------------------
  {
    let calls = 0
    const pending = []
    const reported = []
    const unlisten = guardUnlisten(
      () => {
        calls++
        throw raceError()
      },
      {
        // Held open, so the second call below lands while the first ladder is mid-rung.
        sleep: () => new Promise((resolve) => pending.push(resolve)),
        onGiveUp: (e) => reported.push(e),
      },
    )
    const first = unlisten()
    const second = unlisten()
    ok(first === second, 'a second call during the ladder starts a second ladder over one handle')
    eq(calls, 1, 'a second call re-enters the handle while the first attempt is still pending')
    // Let every ladder that exists run out, so the give-up count below is a statement about
    // the whole thing rather than about whichever one happened to be resumed. Bounded, because
    // a guard that never settles must fail here rather than hang the check.
    for (let round = 0; round < 64 && pending.length > 0; round += 1) {
      for (const resume of pending.splice(0)) resume()
      await Promise.resolve()
      await Promise.resolve()
    }
    await Promise.all([first, second].map((p) => p.catch(() => {})))
    eq(reported.length, 1, 'two calls over one handle reported one subscription twice')
    // A call *after* the ladder settled is a fresh ladder, not a cached answer. Drained the
    // same way, so a guard with no memory fails the count above rather than hanging here.
    const third = unlisten().catch(() => {})
    for (let round = 0; round < 64 && pending.length > 0; round += 1) {
      for (const resume of pending.splice(0)) resume()
      await Promise.resolve()
      await Promise.resolve()
    }
    await third
    eq(calls, UNLISTEN_RETRY_MS.length + 1, 'a call after the ladder finished ran it again')
  }

  // --- an async handle rejecting is the same thing as a sync one throwing -----------------
  //
  // `_unlisten` is `async` and throws on its first line, so it presents as a rejection, not a
  // throw. A guard that only caught the synchronous shape would catch nothing at all.
  {
    let calls = 0
    const reported = []
    const unlisten = guardUnlisten(
      // eslint-disable-next-line @typescript-eslint/require-await
      async () => {
        calls++
        throw raceError()
      },
      { sleep: async () => {}, onGiveUp: (e) => reported.push(e) },
    )
    // `.catch` so that a guard which *does* reject reports the failure it was given rather
    // than killing the check with the very TypeError this module exists to absorb.
    await unlisten().catch(() => {})
    eq(calls, UNLISTEN_RETRY_MS.length + 1, 'a rejecting async handle is not retried')
    eq(reported.length, 1, 'a rejecting async handle is not reported')
  }

  // --- the ladder's shape -----------------------------------------------------------------
  ok(UNLISTEN_RETRY_MS.length > 0, 'the ladder is empty, so nothing is ever retried')
  ok(
    UNLISTEN_RETRY_MS.every((ms, i) => i === 0 || ms > (UNLISTEN_RETRY_MS[i - 1] ?? 0)),
    'the ladder does not back off, so five attempts land inside one macrotask',
  )
  ok(
    UNLISTEN_RETRY_MS.reduce((a, b) => a + b, 0) <= 1000,
    'the ladder outlives a closing webview — the point of giving up is to stop retrying',
  )

  // --- the seam: nothing may reach Tauri's `listen` around the guard -----------------------
  //
  // Comments stripped first. This file and `ipc/client.ts` both spell the import in prose, and
  // a check that matched its own explanation is a failure three assertions in this repository
  // have already shipped with.
  const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
  const client = strip(readFileSync('src/ipc/client.ts', 'utf8'))
  ok(
    /import\s*\{[^}]*listen as tauriListen[^}]*\}\s*from\s*'@tauri-apps\/api\/event'/.test(client),
    "ipc/client.ts imports `listen` under its own name again — the shadowing local `listen` is " +
      'what makes every subscription go through the guard without a call site opting in',
  )
  eq(
    (client.match(/\btauriListen\b/g) ?? []).length,
    2,
    'ipc/client.ts calls Tauri\'s `listen` somewhere other than inside the local wrapper',
  )
  ok(
    /guardUnlisten/.test(client),
    'ipc/client.ts no longer guards the handles it hands out',
  )
  ok(
    /onDragDropEvent\([\s\S]{0,200}?\)\s*\.then\(\(fn\) => guardUnlisten\(fn\)\)/.test(client),
    'the drag-drop handle is unguarded — it removes four listeners in a row, so the first to ' +
      'lose the race abandons the other three',
  )
  const frame = strip(readFileSync('src/chrome/WindowFrame.tsx', 'utf8'))
  ok(
    /guardUnlisten/.test(frame),
    'WindowFrame.tsx is the one other file allowed to reach Tauri directly, and its ' +
      '`onResized` handle is unguarded again',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck:unlisten — ${failed} failure(s)`)
  process.exit(1)
}
console.log('check:unlisten — ok')
