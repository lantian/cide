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
 * # Why all three kinds share one surface
 *
 * A second bottom-right stack would fight this one for the same 12px corner, and the three are
 * the same thing to a user: the outcome of something they just did. What differs is the
 * colour and the ARIA role, and nothing else — see `Failures.tsx` for both.
 *
 * Module-level and per-window. `Failures` mounts once per window (the shell returns before it
 * for a detached pane and renders its own), so there is exactly one reader.
 *
 * # Per window was not a narrow enough scope
 *
 * It was the only scope this had, and it is the right *outer* one — a detached window is
 * another realm and nothing here crosses it. But in the default `Stacked` window mode one
 * window holds every open project, and a toast is the outcome of something done *in* a
 * project. So a pull that failed in one repository stayed in the corner while the user
 * switched to another, reporting the first project's outcome over the second's: the reported
 * bug, "the notification is shared on all of them".
 *
 * The scope is therefore a project as well: [`Notice.project`] carries it, [`notify`] stamps
 * it from an injected getter ([`setNoticeScope`]) so ninety-odd call sites need not change,
 * [`visible`] is the filter `Failures` renders through, and [`admit`]'s two rules — the
 * collapse by message and the cap — are applied within a view rather than across the window.
 * A notice with no project is shown everywhere, which is both the honest answer for an
 * app-level failure and the direction a missing registration has to fail in.
 */

/**
 * What happened. Three, and each one has a colour and an ARIA role of its own.
 *
 *   * `ok`    — the gesture did what it said. Green edge, `role="status" aria-live="polite"`.
 *   * `warn`  — understood, and nothing happened: a refusal, a "there is none here", a partial
 *               result. Yellow edge, `role="alert" aria-live="assertive"`.
 *   * `error` — the command failed. Red edge, `role="alert" aria-live="assertive"`.
 *
 * # Why there are three and not two
 *
 * There were two, `error` and `info`, and the second was doing two unrelated jobs: sixteen real
 * successes ("Merge committed", "Copied a1b2c3d") and twenty-five refusals ("This window has no
 * file tree", "There is no file path under the pointer"). One colour for both is one colour for
 * *worked* and *did not work*, which is the distinction a toast exists to carry.
 *
 * It was worse than that on screen. `info` was painted `--accent`, and `--accent` is #c4452a
 * light / #d97757 dark — Claude's red-orange, a few percent from `--red`'s #c0332b / #e06c75. On
 * a 3px edge nobody can tell them apart, so **every** notice read as a failure, including the
 * ones reporting that a pull had worked.
 *
 * # Why `warn` is assertive
 *
 * The old rule was "interrupt a screen reader for a failure, never for a success" — announcing
 * "fast-forwarded 7 commits" over the sentence someone is reading is a modal dialog for a
 * success. That rule survives for `ok`. A refusal is not a success: "There is no file path under
 * the pointer" is the direct answer to a Ctrl+click the user has just made, and a screen-reader
 * user who is told nothing concludes the gesture is wired to nothing — which is the whole reason
 * this surface exists. So `warn` interrupts, and only `ok` waits for a pause.
 */
export type NoticeKind = 'ok' | 'warn' | 'error'

/**
 * Something the notice offers to do next — *View commits* under a pull's report. (M37)
 *
 * A callback rather than a command id, because the thing to do is bound to the outcome that was
 * just reported — a range of commits in one repository — and no command name carries that.
 * Following it dismisses the toast: the action is the toast's continuation, and a report whose
 * link has already been taken is noise over the surface it opened.
 */
export interface NoticeAction {
  label: string
  run: () => void
}

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
  /** What the notice offers to do next. See [`NoticeAction`]. */
  actions?: readonly NoticeAction[] | undefined
  /**
   * The project this notice is about, or `null` for one that is about the app or the window.
   *
   * # Why a notice has a project at all
   *
   * The stack is module-level and therefore per *window*, which was the whole scope this
   * surface had: a toast raised while one project was active stayed on screen through a switch
   * to the next one, so every project reported every other project's outcomes. A window is the
   * right outer boundary — a detached window is a separate realm and nothing crosses it — but
   * inside one window, in the default `Stacked` mode, every open project shares this corner.
   *
   * # Why `string` and not `ProjectId`
   *
   * The import-free rule at the foot of this file, not laziness. `ProjectId` is a `string`
   * alias, and even a type import pulls `ipc/client.ts` into the one-file program
   * `check-notices.mjs` compiles. `awaitingRule.ts` spells `SessionPhase` out for the same
   * reason.
   *
   * # Why absent means *shown everywhere*
   *
   * Because the other direction fails silently and catastrophically. An unstamped notice —
   * one raised before the scope is registered, or by a window with no project open — is shown
   * in every project, which is exactly the behaviour that shipped before this field existed.
   * A default of "hide unless it matches" turns a missing registration into a stack that
   * renders nothing, which is the control-wired-to-nothing symptom this whole surface was
   * built to remove, arriving from the surface itself.
   *
   * A notice whose project has since been closed is dropped by [`prune`], not hidden.
   */
  project?: string | null | undefined
}

/**
 * How many are shown at once, **in one project's view**. Beyond this the oldest is dropped.
 *
 * The qualifier is the whole of the change: the cap is applied to the notices that would share
 * a screen with the incoming one (see [`capped`]), not to the list. A global cap lets three
 * toasts from a project the user is not looking at evict the one toast they can see — the
 * command they just ran then reports nothing, and no check in this suite can see it happen.
 */
export const MAX_SHOWN = 3

/**
 * Whether two notices belong to the same view.
 *
 * `undefined` and `null` are one bucket — "not about a project" arrives both ways, from a
 * caller that said nothing and from one that said so outright, and they mean the same thing.
 */
function sameScope(a: string | null | undefined, b: string | null | undefined): boolean {
  return (a ?? null) === (b ?? null)
}

/**
 * Enforce [`MAX_SHOWN`] over the notices that share a screen with the one just admitted.
 *
 * The cohort, not the list, and not a bucket per project either: what may never exceed three
 * is what a user can *see at once*, which is this project's notices plus the unscoped ones.
 * `.slice(-MAX_SHOWN)` over the whole list is one line shorter and drops toasts nobody was
 * shown, in favour of ones nobody can see.
 */
function capped(next: readonly Notice[], project: string | null): readonly Notice[] {
  const cohort = visible(next, project)
  if (cohort.length <= MAX_SHOWN) return next
  const oldest = cohort[0]
  // Unreachable — `MAX_SHOWN` is positive, so a longer cohort has a first element — and
  // written out because `noUncheckedIndexedAccess` is on and a `!` here would be the only
  // one in the file.
  if (oldest === undefined) return next
  return next.filter((n) => n !== oldest)
}

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
  /*
   * The comparison is the message **and the project**, and the second half is not tidiness.
   *
   * "Already up to date with origin" in two repositories the user has open is two different
   * events. Collapsed by text alone, the second one either produces no toast at all — the
   * precise symptom this surface exists to remove — or, when it carries a body or a link,
   * refreshes the first in place *and re-stamps it*, so the toast vanishes from the project it
   * was about and appears in the one that is on screen. Both are invisible to every other
   * check in the suite.
   *
   * Within one project nothing moves: five submodules reporting the same sentence still
   * collapse to one toast, which is why `git.pull` and `pushRun` go on aggregating their
   * repositories before they notify (`check-push.mjs`, `check-branches.mjs`).
   */
  const at = current.findIndex((n) => n.text === notice.text && sameScope(n.project, notice.project))
  const shown = current[at]
  if (shown === undefined) return capped([...current, notice], notice.project ?? null)
  /*
   * The same words, and one toast — but the *newer facts* under them. (M37)
   *
   * A repeat used to be declined outright, which was right while a notice was only its text. It
   * is not right for one that carries a commit list and a link: a toast never dismisses itself,
   * so a second pull half an hour later with the identical headline ("1 commit from origin · 1
   * file +1") would be dropped, and the toast still on screen would go on offering *View
   * commits* for the first pull's range — a plausible-looking wrong answer, which is worse than
   * no toast. So a repeat that brings a different body or any action replaces the shown notice
   * in place: same id, so React keeps the element and the open state of its disclosure; same
   * position, so nothing in the stack moves.
   *
   * A repeat that brings nothing new — the retried failure this rule was written for — is still
   * declined by returning the same array, for the reason above.
   */
  if (
    shown.detail === notice.detail &&
    shown.actions === undefined &&
    notice.actions === undefined
  ) {
    return current
  }
  const next = current.slice()
  next[at] = { ...notice, id: shown.id }
  return next
}

/**
 * The notices one project's view shows: its own, plus everything that belongs to no project.
 *
 * Pure and exported so `check-notices.mjs` can drive it, and that is the point rather than a
 * convenience — both ways of getting this wrong are silent. A filter that keeps nothing empties
 * the stack, which looks exactly like a surface that was never wired up; one that keeps
 * everything restores the bug it was written for, and nothing on screen says so.
 *
 * Returns the **same array** when nothing is hidden — the common case, one project open. A
 * fresh array per call in a snapshot position is the infinite render loop `getServerSnapshot`
 * below already carries a note about.
 */
export function visible(current: readonly Notice[], project: string | null): readonly Notice[] {
  const keep = current.filter((n) => n.project === undefined || n.project === null || n.project === project)
  return keep.length === current.length ? current : keep
}

/**
 * Drop the notices of projects that are no longer open.
 *
 * Hidden is not the same as gone: a closed project's toasts can never be shown again and can
 * never be dismissed, so without this they sit in the list for the life of the window. Driven
 * from `store/workspace.ts`'s `applySnapshot`, which is the one place a project closed **in
 * another window** is heard about.
 *
 * Same-array when nothing is dropped, which is every call but the rare one: this runs on every
 * accepted workspace mutation.
 */
export function prune(current: readonly Notice[], open: ReadonlySet<string>): readonly Notice[] {
  const keep = current.filter((n) => n.project === undefined || n.project === null || open.has(n.project))
  return keep.length === current.length ? current : keep
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

// --- which project a notice is about ----------------------------------------------------------

/**
 * How `notify` learns which project this window is showing. Registered by
 * `store/workspace.ts`; see [`setNoticeScope`].
 *
 * **The default answers `null`, and the direction matters more than the value.** An
 * unregistered scope means every notice is unscoped and therefore shown in every project,
 * which is what this surface did before it had a scope at all. The opposite default turns a
 * lost registration into a stack that renders nothing.
 */
let scopeOf: () => string | null = () => null

/**
 * Tell the notices which project this window is showing.
 *
 * A **getter**, not a value: `notify` fires from the first frame to the last, and the
 * bootstrap is an IPC round trip, so a value captured at registration would be `null` for the
 * life of the window. Registered at module scope in `store/workspace.ts` — this app has
 * shipped three features that only worked when somebody remembered to install them (the note
 * at the head of `panes/awaiting.ts` is about the last of them), so it is not an effect and
 * not a call anybody has to make from a component.
 */
export function setNoticeScope(get: () => string | null): void {
  scopeOf = get
}

/**
 * The project to stamp on a notice raised right now.
 *
 * Swallows a throw on purpose. The scope reads a store through a closure it does not own, and
 * the failure this surface must never have is a toast that does not appear: an exception here
 * would take the report down with the getter, on the path whose entire job is to report.
 */
function currentScope(): string | null {
  try {
    return scopeOf()
  } catch {
    return null
  }
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
 *
 * **`kind` is required, and that is the point.** It used to default to `info`, which was safe
 * while `info` meant everything-that-is-not-a-failure: the default could only be vague, never
 * wrong. With three kinds a default silently picks between green and yellow, and a green edge
 * on a refusal is the same mislabel this vocabulary was split up to remove, pointing the other
 * way. There were eight bare `notify(text)` calls and seven of them meant `ok` — the eighth
 * ("this agent has nothing to integrate") is exactly the one a default would have got wrong.
 * `tsc` asks every caller instead; `notifyFailure` is the shorthand for the error case.
 */
export function notify(
  text: string,
  options: {
    kind: NoticeKind
    hint?: string | undefined
    detail?: string | undefined
    actions?: readonly NoticeAction[] | undefined
    /**
     * Which project this outcome belongs to, when the caller knows better than the window
     * does — a pull, a push, an integrate: anything whose answer can land seconds after the
     * gesture, by which time the user may be looking at another project.
     *
     * Omit it and the notice is stamped with whatever project this window is showing *now*,
     * which is right for the ninety-odd call sites that answer a gesture within a frame.
     * `null` says outright that this is about the app or the window and belongs in every
     * project's view.
     */
    project?: string | null | undefined
  },
): void {
  nextId += 1
  const { kind } = options
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
      actions: options.actions,
      /*
       * `=== undefined`, and never `options.project ?? currentScope()`.
       *
       * The `??` spelling is shorter and silently discards an explicit `project: null` — a
       * caller saying "this is app-level, show it everywhere" would have its notice stamped
       * with whatever project happened to be on screen, and then hidden from every other one.
       * Same trap as the `kind ??` default this option bag refuses, and pinned the same way in
       * `check-notices.mjs`.
       */
      project: options.project === undefined ? currentScope() : options.project,
    }),
  )
}

/**
 * Report a rejection. Separate from `notify` only so the kind cannot be got wrong.
 *
 * The options are optional, so all ninety-odd existing callers are untouched and the handful
 * that hold a project id can name it.
 */
export function notifyFailure(reason: unknown, options?: { project?: string | null }): void {
  notify(describe(reason), {
    kind: 'error',
    ...(options?.project === undefined ? {} : { project: options.project }),
  })
}

export function dismiss(id: number): void {
  publish(notices.filter((n) => n.id !== id))
}

/**
 * Forget the notices of every project that is no longer open. See [`prune`].
 *
 * `publish` returns early on the same array, so the overwhelmingly common call — nothing to
 * drop — wakes nobody, which is what makes it safe on the every-mutation path it is driven
 * from.
 */
export function pruneNotices(open: ReadonlySet<string>): void {
  publish(prune(notices, open))
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
