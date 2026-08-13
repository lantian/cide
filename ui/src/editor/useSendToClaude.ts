/**
 * The sending half of *Send lines to Claude*, shared by the two gestures that offer it.
 *
 * # Why a hook rather than a function the menu owns
 *
 * There are two entry points and they must not drift: the code pane's context menu, and
 * `Alt-Enter` inside the buffer. The menu had the only implementation, which made the feature
 * mouse-only — and the mock's Ctrl+P footer already advertises *⌥⏎ send path to Claude*, so
 * ⌥⏎ is this app's stated idiom for the gesture and an editor that did not answer it was the
 * missing half of a promise the app makes elsewhere.
 *
 * # Why the keyboard half is a CodeMirror binding and not an app command
 *
 * `claude.mention.file` **is** in the command registry (`cide-core::commands`), and both of the
 * reasons this file used to give for not routing through it are now out of date — recorded
 * rather than silently deleted, because the note in `codeMenu.tsx` was written against them:
 *
 * * it *is* dispatched, at `keys/dispatch.ts`'s `claude.mention.file` arm; and
 * * it is gated `editorFocused && claudeTarget`, not `claudePaneFocused`, so the palette offers
 *   it from an editor — which is the only place the gesture is ever made.
 *
 * The CodeMirror binding stays anyway, and not out of inertia: ⌥⏎ has to reach the *view* to
 * know what is selected, and a registry command is dispatched with no editor in hand — the
 * dispatcher's arm can only mention the focused file whole. Two entry points, two jobs.
 *
 * The cost is honest and stated: a binding that is not in the app keymap does not appear in the
 * palette, cannot be rebound in settings, and shows no shortcut chip.
 *
 * # Sending is half the gesture; arriving is the other half
 *
 * > *"send lines to Claude also should switch to console that added a selected text"*
 *
 * `useMentionTarget` falls back to *the project's console — its first tab's Claude pane*, so
 * the mention routinely lands in a tab the user is not looking at, and can land in a window
 * they are not looking at. A prompt they cannot see is indistinguishable from a send that did
 * nothing, which is the report this feature already answered once in a different disguise.
 * So a send that lands is followed by `revealPane`, and a reveal that cannot happen is said
 * out loud rather than left to be inferred.
 *
 * # And the pane that was asked for is not always the pane that got it
 *
 * That fallback picks `tabs[0]`'s first Claude pane *in map order*, which asks nothing about
 * whether that pane has a `claude` running — and in a restored workspace it very often does
 * not, because only the console's primary Claude pane is spawned eagerly and the rest sit at a
 * resume splash. So `claude_send_lines` treats the pane named here as a preference and routes
 * to the best Claude in the project that can actually receive; see `cmd::file`.
 *
 * Everything downstream of the send therefore reads the **answer**, not `target.pane`: the
 * reveal goes to `sent.pane`, the diagnostic line records `sent.pane`, and `sent.fallback` gets
 * a sentence of its own. Revealing the asked-for pane after delivering somewhere else would put
 * the user in front of an empty prompt while their selection sat in another conversation —
 * strictly worse than the error it replaced, and the one genuinely harmful outcome this feature
 * has.
 *
 * **Focus follows the reveal, keyboard and all**, and that is the decision worth arguing.
 * The gesture is explicit — a menu item the user chose, or a chord they pressed — and its
 * destination is a *prompt*: the mention is the first half of a question, and the natural next
 * act is typing the second half. Leaving the caret in the source file would send those
 * keystrokes into the buffer instead. The hazard the brief names is real but is a different
 * case: focus stolen from someone still typing. That case is guarded by a document
 * comparison at the call site below, not by declining to move focus at all.
 */
import { useCallback } from 'react'
import type { EditorView } from '@codemirror/view'
import { claudeSend, diag } from '@/ipc/client'
import { useMentionTarget } from './mentionTarget'
import { revealPane } from './revealPane'
import { mentionLabel, rangeOf, type SendRange } from './sendToClaude'

/** What the caller needs to draw the control and to fire it. */
export interface SendToClaude {
  /** `null` when the gesture is possible; otherwise the sentence saying why it is not. */
  unavailable: string | null
  /** The range the current selection means, for the label. `null` is the whole file. */
  range: (view: EditorView) => SendRange | null
  /** Fire it. Never silent: when `unavailable`, it says so instead of doing nothing. */
  send: (view: EditorView, path: string) => void
}

/**
 * Put a sentence on screen through the surface the app already has for this.
 *
 * `chrome/Failures.tsx` listens for `unhandledrejection`, which is how every failed command in
 * this app becomes visible without 35 `.catch` calls. A refusal that never reaches Rust has no
 * rejected promise of its own, so it makes one. That is deliberate and is the point: the
 * keyboard route has no `disabledReason` to show — a keystroke either does something or it does
 * not — and "nothing happened" is the exact report this whole change is answering.
 *
 * The alternative that lost was a toast of its own inside the editor. It would duplicate
 * `Failures` — its stacking, its dedupe, its `role="alert"` — so that the same failure could be
 * announced two different ways depending on which half of the send it happened in.
 */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

/**
 * The sentence for a send that did not go where it was aimed.
 *
 * Names the destination rather than the refusal, because the destination is the thing the user
 * cannot work out for themselves and the refusal is not actionable: the pane they aimed at had
 * no `claude` on cide's IDE server, which for a Claude pane at a resume splash is not a fault
 * to fix, it is what a splash *is*. Telling them to run `/ide` there would be advice to start
 * a session they did not ask for.
 *
 * Deliberately one sentence with the conversation's own title in it, so that the reveal that
 * has just happened and the words on screen agree about which prompt they are looking at.
 */
function rerouted(path: string, span: SendRange | null, title: string): string {
  return (
    `Sent ${mentionLabel(path, span)} to ‘${title}’ — the pane it was aimed at has no ` +
    `Claude connected to cide.`
  )
}

export function useSendToClaude(): SendToClaude {
  const target = useMentionTarget()

  const range = useCallback((view: EditorView): SendRange | null => {
    const { from, to } = view.state.selection.main
    return rangeOf(
      view.state.sliceDoc(from, to),
      view.state.doc.lineAt(from).number,
      view.state.doc.lineAt(to).number,
    )
  }, [])

  const send = useCallback(
    (view: EditorView, path: string) => {
      if (target === null) {
        return report(
          'There is no Claude session in this project to send to. Open the project console, ' +
            'or split a Claude pane, and try again.',
        )
      }
      const { from, to } = view.state.selection.main
      const text = view.state.sliceDoc(from, to)
      const span = range(view)

      /*
       * The document as it is at the gesture, held for the guard on the reveal below.
       *
       * A `Text` is immutable and a state change that does not touch the document keeps the
       * same instance, so this is an identity comparison and not a scan of a five-megabyte
       * rope — moving the caret or changing the selection leaves it alone, typing a character
       * replaces it.
       */
      const doc = view.state.doc

      /*
       * No `.catch`. That is the feature, not an oversight: `claude_send_lines` rejects with a
       * sentence when no `claude` is connected, and `chrome/Failures.tsx` listens for exactly
       * this rejection. Catching here would restore the silent no-op the user reported.
       *
       * `.then` and not `.finally`: **if the send fails, nothing moves.** A tab switch on the
       * back of a mention that never arrived would take the user away from their file to look
       * at a prompt with nothing new in it.
       */
      const sending = claudeSend.lines(
        target.project,
        target.pane,
        path,
        text,
        span?.lineStart,
        span?.lineEnd,
      )

      void sending.then(async (sent) => {
        // The resolved pane, and it is logged whether or not the reveal happens. The old line
        // recorded the pane that was *asked for*, at the moment of asking, whether or not
        // anything arrived — which is why a real log of this failure read "sent … to
        // 07565bbd" on the line directly under the server's own "no connected claude in this
        // pane; dropped". Two lines about one gesture that disagreed with each other.
        void diag.log(`editor: sent ${mentionLabel(path, span)} to ${sent.pane}`)

        /*
         * The user carried on typing while the send was in flight, so they are not waiting
         * to be taken anywhere — and the reveal's last act is to move the keyboard, which
         * would cut a word in half between this buffer and Claude's prompt. Nothing moves;
         * the log records that the choice was made rather than that the feature is missing,
         * which is the distinction this whole round is about.
         *
         * `view.hasFocus` was the obvious guard and is wrong: the context-menu route runs
         * with focus on the menu item for the whole of its life, so it would refuse the
         * reveal on every single mouse-driven send.
         *
         * A reroute is said out loud even here. The user is staying in their buffer by their
         * own act, but the lines still went somewhere other than where they were aimed, and
         * that is the one thing about this gesture they cannot find out any other way.
         */
        if (view.state.doc !== doc) {
          void diag.log('editor: not revealing the Claude pane — the buffer moved on')
          if (sent.fallback) report(rerouted(path, span, sent.title))
          return
        }
        const stuck = await revealPane(target.project, sent.pane)
        if (stuck !== null) {
          /*
           * Sent, but the user is not looking at where it went — the pane closed under us, its
           * window would not come forward, the project was closed mid-flight. This is the case
           * the brief asks about and it gets a sentence rather than silence, because a mention
           * in a prompt nobody can see is exactly as invisible as no mention at all. The
           * wording names the file and the range, so the user can find the conversation by
           * hand from what it says — and names the conversation too when it was not the one
           * asked for, because then "find it by hand" needs to say which hand.
           */
          const where = sent.fallback ? `‘${sent.title}’` : 'Claude'
          report(`Sent ${mentionLabel(path, span)} to ${where} — but ${stuck}.`)
          return
        }
        /*
         * The reveal worked, so the user is now looking at the prompt that got the lines —
         * which is the strongest possible statement of where they went and is why the
         * fallback is safe at all. The sentence is still worth saying: the pane they are
         * looking at is *not* the one they aimed at, and a reveal that quietly moved them
         * would leave them to notice on their own that this is a different conversation.
         */
        if (sent.fallback) report(rerouted(path, span, sent.title))
      })

      // The attempt, logged before the answer and naming the pane the gesture aimed at. A
      // report of "it did nothing" is answerable from a log that records the attempt; one that
      // records only successes says nothing at all about the case being reported.
      void diag.log(`editor: sending ${mentionLabel(path, span)} to ${target.pane}`)
    },
    [target, range],
  )

  return {
    // Only the *static* half is a disabled reason. Whether the pane's `claude` is actually
    // connected can change between the menu opening and the click, and a stale "unavailable"
    // that refuses a send which would have worked is worse than a send that explains itself —
    // so that half is reported by the rejection instead. Both halves are covered; they are
    // just covered by the surface that can be right about them.
    unavailable: target === null ? 'No Claude session in this project to send to' : null,
    range,
    send,
  }
}
