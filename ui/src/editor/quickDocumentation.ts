/**
 * Quick documentation: a page about the symbol under the caret, in a tab. (M60)
 *
 * # Why a page and not a popup
 *
 * The page is the same for every language — LSP hover is markdown by contract, and Godot's
 * BBCode converts to it in Rust — but what it *contains* differs by an order of magnitude: a
 * rustdoc paragraph and Godot's `Node` with its hundred members. A popup fits the first and
 * scrolls the second into uselessness; a tab fits both, keeps its place, and can be followed
 * from (a class page links to its members, and each of those is a page). So a documentation
 * request opens a tab, or activates the one already about the same subject.
 *
 * # One function, three callers
 *
 * F1 through the command registry, the context menu, and — the case that started this —
 * `goToDefinition`'s fallback when a server answers "no file": Godot's built-ins live in the
 * engine, a JDK class in a jar, and the only thing to jump to is a description of them.
 */
import { docs as docsApi, type ProjectId } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { notify } from '@/chrome/notices'

/**
 * Put a sentence on screen. `goToDefinition.ts`'s channel, for its reason: a keystroke has no
 * `disabledReason` to fall back on, so "nothing happened" has to become something readable.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * Ask, then open. Fire-and-forget; every outcome reports itself.
 *
 * `line` and `column` are 1-based, `column` UTF-16 — the wire's units. `word` is what was under
 * the caret, which titles a hover page; empty when the caller had none, and Rust then titles
 * the page by the hover's own first line. `afterDefinition` changes only the sentence for
 * "nothing": the user pressed Go to definition, and the honest answer names both misses.
 */
export function showDocumentation(
  project: ProjectId,
  path: string,
  line: number,
  column: number,
  word: string,
  afterDefinition = false,
): void {
  const what = word === '' ? 'what is under the caret' : `\`${word}\``
  void docsApi
    .lookup(project, path, line, column, word)
    .then((answer) => {
      switch (answer.kind) {
        case 'found':
          void docsApi.openTab(project, answer.subject, answer.page.title).catch((error: unknown) => {
            report(`Could not open the documentation tab: ${errorText(error)}`)
          })
          return
        case 'notFound':
          // A *result*, so a notice rather than the red alert channel — `codeIntel.ts` makes
          // the same distinction for "no symbol here", and with the same kind.
          notify(
            afterDefinition
              ? `No declaration found for ${what}, and nothing documents it.`
              : `Nothing documents ${what}.`,
            { kind: 'warn' },
          )
          return
        case 'unavailable':
          report(answer.reason)
      }
    })
    .catch((error: unknown) => {
      report(`Quick documentation failed: ${errorText(error)}`)
    })
}
