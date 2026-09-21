/**
 * "Reveal that panel — and bring this with you."
 *
 * The sibling of `chrome/focusRequests.ts`, one step earlier in the same gesture. That module
 * answers *"the command opened a panel; now put the caret in its box"*; this one answers the
 * half before it, **"open the panel at all"**, for a caller that is not inside React.
 *
 * # The gap this closes
 *
 * Revealing a panel is a `useState` setter. `App.tsx` holds `sidebar`, hands
 * `(next) => setSidebar((s) => showPanel(s, next))` to `createDispatcher`, and that closure is
 * the *only* way into it. Anything that is neither the dispatcher nor a descendant of `App`
 * simply cannot reveal a panel — and the git log's *Amend…* is exactly that: an item in a
 * context menu built by `gitlog/logMenu.ts`, four levels below `App` and in a different subtree
 * from the sidebar, whose whole job is to put the user in front of the commit box. It shipped
 * **listed and disabled**, pointing at the Git panel in words, because the seam did not exist;
 * `README.md` recorded that as the reason. This module is the seam.
 *
 * # Why a separate module and not a fourth `FocusSurface`
 *
 * The two look alike from a distance and are different in every respect that decided the shape
 * of either:
 *
 *   * **Different answerers.** A focus request is answered by the element that takes the
 *     caret — the search box, the commit textarea, the tree scroller. A reveal request can only
 *     be answered by the *shell*, because the shell is the one component that owns which panel
 *     is mounted. Folding it into `focusRequests` would have `App.tsx` subscribing to a store
 *     whose name promises focus and whose other half it must ignore.
 *   * **A focus request carries nothing; this one carries a payload.** `requestFocus` takes a
 *     closed union of surface names precisely so a typo cannot become a request nobody answers,
 *     and `useFocusRequested(surface): boolean` is sufficient *because* there is nothing to
 *     read. An amend carries a project, a repository, an oid and a whole commit message. Making
 *     `FocusSurface` a discriminated union with payloads would change the type at all five
 *     existing call sites to buy nothing at any of them.
 *   * **Different lifetimes, and this is the load-bearing one.** `focusRequests` has no TTL and
 *     its header argues why: *"every caller here reveals the panel in the same tick, so the
 *     render that consumes the flag is the very next one"*. That is not true here. An amend is
 *     consumed **two** hops later — the shell reveals, React mounts `GitPanel`, and only then
 *     can `useGitPanel` take the payload — and the panel may open a confirmation before it
 *     adopts anything. A two-hop request parked in a module whose header states the one-hop
 *     rule would falsify that header rather than extend it.
 *   * And the practical one: `focusRequests.ts` imports zustand, so no check script can compile
 *     it standalone. Everything below is a rule with an edge case in it — a TTL, a
 *     consumed-once claim, a refusal to overwrite a draft — and this project has paid five
 *     times for rules that lived somewhere no check could run them. `ui/scripts/check-sidebar.mjs`
 *     compiles this file beside `sidebarView.ts` and drives it.
 *
 * # Two mechanisms, because there are two problems
 *
 * **The reveal is a registered opener**, `registerPanelHost` / `requestPanel`, in the shape
 * `keys/dispatch.ts::registerTagDialog` already uses. The thing that answers it — `App` — is
 * mounted before any keystroke can arrive and stays mounted for the window's life, so there is
 * no mount to wait for and nothing to park. What the registration *does* buy is the honest
 * refusal: a window with no sidebar registers nothing, `requestPanel` answers `false`, and the
 * caller reports rather than parking a request nobody will ever drain. That is the fact
 * `DispatchDeps.showSidebar === undefined` used to stand for, moved out of a prop and into a
 * place a caller outside React can ask.
 *
 * **The amend payload is parked, claimed once, and expires.** Its consumer is `useGitPanel`,
 * which is exactly the thing that has not mounted yet on the common path — and which *has*
 * mounted on the second press, so an `autoFocus`-shaped, on-mount-only mechanism would silently
 * do nothing the second time. Both cases are `focusRequests`' two cases, and the answer is the
 * same: park it, notify, let whichever render sees it first take it.
 *
 * Per window by construction, like everything else in this file's neighbourhood: each Tauri
 * window is its own JavaScript realm, so a request made in the shell window is invisible to a
 * detached one. That is right — the log and the Git panel that answers it are in the same
 * window, and a reveal that crossed windows would move a panel the user is not looking at.
 *
 * Nothing here is persisted. A half-made gesture in `workspace.json` would be a question
 * arriving at the next launch with nobody having asked it.
 */
import type { PanelView } from './sidebarView'

/*
 * `ProjectId` and `RepoId` are `string` in `@/ipc/generated`, and they are spelled `string`
 * below rather than imported.
 *
 * Not laziness and not a loss: both aliases are bare `export type X = string`, so the checker
 * sees the identical type either way. What the import would cost is the one property this
 * module is written for — `check-sidebar.mjs` compiles it with a bare `tsc` and no `paths`, so
 * an `@/…` specifier fails to resolve even when it is `import type` and elided from the output.
 * `sidebarWidth.ts` and `sidebarView.ts` are import-free for the same reason and say so.
 *
 * The relative `import type` above is fine because `sidebarView.ts` is compiled in the same
 * invocation, and being type-only it leaves no `require` in the emitted JavaScript for node to
 * choke on.
 */

// --- revealing a panel ----------------------------------------------------------------------

/** What the shell does when something asks for a panel. `App.tsx` registers exactly one. */
export type PanelHost = (view: PanelView) => void

/**
 * A single slot, not a listener list.
 *
 * Two shells answering one request is two sidebars moving, and a second registration winning
 * silently is far easier to notice than a stack that quietly reveals twice. Same reasoning as
 * `registerTagDialog`, which this pair is modelled on.
 */
let host: PanelHost | null = null

/**
 * Install the window's revealer, or `null` on unmount.
 *
 * Passing `null` matters: a window that has gone away must not stay registered, or the next
 * `requestPanel` calls a `setSidebar` belonging to a dead React root. Registration is *not* a
 * side effect of the module being imported — a detached-pane window imports this file too and
 * has no sidebar at all, and it must answer `false` rather than swallow the gesture.
 */
export function registerPanelHost(reveal: PanelHost | null): void {
  host = reveal
}

/**
 * Can anything in this window reveal a panel?
 *
 * For the callers whose refusal is a *sentence* rather than a reveal — `file.reveal` says "this
 * window has no file tree, so there is nothing to select a file in" and must say it before it
 * does any other work. Everything else should just read `requestPanel`'s answer.
 */
export function panelHostPresent(): boolean {
  return host !== null
}

/**
 * Reveal `view`, from anywhere.
 *
 * `false` means **nothing in this window can**, and the caller is expected to say so rather
 * than assume the gesture landed. It is never "the panel refused": `showPanel` always reveals,
 * which is `chrome/sidebarView.ts`'s rule and the reason a command that names a panel cannot
 * toggle one shut.
 */
export function requestPanel(view: PanelView): boolean {
  if (host === null) return false
  host(view)
  return true
}

// --- amending, from the git log ---------------------------------------------------------------

/**
 * *Amend…* on the log's tip commit, on its way to the Git panel's commit box.
 *
 * Everything the panel needs to keep the item's promise — Amend ticked, the message prefilled,
 * and the oid on the wire so the backend can refuse if HEAD moved in between.
 */
export interface AmendRequest {
  /**
   * The project the log row came from — a `ProjectId`.
   *
   * Carried so the panel can refuse a request that is not about the project it is showing. Two
   * projects can be open with the sidebar on one of them, and prefilling a commit message from
   * another project's repository would be a message about work that is not on screen.
   */
  readonly project: string
  /**
   * The repository the row came from — a `RepoId`, not a path.
   *
   * Per repository, not per project: a merged log walk interleaves every root, each root has
   * its own tip, and `CommitRequest.amendOf` is checked against one repository's HEAD.
   */
  readonly repo: string
  /** The full oid. This is what goes on the wire as `CommitRequest.amendOf`. */
  readonly oid: string
  /**
   * The log's own abbreviation of `oid` — `CommitRow.shortOid`.
   *
   * Carried rather than derived here so the dialog and the checkbox name the row the user
   * right-clicked, in the spelling the log drew it with. `logModel::shortenOid` owns that rule
   * and a second copy of it here would be a second abbreviation to keep in step.
   */
  readonly shortOid: string
  /**
   * `CommitDetail.message` — the **full** message, summary line and body, verbatim.
   *
   * Not the summary: `show.rs` says why the wire carries the whole thing, and an amend that
   * silently dropped a commit's body would rewrite history by deletion. The box is a textarea
   * for exactly this reason.
   */
  readonly message: string
}

/**
 * How long a parked amend stays worth honouring.
 *
 * The same number and the same argument as `editor/revealRequest.ts::REVEAL_TTL_MS`, which is
 * the module this half is modelled on. Without it, a request parked for a Git panel the user
 * never opened would fire minutes later when they open that panel for an unrelated reason —
 * ticking Amend and replacing whatever is in the box, for a menu click they have forgotten. It
 * is also the cheap half of a correctness problem: HEAD moves, and an oid parked long enough is
 * a prefill for a commit that is no longer the tip. (The expensive half is
 * `cide_git::commit::require_amend_head`, which refuses rather than trusting this.)
 *
 * Ten seconds is generous for what actually happens: park, reveal, one React commit. The user
 * reading the "replace your draft?" confirmation does *not* spend it — the panel claims the
 * request before it opens that dialog and holds the answer in its own state.
 */
export const AMEND_TTL_MS = 10_000

interface Parked {
  readonly request: AmendRequest
  /** `Date.now()` at the request, for [`AMEND_TTL_MS`]. */
  readonly at: number
}

/**
 * One slot, not a queue.
 *
 * A second *Amend…* while one is unclaimed is the same gesture repeated — the user clicked
 * twice, or clicked a different row — and the newer one is the one they meant. `revealRequest`
 * keeps eight because it is keyed by path and a burst of search-result clicks is real; there is
 * exactly one commit box in a window, so a queue here could only ever mean "prefill it twice".
 */
let parked: Parked | null = null

const listeners = new Set<() => void>()

function notify(): void {
  // Copied before the walk: a listener is free to unsubscribe from inside itself, and mutating
  // the set under its own iterator is how the second subscriber silently stops being told.
  for (const listener of [...listeners]) listener()
}

/**
 * Watch for a parked amend. Returns the unsubscriber.
 *
 * Paired with [`amendPending`] for `useSyncExternalStore`, which is how `useGitPanel` reads it —
 * the same three-function shape `sidebar/GitPanel/partialStore.ts` uses, and for the same
 * reason: the panel is a hook, the store is a module, and React needs to be told when the
 * module changed under it.
 */
export function subscribeAmendRequest(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/**
 * Is there an unclaimed amend? The `getSnapshot` half.
 *
 * A boolean and not the request itself, deliberately. `useSyncExternalStore` re-renders whenever
 * the snapshot is not `Object.is`-equal to the last one, so returning the object would be fine
 * only for as long as nobody rebuilt it — and the *value* is never rendered anyway. It is read,
 * decided upon and consumed inside an effect, which is a different thing from being displayed.
 *
 * Staleness is deliberately **not** applied here: this must be pure, and a snapshot that changed
 * its answer as the clock passed a deadline would flip without notifying anybody. [`claimAmend`]
 * is where the TTL is enforced, because a claim is allowed to have effects.
 */
export function amendPending(): boolean {
  return parked !== null
}

/**
 * The same answer, for `useSyncExternalStore`'s third argument.
 *
 * React refuses to render a store-reading component on the server without one, and that is not
 * hypothetical: `ui/scripts/check-git-render.mjs` renders the whole Git panel under node.
 * Nothing can have asked for an amend at that point, so the module's own answer is the honest
 * one — and reusing the function keeps it one identity rather than two.
 */
export const amendPendingServer = amendPending

/**
 * Ask for the Git panel, with Amend ticked and this commit's message in the box.
 *
 * The one call the git log's *Amend…* makes. `false` means this window has no sidebar, so
 * nothing was parked and nothing was revealed — the caller should say so rather than leave a
 * request nobody can ever answer.
 *
 * Parked **before** the reveal. The order is invisible today, because `showPanel` goes through
 * a `useState` setter and the panel therefore cannot mount until the next React commit; it is
 * written this way so that a future host which reveals synchronously finds the payload already
 * there instead of mounting a panel that claims nothing and then being told.
 *
 * `now` is a parameter so the staleness rule can be exercised without a fake clock — the same
 * seam `claimReveal` has, and the reason both are testable at all.
 */
export function requestAmend(request: AmendRequest, now: number = Date.now()): boolean {
  if (host === null) return false
  parked = { request, at: now }
  notify()
  host('git')
  return true
}

/**
 * Take the parked amend, or `null` when there is none worth honouring.
 *
 * **Consumed, not merely observed** — the rule `focusRequests.ts` states and the bug it names:
 * the Git panel unmounts every time the user clicks another icon in the activity rail, and a
 * request that survived would mean that merely *looking* at the Git panel later ticked Amend and
 * rewrote the commit box. A pulse counter would have exactly that bug; a claim does not.
 *
 * An expired request is spent by being looked at, rather than left to be spent later. Same rule
 * as `takeParked` in `editor/revealRequest.ts`: the alternative leaves a stale entry that the
 * *next* mount would examine and discard, which is one more render carrying a decision that has
 * already been made.
 */
export function claimAmend(now: number = Date.now()): AmendRequest | null {
  const held = parked
  if (held === null) return null
  parked = null
  notify()
  return now - held.at > AMEND_TTL_MS ? null : held.request
}

// --- what to do about a message the user is already writing ------------------------------------

/**
 * What the panel should do with an amend request, given what is already in the box.
 *
 * A rule and not a branch inside an effect, for the reason this whole file is import-free: the
 * one thing this flow can do that has no undo is throw away a commit message somebody was
 * halfway through, and a rule that lives in a React effect is a rule no check script can run.
 */
export type AmendPlan =
  /** The box is empty. Tick Amend, fill it in, and say nothing. */
  | { readonly kind: 'adopt' }
  /**
   * The box holds a draft. Ask first — and adopt only if the answer is yes.
   *
   * The three strings are everything `chrome/ConfirmDestructive.tsx` draws that is specific to
   * this act; the panel supplies the `run` and the empty file list.
   */
  | {
      readonly kind: 'confirm'
      readonly title: string
      readonly body: string
      /** The word on the destructive button. */
      readonly confirmLabel: string
    }

/**
 * Decide, and say why the losing options lost.
 *
 * Three answers were possible for "the user has already typed a commit message":
 *
 *  * **Append.** Destroys nothing, and produces a commit message with two subject lines in it —
 *    silently wrong, in the one file format where the first line is load-bearing. It trades a
 *    visible loss for an invisible corruption, which is the worse trade.
 *  * **Refuse with a sentence.** Safe, cheap, and makes the user clear the box by hand and come
 *    back to the log to repeat a gesture the app has already understood. It is also a refusal
 *    for something the app is perfectly able to *ask* about, which is the shape of "reachable
 *    but does not do the thing it names" that this whole round exists to remove.
 *  * **Ask.** One click either way, the draft named in the dialog so the user can see what they
 *    would be giving up, and Cancel — focused, accented, bound to Escape — leaves everything
 *    exactly as it was. `ConfirmDestructive`'s three rules hold: it names what is at stake, it
 *    appears only when something is at stake, and the reflexive Enter backs out.
 *
 * Asking wins, and the dialog is `ConfirmDestructive` rather than a new one for the reason that
 * component's header gives at length: a second confirmation is a second place where Cancel has
 * to be the safe default, and the moment there are two, one of them stops being it.
 *
 * **Cancel drops the whole request**, not just the prefill. Ticking Amend while leaving the
 * user's own draft in the box would arm a rewrite of HEAD with a message written for something
 * else — an outcome nobody asked for, produced by pressing Cancel. The panel is revealed either
 * way, because revealing loses nothing.
 *
 * `trim()`, so a box holding only the whitespace a stray keystroke left is treated as empty.
 * That is not a draft, and stopping to ask about it would train the user to click through this
 * dialog without reading it — which is how a confirmation stops being one.
 */
export function planAmend(draft: string, request: AmendRequest): AmendPlan {
  if (draft.trim() === '') return { kind: 'adopt' }
  return {
    kind: 'confirm',
    title: 'Replace the commit message?',
    // Both messages are named — the one that would be lost and the one that would replace it —
    // because the user cannot see the second one yet and cannot get the first one back.
    body:
      `The commit box already holds a message you have not committed: “${quoteDraft(draft)}”. `
      + `Amending ${request.shortOid} would replace it with “${quoteDraft(request.message)}”. `
      + 'A draft exists nowhere else — Cancel keeps it, and the Amend checkbox is left alone.',
    confirmLabel: `Use ${request.shortOid}’s message`,
  }
}

/**
 * How long a quoted message may run inside a sentence.
 *
 * Long enough that a conventional subject line — git's own soft limit is 50, and 72 is the wall
 * everything wraps at — arrives whole, so the ordinary case is quoted rather than trimmed.
 */
export const QUOTE_LIMIT = 72

/**
 * A commit message as a phrase inside a sentence.
 *
 * Newlines and runs of spaces collapse to one space, because the dialog's body is a paragraph
 * and a raw `\n` renders as nothing at all there — a two-paragraph message would appear as two
 * sentences jammed together with no gap, which reads as a quote of something the user did not
 * write.
 *
 * It deliberately does **not** go in `ConfirmState.files`, which is where this dialog names what
 * is at stake everywhere else. That list runs each entry through `basename`/`dirname`: a draft
 * reading `fix: handle a/b paths` would be drawn as the file `b paths` in the directory
 * `fix: handle a`. A dialog that misquotes the very text it is asking permission to destroy is
 * worse than one that does not show it, so the quote goes in the body and the list stays empty.
 */
export function quoteDraft(message: string): string {
  const flat = message.replace(/\s+/g, ' ').trim()
  return flat.length <= QUOTE_LIMIT ? flat : `${flat.slice(0, QUOTE_LIMIT - 1)}…`
}

/**
 * Test seam: forget anything parked and unregister the host. Never called by the app.
 *
 * `keys/dispatch.ts::__clearParkedTag` is the precedent. Module state outlives an assertion, and
 * a check that had to claim its way back to a clean slate would be asserting on the cleanup as
 * much as on the rule.
 */
export function __resetPanelRequests(): void {
  parked = null
  host = null
  settingsHost = null
  listeners.clear()
}

// --- the Settings screen in a window with no project ------------------------------------------

/**
 * Show the projectless Settings screen. (M74)
 *
 * The third registered opener in this file's shape, for the same reason as the first: revealing
 * it is a `useState` setter in `App.tsx`, and `keys/dispatch.ts` is not inside React. Without
 * this seam `settings.open` and `settings.keymap` could only refuse with nothing open — which
 * is what they did, and why they carried a `projectOpen` clause that has now gone.
 *
 * The section is a `string` rather than a `SettingsSection` for the reason stated at the top of
 * this file: it must resolve under a bare `tsc` with no `paths`, and `@/ipc/generated` would
 * not. `null` means *wherever it was left* — `settings/SettingsTab.tsx` holds that memory.
 */
export type SettingsFrameHost = (section: string | null) => void

/** One slot, not a listener list — `registerPanelHost`'s rule, and for its reason. */
let settingsHost: SettingsFrameHost | null = null

/**
 * Install the window's Settings opener, or `null` on unmount.
 *
 * Registered only by a shell window. A detached pane or tab window imports this module too and
 * has no work area to draw the screen in, so it must answer `false` rather than swallow the
 * gesture — `registerPanelHost` says the same thing about the sidebar.
 */
export function registerSettingsFrameHost(show: SettingsFrameHost | null): void {
  settingsHost = show
}

/**
 * Show it, from anywhere. `false` means this window cannot, and the caller should say so.
 *
 * Note what this does **not** do: toggle. `chrome/sidebarView.ts` records the rule — a command
 * that names a surface reveals it — and it matches the gesture with a project open, where
 * `settings.open` opens or re-activates the Settings tab and never closes it.
 */
export function requestSettingsFrame(section: string | null): boolean {
  if (settingsHost === null) return false
  settingsHost(section)
  return true
}
