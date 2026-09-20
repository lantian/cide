/**
 * Jump to where the thing under the caret is declared.
 *
 * # One function, several callers
 *
 * `Ctrl+B` through the command registry, `Ctrl+Click` in the buffer (via `codeIntel.ts`'s
 * `ctrlActivate`, whose jump branch inlines the same order rather than calling this), and — since
 * M18 — the empty-answer fallback of Go to implementation, which is the majority of the positions
 * that command is pressed at. Copies of "ask, then reveal, then open" would each be a chance to
 * get the *order* wrong, and the order is the whole correctness of it — see below.
 *
 * This header used to claim "the code pane's context menu" as a third caller. There is no such
 * entry: `grep -rn definition ui/src/menus/` finds nothing, and there never was one. Left recorded
 * rather than quietly deleted, because a comment that names a caller which does not exist is how
 * the next reader concludes a surface is wired when it is not — the exact class of defect the
 * `navigate.*` family keeps being audited for.
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
import { jumpTo } from './jump'
import { showDocumentation } from './quickDocumentation'

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
  onInterfaceMethod?: () => void,
  word = '',
): void {
  void diagnosticsApi
    .definition(project, path, line, column)
    .then((answer) => {
      switch (answer.kind) {
        case 'found':
          /*
           * The one case where the protocol's correct answer is not the useful one. (M18)
           *
           * gopls resolves a call through an interface to the interface's method, because that
           * is where the callee is declared — and the report was that this is not what "go to
           * definition" is for in Go. `interfaceMethod` is decided in Rust by parsing the file
           * this answer points *at*: only a method inside `interface { … }` sets it, so an
           * interface type name and every Rust position are untouched. `cide_lang`'s
           * `interface_method_at` carries the argument for why the target and not the caret.
           *
           * The redirect is **injected by the caller** rather than called from here, for two
           * reasons that both bite. `goToImplementation` lives in `codeIntel.ts`, which imports
           * this module, so reaching for it here is a cycle. And its own empty-answer fallback
           * is this function — so an interface nobody implements would bounce between the two
           * for ever. A caller that passes nothing gets the plain declaration, which is exactly
           * what that fallback needs.
           */
          if (answer.interfaceMethod && onInterfaceMethod !== undefined) {
            onInterfaceMethod()
            return
          }
          /*
           * `jumpTo` **before** `file.open`, which is the design and not a preference.
           *
           * A definition usually lives in a file no pane has open — that is most of what the
           * gesture is for — so the reveal is parked and spent by the mount the open causes.
           * Reversed, the tab opens at line 1 and the caret never moves, which is precisely the
           * case the feature exists to serve. The same sequence `App.tsx`'s `goToSymbol` and the
           * search panel's `onOpenHit` use.
           *
           * Awaiting the lookup first is safe: the 10 s reveal TTL starts here, not at the click.
           *
           * Through `jumpTo` rather than `requestReveal` directly, so the place the user jumped
           * *from* is on the Back stack — this is the gesture people most want to undo. No
           * `endColumn`: `jumpTo` defaults it to `column`, which is a bare caret rather than a
           * selection, and `revealRange` documents that an empty range collapses to the anchor.
           * That is what IDEA does on Go to Declaration — it puts you *at* the name. Selecting
           * would also mean guessing an end from a `Location.range` whose meaning varies by
           * server.
           *
           * The reveal flags are deliberately left alone (no `focus`, no `align`): this gesture
           * shipped without them, and changing where an existing jump lands is a separate
           * decision from recording that it happened.
           */
          jumpTo(project, {
            path: answer.path,
            line: answer.line,
            column: answer.column,
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
          // Not a silence, and since M60 not a sentence either — a second question. A caret on
          // a keyword resolves to nothing, but so does a caret on Godot's `Dictionary` or a JDK
          // class: no file, and a server full of things to say about it. Quick documentation
          // asks, opens a page when there is one, and otherwise says that neither was found —
          // the sentence this arm used to say, with the second miss added.
          showDocumentation(project, path, line, column, word, true)
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
