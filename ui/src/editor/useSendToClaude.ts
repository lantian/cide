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
 */
import { useCallback } from 'react'
import type { EditorView } from '@codemirror/view'
import { claudeSend, diag } from '@/ipc/client'
import { useMentionTarget } from './mentionTarget'
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
       * No `.catch`. That is the feature, not an oversight: `claude_send_lines` rejects with a
       * sentence when no `claude` is connected, and `chrome/Failures.tsx` listens for exactly
       * this rejection. Catching here would restore the silent no-op the user reported.
       */
      void claudeSend.lines(
        target.project,
        target.pane,
        path,
        text,
        span?.lineStart,
        span?.lineEnd,
      )

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
