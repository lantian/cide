/**
 * Jump to where the thing under the caret is declared.
 *
 * # One function, three callers
 *
 * The code pane's context menu, `Ctrl+B` through the command registry, and `Ctrl+Click` in the
 * buffer all land here. Three copies of "ask, then reveal, then open" would be three chances to
 * get the *order* wrong, and the order is the whole correctness of it — see below.
 *
 * # What this is not
 *
 * It is not a name search. `cide-lang` indexes declarations and `Ctrl+Alt+Shift+N` searches them,
 * so jumping to whichever `fn new` happens to share the identifier's spelling would be easy and is
 * refused: with eleven `fn new` in a workspace it is right about one time in eleven, and a wrong
 * answer delivered confidently is worse than the disabled menu item this replaces. The resolution
 * is the language server's — `textDocument/definition` — and when no server is running the answer
 * is a sentence saying so, never a guess.
 */
import { diagnostics as diagnosticsApi, file as fileApi, type ProjectId } from '@/ipc/client'
import { requestReveal } from './revealRequest'

/**
 * Put a sentence on screen.
 *
 * `chrome/Failures.tsx` listens for `unhandledrejection`, which is how a failed command becomes
 * visible without a `.catch` at every call site. The same channel — and for the same reason —
 * `useSendToClaude` uses: a keystroke has no `disabledReason` to fall back on, so "nothing
 * happened" has to be turned into something the user can read.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * Resolve and navigate. Fire-and-forget; every outcome reports itself.
 *
 * `line` and `column` are 1-based, and `column` is a UTF-16 code-unit offset — the convention
 * `RevealTarget` and every position on this wire already use.
 */
export function goToDefinition(
  project: ProjectId,
  path: string,
  line: number,
  column: number,
): void {
  void diagnosticsApi
    .definition(project, path, line, column)
    .then((answer) => {
      switch (answer.kind) {
        case 'found':
          /*
           * `requestReveal` **before** `file.open`, which is the design and not a preference.
           *
           * A definition usually lives in a file no pane has open — that is most of what the
           * gesture is for — so the reveal is parked and spent by the mount the open causes.
           * Reversed, the tab opens at line 1 and the caret never moves, which is precisely the
           * case the feature exists to serve. The same sequence `App.tsx`'s `goToSymbol` and the
           * search panel's `onOpenHit` use.
           *
           * Awaiting the lookup first is safe: the 10 s reveal TTL starts here, not at the click.
           */
          requestReveal(answer.path, {
            line: answer.line,
            column: answer.column,
            // `endColumn === column` is a bare caret, not a selection — `revealRange` documents
            // that an empty range collapses to the anchor. That is what IDEA does on Go to
            // Declaration: it puts you *at* the name, it does not select it. Selecting would also
            // mean guessing an end from a `Location.range` whose meaning varies by server.
            endColumn: answer.column,
          })
          /*
           * Deliberately **uncaught**.
           *
           * An earlier version wrote `.catch(() => {})` here with a comment claiming `Failures`
           * would show it anyway. That was false by construction: `chrome/Failures.tsx` listens
           * for `unhandledrejection`, so catching the rejection is precisely what stops it being
           * reported. A definition that resolved and then failed to open — a file deleted since
           * the server indexed it, a permission error, a path outside every root — would have
           * moved nothing and said nothing, which is the silent no-op this whole feature replaces.
           */
          void fileApi.open(project, answer.path)
          return
        case 'notFound':
          // Deliberately a report and not a silence. A caret on a keyword or inside a comment
          // legitimately resolves to nothing, and a gesture that does nothing at all is
          // indistinguishable from one that is broken — the complaint that produced
          // `useSendToClaude` in the first place.
          report('No declaration found for what is under the caret.')
          return
        case 'unavailable':
          // Carries the server's own sentence: "rust-analyzer is still indexing", "gopls is not
          // running for this project", or the rustup shim's own diagnosis.
          report(answer.reason)
      }
    })
    .catch((error: unknown) => {
      // The command is documented never to reject, so this is the transport failing rather than
      // the lookup. Still worth a sentence: the alternative is a keystroke that silently does
      // nothing, which is the state this whole feature replaced.
      report(`Go to definition failed: ${String(error)}`)
    })
}
