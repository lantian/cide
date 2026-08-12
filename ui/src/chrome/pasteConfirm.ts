/**
 * What the paste confirmation asks, and what each answer means.
 *
 * > *"Paste collisions - yes, should be a confirmation"*
 *
 * The file tree used to answer a collision by itself: `main.rs` pasted onto a `main.rs` became
 * `main copy.rs`, silently. That never destroys anything, which is why it shipped, and it makes
 * the common case — *I meant to replace that file* — impossible without deleting first. This
 * module is the decision half of the dialog that fixes it; `PasteConfirm.tsx` is the markup.
 *
 * Split out for the reason `closeConfirm.ts` is split out of `CloseConfirm.tsx`: the mistakes
 * are all in here, and in here they can be tested. `ui/scripts/check-fs-clipboard.mjs` compiles
 * this file on its own and runs the rules below, so it is DOM-free and import-free —
 * [`CollisionLike`] is a structural subset of the generated `PasteCollision`, declared rather
 * than imported, and TypeScript checks the real one against it at every call site in the
 * component.
 *
 * # The four rules a file-manager confirmation usually gets wrong
 *
 * 1. **Nothing is written before the last question is answered.** `fs.paste_plan` reads; the
 *    answers are collected; only then is `fs_paste` called. So *Cancel* is a command that is
 *    never sent — there is no half-pasted folder to describe and nothing to roll back. The
 *    alternative, asking as each file is written, is the one where backing out at the fourth of
 *    seven questions leaves three files on disk and a dialog apologising for them.
 * 2. **Replace is never the default.** It is the only answer that destroys data, so it is not
 *    focused, not the accent button, and drawn in `--red`. *Keep both* takes the Enter key —
 *    it is the behaviour that shipped, and a stray `main copy.rs` can be deleted while an
 *    overwritten file cannot be brought back.
 * 3. **Ten collisions must not be ten questions.** [`answerAsk`] with `applyToRest` answers the
 *    remaining ones in the same breath, and it downgrades *Replace* to *Keep both* for any of
 *    them that [`canReplace`] refuses — an "apply to all" that silently skipped those, or that
 *    sent a decision the backend rejects, would turn one click into a failed paste.
 * 4. **A folder merge says what it costs, in numbers, before it is chosen.** See [`askBody`].
 */

/** The two answers that reach the disk. `cancel` is not one — it is the call never happening. */
export type PasteAnswer = 'replace' | 'keepBoth'

/**
 * The subset of the generated `PasteCollision` this module reads.
 *
 * `sample` and `truncated` are here because the counts alone are not an informed answer: six
 * names tell the user whether the twelve files are the ones they meant, and a count that hit
 * its walk budget has to say so rather than pretend to be exact.
 */
export interface CollisionLike {
  /** The pasted path this question is about. Carried into the decision unchanged. */
  source: string
  /** The occupied path. Shown in full, because two folders called `src` is the normal case. */
  dest: string
  /** The last component — the name the dialog says. */
  name: string
  /** Both sides are folders, so Replace means *merge*. See [`askBody`]. */
  merge: boolean
  /** Replace is refused for this one, and this sentence says why. From Rust. */
  blocked: string | null
  /** Existing files Replace would overwrite. 1 for a file, N for a folder merge. */
  replaces: number
  /** Entries a merge would leave completely alone. */
  keeps: number
  /** The first few of `replaces`, relative to the folder, for the dialog to list. */
  sample: string[]
  /** The count walk hit its budget; `replaces` and `keeps` are lower bounds. */
  truncated: boolean
}

/** One answer bound to the source it answers for — the generated `PasteDecision`'s shape. */
export interface DecisionLike {
  source: string
  choice: PasteAnswer
}

/**
 * A paste waiting on the user: every collision, and the answers given so far.
 *
 * The answers are a list rather than a map because the questions are asked in order and the
 * count of them *is* the position: `answers.length` is which collision is on screen. A map
 * keyed by path would also lose the second question when a selection names one path twice.
 */
export interface PasteAsk {
  readonly collisions: readonly CollisionLike[]
  readonly answers: readonly PasteAnswer[]
}

/** Begin asking about `collisions`. The caller only reaches here when there is at least one. */
export function startAsk(collisions: readonly CollisionLike[]): PasteAsk {
  return { collisions, answers: [] }
}

/** The collision on screen, or `null` when every one has been answered. */
export function currentCollision(ask: PasteAsk): CollisionLike | null {
  return ask.collisions[ask.answers.length] ?? null
}

/** Every question has an answer, so the paste can be sent. */
export function askIsDone(ask: PasteAsk): boolean {
  return ask.answers.length >= ask.collisions.length
}

/**
 * Whether *Replace* is offered at all for this collision.
 *
 * `false` when one side is a folder and the other is not — anywhere, including inside a folder
 * being merged. Swapping a file for a directory is not a replacement of anything, and Rust
 * refuses it whatever this returns (`FsError::CannotReplace`); this is what keeps the user from
 * meeting that refusal as a failed paste instead of as a greyed button with a reason on it.
 */
export function canReplace(collision: CollisionLike): boolean {
  return collision.blocked === null
}

/**
 * Record `answer` for the collision on screen, and optionally for every one after it.
 *
 * `applyToRest` is what makes a ten-file paste bearable. It is not a blanket policy: each
 * remaining collision is still passed through [`answerFor`], so *Replace to all* keeps both
 * copies of the one file-versus-folder clash in the list rather than sending a decision the
 * backend will refuse and failing the whole paste over it.
 */
export function answerAsk(ask: PasteAsk, answer: PasteAnswer, applyToRest: boolean): PasteAsk {
  const current = currentCollision(ask)
  if (current === null) return ask
  const answers = [...ask.answers, answerFor(current, answer)]
  if (applyToRest) {
    for (const rest of ask.collisions.slice(answers.length)) {
      answers.push(answerFor(rest, answer))
    }
  }
  return { collisions: ask.collisions, answers }
}

/** The answer this collision can actually take. Replace becomes Keep both where it is refused. */
export function answerFor(collision: CollisionLike, answer: PasteAnswer): PasteAnswer {
  return answer === 'replace' && !canReplace(collision) ? 'keepBoth' : answer
}

/**
 * The answers, in the shape `fs.paste` takes.
 *
 * Both kinds are sent, not only the replacements. `keepBoth` is already the backend's default,
 * so listing it changes nothing — but a decision list that names every source is one a reader
 * can check against the questions that were asked, and the absence of an entry then means
 * "never asked" rather than "asked and answered with the default".
 */
export function askDecisions(ask: PasteAsk): DecisionLike[] {
  const out: DecisionLike[] = []
  ask.answers.forEach((choice, at) => {
    const collision = ask.collisions[at]
    if (collision !== undefined) out.push({ source: collision.source, choice })
  })
  return out
}

/** The heading: a statement of fact, not a question with a verb in it. */
export function askTitle(collision: CollisionLike): string {
  const what = collision.merge ? 'A folder' : 'Something'
  return `${what} called “${collision.name}” is already here`
}

/**
 * The sentence under the heading — the whole reason this dialog is worth showing.
 *
 * It has to answer *what Replace costs*, and for a folder that is not obvious. **Replace on two
 * folders merges them**: files in both are overwritten, files only in the destination are left
 * alone. The other reading — remove the existing folder and put the pasted one there — is what
 * "replace" sounds like, and it deletes every file the destination had that the source happens
 * not to contain, which is a set nothing ever put on screen. So the word is spelled out here
 * along with both counts, because "replace src?" with no numbers is a dare rather than a
 * question.
 */
export function askBody(collision: CollisionLike): string {
  if (collision.blocked !== null) return collision.blocked
  if (!collision.merge) {
    return 'Replace overwrites it. What is there now is gone — it does not go to the trash.'
  }
  const kept =
    collision.keeps === 0
      ? 'the existing folder has nothing else in it'
      : `${about(collision, collision.keeps, 'item')} that only the existing folder has ${collision.keeps === 1 ? 'is' : 'are'} kept`
  if (collision.replaces === 0) {
    return `Replace merges the two folders. The two have no files in common, so nothing is overwritten, and ${kept}.`
  }
  return `Replace merges the two folders: ${about(collision, collision.replaces, 'file')} inside ${collision.replaces === 1 ? 'is' : 'are'} overwritten, and ${kept}. Files you cannot see here are not deleted.`
}

/**
 * `12 files`, or `at least 4,096 files` when the count walk gave up.
 *
 * The hedge is not decoration. An exact-looking number that is actually a lower bound is worse
 * than no number at all in a dialog whose only job is being believed.
 */
function about(collision: CollisionLike, n: number, noun: string): string {
  const counted = count(n, noun)
  return collision.truncated ? `at least ${counted}` : counted
}

/**
 * The destructive button's label. Says what it does, and how much of it.
 *
 * "Replace" alone on a folder is the label that makes the merge invisible again, which is the
 * misunderstanding [`askBody`] exists to remove.
 */
export function replaceLabel(collision: CollisionLike): string {
  if (!collision.merge) return 'Replace'
  if (collision.replaces === 0) return 'Merge'
  return `Merge, replacing ${about(collision, collision.replaces, 'file')}`
}

/** The safe button's label — and it is the default one. */
export function keepBothLabel(collision: CollisionLike): string {
  return collision.merge ? 'Keep both folders' : 'Keep both'
}

/** `3 of 7`, or `null` when there is only one question and a counter would be noise. */
export function askProgress(ask: PasteAsk): string | null {
  if (ask.collisions.length < 2) return null
  return `${Math.min(ask.answers.length + 1, ask.collisions.length)} of ${ask.collisions.length}`
}

/**
 * The *apply to all remaining* label, or `null` when this is the last question.
 *
 * Names the number rather than saying "all": a user four questions into seven needs to know it
 * covers three, not seven.
 */
export function applyToRestLabel(ask: PasteAsk): string | null {
  const rest = ask.collisions.length - ask.answers.length - 1
  if (rest < 1) return null
  return `Do the same for the remaining ${count(rest, 'file')}`
}

/**
 * The footnote in the dialog, and the strip after a cancel — deliberately the same claim.
 *
 * It is the property the whole ordering was built for, so it is worth stating twice: the paste
 * has not started. A dialog that appears mid-paste can only ever say "3 files were already
 * copied", and that sentence is the bug this replaces.
 */
export function nothingWrittenYet(): string {
  return 'Nothing has been written yet — cancelling leaves everything exactly as it is.'
}

/** What the panel says after the user backs out. */
export function cancelledNote(): string {
  return 'Paste cancelled. Nothing was written.'
}

/** `1 file` / `3 files`, with the thousands separator a four-digit count needs. */
function count(n: number, noun: string): string {
  const shown = n.toLocaleString('en-US')
  return n === 1 ? `1 ${noun}` : `${shown} ${noun}s`
}
