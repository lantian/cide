/**
 * The notice list: what the bottom-right stack shows, and the rules for what gets in.
 *
 * # Why this is a module and not `useState` inside `Failures.tsx`
 *
 * It used to be the latter, and that was right while the only producer was a window listener
 * living in the same component. It is not right any more: `keys/dispatch.ts` now has an
 * outcome to report — a pull that succeeded — and a keystroke handler cannot reach into a
 * component's state. The alternatives were a context provider threaded through a shell that
 * has no other use for one, or a custom event on `window`, which is a store with worse types.
 *
 * It is also where the rules live, and that is the point. The dedupe, the cap and the drop
 * order are decisions that fail *silently* when they go wrong — five repositories reporting
 * "already up to date" collapsing into one toast looks exactly like four repositories being
 * skipped — so they are pure functions here rather than closures inside a `useState` updater,
 * and `ui/scripts/check-notices.mjs` compiles this file on its own and drives them.
 *
 * # Why errors and successes share one surface
 *
 * A second bottom-right stack would fight this one for the same 12px corner, and the two are
 * the same thing to a user: the outcome of something they just did. What differs is the
 * colour, the ARIA role, and nothing else — see `Failures.tsx` for both.
 *
 * Module-level and per-window. `Failures` mounts once per window (the shell returns before it
 * for a detached pane and renders its own), so there is exactly one reader.
 */

/**
 * Failure or report.
 *
 * The kind is not decoration: it picks `role="alert" aria-live="assertive"` against
 * `role="status" aria-live="polite"`. A screen reader must be interrupted for a command that
 * failed and must **not** be for one that worked — announcing "fast-forwarded 7 commits" over
 * whatever the user is reading is the accessibility equivalent of a modal dialog for a
 * success.
 */
export type NoticeKind = 'error' | 'info'

export interface Notice {
  id: number
  kind: NoticeKind
  text: string
  /** Set when the message is one a user can act on themselves. */
  hint?: string | undefined
  /**
   * The expandable body: the commit list of a pull, git's own transport text.
   *
   * Kept out of `text` so the toast stays one sentence high until asked. Multi-line, and
   * rendered as pre-wrapped text — never parsed, because on the binary route it carries the
   * remote server's own `remote:` lines.
   */
  detail?: string | undefined
}

/** How many are shown at once. Beyond this the oldest is dropped. */
export const MAX_SHOWN = 3

/**
 * Admit one notice to the list, or decline it.
 *
 * Collapsed by message rather than appended: a failure that repeats is usually one gesture
 * the user is retrying, and three identical toasts explain no more than one. The comparison
 * is `text` alone and deliberately ignores `kind` — an error and a report that say the same
 * words are the same words.
 *
 * That collapse is also why `git.pull` aggregates its repositories into a single notice
 * before calling `notify`: five submodules each reporting "Already up to date with origin"
 * would arrive as five identical texts, collapse to one, and under-report four repositories
 * with nothing on screen to suggest it had happened.
 *
 * Pure, and returns the *same array* when nothing is admitted, so a `useSyncExternalStore`
 * reader does not re-render for a duplicate.
 */
export function admit(current: readonly Notice[], notice: Notice): readonly Notice[] {
  if (current.some((n) => n.text === notice.text)) return current
  return [...current, notice].slice(-MAX_SHOWN)
}

/**
 * Turn whatever was thrown into a sentence.
 *
 * Errors cross the IPC boundary as `{ kind, message }` — tagged, never bare strings, so the
 * frontend branches on a variant rather than matching prose. `message` is the half written for
 * a person; `kind` is for code and is deliberately not shown.
 */
export function describe(reason: unknown): string {
  if (typeof reason === 'string' && reason !== '') return reason
  if (reason instanceof Error && reason.message !== '') return reason.message
  if (reason !== null && typeof reason === 'object') {
    const message = (reason as { message?: unknown }).message
    if (typeof message === 'string' && message.length > 0) return message
    /*
     * `detail` before `kind`, because that is the shape every rejected Rust command actually has.
     *
     * `CoreError` is `#[serde(tag = "kind", content = "detail")]`, so an I/O failure arrives as
     * `{kind: "io", detail: "No space left on device (os error 28)"}` — no `message` field at all.
     * Falling through to `kind` turned every disk-full, permission-denied, file-deleted and
     * read-only failure into the single word **io**, which is the tag's name and tells the user
     * nothing they can act on. Autosave is where that stopped being cosmetic: it fails on a timer,
     * silently, and the toast was the only thing that could have said why.
     */
    const detail = (reason as { detail?: unknown }).detail
    if (typeof detail === 'string' && detail.length > 0) return detail
    const kind = (reason as { kind?: unknown }).kind
    if (typeof kind === 'string' && kind !== '') return kind
  }
  /*
   * The tail used to be a bare `String(reason)`, and for any object without one of the two
   * fields above that is the literal text `[object Object]` — the exact symptom this whole
   * surface was built to prevent, arriving from the surface itself. `''`, `null` and
   * `undefined` are the same failure with less to look at: a toast with nothing in it reads
   * as a rendering bug rather than as a command that failed.
   *
   * So anything that would render as one of those gets a sentence that says what happened and
   * where the rest of it is. The console always has the real object — see the listener in
   * `Failures.tsx`, which logs before it reports.
   */
  const text = String(reason)
  if (text !== '' && text !== '[object Object]' && text !== 'null' && text !== 'undefined') {
    return text
  }
  return unreadable(reason)
}

/** The last resort: say that something failed, and where to look. */
function unreadable(reason: unknown): string {
  let shape = ''
  try {
    // Circular structures throw; a `Symbol` or a bare `undefined` returns `undefined`.
    shape = JSON.stringify(reason) ?? ''
  } catch {
    shape = ''
  }
  // Capped, because a rejected `invoke` can carry a whole DTO and a toast is four lines high.
  if (shape.length > 200) shape = `${shape.slice(0, 199)}…`
  return shape === '' || shape === '{}' || shape === 'null'
    ? 'A command failed and its error carried no message. The console has the details.'
    : `A command failed: ${shape}`
}

/**
 * The one failure a user can fix without us, so it is worth naming precisely.
 *
 * Tauri answers an unrecognised command with this, and in development it means one specific
 * thing: the webview has hot-reloaded past the binary. Vite serves the new frontend instantly;
 * the Rust side only changes when it is rebuilt and the app is restarted. Without this hint the
 * message reads as an internal error rather than as "your binary is stale".
 */
export function hintFor(text: string): string | undefined {
  return /not found|unknown command/i.test(text)
    ? 'The running binary predates this command — rebuild with `cargo build -p cide-app` and restart.'
    : undefined
}

// --- the store ------------------------------------------------------------------------------

let notices: readonly Notice[] = []
let nextId = 0
const listeners = new Set<() => void>()

function publish(next: readonly Notice[]): void {
  if (next === notices) return
  notices = next
  for (const listener of listeners) listener()
}

/**
 * Put a notice on screen. The imperative entry point, for callers outside React.
 *
 * Returns nothing on purpose: a caller that wants to know whether its notice was collapsed
 * into an existing one is a caller that is about to build a second surface.
 */
export function notify(
  text: string,
  options: { kind?: NoticeKind; hint?: string | undefined; detail?: string | undefined } = {},
): void {
  nextId += 1
  const kind = options.kind ?? 'info'
  publish(
    admit(notices, {
      id: nextId,
      kind,
      text,
      // The stale-binary hint is a property of the message, so it is derived here rather than
      // asked for: every producer would otherwise have to remember it, and the one that
      // forgot would be the one that needed it.
      hint: options.hint ?? (kind === 'error' ? hintFor(text) : undefined),
      detail: options.detail,
    }),
  )
}

/** Report a rejection. Separate from `notify` only so the kind cannot be got wrong. */
export function notifyFailure(reason: unknown): void {
  notify(describe(reason), { kind: 'error' })
}

export function dismiss(id: number): void {
  publish(notices.filter((n) => n.id !== id))
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => void listeners.delete(listener)
}

export function getSnapshot(): readonly Notice[] {
  return notices
}

/**
 * Server-side rendering never happens here, but `useSyncExternalStore` demands the callback
 * and React calls it during hydration checks. The same frozen empty array every time, because
 * a fresh `[]` per call is an infinite render loop.
 */
const NONE: readonly Notice[] = []
export function getServerSnapshot(): readonly Notice[] {
  return NONE
}

/** Drop everything. Exists for tests and for the check script; nothing in the app calls it. */
export function clearNotices(): void {
  publish([])
}

/*
 * Runtime values only, no imports: `check-notices.mjs` compiles this file alone and loads the
 * emitted `.js` in node. Adding a value import — React, a store, `@/ipc/client` — breaks that,
 * and the check would start failing with a module-resolution error rather than telling anyone
 * why. The same note is at the foot of `chrome/branchModel.ts`.
 */
