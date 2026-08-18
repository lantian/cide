/**
 * The one resolution behind Ctrl+hover and Ctrl+click, and what the click does with it.
 *
 * # The invariant this module exists to hold
 *
 * The underline that appears under Ctrl is **a promise about what the click will do**. If the two
 * can disagree the feature is worse than neither of them: an affordance that lies is one the user
 * learns to ignore, and then the gesture it was advertising goes unused as well.
 *
 * They are kept in agreement structurally rather than carefully:
 *
 * 1. [`resolveWord`] is the **only** function in the app that calls `diagnostics.probe`. Both
 *    gestures go through it. `check:editor` asserts the call appears exactly once.
 * 2. Both normalise the pointer to the same [`WordTarget`] — the word range from CodeMirror's own
 *    `wordAt` — so both compute the same cache key and hit the same entry.
 * 3. Both ask [`intent`] what the answer means. The hover underlines iff it is not `'none'`; the
 *    click switches on the same three values. There is no second list of "underlinable" kinds.
 * 4. Both gestures live in one module, `ctrlLink.ts`, as two handlers on one CodeMirror extension.
 *    They cannot drift into two files that each grew their own idea of the rule.
 *
 * The one asymmetry, stated because it is deliberate: a hover asks with a 600 ms deadline and a
 * click with five seconds, and [`usable`] refuses to let a hover's *"could not be asked"* answer a
 * click. So **the hover can never underline something the click will not act on; the click may act
 * where the hover stayed quiet.** Wrong in the safe direction.
 *
 * # What is here and what is next door
 *
 * Every *decision* is in `codeIntelGate.ts`, which imports nothing and is compiled and driven by
 * `ui/scripts/check-editor.mjs`. What is left here is the IPC, the caches and the timers — the
 * parts a headless check could not run anyway. That split is the same one `jump.ts`/`navHistory.ts`
 * and `find.ts`/`findMatches.ts` already make, and it exists because the two bugs this project has
 * paid most for both hid in a rule written inside an event handler.
 */
import {
  diagnostics as diagnosticsApi,
  file as fileApi,
  type ProjectId,
  type UsagesAnswer,
} from '@/ipc/client'
import { notify } from '@/chrome/notices'
import { closeOverlay, showOverlay, useOverlays } from '@/overlays/store'
import {
  beginUsages,
  cancelUsages,
  failUsages,
  isCurrentUsages,
  showUsages,
} from '@/overlays/usagesStore'
import { noSymbolSentence, noUsagesSentence, type UsagesKind } from '@/overlays/usagesModel'
import { goToDefinition } from './goToDefinition'
import { jumpTo } from './jump'
import {
  CLICK_TIMEOUT_MS,
  HOVER_TIMEOUT_MS,
  intent,
  keyFor,
  keyPrefix,
  recall,
  remember,
  usable,
  type ProbeKind,
  type Resolution,
} from './codeIntelGate'

/**
 * The word under a pointer or a caret, as both gestures see it.
 *
 * `column` is the word's **start**, not the pointer's own column, and that is what makes the cache
 * key and the request agree: one word is one question however many pixels of it the pointer has
 * crossed. The Rust discriminator's containment test is half-open on purpose so that asking at the
 * first character of a declaration still lands inside its own range.
 */
export interface WordTarget {
  /** Document offsets, for the decoration and for the cache key. */
  readonly from: number
  readonly to: number
  /** 1-based. */
  readonly line: number
  /** 1-based UTF-16, at [`from`]. */
  readonly column: number
  /** The identifier itself, for the popup's heading and the empty-result sentence. */
  readonly text: string
}

/**
 * Per-path document generation, bumped on every change to that buffer.
 *
 * Any edit invalidates every answer for the file — an added `use`, a renamed local, the agent
 * rewriting it underneath — so the generation goes into the cache key and old entries become
 * unreachable rather than being hunted down. Deliberately **not** a range map through the change
 * set: a definition answer is not invalidated only by an edit inside the word.
 *
 * Module-level and keyed by path, like `outlineStore`'s and `docSync`'s caches, so two split panes
 * over one file share both the generation and the answers. Per-view would give the same file two
 * caches that disagree about which of them is current.
 */
const generations = new Map<string, number>()

/** Answers, keyed by `path + docGen + word range`. Bounded and LRU — see `codeIntelGate.ts`. */
const resolutions = new Map<string, Resolution>()

/**
 * Requests that have gone out and not come back, so two views over one file — or a click landing
 * on the word the pointer is already settled on — share one round trip instead of racing.
 */
const inFlight = new Map<string, Promise<Resolution>>()

/** The buffer changed. Called from the update listener in `ctrlLink.ts`. */
export function bumpDocGeneration(path: string): void {
  generations.set(path, (generations.get(path) ?? 0) + 1)
}

/** This buffer's generation, for the cache key. */
export function docGeneration(path: string): number {
  return generations.get(path) ?? 0
}

/**
 * Forget everything about one file. Called when the last editor for it goes away.
 *
 * The LRU would evict these eventually; doing it on unmount is what keeps a long session from
 * carrying answers about files that are no longer open. The generation goes too — a file reopened
 * later starts from zero, and it starts with an empty cache, so there is nothing for the reused
 * numbers to collide with.
 */
export function forgetCodeIntel(path: string): void {
  generations.delete(path)
  const prefix = keyPrefix(path)
  for (const key of [...resolutions.keys()]) {
    if (key.startsWith(prefix)) resolutions.delete(key)
  }
  for (const key of [...inFlight.keys()]) {
    if (key.startsWith(prefix)) inFlight.delete(key)
  }
}

/** Testing seam: forget every cached answer. Never called by the app. */
export function resetCodeIntelForTest(): void {
  generations.clear()
  resolutions.clear()
  inFlight.clear()
}

/**
 * Read a cached answer, if there is one this caller may believe. Never asks.
 *
 * The hover's fast path — a pointer re-entering an identifier it has already crossed draws the
 * underline on the same frame, with no request and no settle delay.
 */
export function cachedResolution(
  path: string,
  word: WordTarget,
  wantMs: number,
): Resolution | undefined {
  const key = keyFor(path, docGeneration(path), word.from, word.to)
  const hit = recall(resolutions, key)
  if (hit === undefined) return undefined
  return usable(hit, wantMs, Date.now()) ? hit : undefined
}

/**
 * Resolve what a Ctrl gesture on this word would do.
 *
 * **The only caller of `diagnostics.probe` in the app.** Both gestures come here; see the module
 * header for why that is the whole of the agreement between them.
 *
 * `wantMs` is the deadline the caller is willing to wait, and it is also what decides whether a
 * cached *"could not be asked"* counts as an answer — see `usable`.
 *
 * Never rejects. A transport failure becomes an `unavailable` with the thrown text, because every
 * caller of this either draws nothing or shows a sentence, and neither has anywhere to put an
 * exception.
 */
export function resolveWord(
  project: ProjectId,
  path: string,
  word: WordTarget,
  wantMs: number,
): Promise<Resolution> {
  const generation = docGeneration(path)
  const key = keyFor(path, generation, word.from, word.to)

  const hit = recall(resolutions, key)
  if (hit !== undefined && usable(hit, wantMs, Date.now())) return Promise.resolve(hit)

  const running = inFlight.get(key)
  // Only if it would answer this caller. A hover's short-deadline request in flight is not an
  // answer for a click, which would otherwise inherit a `Timeout` it never agreed to.
  if (running !== undefined && wantMs <= HOVER_TIMEOUT_MS) return running

  const asking = diagnosticsApi
    .probe(project, path, word.line, word.column, wantMs)
    .then((answer): Resolution => {
      const resolution: Resolution = {
        kind: answer.kind as ProbeKind,
        target:
          answer.kind === 'definition'
            ? {
                path: answer.path,
                line: answer.line,
                column: answer.column,
                interfaceMethod: answer.interfaceMethod,
              }
            : undefined,
        reason: answer.kind === 'unavailable' ? answer.reason : undefined,
        askedMs: wantMs,
        at: Date.now(),
      }
      /*
       * Only remembered while the buffer has not moved on.
       *
       * The request was in flight for up to five seconds, and the user may well have typed. The
       * key still names the *old* generation, so writing it would be harmless but useless — it
       * would be a dead entry taking a slot in the LRU. Comparing is cheaper than evicting.
       */
      if (docGeneration(path) === generation) remember(resolutions, key, resolution)
      return resolution
    })
    .catch(
      (error: unknown): Resolution => ({
        // Not cached under a generous deadline: a transport failure is not evidence about the
        // server, and pinning it as the answer for the next ten seconds would suppress the retry
        // that would have worked.
        kind: 'unavailable',
        reason: `The lookup failed: ${String(error)}`,
        askedMs: 0,
        at: Date.now(),
      }),
    )
    .finally(() => {
      // Only if it is still ours. A click that arrived while a hover's short-deadline request was
      // in flight starts its own — see above — and a blind delete here would forget *that* one,
      // so a second click on the same word would start a third.
      if (inFlight.get(key) === asking) inFlight.delete(key)
    })

  inFlight.set(key, asking)
  return asking
}

/**
 * Put a sentence on screen through the failure channel.
 *
 * `chrome/Failures.tsx` listens for `unhandledrejection`, which is how a keystroke's outcome
 * becomes visible without a `.catch` at every call site — the same trick `goToDefinition.ts` uses
 * and for the same reason: a gesture that silently does nothing is indistinguishable from one
 * wired to nothing.
 *
 * Used only for "the server could not be asked". A *result* — no usages, no declaration — goes
 * through `notify` as an `info` instead, because `role="alert"` in red for an answer the user
 * asked for is the accessibility equivalent of a modal dialog announcing a success.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * Ctrl+click, and the whole of what it means.
 *
 * Fire-and-forget; every outcome reports itself. The three branches are `intent`'s three values and
 * nothing else, which is what stops this drifting from the underline.
 */
export function ctrlActivate(project: ProjectId, path: string, word: WordTarget): void {
  void resolveWord(project, path, word, CLICK_TIMEOUT_MS).then((answer) => {
    switch (intent(answer.kind)) {
      case 'jump': {
        const target = answer.target
        if (target === undefined) return
        /*
         * `jumpTo` **before** `file.open`, which is the design and not a preference — the whole
         * argument is written out in `goToDefinition.ts`: a definition usually lives in a file no
         * pane has open, so the reveal is parked and spent by the mount the open causes.
         * Reversed, the tab opens at line 1 and the caret never moves.
         *
         * Through `jumpTo` rather than `requestReveal`, so the place the user jumped *from* is on
         * the Back stack. That matters more here than on Ctrl+B: the discriminator can decide
         * "reference" about something the user considered a declaration — `fn fmt` in an
         * `impl Display` resolves to the trait's — and Back is what makes that recoverable.
         */
        /*
         * The Go interface case, kept identical to Ctrl+B's. (M18)
         *
         * `interfaceMethod` says the declaration this click would open is a method inside an
         * `interface { … }`, which is the one position where the protocol's correct answer is
         * not the one the user meant. `goToImplementation` handles 0 / 1 / many itself and falls
         * back to the declaration when nothing implements it, so nothing is lost — this is a
         * better first guess, not a different feature.
         *
         * Deliberately *after* the `target === undefined` guard and before the jump, so the Back
         * stack records the same thing either way.
         */
        if (target.interfaceMethod) {
          goToImplementation(project, path, word.line, word.column, word.text)
          return
        }
        jumpTo(project, { path: target.path, line: target.line, column: target.column })
        // Deliberately uncaught: `Failures` reports it. A `.catch(() => {})` here is precisely
        // what would turn a definition that resolved and then failed to open into silence.
        void fileApi.open(project, target.path)
        return
      }
      case 'usages':
        findUsages(project, path, word.line, word.column, word.text)
        return
      case 'none':
        if (answer.kind === 'notFound') {
          report('No declaration found for what is under the caret.')
          return
        }
        report(answer.reason ?? 'The language server could not be asked.')
    }
  })
}

/**
 * How long to wait before showing a popup that says it is searching.
 *
 * A references search against a warm server on an ordinary symbol answers in well under this, and
 * then the 0/1/≥2 decision happens with **nothing having been drawn** — so the single-usage jump
 * looks instantaneous and an empty result never flashes a popup. Past it the search is slow enough
 * that silence would read as the keystroke having done nothing, which is the failure this whole
 * family of features exists to end.
 *
 * The same 150 ms the hover settles for, and for a related reason: it is the shortest delay that is
 * not perceived as a flash.
 */
const GRACE_MS = 150

/**
 * Find usages: ⌥F7, the context menu, and the declaration branch of Ctrl+click.
 *
 * `name` may be `null` — `keys/dispatch.ts` has a caret and no `EditorView`, so it reads the word
 * through `caretTrack`'s reader and that reader may legitimately answer nothing. Every sentence
 * below degrades to "the symbol".
 *
 * # The 0 / 1 / ≥2 rule
 *
 * * **0** — a sentence, never a silent no-op, and an `info` notice rather than a failure: "used
 *   nowhere" is a *result*.
 * * **1** — jump, no popup, unconditionally. Even when the answer arrives after the popup went up:
 *   a popup that was on screen for 200 ms and then jumps is still the behaviour that was asked
 *   for, and special-casing it would mean two ways of arriving at one usage.
 * * **≥2** — the popup.
 *
 * "The same file" is deliberately **not** special-cased. A split can show that path in another
 * pane, a background tab is the case `jump.ts` documents at length, and `file.open` on an
 * already-open path is the activation rather than a duplicate tab. One path is one thing to keep
 * correct.
 */
export function findUsages(
  project: ProjectId,
  path: string,
  line: number,
  column: number,
  name: string | null,
): void {
  locationQuery({
    project,
    path,
    line,
    column,
    name,
    kind: 'usages',
    ask: () => diagnosticsApi.usages(project, path, line, column),
    // A caret on a keyword, a comment, punctuation. There is no symbol, so there is nothing to
    // fall back to and the honest answer is a sentence.
    onNoSymbol: () => notify(noSymbolSentence(), { kind: 'info' }),
    failureLead: 'Find usages',
  })
}

/**
 * Go to implementation: Ctrl+Alt+B, and the palette's `navigate.implementation`. (M18)
 *
 * # Why this is not a mode of Go to definition
 *
 * The report was that Go to definition in Go lands on the interface. It does, and gopls is right:
 * `textDocument/definition` on a call through an interface resolves to the interface's method,
 * because that is where the callee is declared. "Take me to the concrete one" is
 * `textDocument/implementation`, a different request that cide asked nowhere.
 *
 * Making Ctrl+B try implementation first *unconditionally* would fix Go by breaking Rust —
 * rust-analyzer answers `implementation` on a struct name with its `impl` blocks and on a trait
 * with its implementors, so Ctrl+click on an ordinary type name would stop opening the
 * declaration. A separate id is what `navigate.usages` already established as this codebase's
 * answer to a gesture that has to guess.
 *
 * Ctrl+B and Ctrl+click *do* now redirect here, from exactly one position: a definition that
 * landed on a method inside an interface. That is decided in Rust by parsing the target file
 * (`cide_lang::interface_method_at`), not by looking at the language, which is what keeps every
 * type name in both languages on the declaration. This command stays because it answers the
 * question from positions the redirect deliberately never fires on.
 *
 * # The same 0 / 1 / ≥2 rule, with one difference
 *
 * **0 falls through to Go to definition** rather than reporting nothing. An interface with no
 * implementors, a caret on a plain function, a language where the distinction does not arise —
 * all of them answer empty, and in every one of them the thing the user wanted next is the
 * declaration. Without the fallback the command would be silent for the majority of positions it
 * is pressed on, which is indistinguishable from unwired.
 *
 * ≥2 is the *common* case here, unlike for definition: an interface with many implementors is the
 * normal shape, which is why this reuses the Find usages popup rather than picking one arbitrarily.
 */
export function goToImplementation(
  project: ProjectId,
  path: string,
  line: number,
  column: number,
  name: string | null,
): void {
  locationQuery({
    project,
    path,
    line,
    column,
    name,
    kind: 'implementations',
    ask: () => diagnosticsApi.implementations(project, path, line, column),
    // `notFound` is "there is no symbol at this position at all", and `[]` is "nothing implements
    // it". Both mean the same thing to a user who pressed this key: ask the other question.
    onNoSymbol: () => goToDefinition(project, path, line, column),
    onEmpty: () => goToDefinition(project, path, line, column),
    failureLead: 'Go to implementation',
  })
}

/**
 * The shared body of the two gestures above: ask, apply the 0/1/≥2 rule, report every outcome.
 *
 * One function and not two copies, because the parts that must not drift are the ones that are
 * invisible in review — the generation check that makes a late answer inert, the grace timer that
 * stops an empty popup flashing, and the *ordering* of `jumpTo` before `file.open` that
 * `goToDefinition.ts` spends a paragraph on. The parts that legitimately differ are the request,
 * the wording, and what an empty answer means, and those are the parameters.
 */
function locationQuery(spec: {
  project: ProjectId
  path: string
  line: number
  column: number
  name: string | null
  kind: UsagesKind
  ask: () => Promise<UsagesAnswer>
  /** The server resolved no symbol at this position. */
  onNoSymbol: () => void
  /** The server resolved a symbol and it has no matches. Defaults to a notice. */
  onEmpty?: (() => void) | undefined
  /** Leads the sentence for a transport failure: `Find usages failed: …`. */
  failureLead: string
}): void {
  const { project, name, kind } = spec
  const generation = beginUsages(project, name, kind)

  const grace = setTimeout(() => {
    // Only if it is still the search we started, and still running. Otherwise this is a popup
    // opening over whatever the user did in the meantime.
    if (isCurrentUsages(generation)) showOverlay('usages')
  }, GRACE_MS)

  void spec
    .ask()
    .then((answer) => {
      clearTimeout(grace)
      /*
       * Dropped on arrival if anything has moved on.
       *
       * Escape, the scrim, a picked row and a second ⌥F7 all bump the generation, so this is the
       * one check that stops a twenty-second answer re-opening a popup over an unrelated screen —
       * and it is also what makes a *cancelled* search silent, since `Unavailable { cancelled }`
       * arrives with a stale generation and never reaches the `failUsages` below.
       */
      if (!isCurrentUsages(generation)) return

      if (answer.kind === 'unavailable') {
        // Kept on screen rather than turned into a toast: the popup is already up by now in every
        // case where the wait was long enough to matter, and moving the explanation to the corner
        // would leave an empty modal behind it.
        if (useOverlays.getState().open === 'usages') {
          failUsages(answer.reason)
        } else {
          report(answer.reason)
          cancelUsages()
        }
        return
      }

      if (answer.kind === 'notFound') {
        dismiss()
        spec.onNoSymbol()
        return
      }

      const rows = answer.rows
      if (rows.length === 0) {
        dismiss()
        if (spec.onEmpty === undefined) notify(noUsagesSentence(name, kind), { kind: 'info' })
        else spec.onEmpty()
        return
      }
      const only = rows[0]
      if (rows.length === 1 && only !== undefined) {
        dismiss()
        // Reveal first, open second — `jumpTo`'s own contract. `endColumn` because a references
        // range *is* the occurrence, unlike a definition's, so selecting it is what makes "this is
        // the usage I sent you to" visible.
        jumpTo(project, {
          path: only.path,
          line: only.line,
          column: only.column,
          endColumn: only.endColumn,
          focus: true,
          align: 'center',
        })
        void fileApi.open(project, only.path)
        return
      }

      showUsages(rows, answer.truncated)
      showOverlay('usages')
    })
    .catch((error: unknown) => {
      clearTimeout(grace)
      if (!isCurrentUsages(generation)) return
      dismiss()
      report(`${spec.failureLead} failed: ${String(error)}`)
    })
}

/** Close the popup if it is up, and stop caring about anything still in flight. */
function dismiss(): void {
  if (useOverlays.getState().open === 'usages') closeOverlay()
  cancelUsages()
}
