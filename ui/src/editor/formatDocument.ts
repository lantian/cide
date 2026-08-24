/**
 * Reformat the focused buffer — Ctrl+Alt+F. (M26)
 *
 * # The order, which is the whole correctness of it
 *
 * 1. **Refuse a read-only buffer** before anything is spawned or asked.
 * 2. **Await `flushDoc`.** `didChange` is a 300 ms trailing throttle, so the language server may
 *    be holding text up to that old. Formatting stale text does not produce a stale result — it
 *    produces a corrupt one, because the edits come back addressed against text that is not what
 *    they land on.
 * 3. **Read the buffer once**, and send exactly those bytes. The answer is computed against them.
 * 4. **Re-check before applying.** The round trip is a language server or a child process and the
 *    user's hands never left the keyboard; a character typed in the meantime makes the answer
 *    describe a document that no longer exists. Applying it would silently eat that keystroke.
 * 5. **Trim to the minimal change**, so the caret, the scroll, the folds and the undo step all
 *    survive. `formatModel.ts` has the argument at length.
 *
 * Steps 2 and 4 are two halves of one problem and neither replaces the other: the flush narrows
 * the window, the re-check closes what is left of it.
 *
 * # Fire and forget, and every outcome speaks
 *
 * Shaped like `goToDefinition.ts`, for the same reason: a keystroke has no `disabledReason` to
 * fall back on, so "nothing happened" has to be turned into something the user can read.
 * `chrome/Failures.tsx` listens for `unhandledrejection`, which is how a sentence reaches the
 * screen without a `.catch` at every call site.
 *
 * The one outcome that is deliberately **silent** is `unchanged`. Reformatting an
 * already-formatted file is what a reflexive Ctrl+Alt+F does, and a notice saying "nothing to do"
 * every time would train the user to ignore the channel that carries the real refusals.
 */
import { diagnostics as diagnosticsApi, type ProjectId } from '@/ipc/client'
import type { FormatActions } from './caretTrack'
import { flushDoc } from './docSync'
import { languageIdFor } from './languages'

/**
 * Put a sentence on screen. See `goToDefinition.ts`'s note on this channel.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * Reformat the buffer `actions` speaks for. Resolves when the round trip is over; never rejects.
 *
 * `actions` comes from `caretTrack.focusedFormat()` — the editor the user is actually in, which
 * in a split showing one file twice is the half with the selection that matters.
 */
export async function formatDocument(project: ProjectId, actions: FormatActions): Promise<void> {
  const path = actions.path()

  // Before anything is spawned or asked: a diff pane and a file with no write permission both
  // answer here, and neither is worth a round trip to be told about.
  if (actions.readOnly()) {
    report('This buffer is read-only.')
    return
  }

  /*
   * A file whose extension resolves to no grammar — a plain `.txt`, a `LICENSE`, an unknown
   * suffix. Refused here rather than sent, because *both* roads are keyed by the language id:
   * the formatter map is, and `server_for` resolves the server through the same extension
   * table. Sending an empty id would produce a true refusal from Rust with a worse sentence,
   * built around a blank where the language should be.
   */
  const languageId = languageIdFor(path)
  if (languageId === null) {
    report('cide does not recognise this file type, so it has no formatter for it.')
    return
  }

  // Load-bearing, and awaited rather than fired — see the header and `flushDoc`'s own docs.
  await flushDoc(path)

  // Read *after* the flush, so what is sent is what the server was just told. Reading before
  // would reintroduce the gap the flush exists to close, in the one place it is invisible.
  const sent = actions.text()
  const range = actions.selection()

  let answer
  try {
    answer = await diagnosticsApi.format(project, path, languageId, sent, range)
  } catch (error) {
    // `format_document` never returns `Err`, so this is `invoke` itself failing — the window
    // tearing down, or the project closing while the request was in flight. Neither is worth a
    // sentence, and both are reachable by ordinary use.
    void error
    return
  }

  switch (answer.kind) {
    case 'unavailable':
      // The sentence is Rust's and is shown verbatim: it names the language, the formatter, or
      // the tool's own first line of stderr. See `cide_ipc::FormatAnswer`.
      report(answer.reason)
      return
    case 'unchanged':
      // Deliberately silent. See the header.
      return
    case 'formatted':
      if (!actions.apply(sent, answer.text)) {
        report('The buffer changed while it was being formatted, so nothing was applied.')
      }
      return
  }
}
