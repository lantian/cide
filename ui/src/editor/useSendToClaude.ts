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
 * `claude.mention.file` **is** in the command registry (`cide-core::commands`), and running it
 * does nothing: `App.tsx`'s dispatcher has no case for it, so it falls through to
 * `diag.log('command not handled by this window')`. Worse, it is gated `.when("claudePaneFocused")`
 * — and this gesture is made *from an editor*, where by definition no Claude pane is focused,
 * so the palette hides it at the only moment it is wanted. Both of those are one line each in
 * files this change does not own (`App.tsx`, `cide-core/src/commands.rs`); see the note in
 * `codeMenu.tsx`. A CodeMirror keymap entry needs neither and works today.
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

      void sending.then(async () => {
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
           */
          if (view.state.doc !== doc) {
            void diag.log('editor: not revealing the Claude pane — the buffer moved on')
            return
          }
          const stuck = await revealPane(target.project, target.pane)
          if (stuck === null) return
          /*
           * Sent, but the user is not looking at where it went — the pane closed under us, its
           * window would not come forward, the project was closed mid-flight. This is the case
           * the brief asks about and it gets a sentence rather than silence, because a mention
           * in a prompt nobody can see is exactly as invisible as no mention at all. The
           * wording names the file and the range, so the user can find the conversation by
           * hand from what it says.
           */
          report(`Sent ${mentionLabel(path, span)} to Claude — but ${stuck}.`)
        })

      // Logged whether or not it lands. A report of "it did nothing" is answerable from a log
      // that records the attempt and the pane it was aimed at; one that records only successes
      // says nothing about the case being reported.
      void diag.log(`editor: sent ${mentionLabel(path, span)} to ${target.pane}`)
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
