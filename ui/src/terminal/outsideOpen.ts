/**
 * "This file is not in your project. Open it anyway?" — the decision half of that dialog.
 *
 * `terminal_open_path` refuses a path outside every root, and since M13 that refusal is a
 * *question* rather than a verdict: the user asked for out-of-project files to open, and they
 * do, once — per click, by name, having read the path. This module decides when that question
 * may be asked at all and what it says; `chrome/outsideOpenStore.ts` holds the pending one and
 * `chrome/ConfirmDestructive.tsx` draws it.
 *
 * Split out for the reason `chrome/pasteConfirmModel.ts` is split out of `PasteConfirm.tsx`, and for
 * the reason this project has now paid for twice: a rule that lives in a `useCallback` or a
 * `.catch` is in the one place no check script can compile. `ui/scripts/check-outside-open.mjs`
 * compiles this file on its own and runs the rules below, so it is DOM-free and import-free —
 * [`OpenRefusal`] is a structural subset of what `cmd::file::TerminalOpenError` serialises,
 * declared rather than imported, and `client.ts` checks the real one against it.
 *
 * # The three rules, and the failure each one prevents
 *
 * 1. **Only a refusal the user can actually overrule may ask.** `kind === 'outside'` *and*
 *    `real !== null`. Rust sends `real` for exactly one case — a well-formed absolute path that
 *    resolves to a real, regular, small-enough file that simply is not in the project — and
 *    sends `null` for a malformed one, which no approval can ever satisfy. Every other refusal
 *    (a directory, a device node, a FIFO, a 2 GB log, a path that no longer exists) is a
 *    sentence in the notice stack and always was. An *Open anyway* button on a refusal that
 *    would be refused again is the dead control this whole feature is careful not to become —
 *    and, worse than dead, it teaches the user that approving is how you make cide stop
 *    complaining.
 * 2. **It names the full path, and the real target when they differ.** A basename would defeat
 *    the entire gate: `credentials.json` looks like a project file and
 *    `/home/you/.claude/.credentials.json` does not. When the click resolves through a symlink,
 *    *both* are named, because the interesting case is a link inside the project pointing out of
 *    it — where the path on screen looks completely ordinary and the target is the point.
 * 3. **The approval is bound to the target, not to the string.** [`OutsideAsk.target`] is the
 *    canonical path the dialog showed, and it goes back to Rust as `approvedTarget`, which
 *    re-canonicalises and compares. Swap the symlink between the question and the answer and the
 *    answer no longer applies — the user is asked again, about what is actually there.
 *
 * # What is deliberately not here
 *
 * **No "don't ask again", and no per-directory memory.** One approval becoming a standing
 * capability for every later line naming a sibling file is precisely what the gate exists to
 * prevent, and terminal output is attacker-influenced by definition: the second line is written
 * by whoever wrote the first. If it is ever wanted it belongs in Rust as durable state, scoped
 * per project and listed in Settings where it can be revoked — not as a boolean in a webview
 * that no window but this one can see.
 */

/** The subset of `cmd::file::TerminalOpenError`'s wire form these rules read. */
export interface OpenRefusal {
  /** `outside` | `missing` | `notAFile` | `tooLarge` | `failed`. */
  readonly kind: string
  /** The sentence Rust wrote. Shown verbatim when this is not a question. */
  readonly message: string
  /** The path as it was asked for — the string the user ctrl+clicked. */
  readonly path: string
  /**
   * The canonical path the click would open, or `null`.
   *
   * `null` is not "unknown": it is Rust saying *there is nothing here to approve*. See rule 1.
   */
  readonly real: string | null
}

/** The confirmation, in the shape `chrome/ConfirmDestructive.tsx`'s `ConfirmState` takes. */
export interface OutsideAsk {
  readonly title: string
  readonly body: string
  /** Every path at stake, named in full. One, or two when a symlink moved it. */
  readonly files: readonly string[]
  readonly confirmLabel: string
  /** The list glyph. `↗` — leaving the project — never the removal dash. */
  readonly mark: string
  /** The canonical path this approval is for. Goes back as `approvedTarget` unchanged. */
  readonly target: string
}

/**
 * The question to ask about this refusal, or `null` when there is none and the message stands.
 *
 * `unknown` in, because it is called from a `.catch` and a rejection is whatever Rust or the
 * bridge produced. Anything that is not the shape above answers `null`, which lands the caller
 * on `notifyFailure` — the honest outcome for a refusal nobody can parse.
 */
export function outsideAsk(reason: unknown): OutsideAsk | null {
  const refusal = asRefusal(reason)
  if (refusal === null) return null
  // Rule 1. Both halves: a non-`outside` kind is final, and an `outside` with no target is a
  // malformed path — not absolute, or carrying a `..` component — that Rust refuses whatever
  // anyone approves.
  if (refusal.kind !== 'outside') return null
  const target = refusal.real
  if (target === null || target === '') return null

  const moved = target !== refusal.path
  return {
    title: 'Open a file from outside this project?',
    body: moved
      ? 'This path is not inside any of the project’s roots, and it resolves somewhere else ' +
        'again. cide will open the file at the second path below. Terminal output is written ' +
        'by whatever is running in the pane, so check both before opening.'
      : 'This path is not inside any of the project’s roots. Terminal output is written by ' +
        'whatever is running in the pane — a build log, a tool result, an agent — so check the ' +
        'path before opening it.',
    files: moved ? [refusal.path, target] : [refusal.path],
    confirmLabel: 'Open anyway',
    mark: 'arrow-up-right',
    target,
  }
}

/**
 * The refusal, if this rejection is one.
 *
 * Written out rather than cast, because the one thing this module must never do is invent a
 * `real` for a refusal that did not carry one — a `reason as OpenRefusal` would happily give
 * `undefined` a pass through rule 1's `!== null`.
 */
function asRefusal(reason: unknown): OpenRefusal | null {
  if (typeof reason !== 'object' || reason === null) return null
  const it = reason as Record<string, unknown>
  if (typeof it['kind'] !== 'string') return null
  if (typeof it['message'] !== 'string') return null
  if (typeof it['path'] !== 'string') return null
  const real = it['real']
  if (real !== null && typeof real !== 'string') return null
  return { kind: it['kind'], message: it['message'], path: it['path'], real }
}

/*
 * Runtime values only, no imports: `check-outside-open.mjs` compiles this file alone with a bare
 * `tsc` and loads the emitted `.js` in node. Adding any import — the generated `TerminalOpenError`
 * type, `ConfirmState` from the component — breaks that, and the check would then fail with a
 * module-resolution error rather than telling anyone what actually broke. The same note is at the
 * foot of `terminal/pathMatch.ts`, `chrome/notices.ts` and `chrome/branchModel.ts`.
 */
